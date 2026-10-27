CREATE TABLE inference_context (
 session_id TEXT NOT NULL REFERENCES sessions(id), source TEXT NOT NULL,
 inference_id TEXT NOT NULL, phase TEXT NOT NULL, phase_epoch INTEGER NOT NULL,
 started_at TEXT NOT NULL, start_sequence INTEGER,
 start_event_id TEXT NOT NULL REFERENCES events(event_id),
 PRIMARY KEY(session_id,source,inference_id));
CREATE TABLE model_projection (
 session_id TEXT NOT NULL REFERENCES sessions(id), channel TEXT NOT NULL,
 source TEXT NOT NULL, source_sequence INTEGER, observed_at TEXT NOT NULL,
 phase_epoch INTEGER NOT NULL, event_id TEXT NOT NULL REFERENCES events(event_id),
 PRIMARY KEY(session_id,channel));
