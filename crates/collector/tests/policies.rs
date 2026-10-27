use collector::ingest::IngestError;
use collector::policies::MAX_POLICIES;
use collector::Store;
use normalized_schema::{Phase, Provider};
use policy_engine::{Action, Policy, Scope};
use rusqlite::Connection;
use tempfile::TempDir;

fn policy_builder(id: &str) -> Policy {
    Policy {
        id: id.into(),
        scope: Scope {
            provider: Some(Provider::Codex),
            project_key: Some("proj-alpha".into()),
            session_id: None,
        },
        phase: Phase::Implementation,
        allowed_model_ids: vec!["gpt-5".into(), "o3".into(), "gpt-5".into()],
        priority: 10,
        action: Action::Notify,
        enabled: true,
    }
}

#[test]
fn file_backed_restart_roundtrip_and_dedup_sorting() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("policies_roundtrip.db");

    {
        let mut store = Store::open(&path).unwrap();
        store.save_policy(&policy_builder("policy-1")).unwrap();
    }

    let store = Store::open(&path).unwrap();
    let list = store.list_policies().unwrap();
    assert_eq!(list.len(), 1);
    let saved = &list[0];
    assert_eq!(saved.id, "policy-1");
    assert_eq!(saved.scope.provider, Some(Provider::Codex));
    assert_eq!(saved.scope.project_key.as_deref(), Some("proj-alpha"));
    assert_eq!(saved.scope.session_id, None);
    assert_eq!(saved.phase, Phase::Implementation);
    assert_eq!(
        saved.allowed_model_ids,
        vec!["gpt-5".to_string(), "o3".to_string()]
    );
    assert_eq!(saved.priority, 10);
    assert_eq!(saved.action, Action::Notify);
    assert!(saved.enabled);

    let db = Connection::open(&path).unwrap();
    let (scope_json, rules_json, action, enabled, priority): (String, String, String, i64, i64) =
        db.query_row(
            "SELECT scope_json, rules_json, action, enabled, priority FROM policies WHERE id = 'policy-1'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&scope_json).unwrap(),
        serde_json::json!({
            "provider": "codex",
            "projectKey": "proj-alpha",
            "sessionId": null
        })
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&rules_json).unwrap(),
        serde_json::json!({
            "phase": "IMPLEMENTATION",
            "allowedModelIds": ["gpt-5", "o3"]
        })
    );
    assert_eq!(action, "notify");
    assert_eq!(enabled, 1);
    assert_eq!(priority, 10);
}

#[test]
fn deterministic_priority_desc_then_id_asc_ordering() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("policies_order.db");
    let mut store = Store::open(&path).unwrap();

    let mut p_low = policy_builder("policy-z");
    p_low.priority = -5;
    p_low.action = Action::Observe;

    let mut p_tie_b = policy_builder("policy-b");
    p_tie_b.priority = 20;

    let mut p_tie_a = policy_builder("policy-a");
    p_tie_a.priority = 20;

    let mut p_top = policy_builder("policy-c");
    p_top.priority = 100;

    store.save_policy(&p_low).unwrap();
    store.save_policy(&p_tie_b).unwrap();
    store.save_policy(&p_tie_a).unwrap();
    store.save_policy(&p_top).unwrap();

    let ids: Vec<String> = store
        .list_policies()
        .unwrap()
        .into_iter()
        .map(|p| p.id)
        .collect();
    assert_eq!(ids, vec!["policy-c", "policy-a", "policy-b", "policy-z"]);
}

#[test]
fn same_id_update_disable_and_capacity_enforcement() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("policies_update.db");
    let mut store = Store::open(&path).unwrap();

    let mut initial = policy_builder("policy-main");
    initial.priority = 5;
    store.save_policy(&initial).unwrap();

    let mut updated = policy_builder("policy-main");
    updated.phase = Phase::Review;
    updated.allowed_model_ids = vec!["claude-sonnet-4".into()];
    updated.priority = 50;
    updated.action = Action::Observe;
    updated.enabled = false;
    store.save_policy(&updated).unwrap();

    let list = store.list_policies().unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "policy-main");
    assert_eq!(list[0].phase, Phase::Review);
    assert_eq!(list[0].allowed_model_ids, vec!["claude-sonnet-4"]);
    assert_eq!(list[0].priority, 50);
    assert_eq!(list[0].action, Action::Observe);
    assert!(!list[0].enabled);

    for idx in 1..MAX_POLICIES {
        let mut p = policy_builder(&format!("policy-fill-{idx:03}"));
        p.priority = idx as i32;
        store.save_policy(&p).unwrap();
    }
    assert_eq!(store.list_policies().unwrap().len(), MAX_POLICIES);

    let overflow = policy_builder("policy-overflow");
    assert!(matches!(
        store.save_policy(&overflow),
        Err(IngestError::Invalid(_))
    ));

    updated.enabled = true;
    store.save_policy(&updated).unwrap();
}

#[test]
fn invalid_ids_empty_models_unassigned_phase_and_block_action_perform_no_writes() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("policies_invalid.db");
    let mut store = Store::open(&path).unwrap();

    let mut bad_id = policy_builder(" bad-id ");
    assert!(matches!(
        store.save_policy(&bad_id),
        Err(IngestError::Validation(_))
    ));
    bad_id.id = "".into();
    assert!(matches!(
        store.save_policy(&bad_id),
        Err(IngestError::Validation(_))
    ));

    let mut bad_project = policy_builder("valid-id");
    bad_project.scope.project_key = Some("bad\nproject".into());
    assert!(matches!(
        store.save_policy(&bad_project),
        Err(IngestError::Validation(_))
    ));

    let mut bad_session = policy_builder("valid-id");
    bad_session.scope.session_id = Some(" ".into());
    assert!(matches!(
        store.save_policy(&bad_session),
        Err(IngestError::Validation(_))
    ));

    let mut bad_model = policy_builder("valid-id");
    bad_model.allowed_model_ids = vec!["gpt-5".into(), "".into()];
    assert!(matches!(
        store.save_policy(&bad_model),
        Err(IngestError::Validation(_))
    ));

    let mut empty_models = policy_builder("valid-id");
    empty_models.allowed_model_ids.clear();
    assert!(matches!(
        store.save_policy(&empty_models),
        Err(IngestError::Invalid(_))
    ));

    let mut too_many_models = policy_builder("valid-id");
    too_many_models.allowed_model_ids = (0..33).map(|i| format!("model-{i}")).collect();
    assert!(matches!(
        store.save_policy(&too_many_models),
        Err(IngestError::Invalid(_))
    ));

    let mut unassigned = policy_builder("valid-id");
    unassigned.phase = Phase::Unassigned;
    assert!(matches!(
        store.save_policy(&unassigned),
        Err(IngestError::Invalid(_))
    ));

    let mut block_action = policy_builder("valid-id");
    block_action.action = Action::BlockNextTurn;
    assert!(matches!(
        store.save_policy(&block_action),
        Err(IngestError::Invalid(_))
    ));

    assert!(store.list_policies().unwrap().is_empty());
    let db = Connection::open(&path).unwrap();
    let count: i64 = db
        .query_row("SELECT COUNT(*) FROM policies", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 0);
}
