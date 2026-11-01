// Copyright 2026 ARCHMELD contributors
// SPDX-License-Identifier: Apache-2.0

//! Evidence ledger: SQLite facts + content-addressed object store.
//!
//! One `Store` owns one writable connection (single writer). SQLite is the
//! authority for snapshots, evidence, entities, relations and run state; the
//! object store holds raw bytes addressed by SHA-256.

mod import;
mod types;
pub(crate) mod validate;

pub use import::{FaultPoint, ImportOutcome, ImportStatus, PublishedCounts};
pub use types::*;

use crate::error::{CoreError, Result};
use crate::hash;
use rusqlite::Connection;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const SCHEMA_VERSION: i64 = 18;

pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

pub(crate) struct BlobInfo {
    pub size: u64,
    pub is_utf8: bool,
    pub path: PathBuf,
}

/// Description used to register a snapshot before analysis batches arrive.
#[derive(Debug, Clone)]
pub struct SnapshotInsert {
    pub snapshot_id: String,
    pub project_id: String,
    pub config_digest: String,
    pub environment_digest: String,
    pub exclusions_digest: String,
    pub manifest_artifact: String,
    pub stable: bool,
    pub coverage: Coverage,
    pub payload: serde_json::Value,
    pub files: Vec<FileEntry>,
}

pub struct Store {
    pub(crate) conn: Connection,
    root: PathBuf,
    objects: PathBuf,
}

