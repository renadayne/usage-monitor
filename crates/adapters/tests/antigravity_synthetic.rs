use adapters::{
    antigravity::{self, Hook},
    metadata::ObservationContext,
};
use normalized_schema::{EventType, ModelEvidenceKind};
fn context() -> ObservationContext {
    ObservationContext {
        integration_id: "synthetic-agy".into(),
        account_key: "opaque".into(),
        event_id: "delivery".into(),
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: "2026-10-28T01:00:01Z".into(),
    }
}

#[test]
fn invocation_common_model_is_never_promoted_to_served_or_authoritative_order() {
    let raw = include_str!("../../../tests/fixtures/antigravity/synthetic-hooks.json");
    for hook in [Hook::PreInvocation, Hook::PostInvocation] {
        let observation = antigravity::normalize_hook(raw, hook, &context()).unwrap();
        let event = &observation.event;
        assert_eq!(event.model_selected.as_deref(), Some("synthetic-model"));
        assert_eq!(
            event.model_evidence_kind,
            ModelEvidenceKind::HookCommonMetadata
        );
        assert_eq!(observation.invocation_num, Some(3));
        assert_eq!(observation.initial_num_steps, Some(10));
        assert!(
            event.model_observed.is_none()
                && event.model_requested.is_none()
                && event.source_inference_id.is_none()
                && event.source_sequence.is_none()
                && event.activity.is_none()
        );
        let encoded = serde_json::to_string(&observation).unwrap();
        assert!(!encoded.contains("PRIVATE"));
        assert!(!encoded.contains("injectSteps"));
        assert!(!encoded.contains("terminationBehavior"));
    }
    assert_eq!(antigravity::OBSERVER_REPLY, "{}");
}

#[test]
fn explicit_start_and_stop_do_not_invent_session_end_or_idle_from_background_work() {
    let raw = r#"{"conversationId":"session","modelName":"model","fullyIdle":false,"terminationReason":"model_stop"}"#;
    assert_eq!(
        antigravity::normalize_hook(raw, Hook::SessionStart, &context())
            .unwrap()
            .event
            .event_type,
        EventType::SessionStarted
    );
    let stop = antigravity::normalize_hook(raw, Hook::Stop, &context()).unwrap();
    assert_eq!(stop.event.event_type, EventType::ModelObserved);
    assert!(stop.event.activity.is_none());
    assert!(antigravity::normalize_hook(raw, Hook::PreInvocation, &context()).is_err());
    let no_model = antigravity::normalize_hook(
        r#"{"conversationId":"session"}"#,
        Hook::PreToolUse,
        &context(),
    )
    .unwrap();
    assert_eq!(
        no_model.event.model_evidence_kind,
        ModelEvidenceKind::Unknown
    );
    assert!(antigravity::normalize_hook(
        r#"{"conversationId":"PRIVATE\n"}"#,
        Hook::Stop,
        &context()
    )
    .is_err());
}
