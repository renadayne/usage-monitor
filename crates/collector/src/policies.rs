//! Durable local policy configuration. Saving or reading policies never makes provider calls.

use normalized_schema::{validate_identifier, Phase};
use policy_engine::{Action, Policy, Scope};
use rusqlite::{params, TransactionBehavior};
use serde::{Deserialize, Serialize};

use crate::ingest::IngestError;
use crate::Store;

pub const MAX_POLICIES: usize = 500;
pub const MAX_ALLOWED_MODELS: usize = 32;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PolicyRules {
    phase: Phase,
    allowed_model_ids: Vec<String>,
}

impl Store {
    /// Save or update a local policy configuration atomically without deleting audit history.
    pub fn save_policy(&mut self, policy: &Policy) -> Result<(), IngestError> {
        let normalized = normalize_policy(policy)?;
        let scope_json = serde_json::to_string(&normalized.scope)?;
        let rules_json = serde_json::to_string(&PolicyRules {
            phase: normalized.phase,
            allowed_model_ids: normalized.allowed_model_ids,
        })?;
        let action = action_text(normalized.action)?;
        let enabled = i64::from(normalized.enabled);
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let exists: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM policies WHERE id = ?1)",
            [&normalized.id],
            |r| r.get(0),
        )?;
        if !exists {
            let count: i64 = tx.query_row("SELECT COUNT(*) FROM policies", [], |r| r.get(0))?;
            if count >= MAX_POLICIES as i64 {
                return Err(IngestError::Invalid("policy limit reached"));
            }
        }
        tx.execute(
            "INSERT INTO policies(id, scope_json, rules_json, action, enabled, priority)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(id) DO UPDATE SET
                 scope_json = excluded.scope_json,
                 rules_json = excluded.rules_json,
                 action = excluded.action,
                 enabled = excluded.enabled,
                 priority = excluded.priority",
            params![
                normalized.id,
                scope_json,
                rules_json,
                action,
                enabled,
                normalized.priority
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// List stored local policies bounded to 500 records in deterministic priority DESC, id ASC order.
    pub fn list_policies(&self) -> Result<Vec<Policy>, IngestError> {
        let mut stmt = self.conn.prepare(
            "SELECT id, scope_json, rules_json, action, enabled, priority
             FROM policies ORDER BY priority DESC, id ASC LIMIT ?1",
        )?;
        let mut rows = stmt.query([MAX_POLICIES as i64])?;
        let mut policies = Vec::new();
        while let Some(row) = rows.next()? {
            let scope: Scope = serde_json::from_str(&row.get::<_, String>(1)?)?;
            let rules: PolicyRules = serde_json::from_str(&row.get::<_, String>(2)?)?;
            let action: Action =
                serde_json::from_value(serde_json::Value::String(row.get::<_, String>(3)?))?;
            let enabled = match row.get::<_, i64>(4)? {
                0 => false,
                1 => true,
                _ => return Err(IngestError::Invalid("invalid policy enabled flag")),
            };
            let priority = i32::try_from(row.get::<_, i64>(5)?)
                .map_err(|_| IngestError::Invalid("policy priority out of range"))?;
            policies.push(normalize_policy(&Policy {
                id: row.get(0)?,
                scope,
                phase: rules.phase,
                allowed_model_ids: rules.allowed_model_ids,
                priority,
                action,
                enabled,
            })?);
        }
        Ok(policies)
    }
}

fn normalize_policy(policy: &Policy) -> Result<Policy, IngestError> {
    validate_identifier("policyId", &policy.id)?;
    if let Some(project_key) = policy.scope.project_key.as_deref() {
        validate_identifier("projectKey", project_key)?;
    }
    if let Some(session_id) = policy.scope.session_id.as_deref() {
        validate_identifier("sessionId", session_id)?;
    }
    if policy.phase == Phase::Unassigned {
        return Err(IngestError::Invalid("policy phase must not be UNASSIGNED"));
    }
    if policy.action == Action::BlockNextTurn {
        return Err(IngestError::Invalid(
            "block_next_turn is unavailable in the desktop observer MVP",
        ));
    }
    if policy.allowed_model_ids.is_empty() || policy.allowed_model_ids.len() > MAX_ALLOWED_MODELS {
        return Err(IngestError::Invalid(
            "policy must allow between 1 and 32 model IDs",
        ));
    }
    let mut allowed_model_ids = policy.allowed_model_ids.clone();
    for model_id in &allowed_model_ids {
        validate_identifier("allowedModelId", model_id)?;
    }
    allowed_model_ids.sort();
    allowed_model_ids.dedup();
    Ok(Policy {
        id: policy.id.clone(),
        scope: policy.scope.clone(),
        phase: policy.phase,
        allowed_model_ids,
        priority: policy.priority,
        action: policy.action,
        enabled: policy.enabled,
    })
}

fn action_text(action: Action) -> Result<String, IngestError> {
    serde_json::to_value(action)?
        .as_str()
        .map(str::to_owned)
        .ok_or(IngestError::Invalid("expected a named policy action"))
}
