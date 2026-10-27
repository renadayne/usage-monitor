use usage_accounting::{
    reconcile_counter, reconcile_with_prior_identity, CounterCheckpoint, CounterSample,
    ReconciliationDisposition, ReconciliationError, SampleIdentity,
};

fn canonical_identity(epoch: &str) -> SampleIdentity {
    SampleIdentity {
        counter_key: "provider.requests.total".to_string(),
        epoch: epoch.to_string(),
        scope_key: "org:acme/project:alpha".to_string(),
        accounting_series: "billable_api_tokens".to_string(),
        unit: "tokens".to_string(),
        metric: "input_tokens".to_string(),
    }
}

fn sample(epoch: &str, revision: u64, value: f64, known_zero: bool) -> CounterSample {
    CounterSample {
        identity: canonical_identity(epoch),
        revision,
        value,
        known_zero,
    }
}

#[test]
fn canonical_progression_replay_and_late_sample() {
    // 1 = 100 (known-zero start)
    let s1 = sample("epoch-1", 1, 100.0, true);
    let r1 = reconcile_counter(&s1, None).expect("rev 1 should succeed");
    assert_eq!(r1.accepted_delta, 100.0);
    assert_eq!(r1.disposition, ReconciliationDisposition::InitialKnownZero);
    assert_eq!(
        r1.checkpoint,
        CounterCheckpoint {
            last_revision: 1,
            last_value: 100.0,
            accepted_total: 100.0,
            baseline_value: 0.0,
            frozen: false,
        }
    );

    // 3 = 150 (newer increasing => +50)
    let s3 = sample("epoch-1", 3, 150.0, false);
    let r3 = reconcile_counter(&s3, Some(&r1.checkpoint)).expect("rev 3 should succeed");
    assert_eq!(r3.accepted_delta, 50.0);
    assert_eq!(r3.disposition, ReconciliationDisposition::AcceptedIncrement);
    assert_eq!(r3.checkpoint.last_revision, 3);
    assert_eq!(r3.checkpoint.last_value, 150.0);
    assert_eq!(r3.checkpoint.accepted_total, 150.0);

    // replay 3 = 150 (same revision, same value => 0 delta, unchanged checkpoint)
    let r3_replay =
        reconcile_counter(&s3, Some(&r3.checkpoint)).expect("replay rev 3 should succeed");
    assert_eq!(r3_replay.accepted_delta, 0.0);
    assert_eq!(
        r3_replay.disposition,
        ReconciliationDisposition::DuplicateReplay
    );
    assert_eq!(r3_replay.checkpoint, r3.checkpoint);

    // late 2 = 125 (older revision => history-only 0 delta, unchanged checkpoint)
    let s2_late = sample("epoch-1", 2, 125.0, false);
    let r2_late = reconcile_counter(&s2_late, Some(&r3_replay.checkpoint))
        .expect("late rev 2 should succeed");
    assert_eq!(r2_late.accepted_delta, 0.0);
    assert_eq!(
        r2_late.disposition,
        ReconciliationDisposition::OlderHistoryOnly
    );
    assert_eq!(r2_late.checkpoint, r3.checkpoint);
}

#[test]
fn decrease_freezes_epoch_and_blocks_recovery() {
    let s1 = sample("epoch-1", 1, 200.0, true);
    let r1 = reconcile_counter(&s1, None).expect("rev 1");
    assert_eq!(r1.accepted_delta, 200.0);

    // Newer revision decreases from 200 -> 180: freeze checkpoint and produce ambiguous zero.
    let s2_decrease = sample("epoch-1", 2, 180.0, false);
    let r2 = reconcile_counter(&s2_decrease, Some(&r1.checkpoint)).expect("rev 2 decrease");
    assert_eq!(r2.accepted_delta, 0.0);
    assert_eq!(
        r2.disposition,
        ReconciliationDisposition::AmbiguousDecreaseFrozen
    );
    assert!(r2.checkpoint.frozen);
    assert_eq!(r2.checkpoint.accepted_total, 200.0);
    assert_eq!(r2.checkpoint.last_revision, 2);
    assert_eq!(r2.checkpoint.last_value, 180.0);

    // Subsequent recovery within the same epoch (e.g., 250 > 200) must still produce zero delta.
    let s3_recover = sample("epoch-1", 3, 250.0, false);
    let r3 = reconcile_counter(&s3_recover, Some(&r2.checkpoint)).expect("rev 3 recovery");
    assert_eq!(r3.accepted_delta, 0.0);
    assert_eq!(r3.disposition, ReconciliationDisposition::FrozenEpochNoOp);
    assert!(r3.checkpoint.frozen);
    assert_eq!(r3.checkpoint.accepted_total, 200.0);
    assert_eq!(r3.checkpoint.last_revision, 3);
    assert_eq!(r3.checkpoint.last_value, 250.0);
}

