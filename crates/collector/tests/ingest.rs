use collector::ingest::{IngestError, IntegrationRegistration};
use collector::Store;
use normalized_schema::{
    Activity, Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Phase, Provider,
};
use rusqlite::Connection;
use tempfile::TempDir;

fn registration(id: &str, account: &str) -> IntegrationRegistration {
    IntegrationRegistration {
        id: id.into(),
        account_key: account.into(),
        provider: Provider::Codex,
        surface: "synthetic".into(),
    }
}
fn event(id: &str) -> NormalizedEvent {
    NormalizedEvent {
        schema_version: 1,
        event_id: id.into(),
        event_type: EventType::SessionStarted,
        integration_id: "integration".into(),
        account_key: "account".into(),
        provider: Provider::Codex,
        surface: "synthetic".into(),
        source: "synthetic.hooks".into(),
        source_event_id: Some(id.into()),
        source_sequence: Some(1),
        source_session_id: "source-session".into(),
        local_session_id: None,
        source_turn_id: None,
        source_inference_id: None,
        parent_agent_id: None,
        phase_at_start: None,
        phase_epoch: None,
        model_selected: None,
        model_requested: None,
        model_resolved: None,
        model_observed: None,
        model_evidence_kind: ModelEvidenceKind::Unknown,
        evidence_ref: None,
        observed_at: "2026-10-27T00:00:00Z".into(),
        received_at: "2026-10-27T00:00:01Z".into(),
        evidence_expires_at: None,
        confidence: Confidence::Direct,
        activity: Some(Activity::Working),
    }
}

#[test]
fn restart_retry_is_idempotent_and_account_namespaces_do_not_collide() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("retry.db");
    let original;
    {
        let mut store = Store::open(&path).unwrap();
        store
            .register_integration(&registration("integration", "account"))
            .unwrap();
        original = store.ingest(&event("event")).unwrap();
        assert!(!original.already_committed);
    }
    let mut store = Store::open(&path).unwrap();
    let mut retry = event("event");
    retry.received_at = "2026-10-27T00:00:02+00:00".into();
    assert!(store.ingest(&retry).unwrap().already_committed);
    retry.source_sequence = Some(2);
    assert!(matches!(
        store.ingest(&retry),
        Err(IngestError::EventIdentityConflict)
    ));
    assert!(matches!(
        store.register_integration(&registration("integration", "other")),
        Err(IngestError::IntegrationBindingConflict)
    ));
    store
        .register_integration(&registration("integration-2", "account-2"))
        .unwrap();
    let mut second = event("event");
    second.integration_id = "integration-2".into();
    second.account_key = "account-2".into();
    let separate = store.ingest(&second).unwrap();
    assert_ne!(separate.event_key, original.event_key);
    assert_ne!(separate.session_id, original.session_id);
    let db = Connection::open(&path).unwrap();
    for table in ["events", "sessions", "outbox", "session_projection"] {
        let count: i64 = db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}

#[test]
fn outbox_failure_rolls_back_event_snapshot_and_checkpoint_then_retry_commits() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("rollback.db");
    let mut store = Store::open(&path).unwrap();
    store
        .register_integration(&registration("integration", "account"))
        .unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    assert!(matches!(
        store.ingest(&event("event")),
        Err(IngestError::Sqlite(_))
    ));
    for table in ["events", "sessions", "outbox", "session_projection"] {
        let count: i64 = db
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }
    db.execute_batch("DROP TRIGGER fail_outbox;").unwrap();
    assert!(!store.ingest(&event("event")).unwrap().already_committed);
}

#[test]
fn phase_authority_and_epoch_checks_prevent_old_or_cross_stream_snapshot_changes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("phase.db");
    let mut store = Store::open(&path).unwrap();
    store
        .register_integration(&registration("integration", "account"))
        .unwrap();
    let start = store.ingest(&event("start")).unwrap();
    let mut phase = event("phase");
    phase.event_type = EventType::PhaseChanged;
    phase.source = "aum.user".into();
    phase.phase_at_start = Some(Phase::Review);
    phase.phase_epoch = Some(1);
    phase.activity = None;
    phase.observed_at = "2026-10-27T00:00:02Z".into();
    phase.received_at = phase.observed_at.clone();
    assert!(
        store.ingest(&phase).is_err(),
        "a source label must not grant phase-change authority"
    );
    store.change_phase(&phase, 0).unwrap();
    assert!(store.change_phase(&phase, 0).unwrap().already_committed);
    let mut stale = phase.clone();
    stale.event_id = "stale-phase".into();
    stale.source_event_id = Some("stale-phase".into());
    assert!(matches!(
        store.change_phase(&stale, 0),
        Err(IngestError::StalePhaseEpoch)
    ));
    let mut end = event("old-end");
    end.event_type = EventType::SessionEnded;
    end.source_sequence = Some(2);
    end.activity = Some(Activity::Completed);
    end.phase_epoch = Some(0);
    end.observed_at = "2026-10-27T00:00:03Z".into();
    end.received_at = end.observed_at.clone();
    store.ingest(&end).unwrap();
    let state = store.list_sessions(10, None).unwrap().items.remove(0);
    assert_eq!(state.phase, "REVIEW");
    assert_eq!(state.activity, "UNKNOWN");
    assert_eq!(state.phase_epoch, 1);
    end.event_id = "cross-source-end".into();
    end.source_event_id = Some("cross-source-end".into());
    end.source = "another.stream".into();
    end.phase_epoch = Some(1);
    store.ingest(&end).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0].activity,
        "UNKNOWN"
    );
    end.event_id = "ordered-end".into();
    end.source_event_id = Some("ordered-end".into());
    end.source = "synthetic.hooks".into();
    end.phase_at_start = Some(Phase::Review);
    store.ingest(&end).unwrap();
    assert_eq!(
        store.list_sessions(10, None).unwrap().items[0].activity,
        "COMPLETED"
    );
    assert_eq!(
        store
            .list_session_events(&start.session_id, 20, None)
            .unwrap()
            .items
            .len(),
        5
    );
}

#[test]
fn invalid_unsupported_and_rebound_events_have_no_side_effects() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("invalid.db");
    let mut store = Store::open(&path).unwrap();
    store
        .register_integration(&registration("integration", "account"))
        .unwrap();
    let mut invalid = event("invalid");
    invalid.account_key = "different-account".into();
    assert!(matches!(
        store.ingest(&invalid),
        Err(IngestError::IntegrationBindingConflict)
    ));
    invalid = event("unsupported");
    invalid.event_type = EventType::InferenceStarted;
    invalid.source_inference_id = Some("inference".into());
    assert!(matches!(
        store.ingest(&invalid),
        Err(IngestError::UnknownSession)
    ));
    invalid = event("model");
    invalid.model_observed = Some("model-a".into());
    assert!(store.ingest(&invalid).is_err());
    invalid = event("identity");
    invalid.local_session_id = Some("forged-local-id".into());
    assert!(store.ingest(&invalid).is_err());
    assert!(store.list_sessions(10, None).unwrap().items.is_empty());
}
