//! Strict account-scoped quota and rate-limit snapshot DTOs.
//!
//! Quota snapshots represent account-level provider quota or rate-limit observations only,
//! never session context-window capacity. Snapshots for distinct windows (such as 5-hour,
//! weekly, monthly, or credit buckets) remain independent, and missing bounds, resets, or
//! counters stay absent rather than being derived, converted, clamped, or aggregated.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::{parse_canonical_utc, validate_identifier, Provider, ValidationError};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaUnit {
    Tokens,
    Requests,
    AiCredits,
    Usd,
    Percentage,
}

impl QuotaUnit {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Tokens => "tokens",
            Self::Requests => "requests",
            Self::AiCredits => "ai_credits",
            Self::Usd => "usd",
            Self::Percentage => "percentage",
        }
    }
}

/// Sanitized validation error for [`QuotaSnapshot`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuotaError {
    Validation(ValidationError),
    InvalidQuota(&'static str),
}

impl fmt::Display for QuotaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Validation(err) => write!(f, "{err}"),
            Self::InvalidQuota(reason) => write!(f, "invalid quota snapshot: {reason}"),
        }
    }
}

impl std::error::Error for QuotaError {}

impl From<ValidationError> for QuotaError {
    fn from(err: ValidationError) -> Self {
        Self::Validation(err)
    }
}

/// Account-scoped quota snapshot DTO.
///
/// Each window snapshot is independent and preserves source-reported facts as-is without
/// clamping overage or deriving missing counters, percentages, or reset timestamps.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QuotaSnapshot {
    /// Caller-facing source key; the state store recomputes its canonical storage key.
    pub quota_key: String,
    pub provider: Provider,
    pub account_key: String,
    pub scope: String,
    pub bucket: String,
    pub unit: QuotaUnit,
    pub window_key: String,
    #[serde(default)]
    pub window_start: Option<String>,
    #[serde(default)]
    pub window_end: Option<String>,
    #[serde(default)]
    pub resets_at: Option<String>,
    #[serde(default)]
    pub used: Option<f64>,
    #[serde(default)]
    pub limit: Option<f64>,
    #[serde(default)]
    pub remaining: Option<f64>,
    #[serde(default)]
    pub used_percent: Option<f64>,
    pub observed_at: String,
    #[serde(default)]
    pub stale_after: Option<String>,
}

impl QuotaSnapshot {
    pub fn validate(&self) -> Result<(), QuotaError> {
        for (field, value) in [
            ("quotaKey", &self.quota_key),
            ("accountKey", &self.account_key),
            ("scope", &self.scope),
            ("bucket", &self.bucket),
            ("windowKey", &self.window_key),
        ] {
            validate_identifier(field, value)?;
        }

        let observed_at = parse_canonical_utc("observedAt", &self.observed_at)?;
        let window_start = self
            .window_start
            .as_deref()
            .map(|raw| parse_canonical_utc("windowStart", raw))
            .transpose()?;
        let window_end = self
            .window_end
            .as_deref()
            .map(|raw| parse_canonical_utc("windowEnd", raw))
            .transpose()?;
        let _ = self
            .resets_at
            .as_deref()
            .map(|raw| parse_canonical_utc("resetsAt", raw))
            .transpose()?;
        let stale_after = self
            .stale_after
            .as_deref()
            .map(|raw| parse_canonical_utc("staleAfter", raw))
            .transpose()?;

        if let (Some(start), Some(end)) = (window_start, window_end) {
            if start >= end {
                return Err(QuotaError::InvalidQuota(
                    "windowStart must be earlier than windowEnd",
                ));
            }
        }
        if let Some(stale) = stale_after {
            if stale <= observed_at {
                return Err(QuotaError::InvalidQuota(
                    "staleAfter must be later than observedAt",
                ));
            }
        }

        for value in [self.used, self.limit, self.used_percent]
            .into_iter()
            .flatten()
        {
            if !value.is_finite() || value < 0.0 {
                return Err(QuotaError::InvalidQuota(
                    "used, limit, and usedPercent must be finite and nonnegative",
                ));
            }
        }
        if let Some(remaining) = self.remaining {
            if !remaining.is_finite() {
                return Err(QuotaError::InvalidQuota("remaining must be finite"));
            }
        }

        if self.unit == QuotaUnit::Percentage
            && (self.used.is_some() || self.limit.is_some() || self.remaining.is_some())
        {
            return Err(QuotaError::InvalidQuota(
                "percentage unit must not carry used, limit, or remaining counts",
            ));
        }

        Ok(())
    }
}
