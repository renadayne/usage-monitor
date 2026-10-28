//! Pure Antigravity App hook metadata. Invocation metadata never proves the served model.
use crate::metadata::{parse, MetadataError, ObservationContext};
use normalized_schema::{Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy)]
pub enum Hook {
    SessionStart,
    PreInvocation,
    PostInvocation,
    PreToolUse,
    PostToolUse,
    Stop,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Raw {
    conversation_id: String,
    model_name: Option<String>,
    invocation_num: Option<u64>,
    initial_num_steps: Option<u64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Observation {
    pub event: NormalizedEvent,
    pub invocation_num: Option<u64>,
    pub initial_num_steps: Option<u64>,
}

/// Event kind is configured by the caller's hook command; it is not guessed from payload fields.
pub fn normalize_hook(
    raw: &str,
    hook: Hook,
    context: &ObservationContext,
) -> Result<Observation, MetadataError> {
    let raw: Raw = parse(raw)?;
    if matches!(hook, Hook::PreInvocation | Hook::PostInvocation)
        && (raw.invocation_num.is_none() || raw.initial_num_steps.is_none())
    {
        return Err(MetadataError::Invalid(
            "invocation metadata requires invocationNum and initialNumSteps",
        ));
    }
    let kind = if matches!(hook, Hook::SessionStart) {
        EventType::SessionStarted
    } else {
        EventType::ModelObserved
    };
    let mut event = context.event(
        Provider::Antigravity,
        "antigravity_app",
        "antigravity.hooks",
        raw.conversation_id,
        kind,
    );
    if let Some(model) = raw.model_name {
        event.model_selected = Some(model);
        event.model_evidence_kind = ModelEvidenceKind::HookCommonMetadata;
        event.confidence = Confidence::Direct;
        event.evidence_ref = Some(context.event_id.clone());
    }
    // Invocation counters can restart. They are not global sequence/inference identities.
    event.validate()?;
    Ok(Observation {
        event,
        invocation_num: raw.invocation_num,
        initial_num_steps: raw.initial_num_steps,
    })
}

/// Observer hooks have no instruction injection or post-call termination side effects.
pub const OBSERVER_REPLY: &str = "{}";
