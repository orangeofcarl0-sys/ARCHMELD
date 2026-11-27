-- Copyright 2026 ARCHMELD contributors
-- SPDX-License-Identifier: Apache-2.0

-- ARCHMELD evidence ledger, schema version 1.
-- Authority: contracts/STORAGE_AND_TRANSACTION_MAP.md and specs/S03.
-- SQLite holds facts and provenance; large blobs live in the CAS object store.

CREATE TABLE schema_migrations (
    version    INTEGER PRIMARY KEY,
    applied_at INTEGER NOT NULL
);

CREATE TABLE projects (
    project_id          TEXT PRIMARY KEY,
    mode                TEXT NOT NULL CHECK (mode IN ('manager', 'expedition')),
    revision            INTEGER NOT NULL DEFAULT 0,
    current_snapshot_id TEXT,
    config_digest       TEXT,
    state               TEXT NOT NULL DEFAULT 'open',
    created_at          INTEGER NOT NULL
);

CREATE TABLE artifacts (
    digest     TEXT PRIMARY KEY,
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    media_type TEXT NOT NULL,
    encoding   TEXT NOT NULL CHECK (encoding IN ('utf-8', 'binary')),
    created_at INTEGER NOT NULL
);

CREATE TABLE snapshots (
    snapshot_id        TEXT PRIMARY KEY,
    project_id         TEXT NOT NULL REFERENCES projects(project_id),
    config_digest      TEXT NOT NULL,
    environment_digest TEXT NOT NULL,
    exclusions_digest  TEXT NOT NULL,
    manifest_artifact  TEXT NOT NULL REFERENCES artifacts(digest),
    stable             INTEGER NOT NULL CHECK (stable IN (0, 1)),
    coverage_json      TEXT NOT NULL,
    payload_json       TEXT NOT NULL,
    created_at         INTEGER NOT NULL,
    UNIQUE (project_id, snapshot_id)
);

CREATE TABLE files (
    project_id   TEXT NOT NULL,
    snapshot_id  TEXT NOT NULL,
    path         TEXT NOT NULL,
    blob_digest  TEXT NOT NULL REFERENCES artifacts(digest),
    size_bytes   INTEGER NOT NULL CHECK (size_bytes >= 0),
    role         TEXT NOT NULL,
    origin       TEXT NOT NULL,
    executable   INTEGER NOT NULL CHECK (executable IN (0, 1)),
    PRIMARY KEY (project_id, snapshot_id, path),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE TABLE generations (
    project_id      TEXT NOT NULL,
    snapshot_id     TEXT NOT NULL,
    adapter_id      TEXT NOT NULL,
    run_id          TEXT NOT NULL,
    adapter_version TEXT NOT NULL,
    config_digest   TEXT NOT NULL,
    status          TEXT NOT NULL CHECK (status IN ('staging', 'published', 'failed')),
    final_seq       INTEGER,
    coverage_json   TEXT,
    started_at      INTEGER NOT NULL,
    finished_at     INTEGER,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE TABLE run_batches (
    project_id       TEXT NOT NULL,
    snapshot_id      TEXT NOT NULL,
    adapter_id       TEXT NOT NULL,
    run_id           TEXT NOT NULL,
    sequence         INTEGER NOT NULL CHECK (sequence >= 0),
    payload_digest   TEXT NOT NULL,
    entity_count     INTEGER NOT NULL,
    evidence_count   INTEGER NOT NULL,
    relation_count   INTEGER NOT NULL,
    diagnostic_count INTEGER NOT NULL,
    is_final         INTEGER NOT NULL CHECK (is_final IN (0, 1)),
    coverage_json    TEXT NOT NULL,
    received_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, sequence),
    FOREIGN KEY (project_id, snapshot_id, adapter_id, run_id)
        REFERENCES generations(project_id, snapshot_id, adapter_id, run_id)
);

CREATE TABLE staging_entities (
    project_id    TEXT NOT NULL,
    snapshot_id   TEXT NOT NULL,
    adapter_id    TEXT NOT NULL,
    run_id        TEXT NOT NULL,
    sequence      INTEGER NOT NULL,
    entity_id     TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    payload_json  TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, entity_id)
);

CREATE TABLE staging_evidence (
    project_id     TEXT NOT NULL,
    snapshot_id    TEXT NOT NULL,
    adapter_id     TEXT NOT NULL,
    run_id         TEXT NOT NULL,
    sequence       INTEGER NOT NULL,
    evidence_id    TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    payload_json   TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, evidence_id)
);

