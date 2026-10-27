use normalized_schema::quota::{QuotaError, QuotaSnapshot, QuotaUnit};
use normalized_schema::{Provider, ValidationError};

fn base_snapshot_json() -> serde_json::Value {
    serde_json::json!({
        "quotaKey": "synthetic-quota-key",
        "provider": "codex",
        "accountKey": "acct-synthetic-1",
        "scope": "account",
        "bucket": "standard",
        "unit": "tokens",
        "windowKey": "5h",
        "windowStart": "2026-10-28T00:00:00Z",
        "windowEnd": "2026-10-28T05:00:00Z",
        "resetsAt": "2026-10-28T05:00:00Z",
        "used": 250.0,
        "limit": 1000.0,
        "remaining": 750.0,
        "usedPercent": 25.0,
        "observedAt": "2026-10-28T01:00:00Z",
        "staleAfter": "2026-10-28T01:05:00Z"
    })
}

#[test]
fn quota_dto_rejects_unknown_or_raw_fields() {
    let valid: QuotaSnapshot = serde_json::from_value(base_snapshot_json()).unwrap();
    valid.validate().unwrap();

    for forbidden_field in [
        "prompt",
        "rawResponse",
        "authorization",
        "contextWindow",
        "sessionId",
        "quota_key",
    ] {
        let mut raw = base_snapshot_json();
        raw[forbidden_field] = serde_json::json!("synthetic-disallowed-payload");
        assert!(
            serde_json::from_value::<QuotaSnapshot>(raw).is_err(),
            "expected field {forbidden_field} to be rejected"
        );
    }
}

#[test]
fn quota_dto_rejects_malformed_timestamps_and_identifiers_with_sanitized_errors() {
    let sensitive_marker = "Bearer synthetic-secret-value-xyz";

    for ts_field in [
        "observedAt",
        "windowStart",
        "windowEnd",
        "resetsAt",
        "staleAfter",
    ] {
        let mut raw = base_snapshot_json();
        raw[ts_field] = serde_json::json!(sensitive_marker);
        let snapshot: QuotaSnapshot = serde_json::from_value(raw).unwrap();
        let err = snapshot.validate().unwrap_err();
        assert_eq!(
            err,
            QuotaError::Validation(ValidationError::InvalidUtcTimestamp { field: ts_field })
        );
        let rendered = format!("{err} | {err:?}");
        assert!(!rendered.contains(sensitive_marker));
    }

    for id_field in ["quotaKey", "accountKey", "scope", "bucket", "windowKey"] {
        let mut raw = base_snapshot_json();
        raw[id_field] = serde_json::json!(format!(" {sensitive_marker}\n"));
        let snapshot: QuotaSnapshot = serde_json::from_value(raw).unwrap();
        let err = snapshot.validate().unwrap_err();
        assert!(matches!(
            err,
            QuotaError::Validation(ValidationError::InvalidIdentifier { field, .. })
            if field == id_field
        ));
        let rendered = format!("{err} | {err:?}");
        assert!(!rendered.contains(sensitive_marker));
    }

    // Window ordering and TTL validity. Past resets remain valid historical facts.
    let mut bad_window = serde_json::from_value::<QuotaSnapshot>(base_snapshot_json()).unwrap();
    bad_window.window_start = Some("2026-10-28T05:00:00Z".into());
    bad_window.window_end = Some("2026-10-28T05:00:00Z".into());
    assert!(matches!(
        bad_window.validate(),
        Err(QuotaError::InvalidQuota(_))
    ));

    let mut bad_reset = serde_json::from_value::<QuotaSnapshot>(base_snapshot_json()).unwrap();
    bad_reset.resets_at = Some("2026-10-28T00:59:59Z".into());
    bad_reset.validate().unwrap();

    let mut exact_reset = serde_json::from_value::<QuotaSnapshot>(base_snapshot_json()).unwrap();
    exact_reset.resets_at = Some(exact_reset.observed_at.clone());
    exact_reset.validate().unwrap();

    let mut bad_stale = serde_json::from_value::<QuotaSnapshot>(base_snapshot_json()).unwrap();
    bad_stale.stale_after = Some(bad_stale.observed_at.clone());
    assert!(matches!(
        bad_stale.validate(),
        Err(QuotaError::InvalidQuota(_))
    ));
}

#[test]
fn quota_dto_rejects_nonfinite_and_negative_non_overage_numbers() {
    let base: QuotaSnapshot = serde_json::from_value(base_snapshot_json()).unwrap();

    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -0.001, -100.0] {
        let mut s = base.clone();
        s.used = Some(bad);
        assert!(matches!(s.validate(), Err(QuotaError::InvalidQuota(_))));

        let mut s = base.clone();
        s.limit = Some(bad);
        assert!(matches!(s.validate(), Err(QuotaError::InvalidQuota(_))));

        let mut s = base.clone();
        s.used_percent = Some(bad);
        assert!(matches!(s.validate(), Err(QuotaError::InvalidQuota(_))));
    }

    for nonfinite in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut s = base.clone();
        s.remaining = Some(nonfinite);
        assert!(matches!(s.validate(), Err(QuotaError::InvalidQuota(_))));
    }
}

