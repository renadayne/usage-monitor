use std::path::PathBuf;

use collector::{
    ingest::{IngestError, IntegrationRegistration},
    usage::{UsageSummary, UsageTotal},
    Store,
};
use normalized_schema::{
    usage::{
        RelationToTotal, UsageMetric, UsageMode, UsageReport, UsageSample, UsageScope, UsageUnit,
    },
    Activity, Confidence, EventType, NormalizedEvent, Provider, ValidationError, SCHEMA_VERSION,
};
use rusqlite::Connection;
use tempfile::TempDir;

const INTEGRATION_ID: &str = "int-synth-1";
const ACCOUNT_KEY: &str = "acct-synth-1";
const SOURCE_PRIMARY: &str = "synth.adapter.primary";
const SOURCE_ALT: &str = "synth.adapter.alt";
const SERIES_SESSION: &str = "series.session.tokens";
const SERIES_INFERENCE: &str = "series.inference.tokens";

#[test]
fn unverified_samples_cannot_poison_direct_counter_order_or_sample_identity() {
    let (_dir, _path, mut store) = setup_store();
    let session = start_session(&mut store, "trust");
    store
        .select_accounting_series(
            &session,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session,
        )
        .unwrap();
    let sample = cumulative_sample("shared-sample", "trust", "epoch", 1, 100.0, true);
    let mut unverified = report("unverified", "trust", SOURCE_PRIMARY, vec![sample.clone()]);
    unverified.event.confidence = Confidence::Unknown;
    unverified.samples[0].mode = UsageMode::Delta;
    unverified.samples[0].known_zero = false;
    unverified.samples[0].value = 999.0;
    store.ingest_usage(&unverified).unwrap();
    store
        .ingest_usage(&report(
            "direct",
            "trust",
            SOURCE_PRIMARY,
            vec![sample.clone()],
        ))
        .unwrap();
    assert_eq!(
        store.session_usage(&session, None).unwrap().items[0].value,
        100.0
    );
    let mut next = sample;
    next.sample_id = "next".into();
    next.revision = Some(2);
    next.value = 150.0;
    next.relation_to_total = RelationToTotal::Exclusive;
    assert!(store
        .ingest_usage(&report(
            "changed-relation",
            "trust",
            SOURCE_PRIMARY,
            vec![next]
        ))
        .is_err());
    assert_eq!(
        store.session_usage(&session, None).unwrap().items[0].value,
        100.0
    );
}

fn setup_store() -> (TempDir, PathBuf, Store) {
    let dir = TempDir::new().expect("tempdir");
    let db_path = dir.path().join("usage.db");
    let mut store = Store::open(&db_path).expect("open store");
    store
        .register_integration(&IntegrationRegistration {
            id: INTEGRATION_ID.into(),
            account_key: ACCOUNT_KEY.into(),
            provider: Provider::Codex,
            surface: "cli".into(),
        })
        .expect("register integration");
    (dir, db_path, store)
}

fn base_event(id: &str, event_type: EventType, session: &str, source: &str) -> NormalizedEvent {
    serde_json::from_value(serde_json::json!({
        "schemaVersion": SCHEMA_VERSION,
        "eventId": id,
        "type": event_type,
        "integrationId": INTEGRATION_ID,
        "accountKey": ACCOUNT_KEY,
        "provider": "codex",
        "surface": "cli",
        "source": source,
        "sourceEventId": id,
        "sourceSequence": 1,
        "sourceSessionId": session,
        "modelEvidenceKind": "unknown",
        "observedAt": "2026-03-01T10:00:00Z",
        "receivedAt": "2026-03-01T10:00:01Z",
        "confidence": "direct",
    }))
    .expect("valid base NormalizedEvent")
}

fn start_session(store: &mut Store, source_session: &str) -> String {
    let mut ev = base_event(
        &format!("start-{source_session}"),
        EventType::SessionStarted,
        source_session,
        SOURCE_PRIMARY,
    );
    ev.activity = Some(Activity::Working);
    store.ingest(&ev).expect("start session").session_id
}

fn cumulative_sample(
    id: &str,
    scope_key: &str,
    epoch: &str,
    rev: i64,
    val: f64,
    kz: bool,
) -> UsageSample {
    UsageSample {
        sample_id: id.into(),
        mode: UsageMode::Cumulative,
        scope: UsageScope::Session,
        scope_key: scope_key.into(),
        counter_key: "ctr-input".into(),
        counter_epoch: epoch.into(),
        revision: Some(rev),
        unit: UsageUnit::Tokens,
        metric: UsageMetric::Input,
        value: val,
        accounting_series: SERIES_SESSION.into(),
        relation_to_total: RelationToTotal::Included,
        known_zero: kz,
    }
}

