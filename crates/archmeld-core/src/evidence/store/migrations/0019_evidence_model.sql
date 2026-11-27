-- Evidence model 1.1: entity role, relation site/multiplicity/producer
-- resolution, git content identity, and index-run provenance.

ALTER TABLE entities ADD COLUMN role TEXT NOT NULL DEFAULT 'other';

ALTER TABLE relations ADD COLUMN call_site_line INTEGER;
ALTER TABLE relations ADD COLUMN producer_resolution TEXT;
ALTER TABLE relations ADD COLUMN occurrence_count INTEGER NOT NULL DEFAULT 1;

ALTER TABLE files ADD COLUMN git_blob TEXT;

CREATE INDEX IF NOT EXISTS entities_by_role
    ON entities (project_id, snapshot_id, role);

CREATE TABLE IF NOT EXISTS index_runs (
    project_id TEXT NOT NULL,
    snapshot_id TEXT NOT NULL,
    run_id TEXT NOT NULL,
    cli_commit TEXT,
    cli_dirty INTEGER NOT NULL DEFAULT 0,
    helper_commit TEXT,
    helper_dirty INTEGER,
    export_schema TEXT,
    index_config_json TEXT NOT NULL DEFAULT '{}',
    parse_json TEXT NOT NULL DEFAULT '{}',
    transport_json TEXT NOT NULL DEFAULT '{}',
    coverage_json TEXT NOT NULL DEFAULT '{}',
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, snapshot_id, run_id)
);

CREATE INDEX IF NOT EXISTS index_runs_latest
    ON index_runs (project_id, snapshot_id, created_at);
