use collector::{ingest::IntegrationRegistration, Store};
use normalized_schema::{EventType, ModelEvidenceKind, NormalizedEvent, Phase, Provider};
use rusqlite::Connection;
use tempfile::TempDir;

fn event(id: &str, kind: EventType, seq: i64) -> NormalizedEvent {
    let mut event: NormalizedEvent = serde_json::from_value(serde_json::json!({
        "schemaVersion":1,"eventId":id,"type":"session.started",
        "integrationId":"integration","accountKey":"account","provider":"codex",
        "surface":"synthetic","source":"synthetic","sourceSessionId":"session",
        "sourceSequence":seq,"modelEvidenceKind":"unknown","confidence":"direct",
        "observedAt":format!("2026-10-28T00:00:{seq:02}Z"),
        "receivedAt":format!("2026-10-28T00:00:{seq:02}Z")
    }))
    .unwrap();
    event.event_type = kind;
    event
}
fn setup(path: &std::path::Path) -> Store {
    let mut store = Store::open(path).unwrap();
    store
        .register_integration(&IntegrationRegistration {
            id: "integration".into(),
            account_key: "account".into(),
            provider: Provider::Codex,
            surface: "synthetic".into(),
        })
        .unwrap();
    store
        .ingest(&event("session-start", EventType::SessionStarted, 0))
        .unwrap();
    store
}
fn inference(id: &str, kind: EventType, seq: i64) -> NormalizedEvent {
    let mut ev = event(id, kind, seq);
    ev.source_inference_id = Some("inference".into());
    ev.phase_at_start = Some(Phase::Unassigned);
    ev.phase_epoch = Some(0);
    ev
}
fn served(id: &str, seq: i64) -> NormalizedEvent {
    let mut ev = inference(id, EventType::InferenceEnded, seq);
    ev.model_evidence_kind = ModelEvidenceKind::ServedResponse;
    ev.model_observed = Some("served-model".into());
    ev.evidence_ref = Some(format!("response-{id}"));
    ev.evidence_expires_at = Some("2026-10-28T00:01:00Z".into());
    ev
}

#[test]
fn selection_and_request_never_become_served_without_a_matching_durable_start() {
    let temp = TempDir::new().unwrap();
    let mut store = setup(&temp.path().join("evidence.db"));
    let mut selection = event("selection", EventType::ModelObserved, 1);
    selection.model_selected = Some("selected-model".into());
    selection.model_evidence_kind = ModelEvidenceKind::HookCommonMetadata;
    selection.evidence_ref = Some("hook".into());
    store.ingest(&selection).unwrap();
    let mut request = event("request", EventType::ModelObserved, 2);
    request.model_requested = Some("requested-model".into());
    request.model_resolved = Some("routed-model".into());
    request.model_evidence_kind = ModelEvidenceKind::RequestAttribute;
    request.evidence_ref = Some("request-attribute".into());
    store.ingest(&request).unwrap();
    store.ingest(&served("unbound-response", 3)).unwrap();
    let state = store.list_sessions(10, None).unwrap().items.remove(0);
    assert_eq!(state.model_selected.as_deref(), Some("selected-model"));
    assert_eq!(state.model_requested.as_deref(), Some("requested-model"));
    assert_eq!(state.model_resolved.as_deref(), Some("routed-model"));
    assert_eq!(state.model_observed, None);
    store
        .ingest(&inference(
            "inference-start",
            EventType::InferenceStarted,
            4,
        ))
        .unwrap();
    let response = served("response", 5);
    store.ingest(&response).unwrap();
    assert!(store.ingest(&response).unwrap().already_committed);
    let state = store.list_sessions(10, None).unwrap().items.remove(0);
    assert_eq!(state.model_observed.as_deref(), Some("served-model"));
    assert_eq!(state.model_evidence_kind, "served_response");
    assert_eq!(state.model_selected.as_deref(), Some("selected-model"));
}