#[allow(clippy::too_many_arguments)]
fn delta_sample(
    id: &str,
    scope: UsageScope,
    scope_key: &str,
    series: &str,
    unit: UsageUnit,
    metric: UsageMetric,
    rel: RelationToTotal,
    val: f64,
) -> UsageSample {
    UsageSample {
        sample_id: id.into(),
        mode: UsageMode::Delta,
        scope,
        scope_key: scope_key.into(),
        counter_key: id.into(),
        counter_epoch: "ep-1".into(),
        revision: None,
        unit,
        metric,
        value: val,
        accounting_series: series.into(),
        relation_to_total: rel,
        known_zero: false,
    }
}

fn report(id: &str, session: &str, source: &str, samples: Vec<UsageSample>) -> UsageReport {
    UsageReport {
        event: base_event(id, EventType::UsageReported, session, source),
        samples,
    }
}

fn total_row(unit: &str, metric: &str, rel: &str, value: f64) -> UsageTotal {
    UsageTotal {
        unit: unit.into(),
        metric: metric.into(),
        relation_to_total: rel.into(),
        value,
    }
}

#[test]
fn cumulative_reconciliation_restart_freezing_and_new_epoch() {
    let (_dir, db_path, mut store) = setup_store();
    let session_id = start_session(&mut store, "sess-cum");
    store
        .select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session,
        )
        .unwrap();

    // Case 1: known-zero rev1=100, rev3=150, restart+retry rev3, late rev2=120 -> total 150.
    let r1 = report(
        "ev-r1",
        "sess-cum",
        SOURCE_PRIMARY,
        vec![cumulative_sample(
            "s-r1", "sess-cum", "ep-1", 1, 100.0, true,
        )],
    );
    let r3 = report(
        "ev-r3",
        "sess-cum",
        SOURCE_PRIMARY,
        vec![cumulative_sample(
            "s-r3", "sess-cum", "ep-1", 3, 150.0, false,
        )],
    );
    assert!(!store.ingest_usage(&r1).unwrap().already_committed);
    assert!(!store.ingest_usage(&r3).unwrap().already_committed);

    drop(store);
    let mut store = Store::open(&db_path).expect("reopen store");
    assert!(store.ingest_usage(&r3).unwrap().already_committed);

    let r2_late = report(
        "ev-r2",
        "sess-cum",
        SOURCE_PRIMARY,
        vec![cumulative_sample(
            "s-r2", "sess-cum", "ep-1", 2, 120.0, false,
        )],
    );
    store.ingest_usage(&r2_late).unwrap();
    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![total_row("tokens", "input", "included", 150.0)],
            partial: false
        }
    );

    // Conflicting existing revision rejects without mutating totals.
    let r2_conflict = report(
        "ev-r2-bad",
        "sess-cum",
        SOURCE_PRIMARY,
        vec![cumulative_sample(
            "s-r2-bad", "sess-cum", "ep-1", 2, 125.0, false,
        )],
    );
    assert!(matches!(
        store.ingest_usage(&r2_conflict),
        Err(IngestError::Invalid(
            "counter revision has conflicting content"
        ))
    ));

    // Case 2: decrease rev4=10 freezes epoch, recovery rev5=200 ignored, new epoch known-zero=10 adds 10 -> 160 partial.
    for (ev, sid, ep, rev, val, kz) in [
        ("ev-r4", "s-r4", "ep-1", 4, 10.0, false),
        ("ev-r5", "s-r5", "ep-1", 5, 200.0, false),
        ("ev-ep2-r1", "s-ep2-r1", "ep-2", 1, 10.0, true),
    ] {
        store
            .ingest_usage(&report(
                ev,
                "sess-cum",
                SOURCE_PRIMARY,
                vec![cumulative_sample(sid, "sess-cum", ep, rev, val, kz)],
            ))
            .unwrap();
    }
    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![total_row("tokens", "input", "included", 160.0)],
            partial: true
        }
    );
}

