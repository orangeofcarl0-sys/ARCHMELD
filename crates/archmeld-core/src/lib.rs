// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! ARCHMELD core: evidence storage, snapshots, identity, query and change
//! services, grouped by concern.
//!
//! The group modules are private. Every public module is re-exported at the
//! crate root, so the pre-group paths (`archmeld_core::store`,
//! `crate::store`, ...) remain valid. `anchor`, `units` and `predicate` are
//! crate-private and re-exported with `pub(crate) use`.
//!
//! Implemented scope so far:
//! * `store` — SQLite evidence ledger with atomic multi-batch import;
//! * `hash` — AM-C14N-1 canonical JSON and content digests;
//! * `snapshot` — byte-exact workspace capture with stability checks;
//! * `identity` — content-derived entity IDs;
//! * `source` — UTF-8 byte / line / UTF-16 coordinate conversion;
//! * `bridge` — CodeGraph NDJSON export to evidence batches;
//! * `changeset`/`apply` — controlled staging, prepared digests, writer
//!   leases and journaled recovery.

// ---- evidence -----------------------------------------------------------
mod evidence {
    pub mod bridge;
    pub mod events;
    pub mod hash;
    pub mod identity;
    pub mod query;
    pub mod retention;
    pub mod snapshot;
    pub mod source;
    pub mod source_api;
    pub mod store;
    pub(crate) mod anchor;
}
pub use evidence::{
    bridge, events, hash, identity, query, retention, snapshot, source, source_api, store,
};
pub(crate) use evidence::anchor;

// ---- change -------------------------------------------------------------
mod change {
    pub mod apply;
    pub mod changeset;
    pub mod plan;
    pub mod sandbox;
    pub mod scenario;
    pub mod verification;
    pub(crate) mod predicate;
    pub(crate) mod units;
}
pub use change::{apply, changeset, plan, sandbox, scenario, verification};
pub(crate) use change::{predicate, units};

// ---- trace --------------------------------------------------------------
mod trace {
    pub mod perfetto;
    pub mod runtime;
    pub mod trace_adapters;
    pub mod trace_processor;
    pub mod tracy_csv;
}
pub use trace::{perfetto, runtime, trace_adapters, trace_processor, tracy_csv};

// ---- analysis -----------------------------------------------------------
mod analysis {
    pub mod blueprint;
    pub mod cognition;
    pub mod cognition_map;
    pub mod context;
    pub mod interpret;
}
pub use analysis::{blueprint, cognition, cognition_map, context, interpret};

// ---- ops ----------------------------------------------------------------
mod ops {
    pub mod drift;
    pub mod encrypted_backup;
    pub mod instigator;
    pub mod jobs;
    pub mod manager;
    pub mod migrate;
    pub mod report;
    pub mod watch;
}
pub use ops::{drift, encrypted_backup, instigator, jobs, manager, migrate, report, watch};

// ---- surface ------------------------------------------------------------
mod surface {
    pub mod brief;
    pub mod error;
    pub mod language;
    pub mod lsp;
    pub mod review;
    pub mod scip;
    pub mod status;
}
pub use surface::{brief, error, language, lsp, review, scip, status};

pub use surface::error::{CoreError, Result};
pub use change::predicate::evaluate_numeric_tolerance_artifacts;

/// Contract schema version implemented by this crate.
pub const CONTRACT_VERSION: &str = "1.1";
