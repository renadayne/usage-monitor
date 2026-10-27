use adapters::codex::*;
use normalized_schema::{Confidence, EventType, ModelEvidenceKind, Provider, ValidationError};

const FIXTURE_SESSION_START: &str =
    include_str!("../../../tests/fixtures/codex/session_start.json");
const FIXTURE_USER_PROMPT_SUBMIT: &str =
    include_str!("../../../tests/fixtures/codex/user_prompt_submit.json");
const FIXTURE_STOP_WITHOUT_MODEL: &str =
    include_str!("../../../tests/fixtures/codex/stop_without_model.json");
const FIXTURE_SUBAGENT_START: &str =
    include_str!("../../../tests/fixtures/codex/subagent_start.json");

fn sample_context(surface: Surface) -> CodexContext {
    CodexContext {
        integration_id: "codex-local-1".to_owned(),
        account_key: "acct-personal".to_owned(),
        surface,
        event_id: "hlp_deliv_0001".to_owned(),
        observed_at: "2026-03-30T10:00:00Z".to_owned(),
        received_at: "2026-03-30T10:00:01Z".to_owned(),
    }
}

fn assert_no_sensitive_markers(serialized: &str) {
    let value: serde_json::Value = serde_json::from_str(serialized).unwrap();
    let object = value.as_object().unwrap();
    for field in [
        "cwd",
        "transcript_path",
        "transcriptPath",
        "prompt",
        "response",
        "code",
        "_fixture",
    ] {
        assert!(!object.contains_key(field), "raw field leaked: {field}");
    }
    for marker in [
        "FAKE_SECRET_WORKSPACE",
        "FAKE_TRANSCRIPT_PATH_001",
        "FAKE_CAMEL_TRANSCRIPT_PATH_001",
        "FAKE_PROMPT_DO_NOT_LEAK_EXPLAIN_PRIVATE_KEY",
        "FAKE_RESPONSE_DO_NOT_LEAK_ASSISTANT_OUTPUT",
        "FAKE_CODE_SNIPPET_SECRET",
        "sk-fake-1234567890",
        "FAKE_SECRET_CWD_MARKER",
        "FAKE_TRANSCRIPT_PATH_MARKER",
        "FAKE_CAMEL_TRANSCRIPT_PATH_MARKER",
        "FAKE_USER_PROMPT_TEXT_MARKER",
        "FAKE_MODEL_RESPONSE_TEXT_MARKER",
        "fake_proprietary_code_marker",
    ] {
        assert!(
            !serialized.contains(marker),
            "serialized normalized event leaked sensitive marker '{marker}': {serialized}"
        );
    }
}

#[test]
fn normalizes_session_start_and_strips_sensitive_markers_across_distinct_surfaces() {
    let vscode_ctx = sample_context(Surface::VsCodeExtension);
    let desktop_ctx = sample_context(Surface::StandaloneDesktop);

    let vscode_event =
        normalize_hook(FIXTURE_SESSION_START, &vscode_ctx).expect("vscode session start");
    let desktop_event =
        normalize_hook(FIXTURE_SESSION_START, &desktop_ctx).expect("desktop session start");

    assert_eq!(vscode_event.surface, "codex_vscode_extension");
    assert_eq!(desktop_event.surface, "codex_desktop_app");
    assert_ne!(vscode_event.surface, desktop_event.surface);

    assert_eq!(vscode_event.event_type, EventType::SessionStarted);
    assert_eq!(vscode_event.provider, Provider::Codex);
    assert_eq!(vscode_event.source, "codex.hooks");
    assert_eq!(vscode_event.event_id, "hlp_deliv_0001");
    assert_eq!(vscode_event.source_event_id, None);
    assert_eq!(vscode_event.source_sequence, None);
    assert_eq!(vscode_event.source_session_id, "sess_synth_codex_001");
    assert_eq!(vscode_event.local_session_id, None);
    assert_eq!(vscode_event.source_turn_id, None);
    assert_eq!(vscode_event.source_inference_id, None);
    assert_eq!(vscode_event.phase_at_start, None);
    assert_eq!(vscode_event.phase_epoch, None);
    assert_eq!(vscode_event.model_selected.as_deref(), Some("gpt-5-codex"));
    assert_eq!(vscode_event.model_requested, None);
    assert_eq!(vscode_event.model_resolved, None);
    assert_eq!(vscode_event.model_observed, None);
    assert_eq!(
        vscode_event.model_evidence_kind,
        ModelEvidenceKind::HookCommonMetadata
    );
    assert_eq!(vscode_event.evidence_ref.as_deref(), Some("hlp_deliv_0001"));
    assert_eq!(vscode_event.confidence, Confidence::Direct);
    assert_eq!(vscode_event.evidence_expires_at, None);
    assert_eq!(vscode_event.activity, None);

    let json = serde_json::to_string(&vscode_event).expect("serialize normalized event");
    assert_no_sensitive_markers(&json);
}

