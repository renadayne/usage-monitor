//! Pure Codex hook event normalization adapter.
//!
//! Synthetic-tested parser only. Does not perform runtime I/O, credential scanning,
//! config file reads, or live target verification.

use std::fmt;

use normalized_schema::{
    Confidence, EventType, ModelEvidenceKind, NormalizedEvent, Provider, ValidationError,
    SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};

pub const MAX_RAW_HOOK_BYTES: usize = 65_536;
pub const CODEX_HOOKS_SOURCE: &str = "codex.hooks";
pub const SURFACE_VSCODE_EXTENSION: &str = "codex_vscode_extension";
pub const SURFACE_STANDALONE_DESKTOP: &str = "codex_desktop_app";

/// Supported Codex first-surface host targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Surface {
    VsCodeExtension,
    StandaloneDesktop,
}

impl Surface {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::VsCodeExtension => SURFACE_VSCODE_EXTENSION,
            Self::StandaloneDesktop => SURFACE_STANDALONE_DESKTOP,
        }
    }
}

/// Caller-supplied setup and helper delivery context.
/// Never derived by scanning local credentials or provider config files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexContext {
    pub integration_id: String,
    pub account_key: String,
    pub surface: Surface,
    /// Stable helper delivery event ID used for idempotent retry handling.
    pub event_id: String,
    pub observed_at: String,
    pub received_at: String,
}

/// Sanitized adapter normalization error. Never contains raw payload text or raw parser messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdapterError {
    PayloadTooLarge,
    InvalidJson,
    MissingRequiredField(&'static str),
    UnsupportedSubagentHook,
    UnsupportedHookEvent,
    Validation(ValidationError),
}

impl fmt::Display for AdapterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge => write!(f, "raw hook payload exceeds 65536 byte limit"),
            Self::InvalidJson => write!(f, "malformed or invalid hook JSON payload"),
            Self::MissingRequiredField(field) => {
                write!(f, "missing required hook field '{field}'")
            }
            Self::UnsupportedSubagentHook => {
                write!(
                    f,
                    "subagent hook events are rejected in the first surface contract"
                )
            }
            Self::UnsupportedHookEvent => write!(f, "unsupported codex hook_event_name"),
            Self::Validation(err) => write!(f, "normalized event validation failed: {err}"),
        }
    }
}

impl std::error::Error for AdapterError {}

impl From<ValidationError> for AdapterError {
    fn from(value: ValidationError) -> Self {
        Self::Validation(value)
    }
}

/// Strict allowlist of raw Codex hook fields.
/// Unknown sensitive fields (`prompt`, `response`, `code`, `cwd`, `transcript_path`, etc.)
/// are ignored during deserialization and never copied or retained.
#[derive(Deserialize)]
struct RawCodexHook {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    hook_event_name: Option<String>,
    #[serde(default)]
    turn_id: Option<String>,
}

/// Normalize a raw Codex hook JSON payload into a validated [`NormalizedEvent`].
pub fn normalize_hook(raw: &str, context: &CodexContext) -> Result<NormalizedEvent, AdapterError> {
    if raw.len() > MAX_RAW_HOOK_BYTES {
        return Err(AdapterError::PayloadTooLarge);
    }

    let parsed: RawCodexHook = serde_json::from_str(raw).map_err(|_| AdapterError::InvalidJson)?;

    let hook_event_name = parsed
        .hook_event_name
        .ok_or(AdapterError::MissingRequiredField("hook_event_name"))?;
    let session_id = parsed
        .session_id
        .ok_or(AdapterError::MissingRequiredField("session_id"))?;

    let event_type = match hook_event_name.as_str() {
        "SessionStart" => EventType::SessionStarted,
        "SessionEnd" => EventType::SessionEnded,
        "PreToolUse" | "PostToolUse" | "UserPromptSubmit" | "Stop" | "PermissionRequest"
        | "Interrupt" => EventType::ModelObserved,
        "SubagentStart" | "SubagentStop" => return Err(AdapterError::UnsupportedSubagentHook),
        _ => return Err(AdapterError::UnsupportedHookEvent),
    };

    let (model_selected, model_evidence_kind, evidence_ref, confidence) = match parsed.model {
        Some(model) => (
            Some(model),
            ModelEvidenceKind::HookCommonMetadata,
            Some(context.event_id.clone()),
            Confidence::Direct,
        ),
        None => (None, ModelEvidenceKind::Unknown, None, Confidence::Unknown),
    };

    let event = NormalizedEvent {
        schema_version: SCHEMA_VERSION,
        event_id: context.event_id.clone(),
        event_type,
        integration_id: context.integration_id.clone(),
        account_key: context.account_key.clone(),
        provider: Provider::Codex,
        surface: context.surface.as_str().to_owned(),
        source: CODEX_HOOKS_SOURCE.to_owned(),
        source_event_id: None,
        source_sequence: None,
        source_session_id: session_id,
        local_session_id: None,
        source_turn_id: parsed.turn_id,
        source_inference_id: None,
        parent_agent_id: None,
        phase_at_start: None,
        phase_epoch: None,
        model_selected,
        model_requested: None,
        model_resolved: None,
        model_observed: None,
        model_evidence_kind,
        evidence_ref,
        observed_at: context.observed_at.clone(),
        received_at: context.received_at.clone(),
        evidence_expires_at: None,
        confidence,
        activity: None,
    };

    event.validate()?;
    Ok(event)
}