#[test]
fn mid_counter_attach_contributes_post_baseline_delta_and_marks_partial() {
    let (_dir, _db_path, mut store) = setup_store();
    let session_id = start_session(&mut store, "sess-mid");
    store
        .select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session,
        )
        .unwrap();

    store
        .ingest_usage(&report(
            "ev-mid-1",
            "sess-mid",
            SOURCE_PRIMARY,
            vec![cumulative_sample(
                "s-mid-1", "sess-mid", "ep-1", 1, 100.0, false,
            )],
        ))
        .unwrap();
    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![],
            partial: true
        }
    );

    store
        .ingest_usage(&report(
            "ev-mid-2",
            "sess-mid",
            SOURCE_PRIMARY,
            vec![cumulative_sample(
                "s-mid-2", "sess-mid", "ep-1", 2, 150.0, false,
            )],
        ))
        .unwrap();
    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![total_row("tokens", "input", "included", 50.0)],
            partial: true
        }
    );
}

#[test]
fn canonical_inference_series_reconciliation_only_copies_and_delta_dedup() {
    let (_dir, db_path, mut store) = setup_store();
    let session_id = start_session(&mut store, "sess-inf");
    store
        .select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_INFERENCE,
            UsageScope::Inference,
        )
        .unwrap();

    assert!(matches!(
        store.select_accounting_series(
            &session_id,
            SOURCE_ALT,
            SERIES_INFERENCE,
            UsageScope::Inference
        ),
        Err(IngestError::Invalid(
            "canonical accounting changes require reconciliation"
        ))
    ));
    assert!(matches!(
        store.select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session
        ),
        Err(IngestError::Invalid(
            "canonical accounting changes require reconciliation"
        ))
    ));
    assert!(matches!(
        store.select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_INFERENCE,
            UsageScope::Account
        ),
        Err(IngestError::Invalid(
            "account counters cannot become session totals"
        ))
    ));

    let inf_delta = delta_sample(
        "s-inf-1",
        UsageScope::Inference,
        "inf-1",
        SERIES_INFERENCE,
        UsageUnit::Tokens,
        UsageMetric::Input,
        RelationToTotal::Included,
        40.0,
    );
    let parent_copy = cumulative_sample("s-parent-1", "sess-inf", "ep-1", 1, 400.0, true);

    let mut ev1 = base_event(
        "ev-inf-1",
        EventType::UsageReported,
        "sess-inf",
        SOURCE_PRIMARY,
    );
    ev1.source_inference_id = Some("inf-1".into());
    store
        .ingest_usage(&UsageReport {
            event: ev1,
            samples: vec![inf_delta.clone(), parent_copy],
        })
        .unwrap();

    let mut ev_alt = base_event(
        "ev-inf-alt",
        EventType::UsageReported,
        "sess-inf",
        SOURCE_ALT,
    );
    ev_alt.source_inference_id = Some("inf-1".into());
    store
        .ingest_usage(&UsageReport {
            event: ev_alt,
            samples: vec![UsageSample {
                sample_id: "s-alt-1".into(),
                ..inf_delta.clone()
            }],
        })
        .unwrap();

    let mut ev_redelivery = base_event(
        "ev-inf-2",
        EventType::UsageReported,
        "sess-inf",
        SOURCE_PRIMARY,
    );
    ev_redelivery.source_inference_id = Some("inf-1".into());
    store
        .ingest_usage(&UsageReport {
            event: ev_redelivery,
            samples: vec![inf_delta],
        })
        .unwrap();

    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![total_row("tokens", "input", "included", 40.0)],
            partial: false
        }
    );

    let conn = Connection::open(&db_path).unwrap();
    let rec_only: i64 = conn.query_row("SELECT COUNT(*) FROM usage_evidence WHERE session_id=?1 AND disposition='reconciliation_only'", [&session_id], |r| r.get(0)).unwrap();
    assert_eq!(rec_only, 2);
}

#[test]
fn outbox_failure_trigger_rolls_back_all_tables_and_succeeds_on_retry() {
    let (_dir, db_path, mut store) = setup_store();
    let session_id = start_session(&mut store, "sess-tx");
    store
        .select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session,
        )
        .unwrap();

    let conn = Connection::open(&db_path).unwrap();
    conn.execute_batch("CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT, 'fail'); END;").unwrap();

    let rep = report(
        "ev-tx-1",
        "sess-tx",
        SOURCE_PRIMARY,
        vec![cumulative_sample(
            "s-tx-1", "sess-tx", "ep-1", 1, 75.0, true,
        )],
    );
    assert!(matches!(
        store.ingest_usage(&rep),
        Err(IngestError::Sqlite(_))
    ));

    for table in ["usage_checkpoints", "usage_evidence", "usage_contributions"] {
        let count: i64 = conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0, "{table} must roll back");
    }
    let usage_events: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM events WHERE type='usage.reported'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(usage_events, 0);

    conn.execute_batch("DROP TRIGGER fail_outbox;").unwrap();
    assert!(!store.ingest_usage(&rep).unwrap().already_committed);
    assert_eq!(
        store.session_usage(&session_id, None).unwrap().items[0].value,
        75.0
    );
}

