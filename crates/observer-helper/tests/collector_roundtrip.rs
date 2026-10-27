//! Synthetic local transport test. No provider hooks or credentials are accessed.
use adapters::codex::Surface;
use collector::Store;
use observer_helper::{parse_config, process_hook, DeliveryOutcome};
use observer_runtime::{start, ObserverBinding};
use std::{
    io::Cursor,
    time::{Duration, Instant},
};

#[tokio::test]
async fn sanitized_helper_delivery_reaches_durable_store_over_loopback() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("roundtrip.db");
    let store = Store::open(&path).unwrap();
    let token = "b".repeat(64);
    let binding = ObserverBinding::new(
        "codex".into(),
        "local".into(),
        Surface::VsCodeExtension,
        token.clone(),
    )
    .unwrap();
    let server = start(store, vec![binding], 0).await.unwrap();
    let config = parse_config(
        &serde_json::json!({"address":server.address.to_string(),"integrationId":"codex",
        "accountKey":"local","surface":"codex_vscode_extension","token":token})
        .to_string(),
    )
    .unwrap();
    let result = tokio::task::spawn_blocking(move || {
        process_hook(
            &config,
            Cursor::new(include_str!(
                "../../../tests/fixtures/codex/session_start.json"
            )),
            Instant::now() + Duration::from_secs(2),
        )
    })
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(result, DeliveryOutcome::Delivered { .. }));
    let view = Store::open(&path).unwrap();
    let sessions = view.list_sessions(10, None).unwrap();
    assert_eq!(sessions.items.len(), 1);
    assert_eq!(
        sessions.items[0].model_selected.as_deref(),
        Some("gpt-5-codex")
    );
    assert_eq!(sessions.items[0].model_observed, None);
    assert_eq!(sessions.items[0].activity, "UNKNOWN");
    let timeline = view
        .list_session_events(&sessions.items[0].id, 10, None)
        .unwrap();
    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].event_type, "session.started");
    server.shutdown().await.unwrap();
}
