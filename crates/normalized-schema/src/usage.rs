//! Usage metadata only. Totals never combine units, metrics, or overlapping populations.
use crate::{validate_identifier, EventType, ModelEvidenceKind, NormalizedEvent, ValidationError};
use serde::{Deserialize, Serialize};

pub const MAX_USAGE_SAMPLES: usize = 64;

macro_rules! wire_enum {
    ($name:ident { $($variant:ident => $wire:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
        pub enum $name { $(#[serde(rename=$wire)] $variant),+ }
        impl $name { pub const fn as_str(self) -> &'static str { match self { $(Self::$variant => $wire),+ } } }
    }
}
wire_enum!(UsageMode { Delta=>"delta", Cumulative=>"cumulative" });
wire_enum!(UsageScope { Inference=>"inference", Turn=>"turn", Session=>"session", Account=>"account" });
wire_enum!(UsageUnit { Tokens=>"tokens", Requests=>"requests", AiCredits=>"ai_credits", Usd=>"usd" });
wire_enum!(UsageMetric { Input=>"input", Output=>"output", CacheRead=>"cache_read", CacheWrite=>"cache_write", Reasoning=>"reasoning", Requests=>"requests", Credits=>"credits", Cost=>"cost" });
wire_enum!(RelationToTotal { Included=>"included", Exclusive=>"exclusive", Unknown=>"unknown" });

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageSample {
    pub sample_id: String,
    pub mode: UsageMode,
    pub scope: UsageScope,
    pub scope_key: String,
    pub counter_key: String,
    pub counter_epoch: String,
    pub revision: Option<i64>,
    pub unit: UsageUnit,
    pub metric: UsageMetric,
    pub value: f64,
    pub accounting_series: String,
    pub relation_to_total: RelationToTotal,
    /// Trusted adapter evidence of counting from zero, never inferred from a lower value.
    #[serde(default)]
    pub known_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UsageReport {
    pub event: NormalizedEvent,
    pub samples: Vec<UsageSample>,
}

impl UsageReport {
    pub fn validate(&self) -> Result<(), ValidationError> {
        self.event.validate()?;
        if self.event.event_type != EventType::UsageReported {
            return Err(ValidationError::InvalidUsage(
                "report requires usage.reported",
            ));
        }
        if self.event.activity.is_some()
            || self.event.model_selected.is_some()
            || self.event.model_requested.is_some()
            || self.event.model_resolved.is_some()
            || self.event.model_observed.is_some()
            || self.event.model_evidence_kind != ModelEvidenceKind::Unknown
        {
            return Err(ValidationError::InvalidUsage(
                "usage report cannot assert activity or model evidence",
            ));
        }
        if self.samples.is_empty() || self.samples.len() > MAX_USAGE_SAMPLES {
            return Err(ValidationError::InvalidUsage(
                "usage report requires 1..64 samples",
            ));
        }
        for sample in &self.samples {
            sample.validate(&self.event)?;
        }
        Ok(())
    }
}

impl UsageSample {
    pub fn validate(&self, event: &NormalizedEvent) -> Result<(), ValidationError> {
        for (field, id) in [
            ("sampleId", &self.sample_id),
            ("scopeKey", &self.scope_key),
            ("counterKey", &self.counter_key),
            ("counterEpoch", &self.counter_epoch),
            ("accountingSeries", &self.accounting_series),
        ] {
            validate_identifier(field, id)?;
        }
        if !self.value.is_finite() || self.value < 0.0 {
            return Err(ValidationError::InvalidUsage(
                "usage must be finite and nonnegative",
            ));
        }
        let valid_unit = matches!(
            (self.unit, self.metric),
            (
                UsageUnit::Tokens,
                UsageMetric::Input
                    | UsageMetric::Output
                    | UsageMetric::CacheRead
                    | UsageMetric::CacheWrite
                    | UsageMetric::Reasoning
            ) | (UsageUnit::Requests, UsageMetric::Requests)
                | (UsageUnit::AiCredits, UsageMetric::Credits)
                | (UsageUnit::Usd, UsageMetric::Cost)
        );
        if !valid_unit {
            return Err(ValidationError::InvalidUsage(
                "incompatible usage unit and metric",
            ));
        }
        if matches!(self.unit, UsageUnit::Tokens | UsageUnit::Requests)
            && (self.value.fract() != 0.0 || self.value > 9_007_199_254_740_991.0)
        {
            return Err(ValidationError::InvalidUsage(
                "count exceeds exact nonnegative integer range",
            ));
        }
        if self.revision.is_some_and(|r| r < 0) {
            return Err(ValidationError::InvalidUsage(
                "revision must be nonnegative",
            ));
        }
        if self.known_zero && (self.mode != UsageMode::Cumulative || self.revision.is_none()) {
            return Err(ValidationError::InvalidUsage(
                "known-zero evidence requires an ordered cumulative sample",
            ));
        }
        let expected = match self.scope {
            UsageScope::Inference => event.source_inference_id.as_deref(),
            UsageScope::Turn => event.source_turn_id.as_deref(),
            UsageScope::Session => Some(event.source_session_id.as_str()),
            UsageScope::Account => Some(event.account_key.as_str()),
        };
        if expected != Some(self.scope_key.as_str()) {
            return Err(ValidationError::InvalidUsage(
                "usage scope does not match event identity",
            ));
        }
        Ok(())
    }
}
