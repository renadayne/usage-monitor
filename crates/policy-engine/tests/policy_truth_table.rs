use normalized_schema::{Confidence, ModelEvidenceKind, Phase, Provider};
use policy_engine::*;

fn context() -> Context {
    Context {
        provider: Provider::Codex,
        project_key: Some("project".into()),
        session_id: "session".into(),
        phase: Phase::Review,
        phase_epoch: 2,
        clock_trusted: true,
    }
}

fn policy(id: &str) -> Policy {
    Policy {
        id: id.into(),
        scope: Scope::default(),
        phase: Phase::Review,
        allowed_model_ids: vec!["model-a".into()],
        priority: 0,
        action: Action::Notify,
        enabled: true,
    }
}

fn evidence() -> Evidence {
    Evidence {
        provider: Provider::Codex,
        session_id: "session".into(),
        model_id: "model-a".into(),
        kind: ModelEvidenceKind::ServedResponse,
        confidence: Confidence::Direct,
        inference_id: Some("call".into()),
        evidence_ref: Some("response".into()),
        phase: Some(Phase::Review),
        phase_epoch: Some(2),
        observed_at_ms: 100,
        expires_at_ms: 200,
    }
}

#[test]
fn missing_context_and_inapplicable_rules_are_never_compliance() {
    let p = policy("global");
    let mut c = context();
    c.phase = Phase::Unassigned;
    assert_eq!(
        evaluate(std::slice::from_ref(&p), &c, Some(&evidence()), &[], 150).decision,
        Decision::Indeterminate
    );
    c.phase = Phase::Planning;
    assert_eq!(
        evaluate(std::slice::from_ref(&p), &c, Some(&evidence()), &[], 150).decision,
        Decision::NotApplicable
    );
    c.phase = Phase::Review;
    let mut disabled = p;
    disabled.enabled = false;
    assert_eq!(
        evaluate(&[disabled], &c, Some(&evidence()), &[], 150).decision,
        Decision::NotApplicable
    );
    assert_eq!(
        evaluate(&[policy("global")], &c, None, &[], 150).decision,
        Decision::Indeterminate
    );
}

#[test]
fn only_fresh_bound_served_proof_can_establish_compliance_or_violation() {
    let p = [policy("global")];
    let c = context();
    let e = evidence();
    assert_eq!(
        evaluate(&p, &c, Some(&e), &[], 150).decision,
        Decision::Compliant
    );
    let mut disallowed = e.clone();
    disallowed.model_id = "model-b".into();
    let result = evaluate(&p, &c, Some(&disallowed), &[], 150);
    assert_eq!(result.decision, Decision::Violation);
    assert!(result.should_notify);
    for now in [99, 200, 250] {
        assert_eq!(
            evaluate(&p, &c, Some(&e), &[], now).reason,
            Reason::StaleOrFutureEvidence
        );
    }
    let mut old = e.clone();
    old.phase_epoch = Some(1);
    assert_eq!(
        evaluate(&p, &c, Some(&old), &[], 150).reason,
        Reason::DifferentPhase
    );
    old = e.clone();
    old.session_id = "another-session".into();
    assert_eq!(
        evaluate(&p, &c, Some(&old), &[], 150).reason,
        Reason::DifferentSession
    );
    old = e.clone();
    old.provider = Provider::Copilot;
    assert_eq!(
        evaluate(&p, &c, Some(&old), &[], 150).reason,
        Reason::DifferentSession
    );
    old = e.clone();
    old.evidence_ref = None;
    assert_eq!(
        evaluate(&p, &c, Some(&old), &[], 150).reason,
        Reason::MissingServedEvidence
    );
    old = e;
    old.confidence = Confidence::Unknown;
    assert_eq!(
        evaluate(&p, &c, Some(&old), &[], 150).decision,
        Decision::Indeterminate
    );
}

#[test]
fn selection_warning_never_claims_served_violation_or_enforcement() {
    let mut p = policy("global");
    p.action = Action::BlockNextTurn;
    let mut e = evidence();
    e.kind = ModelEvidenceKind::SessionSelection;
    e.inference_id = None;
    let matching = evaluate(std::slice::from_ref(&p), &context(), Some(&e), &[], 150);
    assert_eq!(matching.decision, Decision::Indeterminate);
    assert!(!matching.should_notify);
    e.model_id = "model-b".into();
    let mismatch = evaluate(&[p], &context(), Some(&e), &[], 150);
    assert_eq!(mismatch.decision, Decision::Indeterminate);
    assert!(mismatch.selection_mismatch_warning && mismatch.should_notify);
    assert_eq!(mismatch.enforcement, Enforcement::Unavailable);
}

#[test]
fn specificity_precedes_priority_and_equal_rank_rules_intersect() {
    let mut global = policy("global");
    global.priority = 500;
    let mut session = policy("session");
    session.scope.session_id = Some("session".into());
    session.allowed_model_ids = vec!["model-b".into()];
    let r = evaluate(&[global, session], &context(), Some(&evidence()), &[], 150);
    assert_eq!(r.winning_policy_ids, ["session"]);
    assert_eq!(r.shadowed_policy_ids, ["global"]);
    assert_eq!(r.decision, Decision::Violation);
    let a = policy("a");
    let mut b = policy("b");
    b.allowed_model_ids = vec!["model-b".into()];
    assert_eq!(
        evaluate(
            &[a.clone(), b.clone()],
            &context(),
            Some(&evidence()),
            &[],
            150
        )
        .reason,
        Reason::ConfigurationConflict
    );
    b.allowed_model_ids.push("model-a".into());
    assert_eq!(
        evaluate(
            &[b.clone(), a.clone()],
            &context(),
            Some(&evidence()),
            &[],
            150
        ),
        evaluate(&[a, b], &context(), Some(&evidence()), &[], 150)
    );
}

#[test]
fn overrides_expire_at_evaluation_and_cannot_create_proof() {
    let p = [policy("global")];
    let c = context();
    let mut e = evidence();
    e.model_id = "model-b".into();
    let o = Override {
        id: "override".into(),
        policy_id: "global".into(),
        session_id: "session".into(),
        phase_epoch: 2,
        replacement_model_ids: vec!["model-b".into()],
        reason: "manual temporary selection".into(),
        created_at_ms: 100,
        expires_at_ms: 160,
        revoked_at_ms: None,
    };
    assert_eq!(
        evaluate(&p, &c, Some(&e), std::slice::from_ref(&o), 150).decision,
        Decision::Compliant
    );
    assert_eq!(
        evaluate(&p, &c, Some(&e), std::slice::from_ref(&o), 160).decision,
        Decision::Violation
    );
    e.kind = ModelEvidenceKind::RequestAttribute;
    assert_eq!(
        evaluate(&p, &c, Some(&e), std::slice::from_ref(&o), 150).decision,
        Decision::Indeterminate
    );
    let mut revoked = o.clone();
    revoked.revoked_at_ms = Some(140);
    e.kind = ModelEvidenceKind::ServedResponse;
    assert_eq!(
        evaluate(&p, &c, Some(&e), &[revoked], 150).decision,
        Decision::Violation
    );
    assert_eq!(
        evaluate(&p, &c, Some(&e), &[o.clone(), o], 150).reason,
        Reason::ConfigurationConflict
    );
    let mut untrusted = c;
    untrusted.clock_trusted = false;
    assert_eq!(
        evaluate(&p, &untrusted, Some(&e), &[], 150).reason,
        Reason::ClockUntrusted
    );
}
