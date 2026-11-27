-- ARCHMELD schema version 2: jobs, idempotency records, event routing fields.

CREATE TABLE jobs (
    project_id          TEXT NOT NULL,
    job_id              TEXT NOT NULL,
    operation           TEXT NOT NULL,
    state               TEXT NOT NULL CHECK (state IN
        ('queued', 'running', 'succeeded', 'failed', 'cancelled', 'timed_out', 'blocked')),
    progress            REAL NOT NULL DEFAULT 0 CHECK (progress >= 0 AND progress <= 1),
    result_ids_json     TEXT NOT NULL DEFAULT '[]',
    artifact_digests_json TEXT NOT NULL DEFAULT '[]',
    error_json          TEXT,
    revision            INTEGER NOT NULL DEFAULT 1 CHECK (revision >= 1),
    created_at          INTEGER NOT NULL,
    updated_at          INTEGER NOT NULL,
    PRIMARY KEY (project_id, job_id)
);

CREATE INDEX jobs_by_state ON jobs(project_id, state);

CREATE TABLE idempotency (
    project_id     TEXT NOT NULL,
    operation      TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    status_code    INTEGER NOT NULL,
    result_json    TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    PRIMARY KEY (project_id, operation, idempotency_key)
);

ALTER TABLE events ADD COLUMN job_id TEXT;
ALTER TABLE events ADD COLUMN snapshot_id TEXT;
