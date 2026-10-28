//! Synthetic pipeline only; does not install or execute a real Claude Code hook/status line.
use adapters::{claude, metadata::ObservationContext};
use collector::{ingest::IntegrationRegistration, quota::QuotaObservation, Store};
use normalized_schema::{Confidence, Provider};

#[test]
fn independent_statusline_quotas_flow_to_account_view_without_promoting_selected_model() {
    let dir = tempfile::TempDir::new().unwrap();
    let mut store = Store::open(dir.path().join("claude.db")).unwrap();
    let mut context = ObservationContext {
        integration_id: "claude".into(),
        account_key: "opaque-account".into(),
        event_id: "first".into(),
        observed_at: "2026-10-28T01:00:00Z".into(),
        received_at: "2026-10-28T01:00:01Z".into(),
    };
    store
        .register_integration(&IntegrationRegistration {
            id: context.integration_id.clone(),
            account_key: context.account_key.clone(),
            provider: Provider::ClaudeCode,
            surface: "claude_code_cli".into(),
        })
        .unwrap();
    let mut first = None;
    for (index,raw) in [r#"{"session_id":"session-a","model":{"id":"model-a"},"rate_limits":{"five_hour":{"used_percentage":12,"resets_at":1793152800}}}"#,r#"{"session_id":"session-b","model":{"id":"model-b"},"rate_limits":{"seven_day":{"used_percentage":4,"resets_at":1793757600}}}"#].into_iter().enumerate() {
        context.event_id=format!("delivery-{index}");
        let parsed=claude::normalize_statusline(raw,&context).unwrap();
        let mut start_context=context.clone();start_context.event_id=format!("start-{index}");
        let start=claude::normalize_hook(&serde_json::json!({"session_id":parsed.event.source_session_id,"hook_event_name":"SessionStart"}).to_string(),&start_context).unwrap();
        store.ingest(&start).unwrap();
        let session=store.ingest(&parsed.event).unwrap().session_id;
        if first.is_none(){first=Some(session.clone());}
        store.observe_quotas(&QuotaObservation{schema_version:1,event_id:context.event_id.clone(),integration_id:context.integration_id.clone(),account_key:context.account_key.clone(),provider:Provider::ClaudeCode,surface:"claude_code_cli".into(),source:"claude.statusline".into(),source_event_id:None,source_sequence:None,confidence:Confidence::Direct,observed_at:context.observed_at.clone(),received_at:context.received_at.clone(),snapshots:parsed.quotas}).unwrap();
        let detail=store.session_detail(&session,None,"2026-10-28T01:00:02Z",true).unwrap();
        assert!(detail.session.model_observed.is_none());assert_eq!(detail.session.model_evidence_kind,"session_selection");assert!(detail.usage.items.is_empty());
    }
    let detail = store
        .session_detail(&first.unwrap(), None, "2026-10-28T01:00:02Z", true)
        .unwrap();
    assert_eq!(detail.quotas.len(), 2);
    assert!(detail
        .quotas
        .iter()
        .all(|q| q.freshness == "unknown" && q.snapshot.used.is_none()));
}
