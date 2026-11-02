// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! ARCHMELD core: evidence storage, snapshots, identity, query and change
//! services.
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

pub mod anchor;
pub mod apply;
pub mod blueprint;
pub mod bridge;
pub mod changeset;
pub mod context;
pub mod drift;
pub mod encrypted_backup;
pub mod error;
pub mod events;
pub mod hash;
pub mod jobs;
pub mod language;
pub mod lsp;
pub mod manager;
pub mod identity;
pub mod interpret;
pub mod instigator;
pub mod migrate;
pub mod perfetto;
pub mod plan;
mod predicate;
pub mod query;
pub mod report;
pub mod retention;
pub mod review;
pub mod runtime;
pub mod sandbox;
pub mod scenario;
pub mod scip;
pub mod snapshot;
pub mod source;
pub mod source_api;
pub mod status;
pub mod trace_processor;
pub mod store;
pub mod trace_adapters;
pub mod tracy_csv;
pub mod units;
pub mod verification;
pub mod watch;

pub use error::{CoreError, Result};
pub use predicate::evaluate_numeric_tolerance_artifacts;

/// Contract schema version implemented by this crate.
pub const CONTRACT_VERSION: &str = "1.0";
