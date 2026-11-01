// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Deterministic drift watchers. The polling watcher compares the base
//! snapshot to the workspace on an interval; the OS-notification watcher is
//! woken by `notify` and then re-hashes the workspace. Both persist each report
//! in `drift_reports` and never write to the workspace. A notification only
//! wakes the analysis: no event payload is trusted as drift evidence.

use crate::drift::analyze_drift;
use crate::error::{CoreError, Result};
use crate::hash;
use crate::store::{validate, Store};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub struct WatchOptions {
    pub project_id: String,
    pub base_snapshot_id: String,
    pub root: PathBuf,
    pub interval: Duration,
    /// `None` runs until the stop flag is set or the process is terminated.
    pub iterations: Option<usize>,
    pub stop: Option<Arc<AtomicBool>>,
}

pub struct NotifyWatchOptions {
    pub base: WatchOptions,
    /// Quiet period after the last event before the workspace is re-hashed.
    pub debounce: Duration,
}

fn now_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn sleep_interruptible(interval: Duration, stop: Option<&Arc<AtomicBool>>) {
    let mut remaining = interval;
    let slice = Duration::from_millis(25);
    while !remaining.is_zero() {
        if stop.is_some_and(|flag| flag.load(Ordering::SeqCst)) {
            return;
        }
        let step = remaining.min(slice);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

pub fn watch_drift(store: &mut Store, options: &WatchOptions) -> Result<Value> {
    validate::require_id("project_id", &options.project_id)?;
    validate::require_digest("base_snapshot_id", &options.base_snapshot_id)?;
    if !options.root.is_dir() {
        return Err(CoreError::invalid(format!(
            "watch root {} is not a directory",
            options.root.display()
        )));
    }
    let interval = options.interval.max(Duration::from_millis(10));
    let limit = options.iterations.unwrap_or(usize::MAX);
    let mut ticks = 0usize;
    let mut reports = Vec::new();
    let mut stopped_by = "iterations";
    while ticks < limit {
        if options
            .stop
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            stopped_by = "stop_flag";
            break;
        }
        let report = analyze_drift(
            store,
            &options.project_id,
            &options.base_snapshot_id,
            &options.root,
            None,
        )?;
        let entry = persist_watch_report(
            store,
            &options.project_id,
            &options.base_snapshot_id,
            &report,
            "drift.watch",
            ticks,
        )?;
        reports.push(entry);
        ticks += 1;
        if ticks >= limit {
            break;
        }
        sleep_interruptible(interval, options.stop.as_ref());
    }
    Ok(json!({
        "ticks": ticks,
        "interval_ms": interval.as_millis(),
        "stopped_by": stopped_by,
        "reports": reports,
        "watcher_kind": "polling",
        "source_mutation": "none",
        "limitations": ["polling compares content hashes; OS file-notification events are not claimed"],
    }))
}

/// OS-notification watcher. `notify` reports file-system events; every report
/// still re-hashes the workspace, so a coalesced or missed event cannot produce
/// a clean result by itself. The watcher never writes the workspace.
pub fn watch_drift_notify(store: &mut Store, options: &NotifyWatchOptions) -> Result<Value> {
    let base = &options.base;
    validate::require_id("project_id", &base.project_id)?;
    validate::require_digest("base_snapshot_id", &base.base_snapshot_id)?;
    if !base.root.is_dir() {
        return Err(CoreError::invalid(format!(
            "watch root {} is not a directory",
            base.root.display()
        )));
    }
    let debounce = options.debounce.max(Duration::from_millis(10));
    let limit = base.iterations.unwrap_or(usize::MAX);
    let (sender, receiver) = mpsc::channel();
    let mut watcher: RecommendedWatcher = notify::recommended_watcher(move |event| {
        let _ = sender.send(event);
    })
    .map_err(|error| CoreError::unsupported(format!("OS file watcher is unavailable: {error}")))?;
    watcher
        .watch(&base.root, RecursiveMode::Recursive)
        .map_err(|error| CoreError::unsupported(format!("OS file watcher cannot watch the root: {error}")))?;

    let mut reports = Vec::new();
    let mut ticks = 0usize;
    let mut stopped_by = "iterations";
    let initial = analyze_drift(
        store,
        &base.project_id,
        &base.base_snapshot_id,
        &base.root,
        None,
    )?;
    reports.push(persist_watch_report(
        store,
        &base.project_id,
        &base.base_snapshot_id,
        &initial,
        "drift.watch.os",
        ticks,
    )?);
    ticks += 1;
    while ticks < limit {
        if base
            .stop
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            stopped_by = "stop_flag";
            break;
        }
        let wait = debounce.min(Duration::from_millis(50)).max(Duration::from_millis(5));
        match receiver.recv_timeout(wait) {
            Ok(Ok(_)) => {
                // Coalesce the burst until the file system is quiet.
                while !base
                    .stop
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    match receiver.recv_timeout(debounce) {
                        Ok(Ok(_)) => continue,
                        Ok(Err(error)) => {
                            return Err(CoreError::Storage(format!(
                                "OS file watcher error: {error}"
                            )))
                        }
                        Err(RecvTimeoutError::Timeout) => break,
                        Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                if base
                    .stop
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::SeqCst))
                {
                    stopped_by = "stop_flag";
                    break;
                }
                let report = analyze_drift(
                    store,
                    &base.project_id,
                    &base.base_snapshot_id,
                    &base.root,
                    None,
                )?;
                reports.push(persist_watch_report(
                    store,
                    &base.project_id,
                    &base.base_snapshot_id,
                    &report,
                    "drift.watch.os",
                    ticks,
                )?);
                ticks += 1;
            }
            Ok(Err(error)) => {
                return Err(CoreError::Storage(format!(
                    "OS file watcher error: {error}"
                )))
            }
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => {
                return Err(CoreError::Storage(
                    "OS file watcher stopped unexpectedly".to_string(),
                ))
            }
        }
    }
    Ok(json!({
        "ticks": ticks,
        "debounce_ms": debounce.as_millis(),
        "stopped_by": stopped_by,
        "reports": reports,
        "watcher_kind": "os_notify",
        "watcher_backend": "notify::RecommendedWatcher",
        "source_mutation": "none",
        "limitations": [
            "OS notifications may be coalesced or dropped; each report re-hashes the workspace so an event is only a wakeup",
            "no event payload is trusted as drift evidence",
        ],
    }))
}

fn persist_watch_report(
    store: &mut Store,
    project_id: &str,
    base_snapshot_id: &str,
    report: &Value,
    prefix: &str,
    ticks: usize,
) -> Result<Value> {
    let state = report["state"].as_str().unwrap_or("unknown").to_string();
    let report_id = format!("{prefix}.{}.{}", now_nanos(), ticks);
    let summary_json = hash::canonical_json(report)
        .map_err(|err| CoreError::invalid(format!("drift report: {err}")))?;
    store.conn.execute(
        "INSERT INTO drift_reports
         (project_id,report_id,base_snapshot_id,new_snapshot_id,summary_json,created_at)
         VALUES (?1,?2,?3,NULL,?4,?5)",
        (
            project_id,
            &report_id,
            base_snapshot_id,
            &summary_json,
            crate::store::now_unix(),
        ),
    )?;
    Ok(json!({
        "report_id": report_id,
        "state": state,
        "changed": report["changed"],
        "deleted": report["deleted"],
        "added": report["added"],
        "affected_entities": report["affected_entities"],
        "affected_relations": report["affected_relations"],
    }))
}
