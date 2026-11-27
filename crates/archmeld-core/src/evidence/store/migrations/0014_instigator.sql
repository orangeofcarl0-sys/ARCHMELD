-- ARCHMELD schema version 14: optional asynchronous Instigator policy.
-- Its messages can only be NUDGE or SILENT, never task creation, approval,
-- state changes or delivery barriers.

CREATE TABLE instigator_config (
    project_id           TEXT PRIMARY KEY,
    enabled              INTEGER NOT NULL CHECK (enabled IN (0,1)),
    cooldown_seconds     INTEGER NOT NULL CHECK (cooldown_seconds >= 0),
    max_nudges_per_task  INTEGER NOT NULL CHECK (max_nudges_per_task >= 0),
    updated_at           INTEGER NOT NULL
);

CREATE TABLE instigator_messages (
    project_id    TEXT NOT NULL,
    message_id    TEXT NOT NULL,
    task_id       TEXT NOT NULL,
    task_revision INTEGER NOT NULL,
    reason        TEXT NOT NULL,
    scope_ref     TEXT NOT NULL,
    decision      TEXT NOT NULL CHECK (decision IN ('NUDGE','SILENT')),
    state         TEXT NOT NULL CHECK (state IN ('pending','delivered','discarded')),
    expires_at    INTEGER NOT NULL,
    created_at    INTEGER NOT NULL,
    delivered_at  INTEGER,
    discard_reason TEXT,
    PRIMARY KEY (project_id, message_id)
);

CREATE INDEX instigator_by_task ON instigator_messages(project_id, task_id, created_at);