#[test]
fn maps_supported_lifecycle_and_metadata_hook_events_without_activity_or_served_model() {
    let ctx = sample_context(Surface::VsCodeExtension);

    let prompt_event = normalize_hook(FIXTURE_USER_PROMPT_SUBMIT, &ctx)
        .expect("UserPromptSubmit should normalize");
    assert_eq!(prompt_event.event_type, EventType::ModelObserved);
    assert_eq!(
        prompt_event.source_turn_id.as_deref(),
        Some("turn_synth_007")
    );
    assert_eq!(prompt_event.model_selected.as_deref(), Some("gpt-5-codex"));
    assert_eq!(prompt_event.model_observed, None);
    assert_eq!(prompt_event.model_resolved, None);
    assert_eq!(prompt_event.model_requested, None);
    assert_eq!(prompt_event.activity, None);
    let prompt_json = serde_json::to_string(&prompt_event).expect("serialize");
    assert_no_sensitive_markers(&prompt_json);

    for hook_name in [
        "PreToolUse",
        "PostToolUse",
        "UserPromptSubmit",
        "Stop",
        "PermissionRequest",
        "Interrupt",
    ] {
        let raw = format!(
            r#"{{"_fixture":"synthetic","hook_event_name":"{hook_name}","session_id":"sess_1","model":"gpt-5-codex"}}"#
        );
        let ev = normalize_hook(&raw, &ctx).expect(hook_name);
        assert_eq!(ev.event_type, EventType::ModelObserved);
        assert_eq!(ev.activity, None);
        assert_eq!(ev.model_selected.as_deref(), Some("gpt-5-codex"));
        assert_eq!(ev.model_observed, None);
        assert_eq!(
            ev.model_evidence_kind,
            ModelEvidenceKind::HookCommonMetadata
        );
        assert_eq!(ev.confidence, Confidence::Direct);
    }

    let end_raw =
        r#"{"_fixture":"synthetic","hook_event_name":"SessionEnd","session_id":"sess_1"}"#;
    let end_ev = normalize_hook(end_raw, &ctx).expect("SessionEnd");
    assert_eq!(end_ev.event_type, EventType::SessionEnded);
    assert_eq!(end_ev.activity, None);
    assert_eq!(end_ev.model_selected, None);
    assert_eq!(end_ev.model_evidence_kind, ModelEvidenceKind::Unknown);
    assert_eq!(end_ev.evidence_ref, None);
    assert_eq!(end_ev.confidence, Confidence::Unknown);
}

#[test]
fn missing_model_normalizes_to_none_and_unknown_evidence() {
    let ctx = sample_context(Surface::StandaloneDesktop);
    let event = normalize_hook(FIXTURE_STOP_WITHOUT_MODEL, &ctx).expect("Stop without model");
    assert_eq!(event.event_type, EventType::ModelObserved);
    assert_eq!(event.model_selected, None);
    assert_eq!(event.model_observed, None);
    assert_eq!(event.model_evidence_kind, ModelEvidenceKind::Unknown);
    assert_eq!(event.evidence_ref, None);
    assert_eq!(event.confidence, Confidence::Unknown);
    assert_eq!(event.activity, None);

    let json = serde_json::to_string(&event).expect("serialize");
    assert_no_sensitive_markers(&json);
}

#[test]
fn rejects_subagent_unknown_oversized_and_invalid_fields_without_leaking_input() {
    let ctx = sample_context(Surface::VsCodeExtension);

    assert_eq!(
        normalize_hook(FIXTURE_SUBAGENT_START, &ctx),
        Err(AdapterError::UnsupportedSubagentHook)
    );

    let subagent_stop =
        r#"{"_fixture":"synthetic","hook_event_name":"SubagentStop","session_id":"sess_1"}"#;
    assert_eq!(
        normalize_hook(subagent_stop, &ctx),
        Err(AdapterError::UnsupportedSubagentHook)
    );

    let unknown_hook = r#"{"_fixture":"synthetic","hook_event_name":"NotificationSECRET_HOOK","session_id":"sess_1"}"#;
    let err = normalize_hook(unknown_hook, &ctx).unwrap_err();
    assert_eq!(err, AdapterError::UnsupportedHookEvent);
    assert!(!err.to_string().contains("NotificationSECRET_HOOK"));

    let oversized = format!(
        r#"{{"_fixture":"synthetic","hook_event_name":"SessionStart","session_id":"sess_1","prompt":"{}"}}"#,
        "A".repeat(MAX_RAW_HOOK_BYTES)
    );
    assert_eq!(
        normalize_hook(&oversized, &ctx),
        Err(AdapterError::PayloadTooLarge)
    );

    let missing_session = r#"{"_fixture":"synthetic","hook_event_name":"SessionStart"}"#;
    assert_eq!(
        normalize_hook(missing_session, &ctx),
        Err(AdapterError::MissingRequiredField("session_id"))
    );

    let missing_hook = r#"{"_fixture":"synthetic","session_id":"sess_1"}"#;
    assert_eq!(
        normalize_hook(missing_hook, &ctx),
        Err(AdapterError::MissingRequiredField("hook_event_name"))
    );

    let empty_session =
        r#"{"_fixture":"synthetic","hook_event_name":"SessionStart","session_id":"   "}"#;
    assert!(matches!(
        normalize_hook(empty_session, &ctx),
        Err(AdapterError::Validation(
            ValidationError::InvalidIdentifier {
                field: "sourceSessionId",
                ..
            }
        ))
    ));

    let malformed_secret = "{not-json: FAKE_INLINE_SECRET_VALUE}";
    let malformed_err = normalize_hook(malformed_secret, &ctx).unwrap_err();
    assert_eq!(malformed_err, AdapterError::InvalidJson);
    assert!(!malformed_err
        .to_string()
        .contains("FAKE_INLINE_SECRET_VALUE"));
}
