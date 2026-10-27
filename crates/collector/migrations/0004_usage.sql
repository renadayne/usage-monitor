ALTER TABLE usage_checkpoints ADD COLUMN frozen INTEGER NOT NULL DEFAULT 0;
CREATE TABLE accounting_selections (
 session_id TEXT PRIMARY KEY REFERENCES sessions(id), source TEXT NOT NULL,
 accounting_series TEXT NOT NULL, scope TEXT NOT NULL);
CREATE TABLE usage_evidence (
 event_id TEXT NOT NULL REFERENCES events(event_id), sample_index INTEGER NOT NULL,
 session_id TEXT NOT NULL REFERENCES sessions(id), source TEXT NOT NULL,
 sample_id TEXT NOT NULL, counter_key TEXT NOT NULL, counter_epoch TEXT NOT NULL,
 scope_key TEXT NOT NULL, accounting_series TEXT NOT NULL,
 unit TEXT NOT NULL, metric TEXT NOT NULL, relation_to_total TEXT NOT NULL,
 mode TEXT NOT NULL, revision INTEGER, value REAL NOT NULL, sample_json TEXT NOT NULL,
 disposition TEXT NOT NULL, direct INTEGER NOT NULL,
 PRIMARY KEY(event_id,sample_index), UNIQUE(session_id,source,sample_id,direct));
CREATE INDEX idx_usage_counter ON usage_evidence(session_id,source,counter_key,counter_epoch,scope_key,accounting_series,unit,metric,revision);
