//! Independent account quota observations. No session is fabricated and no units are converted.
use crate::{
    ingest::{namespaced_key, IngestError},
    Store,
};
use normalized_schema::quota::QuotaSnapshot;
use normalized_schema::{
    format_canonical_utc, parse_canonical_utc, validate_identifier, Confidence, Provider,
    SCHEMA_VERSION,
};
use rusqlite::{params, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaObservation {
    pub schema_version: u32,
    pub event_id: String,
    pub integration_id: String,
    pub account_key: String,
    pub provider: Provider,
    pub surface: String,
    pub source: String,
    #[serde(default)]
    pub source_event_id: Option<String>,
    #[serde(default)]
    pub source_sequence: Option<i64>,
    #[serde(default)]
    pub confidence: Confidence,
    pub observed_at: String,
    pub received_at: String,
    pub snapshots: Vec<QuotaSnapshot>,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct QuotaOutcome {
    pub event_key: String,
    pub already_committed: bool,
    pub updated: usize,
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountQuota {
    pub snapshot: QuotaSnapshot,
    pub source_integration_id: String,
    pub source: String,
    pub confidence: String,
    pub freshness: String,
}

fn text(value: impl Serialize) -> Result<String, IngestError> {
    serde_json::to_value(value)?
        .as_str()
        .map(str::to_owned)
        .ok_or(IngestError::Invalid("invalid quota enum"))
}
fn canonical_time(field: &'static str, value: &str) -> Result<String, IngestError> {
    Ok(format_canonical_utc(parse_canonical_utc(field, value)?))
}
fn canonical_optional(
    field: &'static str,
    value: &Option<String>,
) -> Result<Option<String>, IngestError> {
    value.as_ref().map(|v| canonical_time(field, v)).transpose()
}

impl QuotaObservation {
    fn canonical(&self) -> Result<Self, IngestError> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(IngestError::Invalid("unsupported quota schema version"));
        }
        for (field, value) in [
            ("eventId", &self.event_id),
            ("integrationId", &self.integration_id),
            ("accountKey", &self.account_key),
            ("surface", &self.surface),
            ("source", &self.source),
        ] {
            validate_identifier(field, value)?;
        }
        if let Some(id) = &self.source_event_id {
            validate_identifier("sourceEventId", id)?;
        }
        if self.source_sequence.is_some_and(|v| v < 0) || !(1..=32).contains(&self.snapshots.len())
        {
            return Err(IngestError::Invalid(
                "quota observation requires 1..32 snapshots and nonnegative source sequence",
            ));
        }
        let mut canonical = self.clone();
        canonical.observed_at = canonical_time("observedAt", &self.observed_at)?;
        canonical.received_at = canonical_time("receivedAt", &self.received_at)?;
        canonical.event_id = namespaced_key(&[
            "event",
            &self.integration_id,
            &self.account_key,
            &self.source,
            self.source_event_id.as_deref().unwrap_or(&self.event_id),
            "quota.updated",
        ]);
        let provider = text(self.provider)?;
        let mut keys = std::collections::BTreeSet::new();
        for snapshot in &mut canonical.snapshots {
            snapshot
                .validate()
                .map_err(|_| IngestError::Invalid("invalid account quota snapshot"))?;
            if snapshot.provider != self.provider || snapshot.account_key != self.account_key {
                return Err(IngestError::IntegrationBindingConflict);
            }
            snapshot.quota_key = namespaced_key(&[
                "quota",
                &provider,
                &self.account_key,
                &snapshot.scope,
                &snapshot.bucket,
                snapshot.unit.as_str(),
                &snapshot.window_key,
            ]);
            if !keys.insert(snapshot.quota_key.clone()) {
                return Err(IngestError::Invalid(
                    "duplicate quota window identity in one observation",
                ));
            }
            snapshot.observed_at = canonical_time("observedAt", &snapshot.observed_at)?;
            snapshot.window_start = canonical_optional("windowStart", &snapshot.window_start)?;
            snapshot.window_end = canonical_optional("windowEnd", &snapshot.window_end)?;
            snapshot.resets_at = canonical_optional("resetsAt", &snapshot.resets_at)?;
            snapshot.stale_after = canonical_optional("staleAfter", &snapshot.stale_after)?;
        }
        Ok(canonical)
    }
}

struct QuotaHead {
    integration: String,
    source: String,
    observed_at: String,
    revision: Option<i64>,
    confidence: Option<String>,
}

impl QuotaHead {
    fn accepts(&self, observation: &QuotaObservation, snapshot: &QuotaSnapshot) -> bool {
        if self.integration != observation.integration_id || self.source != observation.source {
            return false;
        }
        // Unverified history cannot advance the authoritative head or poison its revision.
        if self.confidence.as_deref() == Some("direct")
            && observation.confidence != Confidence::Direct
        {
            return false;
        }
        if self.confidence.as_deref() != Some("direct")
            && observation.confidence == Confidence::Direct
        {
            return true;
        }
        match (self.revision, observation.source_sequence) {
            (Some(old), Some(new)) => new > old && snapshot.observed_at >= self.observed_at,
            (Some(_), None) => false,
            _ => snapshot.observed_at > self.observed_at,
        }
    }
}

impl Store {
    /// Local adapter API. The Codex loopback route does not admit this separate envelope.
    pub fn observe_quotas(
        &mut self,
        observation: &QuotaObservation,
    ) -> Result<QuotaOutcome, IngestError> {
        let canonical = observation.canonical()?;
        let provider = text(canonical.provider)?;
        let confidence = text(canonical.confidence)?;
        let payload = serde_json::to_string(&canonical)?;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let binding: Option<(String, String, String)> = tx
            .query_row(
                "SELECT account_key,provider,surface FROM integrations WHERE id=?1",
                [&canonical.integration_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        match binding {
            None => return Err(IngestError::UnknownIntegration),
            Some(binding)
                if binding
                    != (
                        canonical.account_key.clone(),
                        provider.clone(),
                        canonical.surface.clone(),
                    ) =>
            {
                return Err(IngestError::IntegrationBindingConflict)
            }
            _ => {}
        }
        let previous: Option<String> = tx
            .query_row(
                "SELECT payload_json FROM events WHERE event_id=?1",
                [&canonical.event_id],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(previous) = previous {
            let mut previous: QuotaObservation = serde_json::from_str(&previous)?;
            previous.received_at = canonical.received_at.clone();
            if previous != canonical {
                return Err(IngestError::EventIdentityConflict);
            }
            tx.commit()?;
            return Ok(QuotaOutcome {
                event_key: canonical.event_id,
                already_committed: true,
                updated: 0,
            });
        }
        tx.execute("INSERT INTO events(event_id,integration_id,account_key,session_id,type,source,source_event_id,source_sequence,confidence,observed_at,received_at,payload_json)
            VALUES (?1,?2,?3,NULL,'quota.updated',?4,?5,?6,?7,?8,?9,?10)",params![canonical.event_id,canonical.integration_id,canonical.account_key,canonical.source,canonical.source_event_id,canonical.source_sequence,confidence,canonical.observed_at,canonical.received_at,payload])?;
        let mut updated = 0;
        for (index, snapshot) in canonical.snapshots.iter().enumerate() {
            let head=tx.query_row("SELECT q.source_integration_id,q.source,q.observed_at,p.source_sequence,p.confidence FROM quotas q LEFT JOIN quota_projection p ON p.quota_key=q.quota_key WHERE q.quota_key=?1",[&snapshot.quota_key],|r|Ok(QuotaHead{integration:r.get(0)?,source:r.get(1)?,observed_at:r.get(2)?,revision:r.get(3)?,confidence:r.get(4)?})).optional()?;
            let accepted = head.is_none_or(|head| head.accepts(&canonical, snapshot));
            let encoded = serde_json::to_string(snapshot)?;
            tx.execute("INSERT INTO quota_evidence(event_id,snapshot_index,quota_key,snapshot_json,disposition) VALUES (?1,?2,?3,?4,?5)",params![canonical.event_id,index as i64,snapshot.quota_key,encoded,if accepted {"accepted"} else {"history"}])?;
            if !accepted {
                continue;
            }
            tx.execute("INSERT INTO quotas(quota_key,provider,account_key,scope,bucket,unit,window_key,source_integration_id,source,window_start,window_end,used,limit_value,remaining,used_percent,resets_at,observed_at,stale_after)
                VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)
                ON CONFLICT(quota_key) DO UPDATE SET window_start=excluded.window_start,window_end=excluded.window_end,used=excluded.used,limit_value=excluded.limit_value,remaining=excluded.remaining,used_percent=excluded.used_percent,resets_at=excluded.resets_at,observed_at=excluded.observed_at,stale_after=excluded.stale_after",
                params![snapshot.quota_key,provider,canonical.account_key,snapshot.scope,snapshot.bucket,snapshot.unit.as_str(),snapshot.window_key,canonical.integration_id,canonical.source,snapshot.window_start,snapshot.window_end,snapshot.used,snapshot.limit,snapshot.remaining,snapshot.used_percent,snapshot.resets_at,snapshot.observed_at,snapshot.stale_after])?;
            tx.execute("INSERT INTO quota_projection(quota_key,source_sequence,confidence,event_id) VALUES (?1,?2,?3,?4) ON CONFLICT(quota_key) DO UPDATE SET source_sequence=excluded.source_sequence,confidence=excluded.confidence,event_id=excluded.event_id",params![snapshot.quota_key,canonical.source_sequence,confidence,canonical.event_id])?;
            updated += 1;
        }
        tx.execute(
            "INSERT INTO outbox(id,event_id,kind,payload_json) VALUES (?1,?1,'quota.changed',?2)",
            params![
                canonical.event_id,
                serde_json::json!({"eventId":canonical.event_id,"updated":updated}).to_string()
            ],
        )?;
        tx.execute(
            "UPDATE integrations SET last_seen_at=MAX(COALESCE(last_seen_at,''),?1) WHERE id=?2",
            params![canonical.received_at, canonical.integration_id],
        )?;
        tx.commit()?;
        Ok(QuotaOutcome {
            event_key: canonical.event_id,
            already_committed: false,
            updated,
        })
    }

    /// Up to 200 independently identified windows, newest observation first.
    pub fn account_quotas(
        &self,
        provider: Provider,
        account: &str,
        now: &str,
        clock_trusted: bool,
    ) -> Result<Vec<AccountQuota>, IngestError> {
        validate_identifier("accountKey", account)?;
        let now = canonical_time("now", now)?;
        let mut stmt=self.conn.prepare("SELECT e.snapshot_json,q.source_integration_id,q.source,p.confidence,i.target_verification FROM quotas q JOIN quota_projection p ON p.quota_key=q.quota_key
            JOIN quota_evidence e ON e.event_id=p.event_id AND e.quota_key=q.quota_key JOIN integrations i ON i.id=q.source_integration_id
            WHERE q.provider=?1 AND q.account_key=?2 ORDER BY q.observed_at DESC,q.quota_key ASC LIMIT 200")?;
        let mut rows = stmt.query(params![text(provider)?, account])?;
        let mut quotas = Vec::new();
        while let Some(row) = rows.next()? {
            let snapshot: QuotaSnapshot = serde_json::from_str(&row.get::<_, String>(0)?)?;
            snapshot
                .validate()
                .map_err(|_| IngestError::Invalid("stored quota metadata is invalid"))?;
            let confidence: String = row.get(3)?;
            let verification: String = row.get(4)?;
            let freshness = if !clock_trusted
                || snapshot.observed_at > now
                || snapshot.window_start.as_ref().is_some_and(|t| t > &now)
            {
                "unknown"
            } else if snapshot.window_end.as_ref().is_some_and(|t| t <= &now)
                || snapshot.resets_at.as_ref().is_some_and(|t| t <= &now)
            {
                "expired"
            } else if snapshot.stale_after.as_ref().is_some_and(|t| t <= &now) {
                "stale"
            } else if clock_trusted
                && confidence == "direct"
                && verification == "verified"
                && snapshot.stale_after.is_some()
            {
                "fresh"
            } else {
                "unknown"
            };
            quotas.push(AccountQuota {
                snapshot,
                source_integration_id: row.get(1)?,
                source: row.get(2)?,
                confidence,
                freshness: freshness.into(),
            });
        }
        Ok(quotas)
    }
}
