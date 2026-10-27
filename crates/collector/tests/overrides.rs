use collector::{
    ingest::{IngestError, IntegrationRegistration},
    overrides::OverrideRequest,
    Store,
};
use normalized_schema::{EventType, ModelEvidenceKind, NormalizedEvent, Phase, Provider};
use policy_engine::{Action, Decision, Policy, Reason, Scope};
use rusqlite::Connection;
use tempfile::TempDir;

fn event(id: &str, kind: EventType, second: u8) -> NormalizedEvent {
    let mut event:NormalizedEvent=serde_json::from_value(serde_json::json!({"schemaVersion":1,"eventId":id,"type":"session.started","integrationId":"integration","accountKey":"account","provider":"codex","surface":"synthetic","source":"synthetic","sourceSessionId":"session","sourceSequence":second,"observedAt":format!("2026-10-28T01:00:{second:02}Z"),"receivedAt":format!("2026-10-28T01:00:{second:02}Z"),"modelEvidenceKind":"unknown","confidence":"direct"})).unwrap();
    event.event_type = kind;
    event
}
fn fixture(served: bool) -> (TempDir, Store, String) {
    let dir = TempDir::new().unwrap();
    let mut store = Store::open(dir.path().join("overrides.db")).unwrap();
    store
        .register_integration(&IntegrationRegistration {
            id: "integration".into(),
            account_key: "account".into(),
            provider: Provider::Codex,
            surface: "synthetic".into(),
        })
        .unwrap();
    let session = store
        .ingest(&event("session", EventType::SessionStarted, 0))
        .unwrap()
        .session_id;
    store
        .save_policy(&Policy {
            id: "rule".into(),
            scope: Scope::default(),
            phase: Phase::Review,
            allowed_model_ids: vec!["allowed".into()],
            priority: 0,
            action: Action::Notify,
            enabled: true,
        })
        .unwrap();
    let mut phase = event("phase", EventType::PhaseChanged, 1);
    phase.source = "aum.user".into();
    phase.phase_at_start = Some(Phase::Review);
    phase.phase_epoch = Some(1);
    store.change_phase(&phase, 0).unwrap();
    if served {
        let mut inference = event("inference", EventType::InferenceStarted, 2);
        inference.phase_at_start = Some(Phase::Review);
        inference.phase_epoch = Some(1);
        inference.source_inference_id = Some("inference".into());
        store.ingest(&inference).unwrap();
        let mut response = inference;
        response.event_id = "response".into();
        response.event_type = EventType::InferenceEnded;
        response.source_sequence = Some(3);
        response.model_observed = Some("actual".into());
        response.model_evidence_kind = ModelEvidenceKind::ServedResponse;
        response.evidence_ref = Some("synthetic-response".into());
        response.evidence_expires_at = Some("2026-10-28T03:00:00Z".into());
        store.ingest(&response).unwrap();
    }
    (dir, store, session)
}
fn request(session: &str, id: &str) -> OverrideRequest {
    OverrideRequest {
        id: id.into(),
        policy_id: "rule".into(),
        session_id: session.into(),
        expected_phase_epoch: 1,
        replacement_model_ids: vec!["actual".into()],
        reason: "Synthetic review exception".into(),
        duration_seconds: 60,
    }
}
fn decision(store: &Store, session: &str, now: &str) -> Decision {
    store
        .evaluate_session_policy(session, now, true)
        .unwrap()
        .decision
}

#[test]
fn exception_survives_restart_then_expires_exactly_without_scheduler() {
    let (_dir, mut store, session) = fixture(true);
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:05Z"),
        Decision::Violation
    );
    store
        .create_override(
            &request(&session, "exception"),
            "2026-10-28T01:00:05Z",
            true,
        )
        .unwrap();
    let path = store.path().to_owned();
    drop(store);
    let store = Store::open(path).unwrap();
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:01:04Z"),
        Decision::Compliant
    );
    assert_eq!(
        store
            .evaluate_session_policy(&session, "2026-10-28T01:01:04Z", true)
            .unwrap()
            .override_expires_at_ms,
        Some(
            normalized_schema::parse_canonical_utc("expiresAt", "2026-10-28T01:01:05Z")
                .unwrap()
                .timestamp_millis()
        )
    );
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:01:05Z"),
        Decision::Violation
    );
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:04Z"),
        Decision::Violation
    );
    assert_eq!(
        store
            .evaluate_session_policy(&session, "2026-10-28T01:00:06Z", false)
            .unwrap()
            .reason,
        Reason::ClockUntrusted
    );
    let detail = store
        .session_detail(&session, None, "2026-10-28T01:00:06Z", true)
        .unwrap();
    assert_eq!(detail.overrides.len(), 1);
    assert_eq!(detail.overrides[0].reason, "Synthetic review exception");
}

