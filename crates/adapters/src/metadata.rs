//! Shared bounded metadata parsing. No filesystem, network or provider configuration access.
use normalized_schema::{
    Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider, ValidationError,
};
use serde::de::DeserializeOwned;
use std::fmt;

#[derive(Debug, Clone)]
pub struct ObservationContext {
    pub integration_id: String,
    pub account_key: String,
    pub event_id: String,
    pub observed_at: String,
    pub received_at: String,
}
#[derive(Debug)]
pub enum MetadataError {
    Invalid(&'static str),
    Validation(ValidationError),
}
impl fmt::Display for MetadataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(reason) => f.write_str(reason),
            Self::Validation(error) => write!(f, "invalid provider metadata: {error}"),
        }
    }
}
impl std::error::Error for MetadataError {}
impl From<ValidationError> for MetadataError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}
pub(crate) fn parse<T: DeserializeOwned>(raw: &str) -> Result<T, MetadataError> {
    if raw.len() > 65_536 {
        return Err(MetadataError::Invalid(
            "metadata payload exceeds 65536 bytes",
        ));
    }
    serde_json::from_str(raw).map_err(|_| MetadataError::Invalid("invalid provider metadata JSON"))
}
impl ObservationContext {
    pub(crate) fn event(
        &self,
        provider: Provider,
        surface: &str,
        source: &str,
        session: String,
        event_type: EventType,
    ) -> NormalizedEvent {
        NormalizedEvent {
            schema_version: 1,
            event_id: self.event_id.clone(),
            event_type,
            integration_id: self.integration_id.clone(),
            account_key: self.account_key.clone(),
            provider,
            surface: surface.into(),
            source: source.into(),
            source_event_id: None,
            source_sequence: None,
            source_session_id: session,
            local_session_id: None,
            source_turn_id: None,
            source_inference_id: None,
            parent_agent_id: None,
            phase_at_start: None,
            phase_epoch: None,
            model_selected: None,
            model_requested: None,
            model_resolved: None,
            model_observed: None,
            model_evidence_kind: ModelEvidenceKind::Unknown,
            evidence_ref: None,
            observed_at: self.observed_at.clone(),
            received_at: self.received_at.clone(),
            evidence_expires_at: None,
            confidence: Confidence::Unknown,
            activity: None,
        }
    }
}
