//! Durable, source-selected accounting. Observer samples cannot choose their canonical series.
use crate::{
    ingest::{namespaced_key, IngestError, IngestOutcome},
    Store,
};
use normalized_schema::usage::{UsageMode, UsageReport, UsageSample, UsageScope};
use normalized_schema::{
    format_canonical_utc, parse_canonical_utc, validate_identifier, NormalizedEvent,
};
use rusqlite::{params, OptionalExtension, Transaction, TransactionBehavior};
use usage_accounting::{
    reconcile_counter, CounterCheckpoint, CounterSample, ReconciliationDisposition as D,
    SampleIdentity,
};

#[derive(Debug, Clone, PartialEq)]
pub struct UsageTotal {
    pub unit: String,
    pub metric: String,
    pub relation_to_total: String,
    pub value: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub struct UsageSummary {
    pub items: Vec<UsageTotal>,
    pub partial: bool,
}

impl Store {
    /// Local configuration only. No source switch or retrospective backfill is implied.
    pub fn select_accounting_series(
        &mut self,
        session: &str,
        source: &str,
        series: &str,
        scope: UsageScope,
    ) -> Result<(), IngestError> {
        for (field, value) in [
            ("sessionId", session),
            ("source", source),
            ("accountingSeries", series),
        ] {
            validate_identifier(field, value)?;
        }
        if scope == UsageScope::Account {
            return Err(IngestError::Invalid(
                "account counters cannot become session totals",
            ));
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous:Option<(String,String,String)>=tx.query_row("SELECT source,accounting_series,scope FROM accounting_selections WHERE session_id=?1",[session],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional()?;
        if let Some(previous) = previous {
            if previous != (source.into(), series.into(), scope.as_str().into()) {
                return Err(IngestError::Invalid(
                    "canonical accounting changes require reconciliation",
                ));
            }
        } else {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sessions WHERE id=?1)",
                [session],
                |r| r.get(0),
            )?;
            if !exists {
                return Err(IngestError::UnknownSession);
            }
            let evidence: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM usage_evidence WHERE session_id=?1)",
                [session],
                |r| r.get(0),
            )?;
            if evidence {
                return Err(IngestError::Invalid(
                    "existing accounting evidence requires explicit backfill",
                ));
            }
            tx.execute("INSERT INTO accounting_selections(session_id,source,accounting_series,scope) VALUES (?1,?2,?3,?4)",params![session,source,series,scope.as_str()])?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn ingest_usage(&mut self, report: &UsageReport) -> Result<IngestOutcome, IngestError> {
        report.validate()?;
        self.ingest_metadata(&report.event, None, Some(&report.samples))
    }

