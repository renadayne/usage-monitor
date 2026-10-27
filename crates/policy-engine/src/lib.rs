//! Deterministic advisory evaluation. This crate never authorizes or blocks provider calls.

use std::collections::BTreeSet;

use normalized_schema::{Confidence, ModelEvidenceKind, Phase, Provider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Observe,
    Notify,
    BlockNextTurn,
}

#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub provider: Option<Provider>,
    pub project_key: Option<String>,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Policy {
    pub id: String,
    pub scope: Scope,
    pub phase: Phase,
    pub allowed_model_ids: Vec<String>,
    pub priority: i32,
    pub action: Action,
    pub enabled: bool,
}

#[derive(Debug, Clone)]
pub struct Context {
    pub provider: Provider,
    pub project_key: Option<String>,
    pub session_id: String,
    pub phase: Phase,
    pub phase_epoch: u64,
    pub clock_trusted: bool,
}

#[derive(Debug, Clone)]
pub struct Evidence {
    pub provider: Provider,
    pub session_id: String,
    pub model_id: String,
    pub kind: ModelEvidenceKind,
    pub confidence: Confidence,
    pub inference_id: Option<String>,
    pub evidence_ref: Option<String>,
    pub phase: Option<Phase>,
    pub phase_epoch: Option<u64>,
    pub observed_at_ms: i64,
    pub expires_at_ms: i64,
}

