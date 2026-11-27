-- ARCHMELD schema version 6: normalized runtime traces.
-- Nanosecond timestamps are decimal TEXT so values above 2^53-1 survive.

CREATE TABLE runtime_runs (
    project_id   TEXT NOT NULL,
    run_id       TEXT NOT NULL,
    snapshot_id  TEXT,
    config_digest TEXT NOT NULL,
    build_identity_json TEXT NOT NULL,
    capture_tool TEXT NOT NULL,
    capture_version TEXT NOT NULL,
    clock_kind   TEXT NOT NULL,
    clock_unit   TEXT NOT NULL CHECK (clock_unit = 'ns'),
    clock_origin_ns TEXT NOT NULL,
    lossy        INTEGER NOT NULL CHECK (lossy IN (0,1)),
    loss_notes_json TEXT NOT NULL,
    artifact_digest TEXT REFERENCES artifacts(digest),
    bound        INTEGER NOT NULL CHECK (bound IN (0,1)),
    span_count   INTEGER NOT NULL,
    created_at   INTEGER NOT NULL,
    PRIMARY KEY (project_id, run_id)
);

CREATE TABLE runtime_spans (
    project_id TEXT NOT NULL,
    run_id     TEXT NOT NULL,
    span_id    TEXT NOT NULL,
    parent_id  TEXT,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,
    start_ns   TEXT NOT NULL,
    end_ns     TEXT NOT NULL,
    process_id TEXT,
    thread_id  TEXT,
    attributes_json TEXT NOT NULL,
    PRIMARY KEY (project_id, run_id, span_id),
    FOREIGN KEY (project_id, run_id) REFERENCES runtime_runs(project_id, run_id) ON DELETE CASCADE
);

CREATE INDEX runtime_spans_by_start ON runtime_spans(project_id, run_id, start_ns);