#[test]
fn fresh_explicit_epoch_uses_independent_checkpoint() {
    let s1 = sample("epoch-1", 10, 500.0, true);
    let r1 = reconcile_counter(&s1, None).expect("epoch-1 initial");
    let s2_drop = sample("epoch-1", 11, 100.0, false);
    let r2_frozen = reconcile_counter(&s2_drop, Some(&r1.checkpoint)).expect("epoch-1 frozen");
    assert!(r2_frozen.checkpoint.frozen);

    // Explicit new epoch starts an independent checkpoint rather than guessing reset or staying frozen.
    let s_new_epoch = sample("epoch-2", 1, 40.0, true);
    let r_new = reconcile_with_prior_identity(
        &s_new_epoch,
        Some((&s2_drop.identity, &r2_frozen.checkpoint)),
    )
    .expect("fresh epoch should reconcile independently");

    assert_eq!(r_new.accepted_delta, 40.0);
    assert_eq!(
        r_new.disposition,
        ReconciliationDisposition::InitialKnownZero
    );
    assert!(!r_new.checkpoint.frozen);
    assert_eq!(r_new.checkpoint.accepted_total, 40.0);
    assert_eq!(r_new.checkpoint.baseline_value, 0.0);
}

#[test]
fn mid_counter_baseline_then_increment_accepts_difference_only() {
    // Mid-counter first sample at 100 => delta = 0, baseline = 100
    let s1 = sample("epoch-mid", 5, 100.0, false);
    let r1 = reconcile_counter(&s1, None).expect("mid-counter initial");
    assert_eq!(r1.accepted_delta, 0.0);
    assert_eq!(
        r1.disposition,
        ReconciliationDisposition::InitialMidCounterBaseline
    );
    assert_eq!(
        r1.checkpoint,
        CounterCheckpoint {
            last_revision: 5,
            last_value: 100.0,
            accepted_total: 0.0,
            baseline_value: 100.0,
            frozen: false,
        }
    );

    // Next sample at 150 => delta = 50, accepted_total = 50, baseline remains 100
    let s2 = sample("epoch-mid", 6, 150.0, true); // known_zero ignored after initial checkpoint
    let r2 = reconcile_counter(&s2, Some(&r1.checkpoint)).expect("mid-counter increment");
    assert_eq!(r2.accepted_delta, 50.0);
    assert_eq!(r2.disposition, ReconciliationDisposition::AcceptedIncrement);
    assert_eq!(r2.checkpoint.accepted_total, 50.0);
    assert_eq!(r2.checkpoint.baseline_value, 100.0);
    assert_eq!(r2.checkpoint.last_value, 150.0);
}

#[test]
fn same_revision_conflict_and_invalid_values_reject_without_mutation() {
    let s1 = sample("epoch-err", 4, 100.0, true);
    let r1 = reconcile_counter(&s1, None).expect("initial valid sample");
    let snapshot = r1.checkpoint;

    // Same revision with changed value => conflict error, prior snapshot untouched
    let conflicting = sample("epoch-err", 4, 105.0, false);
    let err = reconcile_counter(&conflicting, Some(&snapshot)).unwrap_err();
    assert_eq!(
        err,
        ReconciliationError::ConflictingSameRevision { revision: 4 }
    );
    assert_eq!(snapshot.last_value, 100.0);
    assert_eq!(snapshot.accepted_total, 100.0);

    // Negative, NaN, and infinite sample values rejected
    for invalid_val in [-0.001, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let bad = sample("epoch-err", 5, invalid_val, false);
        assert_eq!(
            reconcile_counter(&bad, Some(&snapshot)),
            Err(ReconciliationError::InvalidSampleValue)
        );
    }

    // Total overflow rejected without mutating prior checkpoint
    let near_max = CounterCheckpoint {
        last_revision: 1,
        last_value: 0.0,
        accepted_total: f64::MAX,
        baseline_value: 0.0,
        frozen: false,
    };
    let overflow_sample = sample("epoch-err", 2, f64::MAX, false);
    assert_eq!(
        reconcile_counter(&overflow_sample, Some(&near_max)),
        Err(ReconciliationError::ArithmeticOverflow)
    );

    // Blank identity field rejected with sanitized error
    let mut bad_identity_sample = sample("epoch-err", 2, 10.0, false);
    bad_identity_sample.identity.scope_key = "   ".to_string();
    assert_eq!(
        reconcile_counter(&bad_identity_sample, Some(&snapshot)),
        Err(ReconciliationError::InvalidIdentityField { field: "scope_key" })
    );

    // Cross-series / cross-scope mismatch rejected when using identity-bound helper
    let mut other_series = sample("epoch-err", 5, 120.0, false);
    other_series.identity.accounting_series = "other_series".to_string();
    assert_eq!(
        reconcile_with_prior_identity(&other_series, Some((&s1.identity, &snapshot))),
        Err(ReconciliationError::StreamIdentityMismatch)
    );
}
