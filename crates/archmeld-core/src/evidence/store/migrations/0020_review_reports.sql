CREATE TABLE IF NOT EXISTS review_reports (
    project_id TEXT NOT NULL,
    report_id TEXT NOT NULL,
    base_snapshot_id TEXT NOT NULL,
    candidate_snapshot_id TEXT,
    mode TEXT NOT NULL,
    output_digest TEXT NOT NULL,
    output_json TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    PRIMARY KEY (project_id, report_id)
);

CREATE INDEX IF NOT EXISTS review_reports_by_base
    ON review_reports (project_id, base_snapshot_id);

CREATE INDEX IF NOT EXISTS review_reports_by_candidate
    ON review_reports (project_id, candidate_snapshot_id);
