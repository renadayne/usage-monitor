//! Atomic metadata ingestion. Success is returned only after the SQLite commit.

use std::fmt;

use normalized_schema::{
    format_canonical_utc, validate_identifier, Activity, Confidence, EventType, ModelEvidenceKind,
    NormalizedEvent, Phase, Provider,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Store;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct IntegrationRegistration {
    pub id: String,
    pub account_key: String,
    pub provider: Provider,
    pub surface: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IngestOutcome {
    pub event_key: String,
    pub session_id: String,
    pub already_committed: bool,
}

#[derive(Debug)]
pub enum IngestError {
    Validation(normalized_schema::ValidationError),
    Sqlite(rusqlite::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
    IntegrationBindingConflict,
    EventIdentityConflict,
    UnknownIntegration,
    UnknownSession,
    StalePhaseEpoch,
    UnsupportedEvent,
}

impl fmt::Display for IngestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(error) => write!(f, "invalid lifecycle event: {error}"),
            Self::Sqlite(_) => write!(f, "lifecycle transaction failed"),
            Self::Json(_) => write!(f, "invalid normalized event encoding"),
            Self::Invalid(reason) => write!(f, "invalid lifecycle event: {reason}"),
            Self::IntegrationBindingConflict => write!(f, "integration binding conflict"),
            Self::EventIdentityConflict => write!(f, "event identity has conflicting content"),
            Self::UnknownIntegration => write!(f, "integration is not registered"),
            Self::UnknownSession => write!(f, "session is not known"),
            Self::StalePhaseEpoch => write!(f, "phase epoch conflict"),
            Self::UnsupportedEvent => write!(f, "event is not supported by lifecycle ingestion"),
        }
    }
}
impl std::error::Error for IngestError {}
impl From<rusqlite::Error> for IngestError {
    fn from(e: rusqlite::Error) -> Self {
        Self::Sqlite(e)
    }
}
impl From<serde_json::Error> for IngestError {
    fn from(e: serde_json::Error) -> Self {
        Self::Json(e)
    }
}
impl From<normalized_schema::ValidationError> for IngestError {
    fn from(e: normalized_schema::ValidationError) -> Self {
        Self::Validation(e)
    }
}