#[derive(Debug, Clone)]
pub struct Override {
    pub id: String,
    pub policy_id: String,
    pub session_id: String,
    pub phase_epoch: u64,
    pub replacement_model_ids: Vec<String>,
    pub reason: String,
    pub created_at_ms: i64,
    pub expires_at_ms: i64,
    pub revoked_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Compliant,
    Violation,
    NotApplicable,
    Indeterminate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    UnassignedPhase,
    InvalidContext,
    NoApplicablePolicy,
    ConfigurationConflict,
    ClockUntrusted,
    MissingServedEvidence,
    StaleOrFutureEvidence,
    DifferentPhase,
    DifferentSession,
    ModelAllowed,
    ModelNotAllowed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Enforcement {
    Advisory,
    Unavailable,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Evaluation {
    pub decision: Decision,
    pub reason: Reason,
    pub winning_policy_ids: Vec<String>,
    pub shadowed_policy_ids: Vec<String>,
    pub enforcement: Enforcement,
    pub should_notify: bool,
    pub selection_mismatch_warning: bool,
}

/// Select by specificity, then descending priority. Ties intersect exact model IDs.
/// Times are Unix milliseconds supplied by the caller. Overrides never supply evidence.
pub fn evaluate(
    policies: &[Policy],
    context: &Context,
    evidence: Option<&Evidence>,
    overrides: &[Override],
    now_ms: i64,
) -> Evaluation {
    let mut result = Evaluation {
        decision: Decision::Indeterminate,
        reason: Reason::UnassignedPhase,
        winning_policy_ids: Vec::new(),
        shadowed_policy_ids: Vec::new(),
        enforcement: Enforcement::Advisory,
        should_notify: false,
        selection_mismatch_warning: false,
    };
    if context.phase == Phase::Unassigned {
        return result;
    }
    if !valid_id(&context.session_id)
        || context
            .project_key
            .as_deref()
            .is_some_and(|id| !valid_id(id))
    {
        result.reason = Reason::InvalidContext;
        return result;
    }
    let mut matches: Vec<&Policy> = policies.iter().filter(|p| applicable(p, context)).collect();
    let Some(winning_rank) = matches.iter().copied().map(rank).max() else {
        result.decision = Decision::NotApplicable;
        result.reason = Reason::NoApplicablePolicy;
        return result;
    };
    matches.sort_by(|a, b| a.id.cmp(&b.id));
    let winners: Vec<&Policy> = matches
        .iter()
        .copied()
        .filter(|p| rank(p) == winning_rank)
        .collect();
    result.winning_policy_ids = winners.iter().map(|p| p.id.clone()).collect();
    result.shadowed_policy_ids = matches
        .iter()
        .filter(|p| rank(p) != winning_rank)
        .map(|p| p.id.clone())
        .collect();
    if winners.iter().any(|p| p.action == Action::BlockNextTurn) {
        result.enforcement = Enforcement::Unavailable;
    }
    if !context.clock_trusted {
        result.reason = Reason::ClockUntrusted;
        return result;
    }
    let unique: BTreeSet<&str> = winners.iter().map(|p| p.id.as_str()).collect();
    if unique.len() != winners.len()
        || winners
            .iter()
            .any(|p| !valid_id(&p.id) || p.allowed_model_ids.iter().any(|id| !valid_id(id)))
    {
        result.reason = Reason::ConfigurationConflict;
        return result;
    }
    let mut allowed: Option<BTreeSet<String>> = None;
    for policy in &winners {
        let relevant: Vec<&Override> = overrides
            .iter()
            .filter(|o| {
                o.policy_id == policy.id
                    && o.session_id == context.session_id
                    && o.phase_epoch == context.phase_epoch
                    && o.revoked_at_ms.is_none()
            })
            .collect();
        if relevant.iter().any(|o| {
            o.expires_at_ms <= o.created_at_ms
                || !valid_id(&o.id)
                || o.reason.trim().is_empty()
                || o.replacement_model_ids.iter().any(|id| !valid_id(id))
        }) {
            result.reason = Reason::ConfigurationConflict;
            return result;
        }
        let active: Vec<&Override> = relevant
            .into_iter()
            .filter(|o| o.created_at_ms <= now_ms && now_ms < o.expires_at_ms)
            .collect();
        if active.len() > 1 {
            result.reason = Reason::ConfigurationConflict;
            return result;
        }
        let ids = active
            .first()
            .map_or(&policy.allowed_model_ids, |o| &o.replacement_model_ids);
        let set: BTreeSet<String> = ids.iter().cloned().collect();
        allowed = Some(match allowed {
            None => set,
            Some(previous) => previous.intersection(&set).cloned().collect(),
        });
    }
    let allowed = allowed.unwrap_or_default();
    if allowed.is_empty() {
        result.reason = Reason::ConfigurationConflict;
        return result;
    }
    let Some(evidence) = evidence else {
        result.reason = Reason::MissingServedEvidence;
        return result;
    };
    if evidence.provider != context.provider || evidence.session_id != context.session_id {
        result.reason = Reason::DifferentSession;
        return result;
    }
    if evidence.phase != Some(context.phase) || evidence.phase_epoch != Some(context.phase_epoch) {
        result.reason = Reason::DifferentPhase;
        return result;
    }
    if evidence.observed_at_ms > now_ms || now_ms >= evidence.expires_at_ms {
        result.reason = Reason::StaleOrFutureEvidence;
        return result;
    }
    let notify = winners.iter().any(|p| p.action != Action::Observe);
    if evidence.kind != ModelEvidenceKind::ServedResponse
        || evidence.confidence != Confidence::Direct
        || evidence
            .inference_id
            .as_deref()
            .is_none_or(|id| !valid_id(id))
        || evidence
            .evidence_ref
            .as_deref()
            .is_none_or(|id| !valid_id(id))
        || !valid_id(&evidence.model_id)
    {
        result.reason = Reason::MissingServedEvidence;
        result.selection_mismatch_warning = matches!(
            evidence.kind,
            ModelEvidenceKind::SessionSelection
                | ModelEvidenceKind::HookCommonMetadata
                | ModelEvidenceKind::RequestAttribute
        ) && !allowed.contains(&evidence.model_id);
        result.should_notify = notify && result.selection_mismatch_warning;
        return result;
    }
    if allowed.contains(&evidence.model_id) {
        result.decision = Decision::Compliant;
        result.reason = Reason::ModelAllowed;
    } else {
        result.decision = Decision::Violation;
        result.reason = Reason::ModelNotAllowed;
        result.should_notify = notify;
    }
    result
}

fn applicable(policy: &Policy, context: &Context) -> bool {
    policy.enabled
        && policy.phase == context.phase
        && policy.scope.provider.is_none_or(|p| p == context.provider)
        && policy
            .scope
            .project_key
            .as_ref()
            .is_none_or(|p| context.project_key.as_ref() == Some(p))
        && policy
            .scope
            .session_id
            .as_ref()
            .is_none_or(|s| s == &context.session_id)
}

fn rank(policy: &Policy) -> (u8, i32) {
    let specificity = if policy.scope.session_id.is_some() {
        3
    } else if policy.scope.project_key.is_some() {
        2
    } else if policy.scope.provider.is_some() {
        1
    } else {
        0
    };
    (specificity, policy.priority)
}

fn valid_id(value: &str) -> bool {
    normalized_schema::validate_identifier("policyIdentifier", value).is_ok()
}
