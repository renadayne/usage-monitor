//! Integration tests for strict normalized lifecycle and model-evidence validation.

use normalized_schema::{
    format_canonical_utc, Activity, Confidence, EventType, ModelEvidenceKind, NormalizedEvent,
    Phase, Provider, ValidationError, MAX_IDENTIFIER_LEN, SCHEMA_VERSION,
};

fn valid_served_event_json() -> &'static str {
    r#"{
        "schemaVersion": 1,
        "eventId": "evt-synth-served-1",
        "type": "inference.ended",
        "integrationId": "int-codex-vscode-1",
        "accountKey": "acct-opaque-1",
        "provider": "copilot",
        "surface": "vscode_extension",
        "source": "otlp_http_trace",
        "sourceEventId": "trace-1:span-2",
        "sourceSequence": 4,
        "sourceSessionId": "conv-synth-1",
        "localSessionId": null,
        "sourceTurnId": "turn-1",
        "sourceInferenceId": "inf-1",
        "parentAgentId": null,
        "phaseAtStart": "REVIEW",
        "phaseEpoch": 2,
        "modelSelected": "gpt-4.1",
        "modelRequested": "gpt-4.1",
        "modelResolved": "gpt-4.1-2026-01",
        "modelObserved": "gpt-4.1-2026-01",
        "modelEvidenceKind": "served_response",
        "evidenceRef": "span:abc123def456",
        "observedAt": "2026-10-27T12:00:01.250Z",
        "receivedAt": "2026-10-27T12:00:01.500+00:00",
        "evidenceExpiresAt": "2026-10-27T12:05:01.250Z",
        "confidence": "direct",
        "activity": "WORKING"
    }"#
}

#[test]
fn valid_synthetic_json_round_trip_and_canonical_utc() {
    let event: NormalizedEvent =
        serde_json::from_str(valid_served_event_json()).expect("deserialize valid served event");
    assert_eq!(event.schema_version, SCHEMA_VERSION);
    assert_eq!(event.event_type, EventType::InferenceEnded);
    assert_eq!(event.provider, Provider::Copilot);
    assert_eq!(event.phase_at_start, Some(Phase::Review));
    assert_eq!(event.model_evidence_kind, ModelEvidenceKind::ServedResponse);
    assert_eq!(event.confidence, Confidence::Direct);
    assert_eq!(event.activity, Some(Activity::Working));

    let ts = event
        .validate()
        .expect("valid served event must pass validation");
    assert_eq!(
        format_canonical_utc(ts.observed_at_utc),
        "2026-10-27T12:00:01.250000000Z"
    );
    assert_eq!(
        format_canonical_utc(ts.received_at_utc),
        "2026-10-27T12:00:01.500000000Z"
    );

    let serialized = serde_json::to_string(&event).expect("serialize back to JSON");
    let reparsed: NormalizedEvent =
        serde_json::from_str(&serialized).expect("re-parse round-tripped JSON");
    assert_eq!(reparsed, event);
    reparsed.validate().expect("re-validated after round-trip");
}

#[test]
fn rejects_unknown_content_and_deferred_fields_during_deserialization() {
    let forbidden_fields = [
        r#""prompt": "secret user prompt""#,
        r#""response": "assistant output""#,
        r#""code": "fn main() {}""#,
        r#""toolArgs": {"cmd": "dir"}"#,
        r#""transcriptPath": "C:\\Users\\me\\transcript.jsonl""#,
        r#""metadata": {"cwd": "D:\\repo"}"#,
        r#""usage": []"#,
        r#""quota": null"#,
    ];

    for extra_field in forbidden_fields {
        let raw = format!(
            r#"{{
                "schemaVersion": 1,
                "eventId": "evt-1",
                "type": "session.started",
                "integrationId": "int-1",
                "accountKey": "acct-1",
                "provider": "codex",
                "surface": "cli",
                "source": "hook",
                "sourceSessionId": "sess-1",
                "modelEvidenceKind": "unknown",
                "observedAt": "2026-10-27T00:00:00Z",
                "receivedAt": "2026-10-27T00:00:00Z",
                {extra_field}
            }}"#
        );
        let err = serde_json::from_str::<NormalizedEvent>(&raw)
            .expect_err("unknown or deferred field must be rejected by serde");
        assert!(
            err.to_string().contains("unknown field"),
            "expected unknown field error for {extra_field}, got: {err}"
        );
    }

    let unsupported_types = [
        "usage.updated",
        "quota.updated",
        "policy.violation",
        "integration.health",
    ];
    for event_type in unsupported_types {
        let raw = format!(
            r#"{{
                "schemaVersion": 1,
                "eventId": "evt-1",
                "type": "{event_type}",
                "integrationId": "int-1",
                "accountKey": "acct-1",
                "provider": "codex",
                "surface": "cli",
                "source": "hook",
                "sourceSessionId": "sess-1",
                "modelEvidenceKind": "unknown",
                "observedAt": "2026-10-27T00:00:00Z",
                "receivedAt": "2026-10-27T00:00:00Z"
            }}"#
        );
        assert!(
            serde_json::from_str::<NormalizedEvent>(&raw).is_err(),
            "event type {event_type} must not be accepted in this step"
        );
    }
}