impl Store {
    /// Open (or create) a ledger under `root`. Creates `state.sqlite3` and the
    /// `objects/` CAS. Never deletes an existing database: migrations either
    /// apply forward or fail and roll back.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let objects = root.join("objects");
        fs::create_dir_all(objects.join("sha256"))?;
        fs::create_dir_all(objects.join("tmp"))?;
        let conn = Connection::open(root.join("state.sqlite3"))?;
        let mut store = Self {
            conn,
            root,
            objects,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    /// Ledger root. Verification sandboxes and other transient artifacts live
    /// under this directory, never inside the user workspace.
    pub fn state_root(&self) -> &Path {
        &self.root
    }

    /// In-memory SQLite for tests; the CAS still uses `root` on disk so blob
    /// verification runs against real bytes.
    pub fn open_in_memory(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        fs::create_dir_all(&root)?;
        let objects = root.join("objects");
        fs::create_dir_all(objects.join("sha256"))?;
        fs::create_dir_all(objects.join("tmp"))?;
        let conn = Connection::open_in_memory()?;
        let mut store = Self {
            conn,
            root,
            objects,
        };
        store.configure()?;
        store.migrate()?;
        Ok(store)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn configure(&mut self) -> Result<()> {
        self.conn
            .pragma_update(None, "journal_mode", "WAL")?;
        self.conn
            .pragma_update(None, "synchronous", "FULL")?;
        self.conn.pragma_update(None, "foreign_keys", "ON")?;
        self.conn.busy_timeout(Duration::from_secs(5))?;
        Ok(())
    }

    fn migrate(&mut self) -> Result<()> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(CoreError::Storage(format!(
                "database schema {version} is newer than this binary ({SCHEMA_VERSION}); refusing to downgrade"
            )));
        }
        if version < 1 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0001_init.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (1, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 1)?;
            tx.commit()?;
        }
        if version < 2 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0002_jobs_events.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (2, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 2)?;
            tx.commit()?;
        }
        if version < 3 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0003_event_retention.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (3, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 3)?;
            tx.commit()?;
        }
        if version < 4 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0004_project_lifecycle.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (4, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 4)?;
            tx.commit()?;
        }
        if version < 5 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0005_workspace_handles.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (5, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 5)?;
            tx.commit()?;
        }
        if version < 6 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0006_runtime.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (6, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 6)?;
            tx.commit()?;
        }
        if version < 7 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0007_scenarios.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (7, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 7)?;
            tx.commit()?;
        }
        if version < 8 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0008_runtime_causal.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (8, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 8)?;
            tx.commit()?;
        }
        if version < 9 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0009_interpretation.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (9, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 9)?;
            tx.commit()?;
        }
        if version < 10 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0010_plans.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (10, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 10)?;
            tx.commit()?;
        }
        if version < 11 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0011_changesets.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (11, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 11)?;
            tx.commit()?;
        }
        if version < 12 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0012_apply_journal.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (12, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 12)?;
            tx.commit()?;
        }
        if version < 13 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0013_verification.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (13, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 13)?;
            tx.commit()?;
        }
        if version < 14 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0014_instigator.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (14, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 14)?;
            tx.commit()?;
        }
        if version < 15 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0015_drift_backup.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (15, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 15)?;
            tx.commit()?;
        }
        if version < 16 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0016_retention.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (16, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 16)?;
            tx.commit()?;
        }
        if version < 17 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0017_runtime_metrics.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (17, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 17)?;
            tx.commit()?;
        }
        if version < 18 {
            let tx = self.conn.transaction()?;
            tx.execute_batch(include_str!("migrations/0018_scenario_trace_link.sql"))?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (18, ?1)",
                (now_unix(),),
            )?;
            tx.pragma_update(None, "user_version", 18)?;
            tx.commit()?;
        }
        Ok(())
    }

    /// Register a project. The mode is immutable; a conflicting re-registration
    /// is rejected instead of silently changing ownership semantics.
    pub fn ensure_project(&mut self, project_id: &str, mode: &str) -> Result<()> {
        validate::require_id("project_id", project_id)?;
        if !matches!(mode, "manager" | "expedition") {
            return Err(CoreError::invalid(format!(
                "project mode must be manager or expedition, got {mode}"
            )));
        }
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT mode FROM projects WHERE project_id=?1",
                (project_id,),
                |row| row.get(0),
            )
            .ok();
        match existing {
            Some(existing_mode) if existing_mode != mode => Err(CoreError::conflict(format!(
                "project {project_id} is already registered as {existing_mode}"
            ))),
            Some(_) => Ok(()),
            None => {
                self.conn.execute(
                    "INSERT INTO projects (project_id, mode, revision, state, created_at)
                     VALUES (?1, ?2, 0, 'open', ?3)",
                    (project_id, mode, now_unix()),
                )?;
                Ok(())
            }
        }
    }

    pub(crate) fn ensure_project_exists(&self, project_id: &str) -> Result<()> {
        let exists: Option<String> = self
            .conn
            .query_row(
                "SELECT project_id FROM projects WHERE project_id=?1",
                (project_id,),
                |row| row.get(0),
            )
            .ok();
        match exists {
            Some(_) => Ok(()),
            None => Err(CoreError::not_found(format!(
                "project {project_id} is not registered"
            ))),
        }
    }

    /// Register a snapshot manifest. Raw manifest bytes must already be in the
    /// CAS; the row stores their digest, never a copied blob.
    pub fn insert_snapshot(&mut self, insert: SnapshotInsert) -> Result<()> {
        validate::require_digest("snapshot_id", &insert.snapshot_id)?;
        validate::require_digest("config_digest", &insert.config_digest)?;
        validate::require_digest("environment_digest", &insert.environment_digest)?;
        validate::require_digest("exclusions_digest", &insert.exclusions_digest)?;
        validate::require_digest("manifest_artifact", &insert.manifest_artifact)?;
        if self.blob_info(&insert.manifest_artifact)?.is_none() {
            return Err(CoreError::invalid(format!(
                "snapshot manifest {} is not in the object store",
                insert.manifest_artifact
            )));
        }
        let project_mode: Option<String> = self
            .conn
            .query_row(
                "SELECT mode FROM projects WHERE project_id=?1",
                (&insert.project_id,),
                |row| row.get(0),
            )
            .ok();
        if project_mode.is_none() {
            return Err(CoreError::not_found(format!(
                "project {} is not registered",
                insert.project_id
            )));
        }
        let payload_json = hash::canonical_json(&insert.payload)
            .map_err(|err| CoreError::invalid(format!("snapshot payload: {err}")))?;
        let existing: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_json FROM snapshots WHERE snapshot_id=?1",
                (&insert.snapshot_id,),
                |row| row.get(0),
            )
            .ok();
        if let Some(existing) = existing {
            let existing_value: serde_json::Value = serde_json::from_str(&existing)
                .map_err(|err| CoreError::Storage(format!("stored snapshot payload is corrupt: {err}")))?;
            if snapshot_identity(&existing_value)? == snapshot_identity(&insert.payload)? {
                return Ok(());
            }
            return Err(CoreError::conflict(format!(
                "snapshot {} already exists with a different content identity",
                insert.snapshot_id
            )));
        }
        let mut seen_paths = std::collections::HashSet::new();
        for file in &insert.files {
            validate::validate_path("snapshot.files[].path", &file.path)?;
            validate::require_digest("snapshot.files[].blob_digest", &file.blob_digest)?;
            if !seen_paths.insert(file.path.clone()) {
                return Err(CoreError::invalid(format!(
                    "snapshot manifest has a duplicate path: {}",
                    file.path
                )));
            }
            if self.blob_info(&file.blob_digest)?.is_none() {
                return Err(CoreError::invalid(format!(
                    "snapshot file {} references blob {} that is not in the object store",
                    file.path, file.blob_digest
                )));
            }
        }
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO snapshots
             (snapshot_id, project_id, config_digest, environment_digest, exclusions_digest,
              manifest_artifact, stable, coverage_json, payload_json, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            (
                &insert.snapshot_id,
                &insert.project_id,
                &insert.config_digest,
                &insert.environment_digest,
                &insert.exclusions_digest,
                &insert.manifest_artifact,
                if insert.stable { 1 } else { 0 },
                serde_json::to_string(&insert.coverage).expect("coverage serializes"),
                payload_json,
                now_unix(),
            ),
        )?;
        for file in &insert.files {
            tx.execute(
                "INSERT INTO files
                 (project_id, snapshot_id, path, blob_digest, size_bytes, role, origin, executable)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                (
                    &insert.project_id,
                    &insert.snapshot_id,
                    &file.path,
                    &file.blob_digest,
                    file.size_bytes as i64,
                    file.role.as_str(),
                    file.origin.as_str(),
                    if file.executable { 1 } else { 0 },
                ),
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Store raw bytes and return `sha256:<hex>`. The blob is written and
    /// fsynced before the artifact row becomes visible; a crash can only leave
    /// an unreferenced object, never a dangling reference.
    pub fn put_blob(&mut self, bytes: &[u8], media_type: &str) -> Result<String> {
        if media_type.is_empty() {
            return Err(CoreError::invalid("media_type must not be empty"));
        }
        let digest = hash::digest_bytes(bytes);
        let size = bytes.len() as u64;
        let existing: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT size_bytes, media_type FROM artifacts WHERE digest=?1",
                (&digest,),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        if let Some((existing_size, _)) = existing {
            if existing_size as u64 != size {
                return Err(CoreError::Storage(format!(
                    "artifact {digest} is indexed with size {existing_size} but new bytes have size {size}"
                )));
            }
            return Ok(digest);
        }
        let is_utf8 = std::str::from_utf8(bytes).is_ok();
        let path = self.blob_path(&digest);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let temp = self.objects.join("tmp").join(format!(
            "{}.{}.{}.tmp",
            digest.replace(':', "-"),
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        if path.exists() {
            fs::remove_file(&temp)?;
        } else {
            fs::rename(&temp, &path)?;
        }
        self.conn.execute(
            "INSERT INTO artifacts (digest, size_bytes, media_type, encoding, created_at)
             VALUES (?1,?2,?3,?4,?5)",
            (
                &digest,
                size as i64,
                media_type,
                if is_utf8 { "utf-8" } else { "binary" },
                now_unix(),
            ),
        )?;
        Ok(digest)
    }

    pub(crate) fn blob_info(&self, digest: &str) -> Result<Option<BlobInfo>> {
        let row: Option<(i64, String)> = self
            .conn
            .query_row(
                "SELECT size_bytes, encoding FROM artifacts WHERE digest=?1",
                (digest,),
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .ok();
        let Some((size, encoding)) = row else {
            return Ok(None);
        };
        let path = self.blob_path(digest);
        let metadata = fs::metadata(&path).map_err(|err| {
            CoreError::Storage(format!(
                "artifact {digest} is indexed but its object {} is unreadable: {err}",
                path.display()
            ))
        })?;
        if metadata.len() != size as u64 {
            return Err(CoreError::Storage(format!(
                "artifact {digest} size mismatch: index {size}, object {}",
                metadata.len()
            )));
        }
        Ok(Some(BlobInfo {
            size: size as u64,
            is_utf8: encoding == "utf-8",
            path,
        }))
    }

    pub(crate) fn blob_is_char_boundary(&self, info: &BlobInfo, offset: u64) -> Result<bool> {        if offset == 0 || offset == info.size {
            return Ok(true);
        }
        if offset > info.size {
            return Ok(false);
        }
        let mut file = File::open(&info.path)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte)?;
        Ok((byte[0] & 0b1100_0000) != 0b1000_0000)
    }

    /// Generic idempotency record lookup. `scope` is a stable key such as a
    /// workspace handle (open) or project id (close/promote).
    pub fn idempotency_get(
        &self,
        scope: &str,
        operation: &str,
        key: &str,
    ) -> Result<Option<(String, i64, String)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT request_digest, status_code, result_json FROM idempotency
                 WHERE project_id=?1 AND operation=?2 AND idempotency_key=?3",
                (scope, operation, key),
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .ok())
    }

    pub fn idempotency_put(
        &mut self,
        scope: &str,
        operation: &str,
        key: &str,
        request_digest: &str,
        status_code: u16,
        result: &serde_json::Value,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO idempotency
             (project_id, operation, idempotency_key, request_digest, status_code, result_json, created_at)
             VALUES (?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(project_id, operation, idempotency_key) DO NOTHING",
            (
                scope,
                operation,
                key,
                request_digest,
                status_code as i64,
                result.to_string(),
                now_unix(),
            ),
        )?;
        Ok(())
    }

    pub(crate) fn read_blob(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        let Some(info) = self.blob_info(digest)? else {
            return Ok(None);
        };
        let bytes = fs::read(&info.path)?;
        Ok(Some(bytes))
    }

    /// Public read-only CAS access for acceptance tests and review surfaces.
    /// The bytes are still integrity-checked against the recorded size.
    pub fn read_blob_bytes(&self, digest: &str) -> Result<Option<Vec<u8>>> {
        self.read_blob(digest)
    }

    pub fn blob_exists(&self, digest: &str) -> Result<bool> {
        Ok(self.blob_info(digest)?.is_some())
    }

    pub fn blob_size(&self, digest: &str) -> Result<Option<u64>> {
        Ok(self.blob_info(digest)?.map(|info| info.size))
    }

    /// Reference an existing blob from a non-evidence owner (for example a
    /// retained native trace capture). Referencing never copies bytes; it makes
    /// retention and review able to see that the artifact is still in use.
    pub fn reference_artifact(
        &mut self,
        owner_type: &str,
        owner_id: &str,
        digest: &str,
    ) -> Result<()> {
        validate::require_non_empty("owner_type", owner_type)?;
        validate::require_non_empty("owner_id", owner_id)?;
        validate::require_digest("digest", digest)?;
        if !self.blob_exists(digest)? {
            return Err(CoreError::not_found(format!(
                "artifact {digest} is not in the content-addressed store"
            )));
        }
        self.conn.execute(
            "INSERT OR IGNORE INTO artifact_refs (owner_type, owner_id, digest)
             VALUES (?1, ?2, ?3)",
            (owner_type, owner_id, digest),
        )?;
        Ok(())
    }

    pub fn has_artifact_ref(
        &self,
        owner_type: &str,
        owner_id: &str,
        digest: &str,
    ) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM artifact_refs
             WHERE owner_type=?1 AND owner_id=?2 AND digest=?3",
            (owner_type, owner_id, digest),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    fn blob_path(&self, digest: &str) -> PathBuf {
        let hex = digest.strip_prefix("sha256:").unwrap_or(digest);
        let (prefix, rest) = hex.split_at(2.min(hex.len()));
        self.objects.join("sha256").join(prefix).join(rest)
    }

    /// Resolve a backend-local ID to the entity that claims it. Backend IDs are
    /// never public identity; this is the only supported lookup.
    pub fn backend_mapping(
        &self,
        project_id: &str,
        snapshot_id: &str,
        adapter_id: &str,
        run_id: &str,
        backend_id: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT entity_id FROM backend_mappings
                 WHERE project_id=?1 AND snapshot_id=?2 AND adapter_id=?3 AND run_id=?4 AND backend_id=?5",
                (project_id, snapshot_id, adapter_id, run_id, backend_id),
                |row| row.get(0),
            )
            .ok())
    }

    pub fn find_entity_by_qualified_name(
        &self,
        project_id: &str,
        snapshot_id: &str,
        qualified_name: &str,
    ) -> Result<Option<serde_json::Value>> {
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_json FROM entities
                 WHERE project_id=?1 AND snapshot_id=?2 AND qualified_name=?3
                 ORDER BY entity_id LIMIT 1",
                (project_id, snapshot_id, qualified_name),
                |row| row.get(0),
            )
            .ok();
        payload
            .map(|text| {
                serde_json::from_str(&text).map_err(|err| {
                    CoreError::Storage(format!("entity payload corrupt: {err}"))
                })
            })
            .transpose()
    }

    pub fn relation_kinds(
        &self,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<std::collections::BTreeSet<String>> {
        let mut statement = self.conn.prepare(
            "SELECT DISTINCT kind FROM relations WHERE project_id=?1 AND snapshot_id=?2",
        )?;
        let rows = statement.query_map((project_id, snapshot_id), |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<std::collections::BTreeSet<String>>>()?)
    }

    /// Stored relation payloads in a snapshot, ordered by id. Used by query
    /// projections and by acceptance tests that must inspect the exact
    /// resolution/basis of each edge.
    pub fn relation_payloads(
        &self,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json FROM relations
             WHERE project_id=?1 AND snapshot_id=?2 ORDER BY relation_id",
        )?;
        let rows = statement.query_map((project_id, snapshot_id), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| {
            let text = row?;
            serde_json::from_str(&text)
                .map_err(|err| CoreError::Storage(format!("relation payload corrupt: {err}")))
        })
        .collect()
    }

    /// Stored evidence payloads in a snapshot, ordered by id.
    pub fn evidence_payloads(
        &self,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let mut statement = self.conn.prepare(
            "SELECT payload_json FROM evidence
             WHERE project_id=?1 AND snapshot_id=?2 ORDER BY evidence_id",
        )?;
        let rows = statement.query_map((project_id, snapshot_id), |row| {
            row.get::<_, String>(0)
        })?;
        rows.map(|row| {
            let text = row?;
            serde_json::from_str(&text)
                .map_err(|err| CoreError::Storage(format!("evidence payload corrupt: {err}")))
        })
        .collect()
    }

    pub fn entity_ids(&self, project_id: &str, snapshot_id: &str) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare(
            "SELECT entity_id FROM entities WHERE project_id=?1 AND snapshot_id=?2 ORDER BY entity_id",
        )?;
        let rows = statement.query_map((project_id, snapshot_id), |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    /// Registered project ids, for single-project read-only bridges. The
    /// caller still has to resolve an explicit project when more than one
    /// exists.
    pub fn list_projects(&self) -> Result<Vec<String>> {
        let mut statement = self.conn.prepare("SELECT project_id FROM projects ORDER BY project_id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<String>>>()?)
    }

    /// Latest snapshot registered for a project. This is a convenience for
    /// inspection; analysis tools must still pass an explicit snapshot id.
    pub fn latest_snapshot_id(&self, project_id: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT snapshot_id FROM snapshots WHERE project_id=?1 ORDER BY rowid DESC LIMIT 1",
                (project_id,),
                |row| row.get(0),
            )
            .ok())
    }

    pub fn snapshot_manifest_digest(
        &self,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT manifest_artifact FROM snapshots WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
                |row| row.get(0),
            )
            .ok())
    }

    pub fn snapshot_payload(
        &self,
        project_id: &str,
        snapshot_id: &str,
    ) -> Result<Option<serde_json::Value>> {
        let payload: Option<String> = self
            .conn
            .query_row(
                "SELECT payload_json FROM snapshots WHERE project_id=?1 AND snapshot_id=?2",
                (project_id, snapshot_id),
                |row| row.get(0),
            )
            .ok();
        payload
            .map(|text| {
                serde_json::from_str(&text)
                    .map_err(|err| CoreError::Storage(format!("snapshot payload corrupt: {err}")))
            })
            .transpose()
    }

    pub fn entity_snapshot_id(
        &self,
        project_id: &str,
        entity_id: &str,
    ) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT snapshot_id FROM entities
                 WHERE project_id=?1 AND entity_id=?2 ORDER BY rowid DESC LIMIT 1",
                (project_id, entity_id),
                |row| row.get(0),
            )
            .ok())
    }

    /// Introspection for acceptance evidence: whether a table exists in this
    /// schema version. It does not create or mutate anything.
    pub fn table_exists(&self, table: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
            (table,),
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    pub fn job_count(&self, project_id: &str) -> Result<u64> {
        let value: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM jobs WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )?;
        Ok(value.max(0) as u64)
    }

    pub fn snapshot_count(&self, project_id: &str) -> Result<u64> {
        let value: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM snapshots WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )?;
        Ok(value.max(0) as u64)
    }

    /// Number of ChangeSet identities in a project. Used to assert that a
    /// rejected plan/prepare path produced no ChangeSet side effect.
    pub fn changeset_count(&self, project_id: &str) -> Result<u64> {
        let value: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT changeset_id) FROM changesets WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )?;
        Ok(value.max(0) as u64)
    }

    pub(crate) fn next_event_sequence(&self, project_id: &str) -> Result<i64> {
        Ok(self.conn.query_row(
            "SELECT COALESCE(MAX(sequence)+1, 0) FROM events WHERE project_id=?1",
            (project_id,),
            |row| row.get(0),
        )?)
    }
}

/// Content identity of a snapshot: the manifest without `id` and `created_at`
/// (S19 §1). Re-capturing identical bytes keeps the same identity even though
/// the timestamp changes.
fn snapshot_identity(value: &serde_json::Value) -> Result<String> {
    let mut identity = value.clone();
    if let Some(object) = identity.as_object_mut() {
        object.remove("id");
        object.remove("created_at");
    }
    hash::canonical_json(&identity)
        .map_err(|err| CoreError::invalid(format!("snapshot identity: {err}")))
}
