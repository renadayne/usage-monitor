use collector::{
    ingest::{IngestError, IntegrationRegistration},
    quota::{AccountQuota, QuotaObservation},
    Store,
};
use normalized_schema::{quota::QuotaSnapshot, Confidence, Provider};
use rusqlite::Connection;
use tempfile::TempDir;

const NOW: &str = "2026-10-28T01:00:01Z";
fn setup() -> (TempDir, Store) {
    let dir = TempDir::new().unwrap();
    let mut store = Store::open(dir.path().join("quota.db")).unwrap();
    register(&mut store, "integration", "account", Provider::Codex);
    (dir, store)
}
fn register(store: &mut Store, id: &str, account: &str, provider: Provider) {
    store
        .register_integration(&IntegrationRegistration {
            id: id.into(),
            account_key: account.into(),
            provider,
            surface: "synthetic".into(),
        })
        .unwrap();
}
fn snapshot(window: &str) -> QuotaSnapshot {
    serde_json::from_value(serde_json::json!({"quotaKey":"caller-untrusted","provider":"codex","accountKey":"account","scope":"account","bucket":"primary","unit":"percentage","windowKey":window,"usedPercent":62.5,"observedAt":"2026-10-28T01:00:00Z","staleAfter":"2026-10-28T01:05:00Z","resetsAt":"2026-10-28T02:00:00Z"})).unwrap()
}
fn observation(id: &str, revision: i64, windows: &[&str]) -> QuotaObservation {
    QuotaObservation {
        schema_version: 1,
        event_id: id.into(),
        integration_id: "integration".into(),
        account_key: "account".into(),
        provider: Provider::Codex,
        surface: "synthetic".into(),
        source: "synthetic.quota".into(),
        source_event_id: Some(id.into()),
        source_sequence: Some(revision),
        confidence: Confidence::Direct,
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: NOW.into(),
        snapshots: windows.iter().map(|w| snapshot(w)).collect(),
    }
}
fn read(store: &Store) -> Vec<AccountQuota> {
    store
        .account_quotas(Provider::Codex, "account", NOW, true)
        .unwrap()
}
fn scalar(store: &Store, sql: &str) -> i64 {
    Connection::open(store.path())
        .unwrap()
        .query_row(sql, [], |r| r.get(0))
        .unwrap()
}

#[test]
fn independent_windows_keep_missing_values_and_omitted_snapshots_across_restart() {
    let (_dir, mut store) = setup();
    let mut first = observation("first", 1, &["five_hour", "seven_day"]);
    first.snapshots[1].used_percent = None;
    assert_eq!(store.observe_quotas(&first).unwrap().updated, 2);
    let mut next = observation("next", 2, &["five_hour"]);
    next.snapshots[0].used_percent = Some(125.0);
    store.observe_quotas(&next).unwrap();
    let path = store.path().to_owned();
    drop(store);
    let store = Store::open(path).unwrap();
    let values = read(&store);
    assert_eq!(values.len(), 2);
    let five = values
        .iter()
        .find(|v| v.snapshot.window_key == "five_hour")
        .unwrap();
    let week = values
        .iter()
        .find(|v| v.snapshot.window_key == "seven_day")
        .unwrap();
    assert_eq!(five.snapshot.used_percent, Some(125.0));
    assert_eq!(week.snapshot.used_percent, None);
    assert_eq!(week.snapshot.remaining, None);
    assert_eq!(week.snapshot.limit, None);
    assert_ne!(five.snapshot.quota_key, "caller-untrusted");
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM sessions"), 0);
    assert_eq!(
        scalar(
            &store,
            "SELECT COUNT(*) FROM events WHERE session_id IS NULL"
        ),
        2
    );
    assert_eq!(store.export_history(7, NOW).unwrap().rows, 0);
}

