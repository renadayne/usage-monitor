use collector::{ingest::IntegrationRegistration, Store};
use normalized_schema::{EventType, ModelEvidenceKind, NormalizedEvent, Phase};
use policy_engine::{Action, Decision, Policy, Reason, Scope};

fn event(id: &str, kind: EventType, second: u32) -> NormalizedEvent {
    let mut event:NormalizedEvent=serde_json::from_value(serde_json::json!({
        "schemaVersion":1,"eventId":id,"type":"session.started","integrationId":"integration","accountKey":"opaque-local",
        "provider":"codex","surface":"synthetic","source":"synthetic","sourceSessionId":"source-session","sourceSequence":second,
        "modelEvidenceKind":"unknown","confidence":"direct","observedAt":format!("2026-10-28T00:00:{second:02}Z"),"receivedAt":format!("2026-10-28T00:00:{second:02}Z")
    })).unwrap();
    event.event_type = kind;
    event
}

#[test]
fn dry_run_uses_bound_served_proof_freshness_clock_and_current_phase() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("policy.db");
    let mut store = Store::open(&path).unwrap();
    store
        .register_integration(&IntegrationRegistration {
            id: "integration".into(),
            account_key: "opaque-local".into(),
            provider: normalized_schema::Provider::Codex,
            surface: "synthetic".into(),
        })
        .unwrap();
    let start = store
        .ingest(&event("session", EventType::SessionStarted, 0))
        .unwrap();
    store
        .save_policy(&Policy {
            id: "rule".into(),
            scope: Scope::default(),
            phase: Phase::Review,
            allowed_model_ids: vec!["expected".into()],
            priority: 0,
            action: Action::Notify,
            enabled: true,
        })
        .unwrap();
    assert_eq!(
        store
            .evaluate_session_policy(&start.session_id, "2026-10-28T00:00:00Z", true)
            .unwrap()
            .decision,
        Decision::Indeterminate
    );
    let mut phase = event("phase", EventType::PhaseChanged, 1);
    phase.source = "aum.user".into();
    phase.phase_at_start = Some(Phase::Review);
    phase.phase_epoch = Some(1);
    store.change_phase(&phase, 0).unwrap();
    let mut inference = event("inference", EventType::InferenceStarted, 2);
    inference.source_inference_id = Some("inference".into());
    inference.phase_at_start = Some(Phase::Review);
    inference.phase_epoch = Some(1);
    store.ingest(&inference).unwrap();
    let mut response = inference.clone();
    response.event_id = "response".into();
    response.event_type = EventType::InferenceEnded;
    response.source_sequence = Some(3);
    response.observed_at = "2026-10-28T00:00:03Z".into();
    response.received_at = response.observed_at.clone();
    response.model_observed = Some("actual".into());
    response.model_evidence_kind = ModelEvidenceKind::ServedResponse;
    response.evidence_ref = Some("response-ref".into());
    response.evidence_expires_at = Some("2026-10-28T00:01:00Z".into());
    store.ingest(&response).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    let evaluation = store
        .evaluate_session_policy(&start.session_id, "2026-10-28T00:00:04Z", true)
        .unwrap();
    assert_eq!(evaluation.decision, Decision::Violation);
    let detail = store
        .session_detail(&start.session_id, None, "2026-10-28T00:00:04Z", true)
        .unwrap();
    assert_eq!(detail.session.phase, "REVIEW");
    assert_eq!(detail.policy.decision, Decision::Violation);
    assert_eq!(detail.events.last().unwrap().phase_epoch, Some(1));
    assert!(evaluation.should_notify);
    assert_eq!(
        store
            .evaluate_session_policy(&start.session_id, "2026-10-28T00:00:04Z", false)
            .unwrap()
            .reason,
        Reason::ClockUntrusted
    );
    assert_eq!(
        store
            .evaluate_session_policy(&start.session_id, "2026-10-28T00:01:00Z", true)
            .unwrap()
            .decision,
        Decision::Indeterminate
    );
    phase.event_id = "next-phase".into();
    phase.phase_epoch = Some(2);
    phase.phase_at_start = Some(Phase::Planning);
    phase.observed_at = "2026-10-28T00:00:05Z".into();
    phase.received_at = phase.observed_at.clone();
    store.change_phase(&phase, 1).unwrap();
    assert!(store
        .current_model_evidence(&start.session_id)
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .evaluate_session_policy(&start.session_id, "2026-10-28T00:00:06Z", true)
            .unwrap()
            .decision,
        Decision::NotApplicable
    );
}

#[test]
fn metadata_views_and_export_keep_privacy_boundaries_and_escape_csv_formulas() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = Store::open(dir.path().join("export.db")).unwrap();
    store
        .register_integration(&IntegrationRegistration {
            id: "integration".into(),
            account_key: "opaque-local".into(),
            provider: normalized_schema::Provider::Codex,
            surface: "synthetic".into(),
        })
        .unwrap();
    let mut selected = event("formula", EventType::SessionStarted, 0);
    selected.model_selected = Some("=HYPERLINK(\"https://example.invalid\")".into());
    selected.model_evidence_kind = ModelEvidenceKind::HookCommonMetadata;
    selected.evidence_ref = Some("hook".into());
    let result = store.ingest(&selected).unwrap();
    let summary = store.session_summary(&result.session_id).unwrap().unwrap();
    let json = serde_json::to_value(summary).unwrap();
    assert_eq!(json["modelObserved"], serde_json::Value::Null);
    assert!(json.get("payloadJson").is_none());
    let integrations = serde_json::to_value(store.list_integrations().unwrap()).unwrap();
    assert_eq!(integrations[0]["targetVerification"], "unknown");
    assert!(integrations[0].get("installedConfigHash").is_none());
    let export = store.export_history(7, "2026-10-28T00:00:10Z").unwrap();
    assert_eq!(export.rows, 1);
    assert!(!export.truncated);
    assert!(export.csv.contains("\"'=HYPERLINK("));
    assert!(!export.csv.contains("opaque-local"));
    assert_eq!(
        store
            .export_history(7, "2026-11-05T00:00:00Z")
            .unwrap()
            .rows,
        0
    );
    assert_eq!(
        store
            .export_history(30, "2026-11-05T00:00:00Z")
            .unwrap()
            .rows,
        1
    );
    assert!(store.export_history(1, "2026-10-28T00:00:10Z").is_err());
}
