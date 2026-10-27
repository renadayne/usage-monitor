use normalized_schema::usage::{UsageReport, MAX_USAGE_SAMPLES};
use normalized_schema::ValidationError;

fn json_report() -> serde_json::Value {
    serde_json::json!({"event":{
        "schemaVersion":1,"eventId":"event","type":"usage.reported","integrationId":"integration",
        "accountKey":"account","provider":"codex","surface":"synthetic","source":"synthetic",
        "sourceSessionId":"session","modelEvidenceKind":"unknown","confidence":"direct",
        "observedAt":"2026-10-28T00:00:00Z","receivedAt":"2026-10-28T00:00:00Z"
    },"samples":[{"sampleId":"sample","mode":"cumulative","scope":"session","scopeKey":"session",
        "counterKey":"input","counterEpoch":"epoch","revision":1,"unit":"tokens","metric":"input",
        "value":100,"accountingSeries":"canonical","relationToTotal":"exclusive","knownZero":true}]})
}

#[test]
fn usage_dto_rejects_content_fields_and_invalid_scope_unit_value_or_model_claims() {
    let json = json_report();
    let report: UsageReport = serde_json::from_value(json.clone()).unwrap();
    report.validate().unwrap();
    for path in ["event", "sample", "root"] {
        let mut changed = json.clone();
        match path {
            "event" => changed["event"]["prompt"] = serde_json::json!("fake sensitive text"),
            "sample" => changed["samples"][0]["code"] = serde_json::json!("fake sensitive code"),
            _ => changed["credential"] = serde_json::json!("fake credential"),
        };
        assert!(serde_json::from_value::<UsageReport>(changed).is_err());
    }
    for (key, value) in [
        ("value", serde_json::json!(-1)),
        ("value", serde_json::json!(0.5)),
        ("value", serde_json::json!(9_007_199_254_740_992.0)),
        ("unit", serde_json::json!("usd")),
        ("scopeKey", serde_json::json!("different-session")),
        ("revision", serde_json::json!(-1)),
    ] {
        let mut changed = json.clone();
        changed["samples"][0][key] = value;
        let report: UsageReport = serde_json::from_value(changed).unwrap();
        assert!(report.validate().is_err());
    }
    let mut changed = report.clone();
    changed.samples[0].value = f64::NAN;
    assert!(matches!(
        changed.validate(),
        Err(ValidationError::InvalidUsage(_))
    ));
    changed = report.clone();
    changed.samples = vec![report.samples[0].clone(); MAX_USAGE_SAMPLES + 1];
    assert!(changed.validate().is_err());
    changed = report;
    changed.event.model_selected = Some("selected-model".into());
    assert!(changed.validate().is_err());
}
