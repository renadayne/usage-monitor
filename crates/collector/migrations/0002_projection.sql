CREATE TABLE session_projection (session_id TEXT PRIMARY KEY REFERENCES sessions(id), activity_source TEXT NOT NULL, activity_sequence INTEGER, phase_changed_at TEXT NOT NULL);
INSERT INTO session_projection(session_id,activity_source,activity_sequence,phase_changed_at) SELECT id,'legacy',NULL,COALESCE(updated_at,started_at,'1970-01-01T00:00:00.000000000Z') FROM sessions;