#[test]
fn quota_dto_enforces_percentage_only_without_deriving_or_converting_values() {
    let pct_only: QuotaSnapshot = serde_json::from_value(serde_json::json!({
        "quotaKey": "pct-5h",
        "provider": "claude_code",
        "accountKey": "acct-1",
        "scope": "account",
        "bucket": "rolling_5h",
        "unit": "percentage",
        "windowKey": "5h",
        "usedPercent": 62.5,
        "observedAt": "2026-10-28T01:00:00Z"
    }))
    .unwrap();
    pct_only.validate().unwrap();
    assert_eq!(pct_only.unit, QuotaUnit::Percentage);
    assert_eq!(pct_only.used, None);
    assert_eq!(pct_only.limit, None);
    assert_eq!(pct_only.remaining, None);
    assert_eq!(pct_only.resets_at, None);

    for (setter_name, mutate) in [
        ("used", |s: &mut QuotaSnapshot| s.used = Some(62.5)),
        ("limit", |s: &mut QuotaSnapshot| s.limit = Some(100.0)),
        ("remaining", |s: &mut QuotaSnapshot| {
            s.remaining = Some(37.5)
        }),
    ] {
        let mut invalid = pct_only.clone();
        mutate(&mut invalid);
        assert!(
            matches!(invalid.validate(), Err(QuotaError::InvalidQuota(_))),
            "expected percentage unit with {setter_name} to fail"
        );
    }

    // Non-percentage units never compute missing usedPercent or remaining.
    let credits: QuotaSnapshot = serde_json::from_value(serde_json::json!({
        "quotaKey": "credits-monthly",
        "provider": "antigravity",
        "accountKey": "acct-1",
        "scope": "account",
        "bucket": "ai_credits",
        "unit": "ai_credits",
        "windowKey": "monthly",
        "used": 300.0,
        "limit": 1000.0,
        "observedAt": "2026-10-28T01:00:00Z"
    }))
    .unwrap();
    credits.validate().unwrap();
    assert_eq!(credits.remaining, None);
    assert_eq!(credits.used_percent, None);
}

#[test]
fn quota_dto_accepts_explicit_overage_without_clamping() {
    let overage = QuotaSnapshot {
        quota_key: "overage-window".into(),
        provider: Provider::Copilot,
        account_key: "acct-overage".into(),
        scope: "account".into(),
        bucket: "premium_requests".into(),
        unit: QuotaUnit::Requests,
        window_key: "monthly".into(),
        window_start: Some("2026-10-01T00:00:00Z".into()),
        window_end: Some("2026-11-01T00:00:00Z".into()),
        resets_at: Some("2026-11-01T00:00:00Z".into()),
        used: Some(350.0),
        limit: Some(300.0),
        remaining: Some(-50.0),
        used_percent: Some(116.66666666666667),
        observed_at: "2026-10-28T01:00:00Z".into(),
        stale_after: Some("2026-10-28T02:00:00Z".into()),
    };

    overage.validate().unwrap();
    assert_eq!(overage.used, Some(350.0));
    assert_eq!(overage.limit, Some(300.0));
    assert_eq!(overage.remaining, Some(-50.0));
    assert!(overage.used_percent.unwrap() > 100.0);
}

#[test]
fn two_independently_missing_window_snapshots_remain_independent() {
    let five_hour: QuotaSnapshot = serde_json::from_value(serde_json::json!({
        "quotaKey": "acct-1:5h",
        "provider": "codex",
        "accountKey": "acct-1",
        "scope": "account",
        "bucket": "primary",
        "unit": "percentage",
        "windowKey": "5h",
        "usedPercent": 80.0,
        "observedAt": "2026-10-28T01:00:00Z"
    }))
    .unwrap();

    let weekly: QuotaSnapshot = serde_json::from_value(serde_json::json!({
        "quotaKey": "acct-1:weekly",
        "provider": "codex",
        "accountKey": "acct-1",
        "scope": "account",
        "bucket": "primary",
        "unit": "usd",
        "windowKey": "weekly",
        "resetsAt": "2026-11-01T00:00:00Z",
        "remaining": 45.25,
        "observedAt": "2026-10-28T01:00:00Z"
    }))
    .unwrap();

    five_hour.validate().unwrap();
    weekly.validate().unwrap();

    assert_ne!(five_hour.window_key, weekly.window_key);
    assert_eq!(five_hour.resets_at, None);
    assert_eq!(five_hour.remaining, None);
    assert_eq!(weekly.window_start, None);
    assert_eq!(weekly.window_end, None);
    assert_eq!(weekly.used, None);
    assert_eq!(weekly.limit, None);
    assert_eq!(weekly.used_percent, None);
}
