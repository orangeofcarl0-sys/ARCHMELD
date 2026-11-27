-- ARCHMELD schema version 16: count-based snapshot retention.
-- A retention run only deletes snapshots that are outside the newest N and are
-- not referenced by plans, changesets, verification, claims, architectures,
-- drift reports, runtime/scenario evidence, jobs, projects or explicit pins.

CREATE TABLE retention_config (
    project_id      TEXT PRIMARY KEY,
    keep_snapshots  INTEGER NOT NULL CHECK (keep_snapshots >= 1),
    auto_prune      INTEGER NOT NULL CHECK (auto_prune IN (0, 1)),
    updated_at      INTEGER NOT NULL
);

CREATE TABLE retention_runs (
    project_id       TEXT NOT NULL,
    run_id            TEXT NOT NULL,
    mode              TEXT NOT NULL CHECK (mode IN ('manual', 'automatic')),
    keep_snapshots    INTEGER NOT NULL,
    kept_count        INTEGER NOT NULL,
    candidate_count   INTEGER NOT NULL,
    deleted_count     INTEGER NOT NULL,
    details_json      TEXT NOT NULL,
    created_at        INTEGER NOT NULL,
    PRIMARY KEY (project_id, run_id)
);
