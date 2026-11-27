-- ARCHMELD schema version 3: event retention watermark.
-- Without a watermark, pruning every event would erase the evidence that a
-- cursor is stale, and a reconnecting client could silently miss events.

CREATE TABLE event_retention (
    project_id      TEXT PRIMARY KEY,
    oldest_sequence INTEGER NOT NULL CHECK (oldest_sequence >= 0)
);