#[test]
fn provider_account_unit_and_window_identities_do_not_collide() {
    let (_dir, mut store) = setup();
    store
        .observe_quotas(&observation("codex", 1, &["five_hour"]))
        .unwrap();
    register(&mut store, "other-account", "another", Provider::Codex);
    let mut another = observation("account", 1, &["five_hour"]);
    another.integration_id = "other-account".into();
    another.account_key = "another".into();
    another.snapshots[0].account_key = "another".into();
    store.observe_quotas(&another).unwrap();
    register(&mut store, "claude", "account", Provider::ClaudeCode);
    let mut claude = observation("claude", 1, &["five_hour"]);
    claude.integration_id = "claude".into();
    claude.provider = Provider::ClaudeCode;
    claude.snapshots[0].provider = Provider::ClaudeCode;
    store.observe_quotas(&claude).unwrap();
    let mut unit = observation("unit", 2, &["five_hour"]);
    unit.snapshots[0].unit = normalized_schema::quota::QuotaUnit::Requests;
    unit.snapshots[0].remaining = Some(-5.0);
    store.observe_quotas(&unit).unwrap();
    assert_eq!(read(&store).len(), 2);
    assert_eq!(
        store
            .account_quotas(Provider::ClaudeCode, "account", NOW, true)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        store
            .account_quotas(Provider::Codex, "another", NOW, true)
            .unwrap()
            .len(),
        1
    );
    let mut invalid = observation("bad", 3, &["five_hour"]);
    invalid.account_key = "different".into();
    invalid.snapshots[0].account_key = "different".into();
    assert!(matches!(
        store.observe_quotas(&invalid),
        Err(IngestError::IntegrationBindingConflict)
    ));
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM events"), 4);
}

#[test]
fn retry_identity_ignores_receipt_and_caller_quota_keys_but_rejects_changed_body() {
    let (_dir, mut store) = setup();
    let first = observation("event", 1, &["five_hour"]);
    let accepted = store.observe_quotas(&first).unwrap();
    let mut retry = first.clone();
    retry.received_at = "2026-10-28T01:00:02Z".into();
    retry.snapshots[0].quota_key = "other-caller-key".into();
    let repeated = store.observe_quotas(&retry).unwrap();
    assert!(repeated.already_committed);
    assert_eq!(accepted.event_key, repeated.event_key);
    retry.snapshots[0].used_percent = Some(99.0);
    assert!(matches!(
        store.observe_quotas(&retry),
        Err(IngestError::EventIdentityConflict)
    ));
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM events"), 1);
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM outbox"), 1);
}

#[test]
fn late_unsequenced_cross_source_and_unverified_history_cannot_advance_direct_head() {
    let (_dir, mut store) = setup();
    let first = observation("head", 10, &["five_hour"]);
    store.observe_quotas(&first).unwrap();
    for (id, seq, source, confidence) in [
        ("late", Some(9), "synthetic.quota", Confidence::Direct),
        ("unordered", None, "synthetic.quota", Confidence::Direct),
        ("other", Some(11), "other.source", Confidence::Direct),
        (
            "unverified",
            Some(999),
            "synthetic.quota",
            Confidence::Unknown,
        ),
    ] {
        let mut event = observation(id, 11, &["five_hour"]);
        event.source_sequence = seq;
        event.source = source.into();
        event.confidence = confidence;
        event.snapshots[0].used_percent = Some(99.0);
        assert_eq!(store.observe_quotas(&event).unwrap().updated, 0);
    }
    assert_eq!(read(&store)[0].snapshot.used_percent, Some(62.5));
    let mut next = observation("ordered", 11, &["five_hour"]);
    next.snapshots[0].used_percent = Some(63.0);
    assert_eq!(store.observe_quotas(&next).unwrap().updated, 1);
    assert_eq!(
        scalar(
            &store,
            "SELECT COUNT(*) FROM quota_evidence WHERE disposition='history'"
        ),
        4
    );
}

#[test]
fn unverified_revision_cannot_poison_later_direct_observation() {
    let (_dir, mut store) = setup();
    let mut unknown = observation("unknown", 999, &["five_hour"]);
    unknown.confidence = Confidence::Unknown;
    store.observe_quotas(&unknown).unwrap();
    let direct = observation("direct", 1, &["five_hour"]);
    assert_eq!(store.observe_quotas(&direct).unwrap().updated, 1);
    assert_eq!(read(&store)[0].confidence, "direct");
}

