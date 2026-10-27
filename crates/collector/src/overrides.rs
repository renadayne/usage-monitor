//! Explicit local policy exceptions, with durable reason, expiry and revocation history.
use crate::{ingest::IngestError, policies::MAX_ALLOWED_MODELS, Store};
use normalized_schema::{format_canonical_utc, parse_canonical_utc, validate_identifier};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OverrideRequest {
    pub id: String,
    pub policy_id: String,
    pub session_id: String,
    pub expected_phase_epoch: i64,
    pub replacement_model_ids: Vec<String>,
    pub reason: String,
    pub duration_seconds: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OverrideRecord {
    pub id: String,
    pub policy_id: String,
    pub session_id: String,
    pub phase_epoch: i64,
    pub replacement_model_ids: Vec<String>,
    pub reason: String,
    pub created_at: String,
    pub expires_at: String,
    pub revoked_at: Option<String>,
}

impl Store {
    /// Local user action only. An exception never supplies model evidence or provider control.
    pub fn create_override(
        &mut self,
        request: &OverrideRequest,
        now: &str,
        clock_trusted: bool,
    ) -> Result<(), IngestError> {
        if !clock_trusted {
            return Err(IngestError::Invalid(
                "clock is not trusted for override authorization",
            ));
        }
        for (field, value) in [
            ("overrideId", &request.id),
            ("policyId", &request.policy_id),
            ("sessionId", &request.session_id),
        ] {
            validate_identifier(field, value)?;
        }
        if request.expected_phase_epoch < 0
            || request.duration_seconds == 0
            || request.duration_seconds > 86_400
        {
            return Err(IngestError::Invalid(
                "override requires a nonnegative epoch and duration of 1..86400 seconds",
            ));
        }
        if request.reason.trim().is_empty()
            || request.reason.len() > 512
            || request.reason.chars().any(char::is_control)
        {
            return Err(IngestError::Invalid(
                "override requires a bounded single-line reason",
            ));
        }
        if !(1..=MAX_ALLOWED_MODELS).contains(&request.replacement_model_ids.len()) {
            return Err(IngestError::Invalid(
                "override requires 1..32 exact model IDs",
            ));
        }
        let mut models = request.replacement_model_ids.clone();
        for model in &models {
            validate_identifier("replacementModelId", model)?;
        }
        models.sort();
        models.dedup();
        let time = parse_canonical_utc("now", now)?;
        let expires = time
            .checked_add_signed(chrono::TimeDelta::seconds(request.duration_seconds.into()))
            .ok_or(IngestError::Invalid("override expiry is out of range"))?;
        let now = format_canonical_utc(time);
        let expires = format_canonical_utc(expires);
        // Runtime-checked transaction permits read-only Store views on this same connection.
        let tx = Transaction::new_unchecked(&self.conn, TransactionBehavior::Immediate)?;
        let epoch: Option<i64> = tx
            .query_row(
                "SELECT phase_epoch FROM sessions WHERE id=?1",
                [&request.session_id],
                |r| r.get(0),
            )
            .optional()?;
        match epoch {
            None => return Err(IngestError::UnknownSession),
            Some(epoch) if epoch != request.expected_phase_epoch => {
                return Err(IngestError::StalePhaseEpoch)
            }
            _ => {}
        }
        let evaluation = self.evaluate_session_policy(&request.session_id, &now, true)?;
        if !evaluation.winning_policy_ids.contains(&request.policy_id) {
            return Err(IngestError::Invalid(
                "override policy must currently win for this session and phase",
            ));
        }
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM policy_overrides WHERE id=?1)",
            [&request.id],
            |r| r.get(0),
        )?;
        if exists {
            return Err(IngestError::EventIdentityConflict);
        }
        tx.execute("UPDATE policy_overrides SET revoked_at=?1 WHERE policy_id=?2 AND session_id=?3 AND phase_epoch=?4 AND revoked_at IS NULL",params![now,request.policy_id,request.session_id,request.expected_phase_epoch])?;
        tx.execute("INSERT INTO policy_overrides(id,policy_id,session_id,phase_epoch,replacement_models_json,reason,created_at,expires_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",params![request.id,request.policy_id,request.session_id,request.expected_phase_epoch,serde_json::to_string(&models)?,request.reason.trim(),now,expires])?;
        tx.commit()?;
        Ok(())
    }

    /// Revocation removes authorization even when clock trust is lost.
    pub fn revoke_override(
        &mut self,
        id: &str,
        session: &str,
        now: &str,
    ) -> Result<(), IngestError> {
        validate_identifier("overrideId", id)?;
        validate_identifier("sessionId", session)?;
        let now = format_canonical_utc(parse_canonical_utc("now", now)?);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM policy_overrides WHERE id=?1 AND session_id=?2)",
            params![id, session],
            |r| r.get(0),
        )?;
        if !exists {
            return Err(IngestError::Invalid(
                "override does not belong to this session",
            ));
        }
        tx.execute("UPDATE policy_overrides SET revoked_at=COALESCE(revoked_at,?1) WHERE id=?2 AND session_id=?3",params![now,id,session])?;
        tx.commit()?;
        Ok(())
    }

    pub fn override_history(&self, session: &str) -> Result<Vec<OverrideRecord>, IngestError> {
        validate_identifier("sessionId", session)?;
        let mut stmt=self.conn.prepare("SELECT id,policy_id,session_id,phase_epoch,replacement_models_json,reason,created_at,expires_at,revoked_at FROM policy_overrides WHERE session_id=?1 ORDER BY created_at DESC,id ASC LIMIT 200")?;
        let mut rows = stmt.query([session])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            result.push(OverrideRecord {
                id: row.get(0)?,
                policy_id: row.get(1)?,
                session_id: row.get(2)?,
                phase_epoch: row.get(3)?,
                replacement_model_ids: serde_json::from_str(&row.get::<_, String>(4)?)?,
                reason: row.get(5)?,
                created_at: row.get(6)?,
                expires_at: row.get(7)?,
                revoked_at: row.get(8)?,
            });
        }
        Ok(result)
    }

    pub(crate) fn active_overrides(
        &self,
        session: &str,
        epoch: u64,
        now: &str,
    ) -> Result<Vec<policy_engine::Override>, IngestError> {
        let mut stmt=self.conn.prepare("SELECT id,policy_id,replacement_models_json,reason,created_at,expires_at FROM policy_overrides WHERE session_id=?1 AND phase_epoch=?2 AND revoked_at IS NULL AND created_at<=?3 AND expires_at>?3 LIMIT 501")?;
        let mut rows = stmt.query(params![session, epoch, now])?;
        let mut result = Vec::new();
        while let Some(row) = rows.next()? {
            if result.len() == 500 {
                return Err(IngestError::Invalid("too many active overrides"));
            }
            result.push(policy_engine::Override {
                id: row.get(0)?,
                policy_id: row.get(1)?,
                session_id: session.into(),
                phase_epoch: epoch,
                replacement_model_ids: serde_json::from_str(&row.get::<_, String>(2)?)?,
                reason: row.get(3)?,
                created_at_ms: parse_canonical_utc("createdAt", &row.get::<_, String>(4)?)?
                    .timestamp_millis(),
                expires_at_ms: parse_canonical_utc("expiresAt", &row.get::<_, String>(5)?)?
                    .timestamp_millis(),
                revoked_at_ms: None,
            });
        }
        Ok(result)
    }
}
