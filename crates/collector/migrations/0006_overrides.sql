CREATE UNIQUE INDEX idx_override_unrevoked ON policy_overrides(policy_id,session_id,phase_epoch) WHERE revoked_at IS NULL;
CREATE INDEX idx_override_active ON policy_overrides(session_id,phase_epoch,revoked_at,expires_at);
CREATE INDEX idx_override_history ON policy_overrides(session_id,created_at DESC,id);