#[test]
fn phase_change_resets_heads_and_late_or_relabelled_responses_stay_historical() {
    let temp = TempDir::new().unwrap();
    let mut store = setup(&temp.path().join("phase.db"));
    store
        .ingest(&inference("start", EventType::InferenceStarted, 1))
        .unwrap();
    store.ingest(&served("first", 2)).unwrap();
    let mut phase = event("phase", EventType::PhaseChanged, 3);
    phase.source = "aum.user".into();
    phase.phase_at_start = Some(Phase::Review);
    phase.phase_epoch = Some(1);
    store.change_phase(&phase, 0).unwrap();
    store.ingest(&served("late", 4)).unwrap();
    let mut forged = served("relabelled", 5);
    forged.phase_at_start = Some(Phase::Review);
    forged.phase_epoch = Some(1);
    store.ingest(&forged).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0].model_observed,
        None
    );
    let mut start = inference("new-start", EventType::InferenceStarted, 6);
    start.source_inference_id = Some("new-inference".into());
    start.phase_at_start = Some(Phase::Review);
    start.phase_epoch = Some(1);
    store.ingest(&start).unwrap();
    let mut response = served("new-response", 7);
    response.source_inference_id = start.source_inference_id.clone();
    response.phase_at_start = Some(Phase::Review);
    response.phase_epoch = Some(1);
    store.ingest(&response).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0]
            .model_observed
            .as_deref(),
        Some("served-model")
    );
    let mut rebound = start.clone();
    rebound.event_id = "reuse".into();
    assert!(store.ingest(&rebound).is_err());
}

#[test]
fn stale_cross_source_and_unsequenced_updates_cannot_replace_a_snapshot_after_restart() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("order.db");
    let mut store = setup(&path);
    let mut selected = event("head", EventType::ModelObserved, 5);
    selected.model_selected = Some("head-model".into());
    selected.model_evidence_kind = ModelEvidenceKind::SessionSelection;
    selected.evidence_ref = Some("selection".into());
    store.ingest(&selected).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    for (id, seq, source) in [
        ("old", Some(4), "synthetic"),
        ("cross", Some(6), "other"),
        ("unordered", None, "synthetic"),
    ] {
        let mut next = selected.clone();
        next.event_id = id.into();
        next.source_sequence = seq;
        next.source = source.into();
        next.model_selected = Some("should-stay-history".into());
        store.ingest(&next).unwrap();
    }
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0]
            .model_selected
            .as_deref(),
        Some("head-model")
    );
    let mut newer = selected;
    newer.event_id = "newer".into();
    newer.source_sequence = Some(7);
    newer.model_selected = Some("new-model".into());
    newer.observed_at = "2026-10-28T00:00:07Z".into();
    store.ingest(&newer).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0]
            .model_selected
            .as_deref(),
        Some("new-model")
    );
}

#[test]
fn outbox_failure_rolls_back_inference_binding_and_model_head() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollback.db");
    let mut store = setup(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'test'); END;",
    )
    .unwrap();
    let start = inference("start", EventType::InferenceStarted, 1);
    assert!(store.ingest(&start).is_err());
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM inference_context", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_outbox;").unwrap();
    store.ingest(&start).unwrap();
    db.execute_batch(
        "CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'test'); END;",
    )
    .unwrap();
    let response = served("response", 2);
    assert!(store.ingest(&response).is_err());
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0].model_observed,
        None
    );
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM model_projection", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    db.execute_batch("DROP TRIGGER fail_outbox;").unwrap();
    store.ingest(&response).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0].model_evidence_kind,
        "served_response"
    );
}

#[test]
fn codex_fixture_can_be_ingested_without_claiming_served_model_or_idle() {
    use adapters::codex::{normalize_hook, CodexContext, Surface};
    let temp = TempDir::new().unwrap();
    let mut store = Store::open(temp.path().join("codex.db")).unwrap();
    let context = CodexContext {
        integration_id: "codex".into(),
        account_key: "local".into(),
        surface: Surface::VsCodeExtension,
        event_id: "helper-delivery".into(),
        observed_at: "2026-10-28T00:00:00Z".into(),
        received_at: "2026-10-28T00:00:00Z".into(),
    };
    store
        .register_integration(&IntegrationRegistration {
            id: context.integration_id.clone(),
            account_key: context.account_key.clone(),
            provider: Provider::Codex,
            surface: context.surface.as_str().into(),
        })
        .unwrap();
    let normalized = normalize_hook(
        include_str!("../../../tests/fixtures/codex/session_start.json"),
        &context,
    )
    .unwrap();
    store.ingest(&normalized).unwrap();
    let state = store.list_sessions(10, None).unwrap().items.remove(0);
    assert_eq!(state.model_selected.as_deref(), Some("gpt-5-codex"));
    assert_eq!(state.model_observed, None);
    assert_eq!(state.activity, "UNKNOWN");
}