#[test]
fn replacement_revokes_previous_atomically_and_failed_insert_preserves_authorization() {
    let (_dir, mut store, session) = fixture(true);
    store
        .create_override(&request(&session, "first"), "2026-10-28T01:00:05Z", true)
        .unwrap();
    let mut second = request(&session, "second");
    second.replacement_model_ids = vec!["different".into()];
    store
        .create_override(&second, "2026-10-28T01:00:06Z", true)
        .unwrap();
    let history = store.override_history(&session).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history.iter().filter(|o| o.revoked_at.is_none()).count(), 1);
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:07Z"),
        Decision::Violation
    );
    Connection::open(store.path()).unwrap().execute_batch("CREATE TRIGGER fail_override BEFORE INSERT ON policy_overrides BEGIN SELECT RAISE(ABORT,'synthetic'); END;").unwrap();
    assert!(store
        .create_override(&request(&session, "failed"), "2026-10-28T01:00:08Z", true)
        .is_err());
    let history = store.override_history(&session).unwrap();
    assert_eq!(history.len(), 2);
    assert!(history
        .iter()
        .find(|o| o.id == "second")
        .unwrap()
        .revoked_at
        .is_none());
    store
        .revoke_override("second", &session, "2026-10-28T01:00:09Z")
        .unwrap();
    assert!(store
        .override_history(&session)
        .unwrap()
        .iter()
        .all(|o| o.revoked_at.is_some()));
}

#[test]
fn local_exception_requires_current_epoch_winning_rule_reason_models_and_trusted_clock() {
    let (_dir, mut store, session) = fixture(true);
    let base = request(&session, "exception");
    assert!(store
        .create_override(&base, "2026-10-28T01:00:05Z", false)
        .is_err());
    let mut stale = base.clone();
    stale.expected_phase_epoch = 0;
    assert!(matches!(
        store.create_override(&stale, "2026-10-28T01:00:05Z", true),
        Err(IngestError::StalePhaseEpoch)
    ));
    let mut wrong = base.clone();
    wrong.policy_id = "other".into();
    assert!(store
        .create_override(&wrong, "2026-10-28T01:00:05Z", true)
        .is_err());
    for reason in ["", "  ", "line\nbreak"] {
        let mut invalid = base.clone();
        invalid.reason = reason.into();
        assert!(store
            .create_override(&invalid, "2026-10-28T01:00:05Z", true)
            .is_err());
    }
    for duration in [0, 86_401] {
        let mut invalid = base.clone();
        invalid.duration_seconds = duration;
        assert!(store
            .create_override(&invalid, "2026-10-28T01:00:05Z", true)
            .is_err());
    }
    let mut invalid = base.clone();
    invalid.replacement_model_ids.clear();
    assert!(store
        .create_override(&invalid, "2026-10-28T01:00:05Z", true)
        .is_err());
    assert!(store.override_history(&session).unwrap().is_empty());
    store
        .create_override(&base, "2026-10-28T01:00:05Z", true)
        .unwrap();
    assert!(store
        .revoke_override("exception", "other-session", "2026-10-28T01:00:06Z")
        .is_err());
    store
        .revoke_override("exception", &session, "2026-10-28T01:00:06Z")
        .unwrap();
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:06Z"),
        Decision::Violation
    );
}

#[test]
fn exception_never_creates_served_proof_and_is_inactive_after_phase_change() {
    let (_dir, mut store, session) = fixture(false);
    store
        .create_override(
            &request(&session, "exception"),
            "2026-10-28T01:00:05Z",
            true,
        )
        .unwrap();
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:06Z"),
        Decision::Indeterminate
    );
    let mut phase = event("next", EventType::PhaseChanged, 7);
    phase.source = "aum.user".into();
    phase.phase_at_start = Some(Phase::Planning);
    phase.phase_epoch = Some(2);
    store.change_phase(&phase, 1).unwrap();
    assert_eq!(
        decision(&store, &session, "2026-10-28T01:00:08Z"),
        Decision::NotApplicable
    );
    assert!(matches!(
        store.create_override(&request(&session, "stale"), "2026-10-28T01:00:08Z", true),
        Err(IngestError::StalePhaseEpoch)
    ));
}
