-- ARCHMELD schema version 5: workspace handle registry for the local API.
-- A handle is created by the trusted server from an allowlisted workspace; the
-- API never accepts an arbitrary filesystem path.

CREATE TABLE workspace_handles (
    handle     TEXT PRIMARY KEY,
    project_id TEXT NOT NULL REFERENCES projects(project_id),
    root_path  TEXT NOT NULL,
    created_at INTEGER NOT NULL
);