#[test]
fn freshness_requires_verified_source_and_trusted_clock_and_expires_exactly() {
    let (_dir, mut store) = setup();
    store
        .observe_quotas(&observation("first", 1, &["five_hour"]))
        .unwrap();
    assert_eq!(read(&store)[0].freshness, "unknown");
    Connection::open(store.path())
        .unwrap()
        .execute("UPDATE integrations SET target_verification='verified'", [])
        .unwrap();
    assert_eq!(read(&store)[0].freshness, "fresh");
    assert_eq!(
        store
            .account_quotas(Provider::Codex, "account", "2026-10-28T01:05:00Z", true)
            .unwrap()[0]
            .freshness,
        "stale"
    );
    assert_eq!(
        store
            .account_quotas(Provider::Codex, "account", "2026-10-28T02:00:00Z", true)
            .unwrap()[0]
            .freshness,
        "expired"
    );
    assert_eq!(
        store
            .account_quotas(Provider::Codex, "account", NOW, false)
            .unwrap()[0]
            .freshness,
        "unknown"
    );
    let mut old = observation("expired", 2, &["old-window"]);
    old.snapshots[0].resets_at = Some("2026-10-28T00:00:00Z".into());
    store.observe_quotas(&old).unwrap();
    assert_eq!(
        read(&store)
            .iter()
            .find(|q| q.snapshot.window_key == "old-window")
            .unwrap()
            .freshness,
        "expired"
    );
}

#[test]
fn outbox_failure_rolls_back_quota_evidence_event_and_last_seen_atomically() {
    let (_dir, mut store) = setup();
    let first = observation("first", 1, &["five_hour"]);
    store.observe_quotas(&first).unwrap();
    let db = Connection::open(store.path()).unwrap();
    db.execute_batch("CREATE TRIGGER fail_quota BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    let mut second = observation("second", 2, &["five_hour", "seven_day"]);
    second.received_at = "2026-10-28T01:00:02Z".into();
    second.snapshots[0].used_percent = Some(90.0);
    assert!(store.observe_quotas(&second).is_err());
    assert_eq!(read(&store).len(), 1);
    assert_eq!(read(&store)[0].snapshot.used_percent, Some(62.5));
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM events"), 1);
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM quota_evidence"), 1);
    let last_seen: String = db
        .query_row("SELECT last_seen_at FROM integrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(last_seen, "2026-10-28T01:00:01.000000000Z");
}

#[test]
fn sessions_reference_shared_account_quota_without_copying_or_adding_it() {
    let (_dir, mut store) = setup();
    store
        .observe_quotas(&observation("quota", 1, &["five_hour", "seven_day"]))
        .unwrap();
    let mut ids = Vec::new();
    for id in ["session-a", "session-b"] {
        let event = serde_json::from_value(serde_json::json!({
            "schemaVersion":1,"eventId":id,"type":"session.started","integrationId":"integration",
            "accountKey":"account","provider":"codex","surface":"synthetic","source":"synthetic.session",
            "sourceSessionId":id,"observedAt":NOW,"receivedAt":NOW,"confidence":"direct","modelEvidenceKind":"unknown"
        })).unwrap();
        let outcome = store.ingest(&event).unwrap();
        let detail = store
            .session_detail(&outcome.session_id, None, NOW, true)
            .unwrap();
        assert_eq!(detail.quotas.len(), 2);
        assert!(detail.usage.items.is_empty());
        ids.push(
            detail
                .quotas
                .into_iter()
                .map(|q| q.snapshot.quota_key)
                .collect::<Vec<_>>(),
        );
    }
    assert_eq!(ids[0], ids[1]);
    assert_eq!(scalar(&store, "SELECT COUNT(*) FROM quotas"), 2);
    assert_eq!(store.export_history(7, NOW).unwrap().rows, 2);
}
