//! Pure Rust Codex `hooks.json` preview, merge, and three-way revert planner.
//!
//! Operates strictly in memory on caller-supplied strings. Never accesses filesystems,
//! networks, processes, or real provider settings, and never modifies `config.toml`.

use std::fmt;

use serde::{
    de::{self, MapAccess, SeqAccess, Visitor},
    Deserialize,
};
use serde_json::{json, Map, Value};

pub const MAX_CONFIG_BYTES: usize = 1_048_576;
pub const MAX_COMMAND_BYTES: usize = 4_096;
pub const PLANNED_EVENTS: [&str; 4] = ["SessionStart", "SessionEnd", "UserPromptSubmit", "Stop"];

/// Static sanitized setup plan error. Never retains raw config text or user commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanError {
    ConfigTooLarge,
    InvalidConfigJson,
    RootNotObject,
    HooksNotObject,
    EventHooksNotArray(&'static str),
    GroupNotObject(&'static str),
    HandlersNotArray(&'static str),
    HandlerNotObject(&'static str),
    EmptyCommand,
    CommandTooLarge,
    InvalidCommandCharacters,
    InvalidManifest,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigTooLarge => write!(f, "config input exceeds 1048576 byte limit"),
            Self::InvalidConfigJson => write!(f, "config input is not valid JSON"),
            Self::RootNotObject => write!(f, "config root must be a JSON object"),
            Self::HooksNotObject => write!(f, "hooks field must be a JSON object"),
            Self::EventHooksNotArray(ev) => write!(f, "event '{ev}' hooks must be a JSON array"),
            Self::GroupNotObject(ev) => write!(f, "hook group in '{ev}' must be a JSON object"),
            Self::HandlersNotArray(ev) => write!(f, "group hooks in '{ev}' must be a JSON array"),
            Self::HandlerNotObject(ev) => write!(f, "hook handler in '{ev}' must be a JSON object"),
            Self::EmptyCommand => write!(f, "helper command must not be empty"),
            Self::CommandTooLarge => write!(f, "helper command exceeds 4096 byte limit"),
            Self::InvalidCommandCharacters => {
                write!(
                    f,
                    "helper command contains control characters or outer whitespace"
                )
            }
            Self::InvalidManifest => {
                write!(f, "ownership manifest contains invalid group or event")
            }
        }
    }
}

impl std::error::Error for PlanError {}

/// In-memory merged or reverted `hooks.json` container.
/// Deliberately omits `Debug` and `Display` to prevent accidental logging of user configuration.
#[derive(Clone, PartialEq, Eq)]
pub struct MergedConfig {
    json: String,
}

impl MergedConfig {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.json
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.json
    }
}

/// Exact group added by the setup plan for a specific Codex hook event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedEventGroup {
    pub event: &'static str,
    pub group: Value,
}

/// Ownership manifest containing ONLY exact added groups and event names.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct OwnershipManifest {
    pub owned_groups: Vec<OwnedEventGroup>,
}

impl OwnershipManifest {
    pub fn to_json(&self) -> Result<String, PlanError> {
        validate_manifest(self)?;
        let mut entries = Vec::with_capacity(self.owned_groups.len());
        for item in &self.owned_groups {
            let event = canonical_event(item.event).ok_or(PlanError::InvalidManifest)?;
            if !item.group.is_object() {
                return Err(PlanError::InvalidManifest);
            }
            entries.push(json!({
                "event": event,
                "group": item.group,
            }));
        }
        serde_json::to_string(&json!({ "owned_groups": entries }))
            .map_err(|_| PlanError::InvalidManifest)
    }

