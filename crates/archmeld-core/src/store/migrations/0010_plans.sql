-- ARCHMELD schema version 10: frozen behavior contracts and plans.
-- A plan is not a ChangeSet: no source mutation or staging table is created.

CREATE TABLE behavior_contracts (
    project_id   TEXT NOT NULL,
    contract_id  TEXT NOT NULL,
    revision     INTEGER NOT NULL CHECK (revision >= 1),
    kind         TEXT NOT NULL,
    digest       TEXT NOT NULL,
    frozen       INTEGER NOT NULL CHECK (frozen IN (0,1)),
    payload_json TEXT NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, contract_id, revision)
);

CREATE INDEX contracts_by_digest ON behavior_contracts(project_id, digest);

CREATE TABLE plans (
    project_id      TEXT NOT NULL,
    plan_id         TEXT NOT NULL,
    revision        INTEGER NOT NULL CHECK (revision >= 1),
    contract_id     TEXT NOT NULL,
    contract_digest TEXT NOT NULL,
    target_architecture_id TEXT NOT NULL,
    base_snapshot_id TEXT NOT NULL,
    status          TEXT NOT NULL,
    payload_json    TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, plan_id, revision)
);

CREATE INDEX plans_by_contract ON plans(project_id, contract_id);

CREATE TABLE plan_approvals (
    project_id      TEXT NOT NULL,
    plan_id         TEXT NOT NULL,
    revision        INTEGER NOT NULL,
    contract_digest TEXT NOT NULL,
    actor           TEXT NOT NULL,
    decision        TEXT NOT NULL,
    created_at      INTEGER NOT NULL,
    PRIMARY KEY (project_id, plan_id, revision),
    FOREIGN KEY (project_id, plan_id, revision)
        REFERENCES plans(project_id, plan_id, revision)
);