#[test]
fn separate_unit_metric_relation_rows_unknown_confidence_and_malformed_rejections() {
    let (_dir, _db_path, mut store) = setup_store();
    let session_id = start_session(&mut store, "sess-multi");
    store
        .select_accounting_series(
            &session_id,
            SOURCE_PRIMARY,
            SERIES_SESSION,
            UsageScope::Session,
        )
        .unwrap();

    let ds = |id: &str, unit: UsageUnit, metric: UsageMetric, rel: RelationToTotal, val: f64| {
        delta_sample(
            id,
            UsageScope::Session,
            "sess-multi",
            SERIES_SESSION,
            unit,
            metric,
            rel,
            val,
        )
    };
    let multi = report(
        "ev-multi",
        "sess-multi",
        SOURCE_PRIMARY,
        vec![
            ds(
                "s-in",
                UsageUnit::Tokens,
                UsageMetric::Input,
                RelationToTotal::Included,
                100.0,
            ),
            ds(
                "s-rsn",
                UsageUnit::Tokens,
                UsageMetric::Reasoning,
                RelationToTotal::Included,
                25.0,
            ),
            ds(
                "s-cr",
                UsageUnit::Tokens,
                UsageMetric::CacheRead,
                RelationToTotal::Exclusive,
                30.0,
            ),
            ds(
                "s-req",
                UsageUnit::Requests,
                UsageMetric::Requests,
                RelationToTotal::Included,
                2.0,
            ),
            ds(
                "s-crd",
                UsageUnit::AiCredits,
                UsageMetric::Credits,
                RelationToTotal::Unknown,
                1.5,
            ),
            ds(
                "s-usd",
                UsageUnit::Usd,
                UsageMetric::Cost,
                RelationToTotal::Included,
                0.25,
            ),
        ],
    );
    store.ingest_usage(&multi).unwrap();

    let mut unverified = report(
        "ev-unv",
        "sess-multi",
        SOURCE_PRIMARY,
        vec![ds(
            "s-unv",
            UsageUnit::Tokens,
            UsageMetric::Input,
            RelationToTotal::Included,
            999.0,
        )],
    );
    unverified.event.confidence = Confidence::Unknown;
    store.ingest_usage(&unverified).unwrap();

    assert_eq!(
        store.session_usage(&session_id, None).unwrap(),
        UsageSummary {
            items: vec![
                total_row("ai_credits", "credits", "unknown", 1.5),
                total_row("requests", "requests", "included", 2.0),
                total_row("tokens", "cache_read", "exclusive", 30.0),
                total_row("tokens", "input", "included", 100.0),
                total_row("tokens", "reasoning", "included", 25.0),
                total_row("usd", "cost", "included", 0.25),
            ],
            partial: true,
        }
    );

    let mut bad_scope = ds(
        "s-scope",
        UsageUnit::Tokens,
        UsageMetric::Input,
        RelationToTotal::Included,
        10.0,
    );
    bad_scope.scope_key = "other-session".into();
    let bad_samples = [
        ds(
            "s-nan",
            UsageUnit::Tokens,
            UsageMetric::Input,
            RelationToTotal::Included,
            f64::NAN,
        ),
        ds(
            "s-neg",
            UsageUnit::Tokens,
            UsageMetric::Input,
            RelationToTotal::Included,
            -5.0,
        ),
        bad_scope,
        ds(
            "s-unit",
            UsageUnit::Tokens,
            UsageMetric::Cost,
            RelationToTotal::Included,
            10.0,
        ),
    ];
    for (idx, sample) in bad_samples.into_iter().enumerate() {
        let bad_rep = report(
            &format!("ev-bad-{idx}"),
            "sess-multi",
            SOURCE_PRIMARY,
            vec![sample],
        );
        assert!(matches!(
            store.ingest_usage(&bad_rep),
            Err(IngestError::Validation(ValidationError::InvalidUsage(_)))
        ));
    }
}
