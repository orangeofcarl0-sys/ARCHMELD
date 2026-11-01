-- ARCHMELD schema version 8: causal links, clock domains and causal report.
-- Only explicit parent/flow/message/task identifiers form observed causal edges;
-- temporal adjacency is never stored as causality.

ALTER TABLE runtime_runs ADD COLUMN clock_domains_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE runtime_runs ADD COLUMN causal_json TEXT NOT NULL DEFAULT '{}';

ALTER TABLE runtime_spans ADD COLUMN clock_id TEXT NOT NULL DEFAULT 'default';

CREATE TABLE runtime_links (
    project_id     TEXT NOT NULL,
    run_id         TEXT NOT NULL,
    link_id        TEXT NOT NULL,
    source_span_id TEXT NOT NULL,
    target_span_id TEXT NOT NULL,
    kind           TEXT NOT NULL,
    correlation_id TEXT,
    cross_clock    INTEGER NOT NULL CHECK (cross_clock IN (0,1)),
    attributes_json TEXT NOT NULL,
    PRIMARY KEY (project_id, run_id, link_id),
    FOREIGN KEY (project_id, run_id) REFERENCES runtime_runs(project_id, run_id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, run_id, source_span_id)
        REFERENCES runtime_spans(project_id, run_id, span_id) ON DELETE CASCADE,
    FOREIGN KEY (project_id, run_id, target_span_id)
        REFERENCES runtime_spans(project_id, run_id, span_id) ON DELETE CASCADE
);

CREATE INDEX runtime_links_by_source ON runtime_links(project_id, run_id, source_span_id);
CREATE INDEX runtime_links_by_target ON runtime_links(project_id, run_id, target_span_id);
CREATE INDEX runtime_links_by_correlation ON runtime_links(project_id, run_id, correlation_id);