    pub fn from_json(raw: &str) -> Result<Self, PlanError> {
        if raw.len() > MAX_CONFIG_BYTES {
            return Err(PlanError::InvalidManifest);
        }
        let val = serde_json::from_str::<UniqueValue>(raw)
            .map_err(|_| PlanError::InvalidManifest)?
            .0;
        let obj = val.as_object().ok_or(PlanError::InvalidManifest)?;
        if obj.len() != 1 {
            return Err(PlanError::InvalidManifest);
        }
        let arr = obj
            .get("owned_groups")
            .and_then(Value::as_array)
            .ok_or(PlanError::InvalidManifest)?;
        let mut owned_groups = Vec::with_capacity(arr.len());
        for entry in arr {
            let entry_obj = entry.as_object().ok_or(PlanError::InvalidManifest)?;
            if entry_obj.len() != 2 {
                return Err(PlanError::InvalidManifest);
            }
            let ev_str = entry_obj
                .get("event")
                .and_then(Value::as_str)
                .ok_or(PlanError::InvalidManifest)?;
            let event = canonical_event(ev_str).ok_or(PlanError::InvalidManifest)?;
            let group = entry_obj
                .get("group")
                .ok_or(PlanError::InvalidManifest)?
                .clone();
            validate_group(event, &group)?;
            owned_groups.push(OwnedEventGroup { event, group });
        }
        let manifest = Self { owned_groups };
        validate_manifest(&manifest)?;
        Ok(manifest)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeAction {
    AddedGroup,
    SkippedExistingCommand,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventMergePreview {
    pub event: &'static str,
    pub action: MergeAction,
    pub added_groups: usize,
}

/// Redacted merge preview containing ONLY event names and change counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedPreview {
    pub events: Vec<EventMergePreview>,
    pub total_added_groups: usize,
    pub total_skipped_events: usize,
}

/// Result of planning a non-destructive merge. Deliberately omits `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct MergePlan {
    pub merged_config: MergedConfig,
    pub manifest: OwnershipManifest,
    pub preview: RedactedPreview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictReason {
    MissingOrModified,
    DuplicateOwnedGroup,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventConflict {
    pub event: &'static str,
    pub reason: ConflictReason,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertAction {
    RemovedGroup,
    Conflict(ConflictReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventRevertPreview {
    pub event: &'static str,
    pub action: RevertAction,
    pub removed_groups: usize,
}

/// Redacted revert preview containing ONLY event names, change counts, and conflict status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedactedRevertPreview {
    pub events: Vec<EventRevertPreview>,
    pub total_removed_groups: usize,
    pub total_conflicts: usize,
}

/// Result of planning a three-way revert. Deliberately omits `Debug`.
#[derive(Clone, PartialEq, Eq)]
pub struct RevertPlan {
    pub reverted_config: MergedConfig,
    pub conflicts: Vec<EventConflict>,
    pub preview: RedactedRevertPreview,
}

/// Validate a caller-reviewed helper command string.
pub fn validate_command(command: &str) -> Result<(), PlanError> {
    if command.is_empty() {
        return Err(PlanError::EmptyCommand);
    }
    if command.len() > MAX_COMMAND_BYTES {
        return Err(PlanError::CommandTooLarge);
    }
    if command.trim() != command || command.chars().any(char::is_control) {
        return Err(PlanError::InvalidCommandCharacters);
    }
    Ok(())
}

/// Plan a non-destructive merge of the helper command into `current_json`.
pub fn plan_merge(current_json: &str, helper_command: &str) -> Result<MergePlan, PlanError> {
    validate_command(helper_command)?;
    let mut root = parse_root_object(current_json)?;

    let hooks_val = root
        .entry("hooks".to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks_obj = hooks_val.as_object_mut().ok_or(PlanError::HooksNotObject)?;

    for (key, val) in hooks_obj.iter() {
        if let Some(ev) = canonical_event(key) {
            validate_event_array(ev, val)?;
        }
    }

    let mut owned_groups = Vec::new();
    let mut event_previews = Vec::with_capacity(PLANNED_EVENTS.len());
    let mut total_added_groups = 0;
    let mut total_skipped_events = 0;

    for event in PLANNED_EVENTS {
        let arr_val = hooks_obj
            .entry(event.to_owned())
            .or_insert_with(|| Value::Array(Vec::new()));
        let groups = arr_val
            .as_array_mut()
            .ok_or(PlanError::EventHooksNotArray(event))?;

        if event_has_command(event, groups, helper_command)? {
            total_skipped_events += 1;
            event_previews.push(EventMergePreview {
                event,
                action: MergeAction::SkippedExistingCommand,
                added_groups: 0,
            });
        } else {
            let group = build_owned_group(event, helper_command);
            groups.push(group.clone());
            owned_groups.push(OwnedEventGroup { event, group });
            total_added_groups += 1;
            event_previews.push(EventMergePreview {
                event,
                action: MergeAction::AddedGroup,
                added_groups: 1,
            });
        }
    }

    let json = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|_| PlanError::InvalidConfigJson)?;
    if json.len() > MAX_CONFIG_BYTES {
        return Err(PlanError::ConfigTooLarge);
    }

    Ok(MergePlan {
        merged_config: MergedConfig { json },
        manifest: OwnershipManifest { owned_groups },
        preview: RedactedPreview {
            events: event_previews,
            total_added_groups,
            total_skipped_events,
        },
    })
}

/// Plan a three-way revert using `current_json` and an [`OwnershipManifest`].
pub fn plan_revert(
    current_json: &str,
    manifest: &OwnershipManifest,
) -> Result<RevertPlan, PlanError> {
    let mut root = parse_root_object(current_json)?;
    validate_manifest(manifest)?;
    for item in &manifest.owned_groups {
        if canonical_event(item.event).is_none() {
            return Err(PlanError::InvalidManifest);
        }
        validate_group(item.event, &item.group)?;
    }

    if let Some(hooks_val) = root.get("hooks") {
        let hooks_obj = hooks_val.as_object().ok_or(PlanError::HooksNotObject)?;
        for (key, val) in hooks_obj {
            if let Some(ev) = canonical_event(key) {
                validate_event_array(ev, val)?;
            }
        }
    }

    let mut conflicts = Vec::new();
    let mut event_previews = Vec::with_capacity(manifest.owned_groups.len());
    let mut total_removed_groups = 0;

    for item in &manifest.owned_groups {
        let event = item.event;
        let maybe_groups = root
            .get_mut("hooks")
            .and_then(Value::as_object_mut)
            .and_then(|h| h.get_mut(event))
            .and_then(Value::as_array_mut);

        let Some(groups) = maybe_groups else {
            conflicts.push(EventConflict {
                event,
                reason: ConflictReason::MissingOrModified,
            });
            event_previews.push(EventRevertPreview {
                event,
                action: RevertAction::Conflict(ConflictReason::MissingOrModified),
                removed_groups: 0,
            });
            continue;
        };

        let matching_indices: Vec<usize> = groups
            .iter()
            .enumerate()
            .filter_map(|(idx, g)| (g == &item.group).then_some(idx))
            .collect();

        match matching_indices.as_slice() {
            [idx] => {
                groups.remove(*idx);
                total_removed_groups += 1;
                event_previews.push(EventRevertPreview {
                    event,
                    action: RevertAction::RemovedGroup,
                    removed_groups: 1,
                });
            }
            [] => {
                conflicts.push(EventConflict {
                    event,
                    reason: ConflictReason::MissingOrModified,
                });
                event_previews.push(EventRevertPreview {
                    event,
                    action: RevertAction::Conflict(ConflictReason::MissingOrModified),
                    removed_groups: 0,
                });
            }
            _ => {
                conflicts.push(EventConflict {
                    event,
                    reason: ConflictReason::DuplicateOwnedGroup,
                });
                event_previews.push(EventRevertPreview {
                    event,
                    action: RevertAction::Conflict(ConflictReason::DuplicateOwnedGroup),
                    removed_groups: 0,
                });
            }
        }
    }

    let total_conflicts = conflicts.len();
    let json = serde_json::to_string_pretty(&Value::Object(root))
        .map_err(|_| PlanError::InvalidConfigJson)?;

    Ok(RevertPlan {
        reverted_config: MergedConfig { json },
        conflicts,
        preview: RedactedRevertPreview {
            events: event_previews,
            total_removed_groups,
            total_conflicts,
        },
    })
}

fn canonical_event(name: &str) -> Option<&'static str> {
    PLANNED_EVENTS.into_iter().find(|&ev| ev == name)
}

// Only the exact groups emitted by this planner qualify as owned additions.
// A persisted manifest still requires integrity protection by the future apply layer.
fn validate_manifest(manifest: &OwnershipManifest) -> Result<(), PlanError> {
    if manifest.owned_groups.len() > PLANNED_EVENTS.len() {
        return Err(PlanError::InvalidManifest);
    }
    let mut seen = std::collections::BTreeSet::new();
    for item in &manifest.owned_groups {
        if canonical_event(item.event).is_none() || !seen.insert(item.event) {
            return Err(PlanError::InvalidManifest);
        }
        let command = item
            .group
            .get("hooks")
            .and_then(Value::as_array)
            .and_then(|hooks| hooks.first())
            .and_then(|h| h.get("command"))
            .and_then(Value::as_str)
            .ok_or(PlanError::InvalidManifest)?;
        validate_command(command).map_err(|_| PlanError::InvalidManifest)?;
        if item.group != build_owned_group(item.event, command) {
            return Err(PlanError::InvalidManifest);
        }
    }
    Ok(())
}

fn parse_root_object(raw: &str) -> Result<Map<String, Value>, PlanError> {
    if raw.len() > MAX_CONFIG_BYTES {
        return Err(PlanError::ConfigTooLarge);
    }
    let val = serde_json::from_str::<UniqueValue>(raw)
        .map_err(|_| PlanError::InvalidConfigJson)?
        .0;
    match val {
        Value::Object(map) => Ok(map),
        _ => Err(PlanError::RootNotObject),
    }
}

fn validate_event_array(event: &'static str, val: &Value) -> Result<(), PlanError> {
    let groups = val.as_array().ok_or(PlanError::EventHooksNotArray(event))?;
    for group in groups {
        validate_group(event, group)?;
    }
    Ok(())
}

fn validate_group(event: &'static str, group: &Value) -> Result<(), PlanError> {
    let group_obj = group.as_object().ok_or(PlanError::GroupNotObject(event))?;
    let handlers = group_obj
        .get("hooks")
        .and_then(Value::as_array)
        .ok_or(PlanError::HandlersNotArray(event))?;
    for handler in handlers {
        if !handler.is_object() {
            return Err(PlanError::HandlerNotObject(event));
        }
    }
    Ok(())
}

fn event_has_command(
    event: &'static str,
    groups: &[Value],
    target_command: &str,
) -> Result<bool, PlanError> {
    for group in groups {
        let group_obj = group.as_object().ok_or(PlanError::GroupNotObject(event))?;
        let handlers = group_obj
            .get("hooks")
            .and_then(Value::as_array)
            .ok_or(PlanError::HandlersNotArray(event))?;
        for handler in handlers {
            let h_obj = handler
                .as_object()
                .ok_or(PlanError::HandlerNotObject(event))?;
            let matches_cmd = h_obj
                .get("command")
                .and_then(Value::as_str)
                .is_some_and(|c| c == target_command);
            let matches_win = h_obj
                .get("commandWindows")
                .and_then(Value::as_str)
                .is_some_and(|c| c == target_command);
            if matches_cmd || matches_win {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

fn build_owned_group(event: &'static str, command: &str) -> Value {
    let handler = if event == "SessionEnd" {
        json!({
            "type": "command",
            "command": command,
            "timeout": 1
        })
    } else {
        json!({
            "type": "command",
            "command": command,
            "timeout": 1,
            "async": true
        })
    };
    json!({
        "hooks": [handler]
    })
}

// Reject duplicate keys rather than silently losing user settings during a merge.
struct UniqueValue(Value);
impl<'de> Deserialize<'de> for UniqueValue {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct JsonVisitor;
        impl<'de> Visitor<'de> for JsonVisitor {
            type Value = UniqueValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON with unique object keys")
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(UniqueValue(Value::Null))
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> Result<Self::Value, E> {
                serde_json::Number::from_f64(v)
                    .map(|n| UniqueValue(Value::Number(n)))
                    .ok_or_else(|| E::custom("invalid JSON number"))
            }
            fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
                Ok(UniqueValue(v.into()))
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(value) = seq.next_element::<UniqueValue>()? {
                    values.push(value.0);
                }
                Ok(UniqueValue(Value::Array(values)))
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut values = Map::new();
                while let Some((key, value)) = map.next_entry::<String, UniqueValue>()? {
                    if values.insert(key, value.0).is_some() {
                        return Err(de::Error::custom("duplicate JSON key"));
                    }
                }
                Ok(UniqueValue(Value::Object(values)))
            }
        }
        deserializer.deserialize_any(JsonVisitor)
    }
}
