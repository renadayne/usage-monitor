use adapters::{copilot, metadata::ObservationContext};
use collector::{ingest::IntegrationRegistration, Store};
use normalized_schema::{usage::UsageScope, Provider};

#[test]
fn selected_leaf_series_counts_once_without_parent_double_count_or_cache_addition() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = Store::open(dir.path().join("copilot.db")).unwrap();
    store
        .register_integration(&IntegrationRegistration {
            id: "copilot".into(),
            account_key: "opaque".into(),
            provider: Provider::Copilot,
            surface: "copilot_vscode".into(),
        })
        .unwrap();
    let context = ObservationContext {
        integration_id: "copilot".into(),
        account_key: "opaque".into(),
        event_id: "delivery".into(),
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: "2026-10-28T01:00:02Z".into(),
    };
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/copilot/synthetic-otel-spans.json"
    ))
    .unwrap();
    let parent = copilot::normalize_completed_span(
        &fixture["parent"].to_string(),
        "synthetic-session",
        &context,
    )
    .unwrap();
    let leaf = copilot::normalize_completed_span(
        &fixture["leaf"].to_string(),
        "synthetic-session",
        &context,
    )
    .unwrap();
    // The adapter requires a known session and an explicit trace-to-conversation binding.
    let start=serde_json::from_value(serde_json::json!({"schemaVersion":1,"eventId":"known-session-start","type":"session.started","integrationId":"copilot","accountKey":"opaque","provider":"copilot","surface":"copilot_vscode","source":"copilot.otel","sourceSessionId":"synthetic-session","observedAt":"2026-10-28T00:00:00Z","receivedAt":"2026-10-28T00:00:01Z","modelEvidenceKind":"unknown"})).unwrap();
    let session = store.ingest(&start).unwrap().session_id;
    store.ingest(&parent.event).unwrap();
    store
        .select_accounting_series(
            &session,
            "copilot.otel",
            "copilot.chat.tokens",
            UsageScope::Inference,
        )
        .unwrap();
    store.ingest_usage(parent.usage.as_ref().unwrap()).unwrap();
    store.ingest_usage(leaf.usage.as_ref().unwrap()).unwrap();
    assert!(
        store
            .ingest_usage(leaf.usage.as_ref().unwrap())
            .unwrap()
            .already_committed
    );
    let usage = store.session_usage(&session, None).unwrap();
    let input = usage.items.iter().find(|i| i.metric == "input").unwrap();
    assert_eq!(input.value, 100.0);
    let output = usage.items.iter().find(|i| i.metric == "output").unwrap();
    assert_eq!(output.value, 20.0);
    let cached = usage
        .items
        .iter()
        .find(|i| i.metric == "cache_read")
        .unwrap();
    assert_eq!(cached.value, 50.0);
    assert!(usage.partial);
}
