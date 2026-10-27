CREATE TABLE quota_projection (
 quota_key TEXT PRIMARY KEY REFERENCES quotas(quota_key),
 source_sequence INTEGER, confidence TEXT NOT NULL,
 event_id TEXT NOT NULL REFERENCES events(event_id));
CREATE TABLE quota_evidence (
 event_id TEXT NOT NULL REFERENCES events(event_id), snapshot_index INTEGER NOT NULL,
 quota_key TEXT NOT NULL, snapshot_json TEXT NOT NULL, disposition TEXT NOT NULL,
 PRIMARY KEY(event_id,snapshot_index));
CREATE INDEX idx_quota_account_observed ON quotas(provider,account_key,observed_at DESC,quota_key);
