//! Conservative cumulative counter reconciliation with explicit sample identity.
//!
//! # Accounting Invariants
//!
//! - **Caller-Bound Canonical Identity**: Every sample carries an explicit [`SampleIdentity`]
//!   (`counter_key`, `epoch`, `scope_key`, `accounting_series`, `unit`, `metric`). Selecting the
//!   canonical `scope_key` and `accounting_series` is the caller's responsibility.
//! - **No Cross-Series or Parent-Child Addition**: This module reconciles a single cumulative
//!   counter stream within one explicit epoch and identity. Callers must never sum checkpoints or
//!   accepted deltas across distinct `accounting_series` or across parent and child `scope_key`
//!   hierarchies.
//! - **Conservative Baseline & Monotonicity**:
//!   - First observation with `known_zero = true` establishes baseline `0.0` and accepts `value`.
//!   - First observation with `known_zero = false` (mid-counter attach) establishes
//!     `baseline_value = value` and accepts `0.0`.
//!   - Newer increasing revisions emit `value - last_value` if finite and non-overflowing.
//!   - Same revision with identical value is a zero-delta replay; same revision with a different
//!     value is rejected as a conflict without mutating checkpoint state.
//!   - Older revisions are recorded as history-only with zero delta and no checkpoint mutation.
//!   - Newer decreasing revisions are treated as ambiguous within the same epoch: the checkpoint
//!     is permanently frozen for that epoch (`frozen = true`) and yields zero delta, even if later
//!     samples in the same epoch recover above prior values.
//!   - A new explicit `epoch` starts an independent [`CounterCheckpoint`] rather than guessing an
//!     implicit reset from a decrease.

#![forbid(unsafe_code)]

use std::fmt;

const MAX_IDENTITY_FIELD_LEN: usize = 256;

/// Externally bound identity for a cumulative counter stream within an explicit epoch.
///
/// Callers are responsible for selecting the canonical `scope_key` and `accounting_series`.
/// Never aggregate or add values across distinct `accounting_series` or parent/child scopes.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SampleIdentity {
    pub counter_key: String,
    pub epoch: String,
    pub scope_key: String,
    pub accounting_series: String,
    pub unit: String,
    pub metric: String,
}

impl SampleIdentity {
    /// Validates that all identity fields are present, bounded, and free of control characters.
    pub fn validate(&self) -> Result<(), ReconciliationError> {
        validate_identity_field("counter_key", &self.counter_key)?;
        validate_identity_field("epoch", &self.epoch)?;
        validate_identity_field("scope_key", &self.scope_key)?;
        validate_identity_field("accounting_series", &self.accounting_series)?;
        validate_identity_field("unit", &self.unit)?;
        validate_identity_field("metric", &self.metric)?;
        Ok(())
    }

    /// Returns `true` when both identities refer to the same counter stream, scope, series, unit,
    /// and metric, differing only in `epoch` (or having the same `epoch`).
    pub fn matches_stream(&self, other: &Self) -> bool {
        self.counter_key == other.counter_key
            && self.scope_key == other.scope_key
            && self.accounting_series == other.accounting_series
            && self.unit == other.unit
            && self.metric == other.metric
    }
}

/// Validated or raw cumulative counter observation to reconcile against a prior checkpoint.
#[derive(Debug, Clone, PartialEq)]
pub struct CounterSample {
    pub identity: SampleIdentity,
    pub revision: u64,
    pub value: f64,
    /// Only establishes the initial counter baseline when no prior checkpoint exists for the epoch.
    pub known_zero: bool,
}

/// Per-epoch reconciliation checkpoint for a single canonical counter identity.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CounterCheckpoint {
    pub last_revision: u64,
    pub last_value: f64,
    pub accepted_total: f64,
    pub baseline_value: f64,
    pub frozen: bool,
}

impl CounterCheckpoint {
    /// Validates that all numeric fields in an existing checkpoint are finite and non-negative.
    pub fn validate(&self) -> Result<(), ReconciliationError> {
        if !is_valid_nonnegative_finite(self.last_value)
            || !is_valid_nonnegative_finite(self.accepted_total)
            || !is_valid_nonnegative_finite(self.baseline_value)
        {
            return Err(ReconciliationError::InvalidCheckpoint);
        }
        Ok(())
    }
}

