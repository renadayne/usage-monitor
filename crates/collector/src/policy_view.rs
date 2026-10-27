//! Local advisory policy evaluation using the committed model projection.
use crate::{ingest::IngestError, Store};
use normalized_schema::{parse_canonical_utc, NormalizedEvent};
use policy_engine::{Context, Evaluation, Evidence};
use rusqlite::{params, OptionalExtension};

impl Store {
    /// Metadata proof selected by durable projection, never arbitrary latest raw history.
    pub fn current_model_evidence(
        &self,
        session: &str,
    ) -> Result<Option<NormalizedEvent>, IngestError> {
        normalized_schema::validate_identifier("sessionId", session)?;
        let body:Option<String>=self.conn.query_row("SELECT e.payload_json FROM model_projection p JOIN events e ON e.event_id=p.event_id
            WHERE p.session_id=?1 ORDER BY CASE p.channel WHEN 'served' THEN 0 WHEN 'request' THEN 1 ELSE 2 END LIMIT 1",[session],|r|r.get(0)).optional()?;
        body.map(|body| {
            let event: NormalizedEvent = serde_json::from_str(&body)?;
            event.validate()?;
            if event.local_session_id.as_deref() != Some(session) {
                return Err(IngestError::Invalid(
                    "model evidence session binding is invalid",
                ));
            }
            Ok(event)
        })
        .transpose()
    }

    /// Read-only dry run. The caller supplies trusted local time and explicit clock health.
    pub fn evaluate_session_policy(
        &self,
        session: &str,
        now: &str,
        clock_trusted: bool,
    ) -> Result<Evaluation, IngestError> {
        normalized_schema::validate_identifier("sessionId", session)?;
        let now = parse_canonical_utc("now", now)?;
        let context:Option<(String,String,i64,Option<String>)>=self.conn.query_row("SELECT i.provider,s.phase,s.phase_epoch,s.workspace_key FROM sessions s JOIN integrations i ON i.id=s.integration_id WHERE s.id=?1",
            [session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let Some((provider, phase, epoch, project_key)) = context else {
            return Err(IngestError::UnknownSession);
        };
        let context = Context {
            provider: serde_json::from_value(serde_json::Value::String(provider))?,
            project_key,
            session_id: session.into(),
            phase: serde_json::from_value(serde_json::Value::String(phase))?,
            phase_epoch: u64::try_from(epoch)
                .map_err(|_| IngestError::Invalid("invalid stored phase epoch"))?,
            clock_trusted,
        };
        let event = self.current_model_evidence(session)?;
        let evidence = event
            .as_ref()
            .map(|event| -> Result<Evidence, IngestError> {
                let times = event.validate()?;
                Ok(Evidence {
                    provider: event.provider,
                    session_id: session.into(),
                    model_id: event
                        .model_observed
                        .as_ref()
                        .or(event.model_requested.as_ref())
                        .or(event.model_selected.as_ref())
                        .cloned()
                        .unwrap_or_default(),
                    kind: event.model_evidence_kind,
                    confidence: event.confidence,
                    inference_id: event.source_inference_id.clone(),
                    evidence_ref: event.evidence_ref.clone(),
                    phase: event.phase_at_start,
                    phase_epoch: event.phase_epoch.and_then(|e| u64::try_from(e).ok()),
                    observed_at_ms: times.observed_at_utc.timestamp_millis(),
                    expires_at_ms: times
                        .evidence_expires_at_utc
                        .map(|t| t.timestamp_millis())
                        .unwrap_or(i64::MIN),
                })
            })
            .transpose()?;
        Ok(policy_engine::evaluate(
            &self.list_policies()?,
            &context,
            evidence.as_ref(),
            &self.active_overrides(
                session,
                context.phase_epoch,
                &normalized_schema::format_canonical_utc(now),
            )?,
            now.timestamp_millis(),
        ))
    }

    /// Safe current session lookup for a local UI action. No raw payload or workspace paths.
    pub fn session_summary(
        &self,
        session: &str,
    ) -> Result<Option<crate::query::SessionSummary>, IngestError> {
        normalized_schema::validate_identifier("sessionId", session)?;
        Ok(self.conn.query_row("SELECT id,integration_id,source_session_id,activity,phase,phase_epoch,updated_at,
            model_selected,model_requested,model_observed,model_evidence_kind,model_observed_at,model_evidence_expires_at,model_resolved
            FROM sessions WHERE id=?1",params![session],crate::query::session_from_row).optional()?)
    }
}
