-- ARCHMELD schema version 11: ChangeSets, prepared revisions, staging
-- verification and exact-digest approvals.
--
-- A ChangeSet revision is immutable. Re-preparation appends a new revision;
-- an approval names one revision and one prepared_digest, so changing the
-- staging content invalidates the old approval without mutating history.

CREATE TABLE changesets (
    project_id      TEXT NOT NULL,
    changeset_id    TEXT NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision >= 1),
    plan_id         TEXT NOT NULL,
    plan_revision   INTEGER NOT NULL,
    step_id         TEXT NOT NULL,
    base_snapshot_id TEXT NOT NULL,
    contract_digest TEXT NOT NULL,
    state           TEXT NOT NULL,
    prepared_digest TEXT,
    payload_json    TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, changeset_id, revision),
    FOREIGN KEY (project_id, plan_id, plan_revision)
        REFERENCES plans(project_id, plan_id, revision)
);

CREATE INDEX changesets_by_state ON changesets(project_id, state);
CREATE INDEX changesets_by_digest ON changesets(project_id, prepared_digest);

CREATE TABLE change_operations (
    project_id   TEXT NOT NULL,
    changeset_id TEXT NOT NULL,
    revision     INTEGER NOT NULL,
    sequence     INTEGER NOT NULL CHECK (sequence >= 0),
    op           TEXT NOT NULL CHECK (op IN ('create','update','delete','move')),
    path         TEXT NOT NULL,
    destination  TEXT,
    before_digest TEXT,
    before_size  INTEGER,
    after_digest TEXT,
    after_size   INTEGER,
    state        TEXT NOT NULL CHECK (state IN ('pending','applied','failed','restored','conflict')),
    payload_json TEXT NOT NULL,
    PRIMARY KEY (project_id, changeset_id, revision, sequence),
    FOREIGN KEY (project_id, changeset_id, revision)
        REFERENCES changesets(project_id, changeset_id, revision)
);

CREATE TABLE staging_verifications (
    project_id      TEXT NOT NULL,
    changeset_id    TEXT NOT NULL,
    revision        INTEGER NOT NULL,
    verification_id TEXT NOT NULL,
    kind            TEXT NOT NULL,
    status          TEXT NOT NULL,
    evidence_json   TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, changeset_id, revision, verification_id)
);

CREATE TABLE approvals (
    project_id      TEXT NOT NULL,
    approval_id     TEXT NOT NULL,
    target_kind     TEXT NOT NULL CHECK (target_kind IN ('plan_scope','prepared_change')),
    target_id       TEXT NOT NULL,
    target_revision INTEGER NOT NULL,
    target_digest   TEXT NOT NULL,
    actor           TEXT NOT NULL,
    decision        TEXT NOT NULL CHECK (decision IN ('approve','reject')),
    scope_json      TEXT NOT NULL,
    expires_at      INTEGER,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, approval_id)
);

CREATE INDEX approvals_by_target
    ON approvals(project_id, target_kind, target_id, target_digest);
