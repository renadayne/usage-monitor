use adapters::{claude, metadata::ObservationContext};
use normalized_schema::{EventType, ModelEvidenceKind};

fn context() -> ObservationContext {
    ObservationContext {
        integration_id: "synthetic-claude".into(),
        account_key: "opaque-account".into(),
        event_id: "delivery".into(),
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: "2026-10-28T01:00:01Z".into(),
    }
}
fn fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/claude/synthetic-hooks-statusline.json"
    ))
    .unwrap()
}

#[test]
fn switch_and_statusline_are_selection_only_and_drop_sensitive_content() {
    let fixture = fixture();
    let event = claude::normalize_hook(&fixture["modelSwitch"].to_string(), &context()).unwrap();
    assert_eq!(event.model_selected.as_deref(), Some("synthetic-model"));
    assert_eq!(
        event.model_evidence_kind,
        ModelEvidenceKind::SessionSelection
    );
    assert!(event.model_requested.is_none());
    assert!(event.model_observed.is_none());
    assert!(event.source_inference_id.is_none());
    assert!(event.activity.is_none());
    let status =
        claude::normalize_statusline(&fixture["statusline"].to_string(), &context()).unwrap();
    assert_eq!(
        status.event.model_evidence_kind,
        ModelEvidenceKind::SessionSelection
    );
    assert_eq!(status.quotas.len(), 1);
    assert_eq!(status.quotas[0].used_percent, Some(0.0));
    assert_eq!(status.quotas[0].used, None);
    assert_eq!(status.quotas[0].limit, None);
    let encoded = serde_json::to_string(&(event, status)).unwrap();
    for marker in [
        "PRIVATE_",
        "transcript_path",
        "context_window",
        "total_cost_usd",
        "999999",
    ] {
        assert!(!encoded.contains(marker));
    }
}

#[test]
fn windows_are_independently_nullable_without_synthetic_zero_reset_or_quota_from_context() {
    for windows in [
        serde_json::json!({}),
        serde_json::json!({"five_hour":null}),
        serde_json::json!({"seven_day":{"used_percentage":44.0}}),
        serde_json::json!({"five_hour":{},"seven_day":{"used_percentage":22.0}}),
    ] {
        let raw = serde_json::json!({"session_id":"s","rate_limits":windows,"context_window":{"used_percentage":99.0}});
        let parsed = claude::normalize_statusline(&raw.to_string(), &context()).unwrap();
        assert_eq!(
            parsed.quotas.len(),
            windows
                .as_object()
                .unwrap()
                .values()
                .filter(|w| !w.is_null())
                .count()
        );
        assert!(parsed.quotas.iter().all(|q| q.resets_at.is_none()
            && q.stale_after.is_none()
            && q.used.is_none()
            && q.remaining.is_none()));
        if let Some(q) = parsed.quotas.iter().find(|q| q.bucket == "five_hour") {
            assert_eq!(q.used_percent, None);
        }
    }
    let parsed = claude::normalize_statusline(
        r#"{"session_id":"s","context_window":{"used_percentage":99}}"#,
        &context(),
    )
    .unwrap();
    assert!(parsed.quotas.is_empty());
}

#[test]
fn missing_reset_uses_delivery_identity_instead_of_merging_unknown_windows() {
    let raw = r#"{"session_id":"s","rate_limits":{"five_hour":{"used_percentage":5}}}"#;
    let first = claude::normalize_statusline(raw, &context()).unwrap();
    let mut other = context();
    other.event_id = "other-delivery".into();
    let second = claude::normalize_statusline(raw, &other).unwrap();
    assert_ne!(first.quotas[0].window_key, second.quotas[0].window_key);
    let mut fixture = fixture()["statusline"].clone();
    fixture["session_id"] = "another-session".into();
    let known = claude::normalize_statusline(&fixture.to_string(), &other).unwrap();
    let original = claude::normalize_statusline(&fixture.to_string(), &context()).unwrap();
    assert_eq!(known.quotas[0].window_key, original.quotas[0].window_key);
}

#[test]
fn explicit_lifecycle_is_not_guessed_from_stop_and_ambiguous_subagents_are_rejected() {
    let start = claude::normalize_hook(
        r#"{"session_id":"s","hook_event_name":"SessionStart","model":"model"}"#,
        &context(),
    )
    .unwrap();
    assert_eq!(start.event_type, EventType::SessionStarted);
    let stop = claude::normalize_hook(
        r#"{"session_id":"s","hook_event_name":"Stop","response":"PRIVATE"}"#,
        &context(),
    )
    .unwrap();
    assert_eq!(stop.event_type, EventType::ModelObserved);
    assert!(stop.activity.is_none());
    for event in ["SubagentStart", "SubagentStop", "FutureEvent"] {
        let raw = serde_json::json!({"session_id":"s","hook_event_name":event});
        assert!(claude::normalize_hook(&raw.to_string(), &context()).is_err());
    }
    assert!(claude::normalize_hook(
        r#"{"session_id":"s","hook_event_name":"PostModelSwitch"}"#,
        &context()
    )
    .is_err());
    assert!(claude::normalize_statusline(
        r#"{"session_id":"s","agent_id":"subagent"}"#,
        &context()
    )
    .is_err());
    assert!(claude::normalize_statusline(
        r#"{"session_id":"s","task":{"prompt":"PRIVATE"}}"#,
        &context()
    )
    .is_err());
    let bad = r#"{"session_id":"PRIVATE\n"}"#;
    let error = claude::normalize_statusline(bad, &context()).unwrap_err();
    assert!(!format!("{error:?}").contains("PRIVATE"));
    assert!(claude::normalize_statusline(&"x".repeat(65_537), &context()).is_err());
}