/// Classification of how a sample was reconciled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconciliationDisposition {
    /// First sample in epoch with `known_zero = true`; full sample value accepted.
    InitialKnownZero,
    /// First sample in epoch with `known_zero = false`; baseline established with zero delta.
    InitialMidCounterBaseline,
    /// Newer revision with non-decreasing value in an unfrozen epoch.
    AcceptedIncrement,
    /// Exact replay of `last_revision` with identical value; zero delta.
    DuplicateReplay,
    /// Out-of-order revision older than `last_revision`; history-only zero delta.
    OlderHistoryOnly,
    /// Newer revision decreased below `last_value`; checkpoint frozen with ambiguous zero delta.
    AmbiguousDecreaseFrozen,
    /// Newer revision arrived after the epoch checkpoint was already frozen; zero delta.
    FrozenEpochNoOp,
}

/// Pure reconciliation result containing the updated checkpoint, accepted delta, and disposition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReconciliationOutcome {
    pub checkpoint: CounterCheckpoint,
    pub accepted_delta: f64,
    pub disposition: ReconciliationDisposition,
}

/// Sanitized reconciliation error that performs no checkpoint mutation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationError {
    /// An identity field was empty, overly long, or contained control characters.
    InvalidIdentityField { field: &'static str },
    /// Sample value was NaN, infinite, or negative.
    InvalidSampleValue,
    /// Provided prior checkpoint contained NaN, infinite, or negative values.
    InvalidCheckpoint,
    /// Same revision arrived with a different value than recorded in the checkpoint.
    ConflictingSameRevision { revision: u64 },
    /// Computing delta or new accepted total overflowed finite `f64`.
    ArithmeticOverflow,
    /// Attempted to reconcile a sample against a checkpoint bound to a different stream/scope/series.
    StreamIdentityMismatch,
}

impl fmt::Display for ReconciliationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidIdentityField { field } => {
                write!(f, "invalid sample identity field: {field}")
            }
            Self::InvalidSampleValue => {
                write!(f, "sample value must be finite and non-negative")
            }
            Self::InvalidCheckpoint => {
                write!(f, "prior checkpoint contains non-finite or negative values")
            }
            Self::ConflictingSameRevision { revision } => {
                write!(
                    f,
                    "conflicting sample value for existing revision {revision}"
                )
            }
            Self::ArithmeticOverflow => {
                write!(f, "counter reconciliation overflowed finite f64")
            }
            Self::StreamIdentityMismatch => {
                write!(
                    f,
                    "cannot reconcile across different counter streams, scopes, series, units, or metrics"
                )
            }
        }
    }
}

impl std::error::Error for ReconciliationError {}