#[test]
fn rejects_missing_served_evidence_and_selection_only_upgrades() {
    let base: NormalizedEvent =
        serde_json::from_str(valid_served_event_json()).expect("parse base event");

    let mut hook_promoted = base.clone();
    hook_promoted.model_evidence_kind = ModelEvidenceKind::HookCommonMetadata;
    assert_eq!(
        hook_promoted.validate().unwrap_err(),
        ValidationError::ForbiddenEvidencePromotion {
            kind: ModelEvidenceKind::HookCommonMetadata,
            field: "modelObserved",
        }
    );

    let mut selection_promoted = base.clone();
    selection_promoted.model_evidence_kind = ModelEvidenceKind::SessionSelection;
    assert_eq!(
        selection_promoted.validate().unwrap_err(),
        ValidationError::ForbiddenEvidencePromotion {
            kind: ModelEvidenceKind::SessionSelection,
            field: "modelObserved",
        }
    );

    let mut missing_inf = base.clone();
    missing_inf.event_type = EventType::ModelObserved;
    missing_inf.source_inference_id = None;
    assert_eq!(
        missing_inf.validate().unwrap_err(),
        ValidationError::InvalidServedModelEvidence {
            reason: "modelObserved requires sourceInferenceId",
        }
    );

    let mut missing_ref = base.clone();
    missing_ref.evidence_ref = None;
    assert_eq!(
        missing_ref.validate().unwrap_err(),
        ValidationError::InvalidServedModelEvidence {
            reason: "modelObserved requires evidenceRef",
        }
    );

    let mut missing_epoch = base.clone();
    missing_epoch.phase_epoch = None;
    assert_eq!(
        missing_epoch.validate().unwrap_err(),
        ValidationError::InvalidServedModelEvidence {
            reason: "modelObserved requires phaseEpoch",
        }
    );

    let mut served_without_model = base.clone();
    served_without_model.model_observed = None;
    assert_eq!(
        served_without_model.validate().unwrap_err(),
        ValidationError::InvalidServedModelEvidence {
            reason: "served_response requires modelObserved",
        }
    );

    let mut selection_claiming_request = base;
    selection_claiming_request.model_observed = None;
    selection_claiming_request.model_requested = None;
    selection_claiming_request.model_resolved = None;
    selection_claiming_request.model_evidence_kind = ModelEvidenceKind::RequestAttribute;
    assert_eq!(
        selection_claiming_request.validate().unwrap_err(),
        ValidationError::ForbiddenEvidencePromotion {
            kind: ModelEvidenceKind::RequestAttribute,
            field: "modelRequested",
        }
    );
}

#[test]
fn rejects_invalid_identifiers_schema_version_and_non_utc_timestamps() {
    let base: NormalizedEvent =
        serde_json::from_str(valid_served_event_json()).expect("parse base event");

    let mut bad_version = base.clone();
    bad_version.schema_version = 2;
    assert_eq!(
        bad_version.validate().unwrap_err(),
        ValidationError::UnsupportedSchemaVersion(2)
    );

    let mut empty_id = base.clone();
    empty_id.event_id = String::new();
    assert!(matches!(
        empty_id.validate().unwrap_err(),
        ValidationError::InvalidIdentifier {
            field: "eventId",
            ..
        }
    ));

    let mut overlong_id = base.clone();
    overlong_id.account_key = "a".repeat(MAX_IDENTIFIER_LEN + 1);
    assert!(matches!(
        overlong_id.validate().unwrap_err(),
        ValidationError::InvalidIdentifier {
            field: "accountKey",
            ..
        }
    ));

    let mut whitespace_id = base.clone();
    whitespace_id.integration_id = " int-1 ".to_string();
    assert!(matches!(
        whitespace_id.validate().unwrap_err(),
        ValidationError::InvalidIdentifier {
            field: "integrationId",
            ..
        }
    ));

    for bad_ts in [
        "2026-10-27T19:00:00+07:00",
        "2026-10-27T12:00:00-00:00",
        "2026-10-27 12:00:00Z",
        "2026-10-27",
        "not-a-timestamp",
    ] {
        let mut bad_time = base.clone();
        bad_time.observed_at = bad_ts.to_string();
        assert!(
            matches!(
                bad_time.validate().unwrap_err(),
                ValidationError::InvalidUtcTimestamp {
                    field: "observedAt",
                    ..
                }
            ),
            "timestamp '{bad_ts}' must be rejected as non-canonical UTC"
        );
    }
}

#[test]
fn unknown_confidence_cannot_claim_served_proof_and_errors_do_not_echo_input() {
    let mut event: NormalizedEvent = serde_json::from_str(valid_served_event_json()).unwrap();
    event.confidence = Confidence::Unknown;
    assert!(matches!(
        event.validate(),
        Err(ValidationError::InvalidServedModelEvidence { .. })
    ));
    event.confidence = Confidence::Direct;
    event.observed_at = "sensitive-input".into();
    assert!(!event
        .validate()
        .unwrap_err()
        .to_string()
        .contains("sensitive-input"));
}

#[test]
fn canonical_storage_times_sort_in_chronological_order_at_fraction_boundaries() {
    let earlier =
        normalized_schema::parse_canonical_utc("observedAt", "2026-10-27T00:00:00Z").unwrap();
    let later =
        normalized_schema::parse_canonical_utc("observedAt", "2026-10-27T00:00:00.001Z").unwrap();
    assert!(format_canonical_utc(earlier) < format_canonical_utc(later));
}
