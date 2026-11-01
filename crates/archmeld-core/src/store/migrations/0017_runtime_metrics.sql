-- ARCHMELD schema version 17: runtime counters and state intervals.
-- Counters and states are captured only from explicit runtime data (probe
-- events, counter tracks or native trace packets). They never create causal
-- edges, observed calls or architecture facts. A metric is attributed to a
-- source entity only through an explicit entity id or probe mapping; absence
-- of such a declaration is recorded as process-scope, not inferred.

ALTER TABLE runtime_runs ADD COLUMN metric_count INTEGER NOT NULL DEFAULT 0;

CREATE TABLE runtime_metrics (
    project_id     TEXT NOT NULL,
    run_id         TEXT NOT NULL,
    metric_id      TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('counter', 'state')),
    name           TEXT NOT NULL,
    unit           TEXT,
    clock_id       TEXT NOT NULL,
    entity_id      TEXT,
    attributes_json TEXT NOT NULL,
    PRIMARY KEY (project_id, run_id, metric_id),
    FOREIGN KEY (project_id, run_id) REFERENCES runtime_runs(project_id, run_id) ON DELETE CASCADE
);

CREATE TABLE runtime_metric_samples (
    project_id TEXT NOT NULL,
    run_id     TEXT NOT NULL,
    metric_id  TEXT NOT NULL,
    seq        INTEGER NOT NULL,
    start_ns   TEXT NOT NULL,
    end_ns     TEXT,
    value_json TEXT NOT NULL,
    process_id TEXT,
    thread_id  TEXT,
    PRIMARY KEY (project_id, run_id, metric_id, seq),
    FOREIGN KEY (project_id, run_id, metric_id)
        REFERENCES runtime_metrics(project_id, run_id, metric_id) ON DELETE CASCADE
);

CREATE INDEX runtime_metric_samples_by_start
    ON runtime_metric_samples(project_id, run_id, start_ns);
