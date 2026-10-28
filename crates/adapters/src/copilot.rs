//! Completed OTLP JSON span parsing only; no listener, telemetry setting writes or served proof.
use crate::metadata::{parse, MetadataError, ObservationContext};
use normalized_schema::{
    usage::{
        RelationToTotal, UsageMetric, UsageMode, UsageReport, UsageSample, UsageScope, UsageUnit,
    },
    Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Span {
    trace_id: String,
    span_id: String,
    start_time_unix_nano: String,
    end_time_unix_nano: String,
    attributes: Vec<Attribute>,
}
#[derive(Deserialize)]
struct Attribute {
    key: String,
    value: Value,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpanObservation {
    pub operation: String,
    pub event: NormalizedEvent,
    pub usage: Option<UsageReport>,
}

/// `session` must come from an explicit verified trace-to-conversation binding by the caller.
/// A leaf span is never joined to a session by model, timestamp or another heuristic.
pub fn normalize_completed_span(
    raw: &str,
    session: &str,
    context: &ObservationContext,
) -> Result<SpanObservation, MetadataError> {
    let raw: Span = parse(raw)?;
    normalized_schema::validate_identifier("sourceSessionId", session)?;
    for (value, length) in [(&raw.trace_id, 32), (&raw.span_id, 16)] {
        if value.len() != length
            || !value.bytes().all(|b| b.is_ascii_hexdigit())
            || value.bytes().all(|b| b == b'0')
        {
            return Err(MetadataError::Invalid("invalid OTLP trace/span identity"));
        }
    }
    let start = raw
        .start_time_unix_nano
        .parse::<u64>()
        .map_err(|_| MetadataError::Invalid("invalid span start time"))?;
    let end = raw
        .end_time_unix_nano
        .parse::<u64>()
        .map_err(|_| MetadataError::Invalid("invalid span end time"))?;
    if start == 0 || end < start {
        return Err(MetadataError::Invalid(
            "span is incomplete or time order is invalid",
        ));
    }
    let observed = chrono::DateTime::from_timestamp(
        (end / 1_000_000_000) as i64,
        (end % 1_000_000_000) as u32,
    )
    .ok_or(MetadataError::Invalid("span time is out of range"))?;
    if raw.attributes.len() > 256 {
        return Err(MetadataError::Invalid("too many span attributes"));
    }
    let mut attributes = BTreeMap::new();
    for attribute in raw.attributes {
        if matches!(
            attribute.key.as_str(),
            "gen_ai.operation.name"
                | "gen_ai.conversation.id"
                | "gen_ai.request.model"
                | "gen_ai.response.model"
                | "gen_ai.usage.input_tokens"
                | "gen_ai.usage.output_tokens"
                | "gen_ai.usage.cache_read.input_tokens"
                | "gen_ai.usage.cache_creation.input_tokens"
                | "gen_ai.usage.reasoning.output_tokens"
        ) {
            if attribute
                .value
                .as_object()
                .is_none_or(|value| value.len() != 1)
            {
                return Err(MetadataError::Invalid("invalid OTLP attribute value"));
            }
            if attributes.insert(attribute.key, attribute.value).is_some() {
                return Err(MetadataError::Invalid(
                    "duplicate recognized span attribute",
                ));
            }
        }
    }
    let string = |key: &str| -> Result<Option<String>, MetadataError> {
        attributes
            .get(key)
            .map(|value| {
                value
                    .get("stringValue")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or(MetadataError::Invalid("expected OTLP string attribute"))
            })
            .transpose()
    };
    let operation =
        string("gen_ai.operation.name")?.ok_or(MetadataError::Invalid("missing span operation"))?;
    if !matches!(operation.as_str(), "chat" | "invoke_agent") {
        return Err(MetadataError::Invalid("unsupported span operation"));
    }
    if string("gen_ai.conversation.id")?
        .as_deref()
        .is_some_and(|id| id != session)
    {
        return Err(MetadataError::Invalid("span conversation binding conflict"));
    }
    let identity = format!(
        "{}:{}",
        raw.trace_id.to_ascii_lowercase(),
        raw.span_id.to_ascii_lowercase()
    );
    let mut event = context.event(
        Provider::Copilot,
        "copilot_vscode",
        "copilot.otel",
        session.into(),
        EventType::ModelObserved,
    );
    event.source_event_id = Some(identity.clone());
    event.observed_at = normalized_schema::format_canonical_utc(observed);
    event.model_requested = string("gen_ai.request.model")?;
    event.model_resolved = string("gen_ai.response.model")?;
    if event.model_requested.is_some() {
        event.model_evidence_kind = ModelEvidenceKind::RequestAttribute;
    }
    event.evidence_ref = Some(identity.clone());
    event.confidence = Confidence::Direct;
    event.validate()?;
    let mut usage_event = event.clone();
    usage_event.event_type = EventType::UsageReported;
    usage_event.model_requested = None;
    usage_event.model_resolved = None;
    usage_event.model_evidence_kind = ModelEvidenceKind::Unknown;
    let leaf = operation == "chat";
    if leaf {
        usage_event.source_inference_id = Some(identity.clone());
    }
    let mut samples = Vec::new();
    for (key, metric) in [
        ("gen_ai.usage.input_tokens", UsageMetric::Input),
        ("gen_ai.usage.output_tokens", UsageMetric::Output),
        (
            "gen_ai.usage.cache_read.input_tokens",
            UsageMetric::CacheRead,
        ),
        (
            "gen_ai.usage.cache_creation.input_tokens",
            UsageMetric::CacheWrite,
        ),
        (
            "gen_ai.usage.reasoning.output_tokens",
            UsageMetric::Reasoning,
        ),
    ] {
        let Some(raw) = attributes.get(key) else {
            continue;
        };
        let value = raw
            .get("intValue")
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_u64())
            })
            .filter(|value| *value <= 9_007_199_254_740_991)
            .ok_or(MetadataError::Invalid(
                "token count exceeds exact nonnegative integer range",
            ))?;
        samples.push(UsageSample {
            sample_id: format!("{identity}:{}", metric.as_str()),
            mode: if leaf {
                UsageMode::Delta
            } else {
                UsageMode::Cumulative
            },
            scope: if leaf {
                UsageScope::Inference
            } else {
                UsageScope::Session
            },
            scope_key: if leaf {
                identity.clone()
            } else {
                session.into()
            },
            counter_key: metric.as_str().into(),
            counter_epoch: identity.clone(),
            revision: None,
            unit: UsageUnit::Tokens,
            metric,
            value: value as f64,
            accounting_series: if leaf {
                "copilot.chat.tokens"
            } else {
                "copilot.session.snapshot"
            }
            .into(),
            relation_to_total: RelationToTotal::Unknown,
            known_zero: false,
        });
    }
    let usage = if samples.is_empty() {
        None
    } else {
        let report = UsageReport {
            event: usage_event,
            samples,
        };
        report.validate()?;
        Some(report)
    };
    Ok(SpanObservation {
        operation,
        event,
        usage,
    })
}
