-- ARCHMELD schema version 15: drift reports, retention pins and backup runs.
-- Drift is reported, never silently applied; backups are consistent DB copies
-- plus a manifest of reachable CAS objects; pins protect objects from GC.

CREATE TABLE drift_reports (
    project_id       TEXT NOT NULL,
    report_id        TEXT NOT NULL,
    base_snapshot_id TEXT NOT NULL,
    new_snapshot_id  TEXT,
    summary_json     TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, report_id)
);

CREATE TABLE retention_pins (
    project_id TEXT NOT NULL,
    pin_id     TEXT NOT NULL,
    kind       TEXT NOT NULL,
    target     TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, pin_id)
);

CREATE TABLE backup_runs (
    project_id      TEXT NOT NULL,
    backup_id       TEXT NOT NULL,
    directory       TEXT NOT NULL,
    db_digest       TEXT NOT NULL,
    manifest_digest TEXT NOT NULL,
    object_count    INTEGER NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, backup_id)
);
