CREATE TABLE integrations (
 id TEXT PRIMARY KEY, provider TEXT NOT NULL, surface TEXT NOT NULL,
 account_key TEXT NOT NULL, version TEXT, target_verification TEXT NOT NULL DEFAULT 'unknown',
 capabilities_json TEXT NOT NULL, health TEXT NOT NULL, last_seen_at TEXT,
 installed_config_hash TEXT, enabled INTEGER NOT NULL);
CREATE TABLE sessions (
 id TEXT PRIMARY KEY, integration_id TEXT NOT NULL REFERENCES integrations(id),
 source_session_id TEXT, workspace_key TEXT, parent_session_id TEXT,
 started_at TEXT, updated_at TEXT, ended_at TEXT, activity TEXT NOT NULL,
 activity_evidence TEXT, model_selected TEXT, model_requested TEXT, model_resolved TEXT,
 model_observed TEXT, model_evidence_kind TEXT NOT NULL DEFAULT 'unknown',
 model_evidence TEXT, model_observed_at TEXT, model_evidence_expires_at TEXT,
 phase TEXT NOT NULL DEFAULT 'UNASSIGNED', phase_epoch INTEGER NOT NULL DEFAULT 0);
CREATE TABLE events (
 event_id TEXT PRIMARY KEY, integration_id TEXT NOT NULL REFERENCES integrations(id),
 account_key TEXT NOT NULL, session_id TEXT REFERENCES sessions(id), type TEXT NOT NULL,
 source TEXT NOT NULL, source_event_id TEXT, source_sequence INTEGER,
 confidence TEXT NOT NULL, observed_at TEXT NOT NULL, received_at TEXT NOT NULL,
 source_turn_id TEXT, source_inference_id TEXT, phase_at_start TEXT, phase_epoch INTEGER,
 payload_json TEXT NOT NULL);
CREATE INDEX idx_event_session_time ON events(session_id, observed_at, event_id);
CREATE TABLE usage_checkpoints (
 integration_id TEXT NOT NULL REFERENCES integrations(id), account_key TEXT NOT NULL,
 counter_key TEXT NOT NULL, counter_epoch TEXT NOT NULL, scope_key TEXT NOT NULL,
 accounting_series TEXT NOT NULL, unit TEXT NOT NULL, metric TEXT NOT NULL,
 last_revision INTEGER NOT NULL, last_value REAL NOT NULL, accepted_total REAL NOT NULL,
 baseline_value REAL NOT NULL, coverage_start TEXT NOT NULL,
 last_event_id TEXT NOT NULL REFERENCES events(event_id),
 PRIMARY KEY(integration_id, account_key, counter_key, counter_epoch, scope_key, accounting_series, unit, metric));
CREATE TABLE usage_contributions (
 event_id TEXT NOT NULL REFERENCES events(event_id), sample_index INTEGER NOT NULL,
 accounting_series TEXT NOT NULL, unit TEXT NOT NULL, metric TEXT NOT NULL,
 accepted_delta REAL NOT NULL, PRIMARY KEY(event_id, sample_index));
CREATE TABLE quotas (
 quota_key TEXT PRIMARY KEY, provider TEXT NOT NULL, account_key TEXT NOT NULL,
 scope TEXT NOT NULL, bucket TEXT NOT NULL, unit TEXT NOT NULL, window_key TEXT NOT NULL,
 source_integration_id TEXT NOT NULL REFERENCES integrations(id), source TEXT NOT NULL,
 window_start TEXT, window_end TEXT, used REAL, limit_value REAL, remaining REAL,
 used_percent REAL, resets_at TEXT, observed_at TEXT NOT NULL, stale_after TEXT);
CREATE TABLE policies (
 id TEXT PRIMARY KEY, scope_json TEXT NOT NULL, rules_json TEXT NOT NULL,
 action TEXT NOT NULL, enabled INTEGER NOT NULL, priority INTEGER NOT NULL);
CREATE TABLE policy_overrides (
 id TEXT PRIMARY KEY, policy_id TEXT NOT NULL REFERENCES policies(id),
 session_id TEXT NOT NULL REFERENCES sessions(id), phase_epoch INTEGER NOT NULL,
 replacement_models_json TEXT NOT NULL, reason TEXT NOT NULL, created_at TEXT NOT NULL,
 expires_at TEXT NOT NULL, revoked_at TEXT);
CREATE TABLE violations (
 id TEXT PRIMARY KEY, policy_id TEXT NOT NULL REFERENCES policies(id),
 session_id TEXT NOT NULL REFERENCES sessions(id), event_id TEXT NOT NULL REFERENCES events(event_id),
 observed_model TEXT, model_evidence_kind TEXT NOT NULL, expected_json TEXT,
 phase TEXT, phase_epoch INTEGER, created_at TEXT NOT NULL, acknowledged_at TEXT, resolution TEXT,
 UNIQUE(policy_id, event_id, phase_epoch));
CREATE TABLE outbox (
 id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES events(event_id), kind TEXT NOT NULL,
 payload_json TEXT NOT NULL, delivered_at TEXT);
