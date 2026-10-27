//! Synthetic-tested Claude Code hooks/status-line normalization only.
use crate::metadata::{parse, MetadataError, ObservationContext};
use normalized_schema::{
    quota::{QuotaSnapshot, QuotaUnit},
    Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider,
};
use serde::{de::IgnoredAny, Deserialize, Serialize};

#[derive(Deserialize)]
struct Hook {
    session_id: String,
    hook_event_name: String,
    model: Option<String>,
    to_model: Option<String>,
    agent_id: Option<IgnoredAny>,
}
pub fn normalize_hook(
    raw: &str,
    context: &ObservationContext,
) -> Result<NormalizedEvent, MetadataError> {
    let raw: Hook = parse(raw)?;
    if raw.agent_id.is_some() {
        return Err(MetadataError::Invalid("subagent binding is unavailable"));
    }
    let (kind, model) = match raw.hook_event_name.as_str() {
        "SessionStart" => (EventType::SessionStarted, raw.model),
        "SessionEnd" => (EventType::SessionEnded, None),
        "PostModelSwitch" => (
            EventType::ModelObserved,
            Some(
                raw.to_model
                    .ok_or(MetadataError::Invalid("PostModelSwitch requires to_model"))?,
            ),
        ),
        "UserPromptSubmit" | "Stop" | "PreToolUse" | "PostToolUse" | "PermissionRequest" => {
            (EventType::ModelObserved, None)
        }
        _ => return Err(MetadataError::Invalid("unsupported Claude Code hook event")),
    };
    let mut event = context.event(
        Provider::ClaudeCode,
        "claude_code_cli",
        "claude.hooks",
        raw.session_id,
        kind,
    );
    if let Some(model) = model {
        event.model_selected = Some(model);
        event.model_evidence_kind = ModelEvidenceKind::SessionSelection;
        event.evidence_ref = Some(context.event_id.clone());
        event.confidence = Confidence::Direct;
    }
    event.validate()?;
    Ok(event)
}

#[derive(Deserialize)]
struct Model {
    id: Option<String>,
}
#[derive(Deserialize)]
struct StatusLine {
    session_id: String,
    model: Option<Model>,
    rate_limits: Option<Limits>,
    agent_id: Option<IgnoredAny>,
    task: Option<IgnoredAny>,
    tasks: Option<IgnoredAny>,
}
#[derive(Deserialize)]
struct Limits {
    five_hour: Option<Window>,
    seven_day: Option<Window>,
}
#[derive(Deserialize)]
struct Window {
    used_percentage: Option<f64>,
    resets_at: Option<u64>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusObservation {
    pub event: NormalizedEvent,
    pub quotas: Vec<QuotaSnapshot>,
}

pub fn normalize_statusline(
    raw: &str,
    context: &ObservationContext,
) -> Result<StatusObservation, MetadataError> {
    let raw: StatusLine = parse(raw)?;
    if raw.agent_id.is_some() || raw.task.is_some() || raw.tasks.is_some() {
        return Err(MetadataError::Invalid(
            "subagent status-line binding is unavailable",
        ));
    }
    let mut event = context.event(
        Provider::ClaudeCode,
        "claude_code_cli",
        "claude.statusline",
        raw.session_id,
        EventType::ModelObserved,
    );
    if let Some(model) = raw.model.and_then(|m| m.id) {
        event.model_selected = Some(model);
        event.model_evidence_kind = ModelEvidenceKind::SessionSelection;
        event.evidence_ref = Some(context.event_id.clone());
        event.confidence = Confidence::Direct;
    }
    event.validate()?;
    let mut quotas = Vec::new();
    if let Some(limits) = raw.rate_limits {
        for (bucket, window) in [
            ("five_hour", limits.five_hour),
            ("seven_day", limits.seven_day),
        ] {
            let Some(window) = window else {
                continue;
            };
            let resets_at = window
                .resets_at
                .map(|seconds| {
                    let seconds = i64::try_from(seconds)
                        .map_err(|_| MetadataError::Invalid("quota reset is out of range"))?;
                    let time = chrono::DateTime::from_timestamp(seconds, 0)
                        .ok_or(MetadataError::Invalid("quota reset is out of range"))?;
                    Ok::<_, MetadataError>(normalized_schema::format_canonical_utc(time))
                })
                .transpose()?;
            // Without reset identity, separate deliveries cannot claim to be the same window.
            let window_key = window
                .resets_at
                .map_or_else(|| context.event_id.clone(), |value| value.to_string());
            let snapshot = QuotaSnapshot {
                quota_key: bucket.into(),
                provider: Provider::ClaudeCode,
                account_key: context.account_key.clone(),
                scope: "account".into(),
                bucket: bucket.into(),
                unit: QuotaUnit::Percentage,
                window_key,
                window_start: None,
                window_end: None,
                resets_at,
                used: None,
                limit: None,
                remaining: None,
                used_percent: window.used_percentage,
                observed_at: context.observed_at.clone(),
                stale_after: None,
            };
            snapshot
                .validate()
                .map_err(|_| MetadataError::Invalid("invalid Claude Code quota metadata"))?;
            quotas.push(snapshot);
        }
    }
    Ok(StatusObservation { event, quotas })
}
