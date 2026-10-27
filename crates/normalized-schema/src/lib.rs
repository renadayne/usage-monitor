//! Strict normalized lifecycle and model-evidence telemetry schema (Step T02.2a).

use std::fmt;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
pub mod usage;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_IDENTIFIER_LEN: usize = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Codex,
    Antigravity,
    Copilot,
    ClaudeCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Activity {
    Working,
    WaitingUser,
    Idle,
    Completed,
    Failed,
    Interrupted,
    Disconnected,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Phase {
    Unassigned,
    Planning,
    Implementation,
    Review,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelEvidenceKind {
    ServedResponse,
    RequestAttribute,
    SessionSelection,
    HookCommonMetadata,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Direct,
    Inferred,
    #[default]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EventType {
    #[serde(rename = "session.started")]
    SessionStarted,
    #[serde(rename = "session.ended")]
    SessionEnded,
    #[serde(rename = "turn.started")]
    TurnStarted,
    #[serde(rename = "turn.ended")]
    TurnEnded,
    #[serde(rename = "inference.started")]
    InferenceStarted,
    #[serde(rename = "inference.ended")]
    InferenceEnded,
    #[serde(rename = "model.observed")]
    ModelObserved,
    #[serde(rename = "phase.changed")]
    PhaseChanged,
    #[serde(rename = "usage.reported")]
    UsageReported,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizedEvent {
    pub schema_version: u32,
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: EventType,
    pub integration_id: String,
    pub account_key: String,
    pub provider: Provider,
    pub surface: String,
    pub source: String,
    #[serde(default)]
    pub source_event_id: Option<String>,
    #[serde(default)]
    pub source_sequence: Option<i64>,
    pub source_session_id: String,
    #[serde(default)]
    pub local_session_id: Option<String>,
    #[serde(default)]
    pub source_turn_id: Option<String>,
    #[serde(default)]
    pub source_inference_id: Option<String>,
    #[serde(default)]
    pub parent_agent_id: Option<String>,
    #[serde(default)]
    pub phase_at_start: Option<Phase>,
    #[serde(default)]
    pub phase_epoch: Option<i64>,
    #[serde(default)]
    pub model_selected: Option<String>,
    #[serde(default)]
    pub model_requested: Option<String>,
    #[serde(default)]
    pub model_resolved: Option<String>,
    #[serde(default)]
    pub model_observed: Option<String>,
    #[serde(rename = "modelEvidenceKind", alias = "evidenceKind")]
    pub model_evidence_kind: ModelEvidenceKind,
    #[serde(default)]
    pub evidence_ref: Option<String>,
    pub observed_at: String,
    pub received_at: String,
    #[serde(default)]
    pub evidence_expires_at: Option<String>,
    #[serde(default)]
    pub confidence: Confidence,
    #[serde(default)]
    pub activity: Option<Activity>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedTimestamps {
    pub observed_at_utc: DateTime<Utc>,
    pub received_at_utc: DateTime<Utc>,
    pub evidence_expires_at_utc: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationError {
    InvalidUsage(&'static str),
    UnsupportedSchemaVersion(u32),
    InvalidIdentifier {
        field: &'static str,
        reason: &'static str,
    },
    NegativeSequenceOrEpoch {
        field: &'static str,
        value: i64,
    },
    InvalidUtcTimestamp {
        field: &'static str,
    },
    MissingEventField {
        event_type: EventType,
        field: &'static str,
    },
    InvalidServedModelEvidence {
        reason: &'static str,
    },
    ForbiddenEvidencePromotion {
        kind: ModelEvidenceKind,
        field: &'static str,
    },
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUsage(reason) => write!(f, "invalid usage: {reason}"),
            Self::UnsupportedSchemaVersion(v) => write!(f, "unsupported schemaVersion {v}"),
            Self::InvalidIdentifier { field, reason } => {
                write!(f, "invalid identifier '{field}': {reason}")
            }
            Self::NegativeSequenceOrEpoch { field, value } => {
                write!(f, "{field} must be non-negative, got {value}")
            }
            Self::InvalidUtcTimestamp { field, .. } => {
                write!(f, "invalid canonical UTC timestamp in '{field}'")
            }
            Self::MissingEventField { event_type, field } => {
                write!(f, "event {:?} requires '{field}'", event_type)
            }
            Self::InvalidServedModelEvidence { reason } => {
                write!(f, "invalid served model evidence: {reason}")
            }
            Self::ForbiddenEvidencePromotion { kind, field } => {
                write!(f, "evidence kind {:?} cannot populate '{field}'", kind)
            }
        }
    }
}

impl std::error::Error for ValidationError {}

impl NormalizedEvent {
    pub fn validate(&self) -> Result<ValidatedTimestamps, ValidationError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(ValidationError::UnsupportedSchemaVersion(
                self.schema_version,
            ));
        }

        validate_id("eventId", &self.event_id)?;
        validate_id("integrationId", &self.integration_id)?;
        validate_id("accountKey", &self.account_key)?;
        validate_id("surface", &self.surface)?;
        validate_id("source", &self.source)?;
        validate_id("sourceSessionId", &self.source_session_id)?;

        validate_opt_id("sourceEventId", self.source_event_id.as_deref())?;
        validate_opt_id("localSessionId", self.local_session_id.as_deref())?;
        validate_opt_id("sourceTurnId", self.source_turn_id.as_deref())?;
        validate_opt_id("sourceInferenceId", self.source_inference_id.as_deref())?;
        validate_opt_id("parentAgentId", self.parent_agent_id.as_deref())?;
        validate_opt_id("modelSelected", self.model_selected.as_deref())?;
        validate_opt_id("modelRequested", self.model_requested.as_deref())?;
        validate_opt_id("modelResolved", self.model_resolved.as_deref())?;
        validate_opt_id("modelObserved", self.model_observed.as_deref())?;
        validate_opt_id("evidenceRef", self.evidence_ref.as_deref())?;

        if let Some(seq) = self.source_sequence {
            if seq < 0 {
                return Err(ValidationError::NegativeSequenceOrEpoch {
                    field: "sourceSequence",
                    value: seq,
                });
            }
        }
        if let Some(epoch) = self.phase_epoch {
            if epoch < 0 {
                return Err(ValidationError::NegativeSequenceOrEpoch {
                    field: "phaseEpoch",
                    value: epoch,
                });
            }
        }

        let observed_at_utc = parse_canonical_utc("observedAt", &self.observed_at)?;
        let received_at_utc = parse_canonical_utc("receivedAt", &self.received_at)?;
        let evidence_expires_at_utc = match self.evidence_expires_at.as_deref() {
            Some(raw) => Some(parse_canonical_utc("evidenceExpiresAt", raw)?),
            None => None,
        };

        self.validate_event_type_requirements()?;
        self.validate_model_evidence()?;

        Ok(ValidatedTimestamps {
            observed_at_utc,
            received_at_utc,
            evidence_expires_at_utc,
        })
    }

    fn validate_event_type_requirements(&self) -> Result<(), ValidationError> {
        match self.event_type {
            EventType::TurnStarted | EventType::TurnEnded if self.source_turn_id.is_none() => {
                Err(ValidationError::MissingEventField {
                    event_type: self.event_type,
                    field: "sourceTurnId",
                })
            }
            EventType::InferenceStarted | EventType::InferenceEnded
                if self.source_inference_id.is_none() =>
            {
                Err(ValidationError::MissingEventField {
                    event_type: self.event_type,
                    field: "sourceInferenceId",
                })
            }
            EventType::PhaseChanged if self.phase_at_start.is_none() => {
                Err(ValidationError::MissingEventField {
                    event_type: self.event_type,
                    field: "phaseAtStart",
                })
            }
            EventType::PhaseChanged if self.phase_epoch.is_none() => {
                Err(ValidationError::MissingEventField {
                    event_type: self.event_type,
                    field: "phaseEpoch",
                })
            }
            _ => Ok(()),
        }
    }

    fn validate_model_evidence(&self) -> Result<(), ValidationError> {
        if self.model_observed.is_some() {
            if self.model_evidence_kind != ModelEvidenceKind::ServedResponse {
                return Err(ValidationError::ForbiddenEvidencePromotion {
                    kind: self.model_evidence_kind,
                    field: "modelObserved",
                });
            }
            if self.source_inference_id.is_none() {
                return Err(ValidationError::InvalidServedModelEvidence {
                    reason: "modelObserved requires sourceInferenceId",
                });
            }
            if self.evidence_ref.is_none() {
                return Err(ValidationError::InvalidServedModelEvidence {
                    reason: "modelObserved requires evidenceRef",
                });
            }
            if self.phase_epoch.is_none() {
                return Err(ValidationError::InvalidServedModelEvidence {
                    reason: "modelObserved requires phaseEpoch",
                });
            }
            if self.confidence != Confidence::Direct {
                return Err(ValidationError::InvalidServedModelEvidence {
                    reason: "modelObserved requires direct response evidence",
                });
            }
        }

        match self.model_evidence_kind {
            ModelEvidenceKind::ServedResponse => {
                if self.model_observed.is_none() {
                    return Err(ValidationError::InvalidServedModelEvidence {
                        reason: "served_response requires modelObserved",
                    });
                }
            }
            ModelEvidenceKind::SessionSelection | ModelEvidenceKind::HookCommonMetadata => {
                if self.model_requested.is_some() {
                    return Err(ValidationError::ForbiddenEvidencePromotion {
                        kind: self.model_evidence_kind,
                        field: "modelRequested",
                    });
                }
                if self.model_resolved.is_some() {
                    return Err(ValidationError::ForbiddenEvidencePromotion {
                        kind: self.model_evidence_kind,
                        field: "modelResolved",
                    });
                }
            }
            ModelEvidenceKind::RequestAttribute => {
                if self.model_requested.is_none() {
                    return Err(ValidationError::ForbiddenEvidencePromotion {
                        kind: self.model_evidence_kind,
                        field: "modelRequested",
                    });
                }
            }
            ModelEvidenceKind::Unknown => {}
        }

        Ok(())
    }
}

fn validate_opt_id(field: &'static str, value: Option<&str>) -> Result<(), ValidationError> {
    if let Some(v) = value {
        validate_id(field, v)?;
    }
    Ok(())
}

pub fn validate_identifier(field: &'static str, value: &str) -> Result<(), ValidationError> {
    validate_id(field, value)
}

fn validate_id(field: &'static str, value: &str) -> Result<(), ValidationError> {
    if value.is_empty() {
        return Err(ValidationError::InvalidIdentifier {
            field,
            reason: "must not be empty",
        });
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        return Err(ValidationError::InvalidIdentifier {
            field,
            reason: "exceeds 256 bytes",
        });
    }
    if value.trim() != value || value.chars().any(char::is_control) {
        return Err(ValidationError::InvalidIdentifier {
            field,
            reason: "contains leading/trailing whitespace or control characters",
        });
    }
    Ok(())
}

pub fn parse_canonical_utc(
    field: &'static str,
    raw: &str,
) -> Result<DateTime<Utc>, ValidationError> {
    let has_utc_suffix = raw.len() <= 35 && (raw.ends_with('Z') || raw.ends_with("+00:00"));
    if !has_utc_suffix || !raw.contains('T') {
        return Err(ValidationError::InvalidUtcTimestamp { field });
    }
    let parsed = DateTime::parse_from_rfc3339(raw)
        .map_err(|_| ValidationError::InvalidUtcTimestamp { field })?;
    if parsed.offset().local_minus_utc() != 0 {
        return Err(ValidationError::InvalidUtcTimestamp { field });
    }
    Ok(parsed.with_timezone(&Utc))
}

pub fn format_canonical_utc(dt: DateTime<Utc>) -> String {
    dt.to_rfc3339_opts(SecondsFormat::Nanos, true)
}