impl Store {
    /// Register a synthetic/local binding. This does not install hooks or verify a provider.
    pub fn register_integration(
        &mut self,
        registration: &IntegrationRegistration,
    ) -> Result<(), IngestError> {
        validate_identifier("integrationId", &registration.id)?;
        validate_identifier("accountKey", &registration.account_key)?;
        validate_identifier("surface", &registration.surface)?;
        let provider = enum_text(&registration.provider)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let existing: Option<(String, String, String)> = tx
            .query_row(
                "SELECT account_key,provider,surface FROM integrations WHERE id=?1",
                [&registration.id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match existing {
            Some((account, p, surface))
                if account == registration.account_key
                    && p == provider
                    && surface == registration.surface => {}
            Some(_) => return Err(IngestError::IntegrationBindingConflict),
            None => {
                tx.execute("INSERT INTO integrations(id,provider,surface,account_key,capabilities_json,health,enabled) VALUES (?1,?2,?3,?4,'[]','unknown',1)",params![registration.id,provider,registration.surface,registration.account_key])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Observer entry point. A source label cannot grant authority to change phases.
    pub fn ingest(&mut self, event: &NormalizedEvent) -> Result<IngestOutcome, IngestError> {
        self.ingest_metadata(event, None, None)
    }

    /// Trusted local user-action entry point for a future desktop IPC handler.
    /// Hook/HTTP observer handlers must call `ingest`, never this method.
    pub fn change_phase(
        &mut self,
        event: &NormalizedEvent,
        expected_epoch: i64,
    ) -> Result<IngestOutcome, IngestError> {
        if event.event_type != EventType::PhaseChanged
            || event.source != "aum.user"
            || expected_epoch < 0
        {
            return Err(IngestError::Invalid(
                "phase change requires a local user action",
            ));
        }
        self.ingest_metadata(event, Some(expected_epoch), None)
    }

    pub(crate) fn ingest_metadata(
        &mut self,
        event: &NormalizedEvent,
        expected_epoch: Option<i64>,
        samples: Option<&[normalized_schema::usage::UsageSample]>,
    ) -> Result<IngestOutcome, IngestError> {
        let times = event.validate()?;
        if (event.event_type == EventType::UsageReported) != samples.is_some() {
            return Err(IngestError::UnsupportedEvent);
        }
        if event.event_type == EventType::PhaseChanged && expected_epoch.is_none() {
            return Err(IngestError::Invalid("observer events cannot change phase"));
        }
        if event.event_type == EventType::PhaseChanged
            && (event.model_selected.is_some()
                || event.model_requested.is_some()
                || event.model_resolved.is_some()
                || event.model_observed.is_some()
                || event.model_evidence_kind != ModelEvidenceKind::Unknown)
        {
            return Err(IngestError::Invalid(
                "phase changes cannot assert model evidence",
            ));
        }
        let event_type = enum_text(&event.event_type)?;
        let event_key = namespaced_key(&[
            "event",
            &event.integration_id,
            &event.account_key,
            &event.source,
            event.source_event_id.as_deref().unwrap_or(&event.event_id),
            &event_type,
        ]);
        let session_id = namespaced_key(&[
            "session",
            &event.integration_id,
            &event.account_key,
            &event.source_session_id,
        ]);
        if event
            .local_session_id
            .as_ref()
            .is_some_and(|id| id != &session_id)
        {
            return Err(IngestError::Invalid(
                "local session identity does not match its source binding",
            ));
        }
        let mut canonical = event.clone();
        canonical.event_id = event_key.clone();
        canonical.local_session_id = Some(session_id.clone());
        canonical.observed_at = format_canonical_utc(times.observed_at_utc);
        canonical.received_at = format_canonical_utc(times.received_at_utc);
        canonical.evidence_expires_at = times.evidence_expires_at_utc.map(format_canonical_utc);
        let payload = if let Some(samples) = samples {
            serde_json::to_string(&normalized_schema::usage::UsageReport {
                event: canonical.clone(),
                samples: samples.to_vec(),
            })?
        } else {
            serde_json::to_string(&canonical)?
        };
        let provider = enum_text(&event.provider)?;
        let confidence = enum_text(&event.confidence)?;
        let phase = event.phase_at_start.as_ref().map(enum_text).transpose()?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding: Option<(String, String, String)> = tx
            .query_row(
                "SELECT account_key,provider,surface FROM integrations WHERE id=?1",
                [&event.integration_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match binding {
            None => return Err(IngestError::UnknownIntegration),
            Some((account, p, surface))
                if account != event.account_key || p != provider || surface != event.surface =>
            {
                return Err(IngestError::IntegrationBindingConflict)
            }
            Some(_) => {}
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM events WHERE event_id=?1",
                [&event_key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = existing {
            let mut previous: NormalizedEvent = if let Some(samples) = samples {
                let prior: normalized_schema::usage::UsageReport = serde_json::from_str(&previous)?;
                if prior.samples != samples {
                    return Err(IngestError::EventIdentityConflict);
                }
                prior.event
            } else {
                serde_json::from_str(&previous)?
            };
            previous.received_at = canonical.received_at.clone();
            if previous != canonical {
                return Err(IngestError::EventIdentityConflict);
            }
            tx.commit()?;
            return Ok(IngestOutcome {
                event_key,
                session_id,
                already_committed: true,
            });
        }
        let session: Option<(String, i64)> = tx
            .query_row(
                "SELECT phase,phase_epoch FROM sessions WHERE id=?1",
                [&session_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if session.is_some() {
            let projection_exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM session_projection WHERE session_id=?1)",
                [&session_id],
                |r| r.get(0),
            )?;
            if !projection_exists {
                return Err(IngestError::Invalid(
                    "session projection checkpoint is missing",
                ));
            }
        }
        if event.event_type == EventType::SessionStarted {
            if !matches!(event.phase_at_start, None | Some(Phase::Unassigned))
                || !matches!(event.phase_epoch, None | Some(0))
            {
                return Err(IngestError::Invalid(
                    "session creation must start unassigned at epoch zero",
                ));
            }
            if session.is_none() {
                if matches!(
                    event.activity,
                    Some(
                        Activity::Completed
                            | Activity::Failed
                            | Activity::Interrupted
                            | Activity::Disconnected
                    )
                ) {
                    return Err(IngestError::Invalid(
                        "session start cannot assert a terminal activity",
                    ));
                }
                let activity = if event.confidence == Confidence::Direct {
                    event.activity.unwrap_or(Activity::Unknown)
                } else {
                    Activity::Unknown
                };
                tx.execute("INSERT INTO sessions(id,integration_id,source_session_id,started_at,updated_at,activity) VALUES (?1,?2,?3,?4,?4,?5)",params![session_id,event.integration_id,event.source_session_id,canonical.observed_at,enum_text(&activity)?])?;
                tx.execute("INSERT INTO session_projection(session_id,activity_source,activity_sequence,phase_changed_at) VALUES (?1,?2,?3,?4)",params![session_id,event.source,event.source_sequence,canonical.observed_at])?;
            }
            // A repeated start/resume with a new event ID is historical evidence, never a reset.
        } else {
            let Some((current_phase, current_epoch)) = session else {
                return Err(IngestError::UnknownSession);
            };
            if event.event_type == EventType::PhaseChanged {
                let expected = expected_epoch.ok_or(IngestError::StalePhaseEpoch)?;
                let next = expected
                    .checked_add(1)
                    .ok_or(IngestError::StalePhaseEpoch)?;
                if current_epoch != expected || event.phase_epoch != Some(next) {
                    return Err(IngestError::StalePhaseEpoch);
                }
                tx.execute("UPDATE sessions SET phase=?1,phase_epoch=?2,updated_at=?3,activity='UNKNOWN',model_selected=NULL,model_requested=NULL,model_resolved=NULL,model_observed=NULL,model_evidence_kind='unknown',model_evidence=NULL,model_observed_at=NULL,model_evidence_expires_at=NULL WHERE id=?4",params![phase,next,canonical.received_at,session_id])?;
                tx.execute(
                    "UPDATE session_projection SET phase_changed_at=?1 WHERE session_id=?2",
                    params![canonical.received_at, session_id],
                )?;
                tx.execute(
                    "DELETE FROM model_projection WHERE session_id=?1",
                    [&session_id],
                )?;
            } else if event.event_type == EventType::SessionEnded {
                let (source,sequence,boundary):(String,Option<i64>,String)=tx.query_row("SELECT activity_source,activity_sequence,phase_changed_at FROM session_projection WHERE session_id=?1",[&session_id],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?;
                let epoch_matches = event.phase_epoch == Some(current_epoch)
                    || (current_epoch == 0 && event.phase_epoch.is_none());
                let phase_matches = phase.as_ref().is_none_or(|p| p == &current_phase);
                let ordered = source == event.source
                    && event
                        .source_sequence
                        .is_some_and(|new| sequence.is_none_or(|old| new > old));
                let terminal = match event.activity {
                    None => Activity::Completed,
                    Some(
                        a @ (Activity::Completed
                        | Activity::Failed
                        | Activity::Interrupted
                        | Activity::Disconnected
                        | Activity::Unknown),
                    ) => a,
                    Some(_) => {
                        return Err(IngestError::Invalid(
                            "session end cannot assert a nonterminal activity",
                        ))
                    }
                };
                if ordered
                    && epoch_matches
                    && phase_matches
                    && canonical.observed_at >= boundary
                    && event.confidence == Confidence::Direct
                {
                    tx.execute(
                        "UPDATE sessions SET activity=?1,updated_at=?2,ended_at=?2 WHERE id=?3",
                        params![enum_text(&terminal)?, canonical.observed_at, session_id],
                    )?;
                    tx.execute(
                        "UPDATE session_projection SET activity_sequence=?1 WHERE session_id=?2",
                        params![event.source_sequence, session_id],
                    )?;
                }
            }
        }
        tx.execute("INSERT INTO events(event_id,integration_id,account_key,session_id,type,source,source_event_id,source_sequence,confidence,observed_at,received_at,source_turn_id,source_inference_id,phase_at_start,phase_epoch,payload_json) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",params![event_key,event.integration_id,event.account_key,session_id,event_type,event.source,event.source_event_id,event.source_sequence,confidence,canonical.observed_at,canonical.received_at,event.source_turn_id,event.source_inference_id,phase,event.phase_epoch,payload])?;
        if let Some(samples) = samples {
            crate::usage::apply(&tx, &canonical, &session_id, samples)?;
        } else {
            crate::model_projection::apply(&tx, &canonical, &session_id)?;
        }
        let marker = serde_json::json!({"eventId":event_key,"sessionId":session_id}).to_string();
        tx.execute(
            "UPDATE integrations SET last_seen_at=MAX(COALESCE(last_seen_at,''),?1) WHERE id=?2",
            params![canonical.received_at, event.integration_id],
        )?;
        tx.execute(
            "INSERT INTO outbox(id,event_id,kind,payload_json) VALUES (?1,?1,'session.changed',?2)",
            params![event_key, marker],
        )?;
        tx.commit()?;
        Ok(IngestOutcome {
            event_key,
            session_id,
            already_committed: false,
        })
    }
}

fn enum_text(value: &impl Serialize) -> Result<String, IngestError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or(IngestError::Invalid("expected a named enum"))
}

pub(crate) fn namespaced_key(parts: &[&str]) -> String {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update((part.len() as u64).to_be_bytes());
        hash.update(part.as_bytes());
    }
    format!("{:x}", hash.finalize())
}
