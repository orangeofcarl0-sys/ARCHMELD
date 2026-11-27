-- ARCHMELD schema version 9: claims, investigations and candidate architectures.
-- Model/tool output is proposal-only: no row here is an analysis fact and no
-- architecture import may insert entities or relations into the evidence ledger.

CREATE TABLE claims (
    project_id       TEXT NOT NULL,
    claim_id         TEXT NOT NULL,
    snapshot_id      TEXT NOT NULL,
    config_digest    TEXT NOT NULL,
    revision         INTEGER NOT NULL CHECK (revision >= 1),
    claim_type       TEXT NOT NULL,
    text             TEXT NOT NULL,
    status           TEXT NOT NULL,
    scope            TEXT NOT NULL,
    producer_json    TEXT NOT NULL,
    support_ids_json TEXT NOT NULL,
    counter_ids_json TEXT NOT NULL,
    questions_json   TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, claim_id, revision),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE INDEX claims_by_status ON claims(project_id, status);
CREATE INDEX claims_by_snapshot ON claims(project_id, snapshot_id);

CREATE TABLE claim_reviews (
    project_id  TEXT NOT NULL,
    claim_id    TEXT NOT NULL,
    revision    INTEGER NOT NULL,
    action      TEXT NOT NULL,
    reason      TEXT NOT NULL,
    actor_json  TEXT NOT NULL,
    created_at  INTEGER NOT NULL,
    PRIMARY KEY (project_id, claim_id, revision),
    FOREIGN KEY (project_id, claim_id, revision)
        REFERENCES claims(project_id, claim_id, revision)
);

CREATE TABLE investigations (
    project_id       TEXT NOT NULL,
    investigation_id TEXT NOT NULL,
    snapshot_id      TEXT NOT NULL,
    config_digest    TEXT NOT NULL,
    revision         INTEGER NOT NULL CHECK (revision >= 1),
    question         TEXT NOT NULL,
    scope            TEXT NOT NULL,
    status           TEXT NOT NULL,
    facts_json       TEXT NOT NULL,
    hypotheses_json  TEXT NOT NULL,
    missing_json     TEXT NOT NULL,
    experiments_json TEXT NOT NULL,
    conclusions_json TEXT NOT NULL,
    residual_json    TEXT NOT NULL,
    payload_json     TEXT NOT NULL,
    created_at       INTEGER NOT NULL,
    PRIMARY KEY (project_id, investigation_id, revision),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE TABLE architectures (
    project_id      TEXT NOT NULL,
    architecture_id TEXT NOT NULL,
    snapshot_id     TEXT NOT NULL,
    config_digest   TEXT NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision >= 1),
    layer           TEXT NOT NULL,
    status          TEXT NOT NULL,
    producer_json   TEXT NOT NULL,
    artifact_digest TEXT REFERENCES artifacts(digest),
    commands_ignored INTEGER NOT NULL DEFAULT 0,
    payload_json    TEXT NOT NULL,
    verification_json TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, architecture_id, revision),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);

CREATE INDEX architectures_by_status ON architectures(project_id, status);

CREATE TABLE architecture_reviews (
    project_id      TEXT NOT NULL,
    architecture_id TEXT NOT NULL,
    revision        INTEGER NOT NULL,
    action          TEXT NOT NULL,
    reason          TEXT NOT NULL,
    actor_json      TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, architecture_id, revision),
    FOREIGN KEY (project_id, architecture_id, revision)
        REFERENCES architectures(project_id, architecture_id, revision)
);
