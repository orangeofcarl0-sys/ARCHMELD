// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! CLI command implementations, grouped by operator domain.

pub(crate) mod admin;
pub(crate) mod backup;
pub(crate) mod brief;
pub(crate) mod common;
pub(crate) mod evidence;
pub(crate) mod index;
#[cfg(test)]
mod index_tests;
pub(crate) mod init;
pub(crate) mod report;
pub(crate) mod review;

pub(crate) use admin::*;
pub(crate) use backup::*;
pub(crate) use brief::*;
pub(crate) use evidence::*;
pub(crate) use index::*;
pub(crate) use init::*;
pub(crate) use report::*;
pub(crate) use review::*;
pub(crate) use common::*;

pub(crate) use archmeld_api::onboard;
pub(crate) use archmeld_core::bridge::{import_codegraph_export, BridgeConfig};
pub(crate) use archmeld_core::brief::{
    build_brief_with_budget, render_markdown, write_session_card, BriefBudget, BriefRecipe,
    BriefRequest,
};
pub(crate) use archmeld_core::cognition_map::{check_map, maintain, render_markdown as render_map_markdown};
pub(crate) use archmeld_core::encrypted_backup::{
    create_encrypted_backup, restore_encrypted_backup, retain_encrypted_backup,
    rotate_encrypted_backup, verify_encrypted_backup,
};
pub(crate) use archmeld_core::evaluate_numeric_tolerance_artifacts;
pub(crate) use archmeld_core::hash;
pub(crate) use archmeld_core::migrate::migrate_ledger;
pub(crate) use archmeld_core::perfetto::perfetto_trace;
pub(crate) use archmeld_core::report::{export_report, verify_bundle};
pub(crate) use archmeld_core::review::{
    diff_snapshots, freeze_review_candidate, persist_review_report, review_workspace_diff,
};
pub(crate) use archmeld_core::runtime::{import_trace, RuntimeImport};
pub(crate) use archmeld_core::sandbox::{materialize_all, sanitize_id};
pub(crate) use archmeld_core::scenario::{run_scenario, ExecutionGrant, ScenarioSandbox, ScenarioSpec};
pub(crate) use archmeld_core::snapshot::{capture_snapshot, SnapshotInputs, SnapshotOptions};
pub(crate) use archmeld_core::status::status_report;
pub(crate) use archmeld_core::store::Store;
pub(crate) use archmeld_core::trace_adapters::{chrome_trace, probe_jsonl, TraceLocator, TraceMeta};
pub(crate) use archmeld_core::trace_processor::perfetto_trace_via_processor;
pub(crate) use archmeld_core::tracy_csv::import_tracy_capture;
pub(crate) use archmeld_core::watch::{watch_drift, watch_drift_notify, NotifyWatchOptions, WatchOptions};
pub(crate) use clap::Subcommand;
pub(crate) use serde_json::{json, Value};
pub(crate) use std::fs;
pub(crate) use std::path::PathBuf;
pub(crate) use std::process::{Command, ExitCode};
pub(crate) use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
