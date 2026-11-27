-- ARCHMELD schema version 13: independent verification runs and per-check
-- results. A verification is bound to one resulting snapshot, one frozen
-- contract digest and one toolchain digest. Missing/optional checks are never
-- rewritten into "passed".

CREATE TABLE verification_runs (
    project_id       TEXT NOT NULL,
    verification_id  TEXT NOT NULL,
    changeset_id     TEXT NOT NULL,
    changeset_revision INTEGER NOT NULL,
    snapshot_id      TEXT NOT NULL,
    base_snapshot_id TEXT NOT NULL,
    contract_id      TEXT NOT NULL,
    contract_digest  TEXT NOT NULL,
    toolchain_digest TEXT NOT NULL,
    environment_json TEXT NOT NULL,
    state            TEXT NOT NULL,
    verdict_json     TEXT NOT NULL,
    started_at       INTEGER NOT NULL,
    finished_at      INTEGER,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, verification_id)
);

CREATE INDEX verification_by_changeset
    ON verification_runs(project_id, changeset_id, snapshot_id);

CREATE TABLE check_results (
    project_id     TEXT NOT NULL,
    verification_id TEXT NOT NULL,
    check_id       TEXT NOT NULL,
    kind           TEXT NOT NULL,
    required       INTEGER NOT NULL CHECK (required IN (0,1)),
    state          TEXT NOT NULL,
    command_json   TEXT,
    exit_code      INTEGER,
    timed_out      INTEGER NOT NULL CHECK (timed_out IN (0,1)),
    stdout_digest  TEXT,
    stderr_digest  TEXT,
    started_ms     INTEGER,
    finished_ms    INTEGER,
    artifacts_json TEXT NOT NULL,
    detail_json    TEXT NOT NULL,
    PRIMARY KEY (project_id, verification_id, check_id),
    FOREIGN KEY (project_id, verification_id)
        REFERENCES verification_runs(project_id, verification_id)
        ON DELETE CASCADE
);