    /// Separate metric/unit/relationship rows; callers must not sum these into mixed totals.
    /// `since` filters contribution observation times, not an invented provider billing window.
    pub fn session_usage(
        &self,
        session: &str,
        since: Option<&str>,
    ) -> Result<UsageSummary, IngestError> {
        validate_identifier("sessionId", session)?;
        let since = since
            .map(|s| parse_canonical_utc("since", s).map(format_canonical_utc))
            .transpose()?;
        let mut stmt=self.conn.prepare("SELECT c.unit,c.metric,u.relation_to_total,SUM(c.accepted_delta)
            FROM usage_contributions c JOIN events e ON e.event_id=c.event_id
            JOIN usage_evidence u ON u.event_id=c.event_id AND u.sample_index=c.sample_index
            WHERE e.session_id=?1 AND (?2 IS NULL OR e.observed_at>=?2)
            GROUP BY c.unit,c.metric,u.relation_to_total ORDER BY c.unit,c.metric,u.relation_to_total")?;
        let items = stmt
            .query_map(params![session, since], |r| {
                Ok(UsageTotal {
                    unit: r.get(0)?,
                    metric: r.get(1)?,
                    relation_to_total: r.get(2)?,
                    value: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if items.iter().any(|i| {
            !i.value.is_finite()
                || (matches!(i.unit.as_str(), "tokens" | "requests")
                    && i.value > 9_007_199_254_740_991.0)
        }) {
            return Err(IngestError::Invalid(
                "usage aggregate exceeds representable range",
            ));
        }
        let partial:bool=self.conn.query_row("SELECT NOT EXISTS(SELECT 1 FROM accounting_selections WHERE session_id=?1)
            OR EXISTS(SELECT 1 FROM usage_evidence WHERE session_id=?1 AND disposition IN ('baseline','frozen','unsequenced','unverified'))",
            [session],|r|r.get(0))?;
        Ok(UsageSummary { items, partial })
    }
}

pub(crate) fn apply(
    tx: &Transaction<'_>,
    event: &NormalizedEvent,
    session: &str,
    samples: &[UsageSample],
) -> Result<(), IngestError> {
    let selection: Option<(String, String, String)> = tx
        .query_row(
            "SELECT source,accounting_series,scope FROM accounting_selections WHERE session_id=?1",
            [session],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    for (index, sample) in samples.iter().enumerate() {
        let json = serde_json::to_string(sample)?;
        let direct = event.confidence == normalized_schema::Confidence::Direct;
        let duplicate:Option<String>=tx.query_row("SELECT sample_json FROM usage_evidence WHERE session_id=?1 AND source=?2 AND sample_id=?3 AND direct=?4",params![session,event.source,sample.sample_id,direct],|r|r.get(0)).optional()?;
        if let Some(previous) = duplicate {
            let previous: UsageSample = serde_json::from_str(&previous)?;
            if &previous != sample {
                return Err(IngestError::EventIdentityConflict);
            }
            continue; // This event retains the duplicate evidence in its immutable report body.
        }
        let counter = namespaced_key(&[
            "counter",
            session,
            &event.source,
            sample.scope.as_str(),
            &sample.counter_key,
        ]);
        let mut delta = 0.0;
        let matches_selection = selection.as_ref().is_some_and(|(source, series, scope)| {
            source == &event.source
                && series == &sample.accounting_series
                && scope == sample.scope.as_str()
        });
        let canonical = matches_selection && direct;
        let mut disposition = if matches_selection && !canonical {
            "unverified"
        } else {
            "reconciliation_only"
        };
        if canonical {
            let modes:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_evidence WHERE session_id=?1 AND source=?2 AND counter_key=?3 AND counter_epoch=?4 AND scope_key=?5 AND accounting_series=?6 AND unit=?7 AND metric=?8 AND mode<>?9 AND direct=1)",
                params![session,event.source,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),sample.mode.as_str()],|r|r.get(0))?;
            if modes {
                return Err(IngestError::Invalid(
                    "counter mode changed within an accounting epoch",
                ));
            }
            let relation_changed:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM usage_evidence WHERE session_id=?1 AND source=?2 AND counter_key=?3 AND counter_epoch=?4 AND scope_key=?5 AND accounting_series=?6 AND unit=?7 AND metric=?8 AND relation_to_total<>?9 AND direct=1)",
                params![session,event.source,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),sample.relation_to_total.as_str()],|r|r.get(0))?;
            if relation_changed {
                return Err(IngestError::Invalid(
                    "counter relationship changed within an accounting epoch",
                ));
            }
            if sample.mode == UsageMode::Delta {
                delta = sample.value;
                disposition = "delta";
            } else if let Some(revision) = sample.revision {
                let historical:Option<f64>=tx.query_row("SELECT value FROM usage_evidence WHERE session_id=?1 AND source=?2 AND counter_key=?3 AND counter_epoch=?4 AND scope_key=?5 AND accounting_series=?6 AND unit=?7 AND metric=?8 AND revision=?9 AND direct=1 LIMIT 1",
                    params![session,event.source,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),revision],|r|r.get(0)).optional()?;
                if historical.is_some_and(|value| value != sample.value) {
                    return Err(IngestError::Invalid(
                        "counter revision has conflicting content",
                    ));
                }
                let prior:Option<CounterCheckpoint>=tx.query_row("SELECT last_revision,last_value,accepted_total,baseline_value,frozen FROM usage_checkpoints
                    WHERE integration_id=?1 AND account_key=?2 AND counter_key=?3 AND counter_epoch=?4 AND scope_key=?5 AND accounting_series=?6 AND unit=?7 AND metric=?8",
                    params![event.integration_id,event.account_key,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str()],
                    |r|Ok(CounterCheckpoint{last_revision:r.get(0)?,last_value:r.get(1)?,accepted_total:r.get(2)?,baseline_value:r.get(3)?,frozen:r.get(4)?})).optional()?;
                let outcome = reconcile_counter(
                    &CounterSample {
                        identity: SampleIdentity {
                            counter_key: counter.clone(),
                            epoch: sample.counter_epoch.clone(),
                            scope_key: sample.scope_key.clone(),
                            accounting_series: sample.accounting_series.clone(),
                            unit: sample.unit.as_str().into(),
                            metric: sample.metric.as_str().into(),
                        },
                        revision: revision as u64,
                        value: sample.value,
                        known_zero: sample.known_zero,
                    },
                    prior.as_ref(),
                )
                .map_err(|_| IngestError::Invalid("counter reconciliation failed"))?;
                delta = outcome.accepted_delta;
                disposition = match outcome.disposition {
                    D::InitialMidCounterBaseline => "baseline",
                    D::AmbiguousDecreaseFrozen | D::FrozenEpochNoOp => "frozen",
                    D::DuplicateReplay => "replay",
                    D::OlderHistoryOnly => "history",
                    _ => "cumulative",
                };
                let cp = outcome.checkpoint;
                if prior.as_ref() != Some(&cp) {
                    tx.execute("INSERT INTO usage_checkpoints(integration_id,account_key,counter_key,counter_epoch,scope_key,accounting_series,unit,metric,last_revision,last_value,accepted_total,baseline_value,coverage_start,last_event_id,frozen)
                        VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15)
                        ON CONFLICT(integration_id,account_key,counter_key,counter_epoch,scope_key,accounting_series,unit,metric) DO UPDATE SET last_revision=excluded.last_revision,last_value=excluded.last_value,accepted_total=excluded.accepted_total,last_event_id=excluded.last_event_id,frozen=excluded.frozen",
                        params![event.integration_id,event.account_key,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),cp.last_revision,cp.last_value,cp.accepted_total,cp.baseline_value,event.observed_at,event.event_id,cp.frozen])?;
                }
            } else {
                disposition = "unsequenced";
            }
        }
        tx.execute("INSERT INTO usage_evidence(event_id,sample_index,session_id,source,sample_id,counter_key,counter_epoch,scope_key,accounting_series,unit,metric,relation_to_total,mode,revision,value,sample_json,disposition,direct)
            VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18)",
            params![event.event_id,index as i64,session,event.source,sample.sample_id,counter,sample.counter_epoch,sample.scope_key,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),sample.relation_to_total.as_str(),sample.mode.as_str(),sample.revision,sample.value,json,disposition,direct])?;
        if canonical && delta > 0.0 {
            let previous:f64=tx.query_row("SELECT COALESCE(SUM(c.accepted_delta),0) FROM usage_contributions c JOIN usage_evidence u ON u.event_id=c.event_id AND u.sample_index=c.sample_index WHERE u.session_id=?1 AND c.unit=?2 AND c.metric=?3 AND u.relation_to_total=?4",
                params![session,sample.unit.as_str(),sample.metric.as_str(),sample.relation_to_total.as_str()],|r|r.get(0))?;
            let total = previous + delta;
            if !total.is_finite()
                || (matches!(
                    sample.unit,
                    normalized_schema::usage::UsageUnit::Tokens
                        | normalized_schema::usage::UsageUnit::Requests
                ) && total > 9_007_199_254_740_991.0)
            {
                return Err(IngestError::Invalid(
                    "usage total exceeds representable range",
                ));
            }
            tx.execute("INSERT INTO usage_contributions(event_id,sample_index,accounting_series,unit,metric,accepted_delta) VALUES (?1,?2,?3,?4,?5,?6)",
                params![event.event_id,index as i64,sample.accounting_series,sample.unit.as_str(),sample.metric.as_str(),delta])?;
        }
    }
    Ok(())
}
