CREATE TABLE scenario_runs (
    project_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    config_digest TEXT NOT NULL,
    manifest_json TEXT NOT NULL,
    exit_code INTEGER,
    timed_out INTEGER NOT NULL CHECK (timed_out IN (0,1)),
    stdout_digest TEXT,
    stderr_digest TEXT,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, run_id),
    FOREIGN KEY (project_id, snapshot_id) REFERENCES snapshots(project_id, snapshot_id)
);
