use adapters::{copilot, metadata::ObservationContext};
use normalized_schema::{
    usage::{UsageMode, UsageScope},
    ModelEvidenceKind,
};
fn context() -> ObservationContext {
    ObservationContext {
        integration_id: "copilot".into(),
        account_key: "opaque".into(),
        event_id: "delivery".into(),
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: "2026-10-28T01:00:02Z".into(),
    }
}
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/copilot/synthetic-otel-spans.json"
    ))
    .unwrap()
}

#[test]
fn request_resolution_and_usage_remain_separate_from_served_proof_and_content() {
    let parsed = copilot::normalize_completed_span(
        &fixture()["leaf"].to_string(),
        "synthetic-session",
        &context(),
    )
    .unwrap();
    assert_eq!(
        parsed.event.model_requested.as_deref(),
        Some("requested-model")
    );
    assert_eq!(
        parsed.event.model_resolved.as_deref(),
        Some("resolved-model")
    );
    assert_eq!(
        parsed.event.model_evidence_kind,
        ModelEvidenceKind::RequestAttribute
    );
    assert!(parsed.event.model_observed.is_none() && parsed.event.phase_epoch.is_none());
    let usage = parsed.usage.as_ref().unwrap();
    assert!(usage.event.model_requested.is_none());
    assert_eq!(usage.samples[0].mode, UsageMode::Delta);
    assert_eq!(usage.samples[0].scope, UsageScope::Inference);
    let encoded = serde_json::to_string(&parsed).unwrap();
    assert!(!encoded.contains("PRIVATE"));
    assert!(!encoded.contains("gen_ai.input.messages"));
}

#[test]
fn parent_aggregate_uses_distinct_partial_series_and_does_not_pretend_to_be_leaf_delta() {
    let fixture = fixture();
    let parent = copilot::normalize_completed_span(
        &fixture["parent"].to_string(),
        "synthetic-session",
        &context(),
    )
    .unwrap();
    let leaf = copilot::normalize_completed_span(
        &fixture["leaf"].to_string(),
        "synthetic-session",
        &context(),
    )
    .unwrap();
    let parent = &parent.usage.unwrap().samples[0];
    let leaf = &leaf.usage.unwrap().samples[0];
    assert_ne!(parent.accounting_series, leaf.accounting_series);
    assert_eq!(parent.mode, UsageMode::Cumulative);
    assert_eq!(parent.revision, None);
    assert!(!parent.known_zero);
}

#[test]
fn rejects_ambiguous_attributes_incomplete_spans_wrong_session_and_invalid_counts() {
    let fixture = fixture();
    assert!(copilot::normalize_completed_span(
        &fixture["parent"].to_string(),
        "other-session",
        &context()
    )
    .is_err());
    for (field, value) in [("traceId", "invalid"), ("endTimeUnixNano", "0")] {
        let mut raw = fixture["leaf"].clone();
        raw[field] = value.into();
        assert!(copilot::normalize_completed_span(
            &raw.to_string(),
            "synthetic-session",
            &context()
        )
        .is_err());
    }
    let mut raw = fixture["leaf"].clone();
    let repeated = raw["attributes"][0].clone();
    raw["attributes"].as_array_mut().unwrap().push(repeated);
    assert!(
        copilot::normalize_completed_span(&raw.to_string(), "synthetic-session", &context())
            .is_err()
    );
    for count in ["-1", "1.5", "9007199254740992"] {
        let mut raw = fixture["leaf"].clone();
        raw["attributes"][3]["value"]["intValue"] = count.into();
        assert!(copilot::normalize_completed_span(
            &raw.to_string(),
            "synthetic-session",
            &context()
        )
        .is_err());
    }
}
