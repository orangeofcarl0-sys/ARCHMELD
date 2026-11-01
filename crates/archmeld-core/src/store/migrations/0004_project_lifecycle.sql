-- ARCHMELD schema version 4: project roots and lifecycle sessions.

ALTER TABLE projects ADD COLUMN config_json TEXT;

CREATE TABLE project_roots (
    project_id TEXT NOT NULL REFERENCES projects(project_id),
    root_path  TEXT NOT NULL,
    first_seen INTEGER NOT NULL,
    last_seen  INTEGER NOT NULL,
    active     INTEGER NOT NULL CHECK (active IN (0, 1)),
    PRIMARY KEY (project_id, root_path)
);

CREATE TABLE sessions (
    session_id     TEXT PRIMARY KEY,
    project_id     TEXT NOT NULL REFERENCES projects(project_id),
    mode           TEXT NOT NULL CHECK (mode IN ('manager', 'expedition')),
    state          TEXT NOT NULL CHECK (state IN ('open', 'closed', 'promoted', 'interrupted')),
    workspace_root TEXT NOT NULL,
    state_root     TEXT NOT NULL,
    owner_pid      INTEGER NOT NULL,
    owner_token    TEXT NOT NULL,
    created_at     INTEGER NOT NULL,
    closed_at      INTEGER,
    promoted_to    TEXT
);

CREATE INDEX sessions_by_project ON sessions(project_id, created_at);