CREATE TABLE staging_relations (
    project_id     TEXT NOT NULL,
    snapshot_id    TEXT NOT NULL,
    adapter_id     TEXT NOT NULL,
    run_id         TEXT NOT NULL,
    sequence       INTEGER NOT NULL,
    relation_id    TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    target_id      TEXT NOT NULL,
    kind           TEXT NOT NULL,
    basis          TEXT NOT NULL,
    resolution     TEXT NOT NULL,
    payload_digest TEXT NOT NULL,
    payload_json   TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, relation_id)
);

CREATE TABLE staging_relation_evidence (
    project_id  TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    adapter_id  TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    relation_id TEXT NOT NULL,
    evidence_id TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, relation_id, evidence_id)
);

CREATE TABLE staging_diagnostics (
    project_id  TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    adapter_id  TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    sequence    INTEGER NOT NULL,
    ordinal     INTEGER NOT NULL,
    code        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    message     TEXT NOT NULL,
    entity_id   TEXT,
    path        TEXT,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, sequence, ordinal)
);

CREATE TABLE entities (
    project_id      TEXT NOT NULL,
    snapshot_id     TEXT NOT NULL,
    entity_id       TEXT NOT NULL,
    config_digest   TEXT NOT NULL,
    kind            TEXT NOT NULL,
    language        TEXT NOT NULL,
    qualified_name  TEXT NOT NULL,
    external        INTEGER NOT NULL CHECK (external IN (0, 1)),
    adapter_id      TEXT NOT NULL,
    run_id          TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, entity_id),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE INDEX entities_by_name ON entities(project_id, snapshot_id, qualified_name);
CREATE INDEX entities_by_kind ON entities(project_id, snapshot_id, kind);

CREATE TABLE backend_mappings (
    project_id  TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    adapter_id  TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    backend_id  TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, backend_id),
    FOREIGN KEY (project_id, snapshot_id, entity_id)
        REFERENCES entities(project_id, snapshot_id, entity_id)
);

CREATE TABLE evidence (
    project_id       TEXT NOT NULL,
    snapshot_id      TEXT NOT NULL,
    evidence_id      TEXT NOT NULL,
    config_digest    TEXT NOT NULL,
    kind             TEXT NOT NULL,
    producer_name    TEXT NOT NULL,
    producer_version TEXT NOT NULL,
    artifact_digest  TEXT NOT NULL REFERENCES artifacts(digest),
    coverage_json    TEXT NOT NULL,
    freshness        TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, evidence_id),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE TABLE relations (
    project_id     TEXT NOT NULL,
    snapshot_id    TEXT NOT NULL,
    relation_id    TEXT NOT NULL,
    config_digest  TEXT NOT NULL,
    source_id      TEXT NOT NULL,
    target_id      TEXT NOT NULL,
    kind           TEXT NOT NULL,
    basis          TEXT NOT NULL,
    resolution     TEXT NOT NULL,
    payload_json   TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, relation_id),
    FOREIGN KEY (project_id, snapshot_id, source_id)
        REFERENCES entities(project_id, snapshot_id, entity_id),
    FOREIGN KEY (project_id, snapshot_id, target_id)
        REFERENCES entities(project_id, snapshot_id, entity_id)
);

CREATE INDEX relations_by_source ON relations(project_id, snapshot_id, source_id);
CREATE INDEX relations_by_target ON relations(project_id, snapshot_id, target_id);
CREATE INDEX relations_by_kind ON relations(project_id, snapshot_id, kind);

CREATE TABLE relation_evidence (
    project_id  TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    relation_id TEXT NOT NULL,
    evidence_id TEXT NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, relation_id, evidence_id),
    FOREIGN KEY (project_id, snapshot_id, relation_id)
        REFERENCES relations(project_id, snapshot_id, relation_id),
    FOREIGN KEY (project_id, snapshot_id, evidence_id)
        REFERENCES evidence(project_id, snapshot_id, evidence_id)
);

CREATE TABLE diagnostics (
    project_id  TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    adapter_id  TEXT NOT NULL,
    run_id      TEXT NOT NULL,
    sequence    INTEGER NOT NULL,
    ordinal     INTEGER NOT NULL,
    code        TEXT NOT NULL,
    severity    TEXT NOT NULL,
    message     TEXT NOT NULL,
    entity_id   TEXT,
    path        TEXT,
    PRIMARY KEY (project_id, snapshot_id, adapter_id, run_id, sequence, ordinal)
);

CREATE TABLE events (
    project_id   TEXT NOT NULL,
    sequence     INTEGER NOT NULL,
    event_id     TEXT NOT NULL UNIQUE,
    type         TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, sequence)
);

CREATE TABLE artifact_refs (
    owner_type TEXT NOT NULL,
    owner_id   TEXT NOT NULL,
    digest     TEXT NOT NULL REFERENCES artifacts(digest),
    PRIMARY KEY (owner_type, owner_id, digest)
);
