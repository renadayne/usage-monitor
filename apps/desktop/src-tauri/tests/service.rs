use adapters::codex::Surface;
use collector::{ingest::IntegrationRegistration, Store};
use normalized_schema::{Phase, Provider};
use policy_engine::{Action, Decision, Policy, Scope};
use usage_monitor_desktop::{profile::ensure_profile, DesktopService};

#[test]
fn native_command_service_uses_real_store_and_local_phase_authority() {
    let dir = tempfile::TempDir::new().unwrap();
    let address = "127.0.0.1:4319".parse().unwrap();
    let profile = ensure_profile(dir.path(), Surface::VsCodeExtension, address).unwrap();
    let helper = observer_helper::load_config(profile.config_path()).unwrap();
    let path = dir.path().join("service.db");
    let mut collector = Store::open(&path).unwrap();
    collector
        .register_integration(&IntegrationRegistration {
            id: helper.integration_id().into(),
            account_key: helper.account_key().into(),
            provider: Provider::Codex,
            surface: helper.surface().as_str().into(),
        })
        .unwrap();
    let (event, _) = observer_helper::normalize_hook_payload(
        include_str!("../../../../tests/fixtures/codex/session_start.json"),
        &helper,
    )
    .unwrap();
    let outcome = collector.ingest(&event).unwrap();
    let mut service = DesktopService::open(&path).unwrap();
    let snapshot = service.dashboard(None, Some("127.0.0.1:4319")).unwrap();
    assert_eq!(snapshot["sessions"][0]["modelSelected"], "gpt-5-codex");
    assert_eq!(
        snapshot["sessions"][0]["modelObserved"],
        serde_json::Value::Null
    );
    assert_eq!(snapshot["integrations"][0]["targetVerification"], "unknown");
    service
        .change_phase(&outcome.session_id, 0, Phase::Review)
        .unwrap();
    assert!(service
        .change_phase(&outcome.session_id, 0, Phase::Done)
        .is_err());
    service
        .save_policy(&Policy {
            id: "review-rule".into(),
            scope: Scope::default(),
            phase: Phase::Review,
            allowed_model_ids: vec!["expected-model".into()],
            priority: 0,
            action: Action::Observe,
            enabled: true,
        })
        .unwrap();
    let detail = service.session_detail(&outcome.session_id, None).unwrap();
    assert_eq!(detail.session.phase, "REVIEW");
    assert_eq!(detail.session.phase_epoch, 1);
    assert_eq!(detail.session.model_selected, None);
    assert_eq!(detail.policy.decision, Decision::Indeterminate);
    assert_eq!(detail.events.len(), 2);
    assert!(detail.usage.partial);
    assert_eq!(service.export_history(7).unwrap().rows, 2);
    drop(service);
    let service = DesktopService::open(&path).unwrap();
    assert_eq!(service.list_policies().unwrap().len(), 1);
}
