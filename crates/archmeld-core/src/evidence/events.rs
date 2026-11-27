// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Durable, replayable project events (AT073).
//!
//! Events are written in the same transaction as the business state they
//! describe. `sequence` is a per-project decimal string; a cursor older than the
//! retained window returns an explicit `resync_required` instead of silently
//! skipping events.

use crate::error::{CoreError, Result};
use crate::hash;
use crate::snapshot::rfc3339_from_unix;
use crate::store::{validate, Store};
use serde_json::{json, Value};

pub struct EventPage {
    pub events: Vec<Value>,
    pub resync_required: bool,
}

pub(crate) fn append_event(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
    job_id: Option<&str>,
    snapshot_id: Option<&str>,
    event_type: &str,
    payload: Value,
) -> Result<i64> {
    let sequence = next_sequence(tx, project_id)?;
    let digest = hash::digest_value(&json!({
        "project_id": project_id,
        "type": event_type,
        "sequence": sequence,
    }))
    .map_err(|err| CoreError::invalid(format!("event identity: {err}")))?;
    let hex = digest
        .strip_prefix("sha256:")
        .expect("digest prefix");
    let event_id = format!("event.{}", &hex[..32]);
    tx.execute(
        "INSERT INTO events
         (project_id, sequence, event_id, type, payload_json, created_at, job_id, snapshot_id)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        (
            project_id,
            sequence,
            &event_id,
            event_type,
            payload.to_string(),
            crate::store::now_unix(),
            job_id,
            snapshot_id,
        ),
    )?;
    Ok(sequence)
}

/// Replay events after `after_sequence` (inclusive of everything greater).
/// Returns `resync_required` when the cursor predates the retained window.
pub(crate) fn next_sequence(
    tx: &rusqlite::Transaction<'_>,
    project_id: &str,
) -> Result<i64> {
    let max: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence), -1) FROM events WHERE project_id=?1",
        (project_id,),
        |row| row.get(0),
    )?;
    let watermark: Option<i64> = tx
        .query_row(
            "SELECT oldest_sequence FROM event_retention WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )
        .ok();
    Ok(max.saturating_add(1).max(watermark.unwrap_or(0)))
}

pub fn events_after(
    store: &Store,
    project_id: &str,
    after_sequence: Option<u64>,
) -> Result<EventPage> {
    validate::require_id("project_id", project_id)?;
    let oldest: Option<i64> = store
        .conn
        .query_row(
            "SELECT oldest_sequence FROM event_retention WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )
        .ok();
    let oldest = match oldest {
        Some(watermark) => Some(watermark),
        None => store
            .conn
            .query_row(
                "SELECT MIN(sequence) FROM events WHERE project_id=?1",
                (project_id,),
                |row| row.get::<_, Option<i64>>(0),
            )?,
    };
    let (lower, after) = match after_sequence {
        Some(after) => {
            let after = after.min(i64::MAX as u64) as i64;
            (after.saturating_add(1), Some(after))
        }
        // Omitted cursor means "replay everything retained", not "after 0".
        None => (0, None),
    };
    if let (Some(oldest), Some(after)) = (oldest, after) {
        if after.saturating_add(1) < oldest {
            return Ok(EventPage {
                events: Vec::new(),
                resync_required: true,
            });
        }
    }
    let mut statement = store.conn.prepare(
        "SELECT sequence, event_id, type, created_at, job_id, snapshot_id, payload_json
         FROM events WHERE project_id=?1 AND sequence>=?2 ORDER BY sequence",
    )?;
    let rows = statement.query_map((project_id, lower), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, i64>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
        ))
    })?;
    let mut events = Vec::new();
    for row in rows {
        let (sequence, event_id, event_type, created_at, job_id, snapshot_id, payload) = row?;
        let payload: Value = serde_json::from_str(&payload)
            .map_err(|err| CoreError::Storage(format!("event payload corrupt: {err}")))?;
        events.push(json!({
            "event_id": event_id,
            "project_id": project_id,
            "job_id": job_id,
            "sequence": sequence.to_string(),
            "type": event_type,
            "timestamp": rfc3339_from_unix(created_at),
            "snapshot_id": snapshot_id,
            "payload": payload,
        }));
    }
    Ok(EventPage {
        events,
        resync_required: false,
    })
}

/// Retention hook: remove events older than `keep_from`. Returns the number of
/// deleted rows. Used by tests now and by the retention job later.
pub fn prune_events_before(store: &mut Store, project_id: &str, keep_from: u64) -> Result<usize> {
    let keep_from = keep_from.min(i64::MAX as u64) as i64;
    let tx = store.conn.transaction()?;
    let latest: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence), -1) FROM events WHERE project_id=?1",
        (project_id,),
        |row| row.get(0),
    )?;
    let deleted = tx.execute(
        "DELETE FROM events WHERE project_id=?1 AND sequence<?2",
        (project_id, keep_from),
    )?;
    // A pruning request may name any future point; the watermark is the first
    // sequence that still exists (or the next one if nothing is retained).
    let watermark = keep_from.min(latest.saturating_add(1)).max(0);
    tx.execute(
        "INSERT INTO event_retention (project_id, oldest_sequence) VALUES (?1, ?2)
         ON CONFLICT(project_id) DO UPDATE SET oldest_sequence=excluded.oldest_sequence",
        (project_id, watermark),
    )?;
    tx.commit()?;
    Ok(deleted)
}