/// Reconciles a cumulative counter sample against an optional prior checkpoint for the same epoch.
///
/// This function is pure and never mutates caller state on error.
pub fn reconcile_counter(
    sample: &CounterSample,
    prior: Option<&CounterCheckpoint>,
) -> Result<ReconciliationOutcome, ReconciliationError> {
    sample.identity.validate()?;
    if !is_valid_nonnegative_finite(sample.value) {
        return Err(ReconciliationError::InvalidSampleValue);
    }
    let value = normalize_zero(sample.value);

    let Some(cp) = prior else {
        return if sample.known_zero {
            Ok(ReconciliationOutcome {
                checkpoint: CounterCheckpoint {
                    last_revision: sample.revision,
                    last_value: value,
                    accepted_total: value,
                    baseline_value: 0.0,
                    frozen: false,
                },
                accepted_delta: value,
                disposition: ReconciliationDisposition::InitialKnownZero,
            })
        } else {
            Ok(ReconciliationOutcome {
                checkpoint: CounterCheckpoint {
                    last_revision: sample.revision,
                    last_value: value,
                    accepted_total: 0.0,
                    baseline_value: value,
                    frozen: false,
                },
                accepted_delta: 0.0,
                disposition: ReconciliationDisposition::InitialMidCounterBaseline,
            })
        };
    };

    cp.validate()?;
    let last_value = normalize_zero(cp.last_value);
    let accepted_total = normalize_zero(cp.accepted_total);
    let baseline_value = normalize_zero(cp.baseline_value);

    if sample.revision < cp.last_revision {
        return Ok(ReconciliationOutcome {
            checkpoint: CounterCheckpoint {
                last_revision: cp.last_revision,
                last_value,
                accepted_total,
                baseline_value,
                frozen: cp.frozen,
            },
            accepted_delta: 0.0,
            disposition: ReconciliationDisposition::OlderHistoryOnly,
        });
    }

    if sample.revision == cp.last_revision {
        if value.to_bits() == last_value.to_bits() {
            return Ok(ReconciliationOutcome {
                checkpoint: CounterCheckpoint {
                    last_revision: cp.last_revision,
                    last_value,
                    accepted_total,
                    baseline_value,
                    frozen: cp.frozen,
                },
                accepted_delta: 0.0,
                disposition: ReconciliationDisposition::DuplicateReplay,
            });
        }
        return Err(ReconciliationError::ConflictingSameRevision {
            revision: sample.revision,
        });
    }

    // Newer revision (`sample.revision > cp.last_revision`)
    if cp.frozen {
        return Ok(ReconciliationOutcome {
            checkpoint: CounterCheckpoint {
                last_revision: sample.revision,
                last_value: value,
                accepted_total,
                baseline_value,
                frozen: true,
            },
            accepted_delta: 0.0,
            disposition: ReconciliationDisposition::FrozenEpochNoOp,
        });
    }

    if value < last_value {
        return Ok(ReconciliationOutcome {
            checkpoint: CounterCheckpoint {
                last_revision: sample.revision,
                last_value: value,
                accepted_total,
                baseline_value,
                frozen: true,
            },
            accepted_delta: 0.0,
            disposition: ReconciliationDisposition::AmbiguousDecreaseFrozen,
        });
    }

    let delta = normalize_zero(value - last_value);
    if !is_valid_nonnegative_finite(delta) {
        return Err(ReconciliationError::ArithmeticOverflow);
    }

    let next_total = normalize_zero(accepted_total + delta);
    if !is_valid_nonnegative_finite(next_total) {
        return Err(ReconciliationError::ArithmeticOverflow);
    }

    Ok(ReconciliationOutcome {
        checkpoint: CounterCheckpoint {
            last_revision: sample.revision,
            last_value: value,
            accepted_total: next_total,
            baseline_value,
            frozen: false,
        },
        accepted_delta: delta,
        disposition: ReconciliationDisposition::AcceptedIncrement,
    })
}

/// Reconciles a sample against a prior identity-bound checkpoint, starting an independent
/// checkpoint when the explicit `epoch` changes for the same stream, and rejecting cross-stream,
/// cross-scope, or cross-series combinations.
pub fn reconcile_with_prior_identity(
    sample: &CounterSample,
    prior: Option<(&SampleIdentity, &CounterCheckpoint)>,
) -> Result<ReconciliationOutcome, ReconciliationError> {
    sample.identity.validate()?;
    let effective_prior = match prior {
        None => None,
        Some((prior_id, prior_cp)) => {
            prior_id.validate()?;
            if !sample.identity.matches_stream(prior_id) {
                return Err(ReconciliationError::StreamIdentityMismatch);
            }
            if sample.identity.epoch == prior_id.epoch {
                Some(prior_cp)
            } else {
                // New explicit epoch uses an independent checkpoint without guessing reset.
                None
            }
        }
    };
    reconcile_counter(sample, effective_prior)
}

fn validate_identity_field(field: &'static str, value: &str) -> Result<(), ReconciliationError> {
    let trimmed = value.trim();
    if trimmed.is_empty()
        || trimmed != value
        || value.len() > MAX_IDENTITY_FIELD_LEN
        || value.chars().any(|c| c.is_control())
    {
        return Err(ReconciliationError::InvalidIdentityField { field });
    }
    Ok(())
}

fn is_valid_nonnegative_finite(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}

fn normalize_zero(v: f64) -> f64 {
    if v == 0.0 {
        0.0
    } else {
        v
    }
}
