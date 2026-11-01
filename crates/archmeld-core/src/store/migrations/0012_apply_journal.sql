-- ARCHMELD schema version 12: writer leases and the durable apply journal.
--
-- The journal is written before any filesystem replacement. File replacement
-- and SQLite are not one atomic transaction, so a crash is detected from the
-- journal, never inferred from the changeset state alone.

CREATE TABLE writer_leases (
    project_id    TEXT PRIMARY KEY,
    owner         TEXT NOT NULL,
    fencing_token INTEGER NOT NULL CHECK (fencing_token >= 1),
    expires_at    INTEGER NOT NULL,
    released_at   INTEGER,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE apply_journal (
    project_id     TEXT NOT NULL,
    changeset_id   TEXT NOT NULL,
    revision       INTEGER NOT NULL,
    sequence       INTEGER NOT NULL CHECK (sequence >= 0),
    op             TEXT NOT NULL,
    path           TEXT NOT NULL,
    destination    TEXT,
    preimage_digest TEXT,
    postimage_digest TEXT,
    phase          TEXT NOT NULL CHECK (phase IN (
        'intent','replace_started','replaced','restore_started','restored','conflict'
    )),
    observed_digest TEXT,
    detail_json    TEXT NOT NULL,
    recorded_at    INTEGER NOT NULL,
    PRIMARY KEY (project_id, changeset_id, revision, sequence)
);

CREATE INDEX apply_journal_by_phase ON apply_journal(project_id, phase);
