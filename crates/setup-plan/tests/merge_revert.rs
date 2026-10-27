use serde_json::{json, Value};
use setup_plan::{
    plan_merge, plan_revert, validate_command, ConflictReason, EventConflict, MergeAction,
    OwnershipManifest, PlanError, RevertAction, MAX_COMMAND_BYTES, MAX_CONFIG_BYTES,
    PLANNED_EVENTS,
};

const HELPER_CMD: &str = "usage-monitor-codex-helper --emit-hook";

#[test]
fn merge_creates_official_schema_and_omits_async_only_on_session_end() {
    let merge = plan_merge("{}", HELPER_CMD).expect("merge should succeed");
    assert_eq!(merge.preview.total_added_groups, 4);
    assert_eq!(merge.preview.total_skipped_events, 0);
    assert_eq!(merge.manifest.owned_groups.len(), 4);

    let parsed: Value = serde_json::from_str(merge.merged_config.as_str()).unwrap();
    let hooks = parsed.get("hooks").and_then(Value::as_object).unwrap();

    for event in PLANNED_EVENTS {
        let groups = hooks.get(event).and_then(Value::as_array).unwrap();
        assert_eq!(groups.len(), 1);
        let group = groups[0].as_object().unwrap();
        // No arbitrary marker fields in Codex config group.
        assert_eq!(group.len(), 1);
        let handlers = group.get("hooks").and_then(Value::as_array).unwrap();
        assert_eq!(handlers.len(), 1);
        let handler = handlers[0].as_object().unwrap();
        assert_eq!(handler.get("type").and_then(Value::as_str), Some("command"));
        assert_eq!(
            handler.get("command").and_then(Value::as_str),
            Some(HELPER_CMD)
        );
        assert_eq!(handler.get("timeout").and_then(Value::as_u64), Some(1));

        if event == "SessionEnd" {
            assert!(
                !handler.contains_key("async"),
                "SessionEnd is always sync and must omit async"
            );
            assert_eq!(handler.len(), 3);
        } else {
            assert_eq!(handler.get("async").and_then(Value::as_bool), Some(true));
            assert_eq!(handler.len(), 4);
        }
    }
}

#[test]
fn roundtrip_semantic_preservation_of_unknown_root_group_and_handler_fields() {
    let initial = json!({
        "$schema": "https://learn.chatgpt.com/schemas/hooks.json",
        "customRootField": { "nested": [1, 2, 3], "enabled": true },
        "hooks": {
            "CustomEvent": [
                { "customEventField": "kept" }
            ],
            "SessionStart": [
                {
                    "matcher": "startup",
                    "unknownGroupKey": { "preserve": "me" },
                    "hooks": [
                        {
                            "type": "command",
                            "command": "echo existing-start",
                            "timeout": 5,
                            "async": false,
                            "customHandlerMeta": "alpha"
                        }
                    ]
                }
            ],
            "SessionEnd": [
                {
                    "groupTag": 42,
                    "hooks": [
                        {
                            "type": "command",
                            "command": "echo existing-end",
                            "extraHandlerFlag": true
                        }
                    ]
                }
            ]
        }
    });
    let initial_str = serde_json::to_string_pretty(&initial).unwrap();

    let merge = plan_merge(&initial_str, HELPER_CMD).expect("merge should succeed");
    assert_eq!(merge.preview.total_added_groups, 4);

    // Manifest roundtrip via JSON.
    let manifest_json = merge.manifest.to_json().expect("manifest serializes");
    assert!(!manifest_json.contains("customRootField"));
    assert!(!manifest_json.contains("echo existing-start"));
    let restored_manifest =
        OwnershipManifest::from_json(&manifest_json).expect("manifest deserializes");
    assert_eq!(restored_manifest, merge.manifest);

    // Empty event arrays created by the planner may remain; user configuration is preserved.
    let revert = plan_revert(merge.merged_config.as_str(), &restored_manifest)
        .expect("revert should succeed");
    assert!(revert.conflicts.is_empty());
    assert_eq!(revert.preview.total_removed_groups, 4);
    assert_eq!(revert.preview.total_conflicts, 0);

    let reverted_val: Value = serde_json::from_str(revert.reverted_config.as_str()).unwrap();
    assert_eq!(reverted_val["$schema"], initial["$schema"]);
    assert_eq!(reverted_val["customRootField"], initial["customRootField"]);
    assert_eq!(
        reverted_val["hooks"]["CustomEvent"],
        initial["hooks"]["CustomEvent"]
    );
    assert_eq!(
        reverted_val["hooks"]["SessionStart"],
        initial["hooks"]["SessionStart"]
    );
    assert_eq!(
        reverted_val["hooks"]["SessionEnd"],
        initial["hooks"]["SessionEnd"]
    );
}

#[test]
fn roundtrip_exact_json_value_equality_when_all_four_events_present() {
    let initial = json!({
        "rootExtra": "retained",
        "hooks": {
            "SessionStart": [{
                "matcher": "init",
                "groupExtra": 1,
                "hooks": [{ "type": "command", "command": "user-start", "handlerExtra": "x" }]
            }],
            "SessionEnd": [{
                "groupExtra": 2,
                "hooks": [{ "type": "command", "command": "user-end", "handlerExtra": "y" }]
            }],
            "UserPromptSubmit": [{
                "groupExtra": 3,
                "hooks": [{ "type": "command", "command": "user-prompt", "handlerExtra": "z" }]
            }],
            "Stop": [{
                "groupExtra": 4,
                "hooks": [{ "type": "command", "command": "user-stop", "handlerExtra": "w" }]
            }],
            "PreToolUse": [{
                "hooks": [{ "type": "command", "command": "user-tool" }]
            }]
        }
    });
    let initial_str = serde_json::to_string(&initial).unwrap();

    let merge = plan_merge(&initial_str, HELPER_CMD).unwrap();
    let revert = plan_revert(merge.merged_config.as_str(), &merge.manifest).unwrap();
    assert!(revert.conflicts.is_empty());

    let after_revert: Value = serde_json::from_str(revert.reverted_config.as_str()).unwrap();
    assert_eq!(after_revert, initial);
}

#[test]
fn ambiguous_json_and_nonowned_or_duplicate_manifest_entries_are_rejected() {
    for raw in [
        r#"{"hooks":{},"hooks":{}}"#,
        r#"{"unknown":{"secret":1,"secret":2}}"#,
    ] {
        assert_eq!(
            plan_merge(raw, HELPER_CMD).err(),
            Some(PlanError::InvalidConfigJson)
        );
    }
    let merge = plan_merge("{}", HELPER_CMD).unwrap();
    let mut manifest = merge.manifest.clone();
    manifest.owned_groups[0].group["matcher"] = json!("user-owned");
    assert!(plan_revert(merge.merged_config.as_str(), &manifest).is_err());
    let mut manifest = merge.manifest;
    manifest.owned_groups.push(manifest.owned_groups[0].clone());
    assert!(manifest.to_json().is_err());
}

#[test]
fn existing_identical_hook_via_command_or_command_windows_is_not_owned_or_duplicated() {
    let initial = json!({
        "hooks": {
            "SessionStart": [
                {
                    "matcher": "custom",
                    "hooks": [
                        { "type": "command", "command": "other-cmd" },
                        {
                            "type": "command",
                            "command": HELPER_CMD,
                            "timeout": 10,
                            "customField": "user-owned"
                        }
                    ]
                }
            ],
            "UserPromptSubmit": [
                {
                    "hooks": [
                        {
                            "type": "command",
                            "command": "unix-helper",
                            "commandWindows": HELPER_CMD,
                            "timeout": 5
                        }
                    ]
                }
            ]
        }
    });
    let initial_str = serde_json::to_string(&initial).unwrap();

    let merge = plan_merge(&initial_str, HELPER_CMD).expect("merge should succeed");
    assert_eq!(merge.preview.total_added_groups, 2);
    assert_eq!(merge.preview.total_skipped_events, 2);

    // Only SessionEnd and Stop should be owned.
    let owned_events: Vec<&str> = merge
        .manifest
        .owned_groups
        .iter()
        .map(|g| g.event)
        .collect();
    assert_eq!(owned_events, vec!["SessionEnd", "Stop"]);

    let merged_val: Value = serde_json::from_str(merge.merged_config.as_str()).unwrap();
    // Preexisting SessionStart and UserPromptSubmit hooks must be completely untouched.
    assert_eq!(
        merged_val["hooks"]["SessionStart"],
        initial["hooks"]["SessionStart"]
    );
    assert_eq!(
        merged_val["hooks"]["UserPromptSubmit"],
        initial["hooks"]["UserPromptSubmit"]
    );

    // Reverting removes only SessionEnd and Stop, leaving the preexisting identical hooks intact.
    let revert = plan_revert(merge.merged_config.as_str(), &merge.manifest).unwrap();
    assert!(revert.conflicts.is_empty());
    assert_eq!(revert.preview.total_removed_groups, 2);

    let reverted_val: Value = serde_json::from_str(revert.reverted_config.as_str()).unwrap();
    assert_eq!(
        reverted_val["hooks"]["SessionStart"],
        initial["hooks"]["SessionStart"]
    );
    assert_eq!(
        reverted_val["hooks"]["UserPromptSubmit"],
        initial["hooks"]["UserPromptSubmit"]
    );
}

#[test]
fn revert_retains_unrelated_edits_and_group_reordering() {
    let initial = json!({
        "hooks": {
            "SessionStart": [
                { "hooks": [{ "type": "command", "command": "first-user-cmd" }] }
            ]
        }
    });
    let merge = plan_merge(&serde_json::to_string(&initial).unwrap(), HELPER_CMD).unwrap();

    // Simulate user editing unrelated fields, adding new groups before/after, and reordering groups.
    let mut edited: Value = serde_json::from_str(merge.merged_config.as_str()).unwrap();
    edited["userAddedRoot"] = json!("new-value");
    let start_groups = edited["hooks"]["SessionStart"].as_array_mut().unwrap();
    let owned_start_group = start_groups.pop().unwrap();
    start_groups.insert(0, owned_start_group);
    start_groups.push(json!({
        "matcher": "after",
        "hooks": [{ "type": "command", "command": "second-user-cmd", "extra": 99 }]
    }));

    let edited_str = serde_json::to_string(&edited).unwrap();
    let revert = plan_revert(&edited_str, &merge.manifest).expect("revert should succeed");
    assert!(revert.conflicts.is_empty());
    assert_eq!(revert.preview.total_removed_groups, 4);

    let final_val: Value = serde_json::from_str(revert.reverted_config.as_str()).unwrap();
    assert_eq!(final_val["userAddedRoot"], json!("new-value"));
    assert_eq!(
        final_val["hooks"]["SessionStart"],
        json!([
            { "hooks": [{ "type": "command", "command": "first-user-cmd" }] },
            {
                "matcher": "after",
                "hooks": [{ "type": "command", "command": "second-user-cmd", "extra": 99 }]
            }
        ])
    );
}

#[test]
fn revert_reports_conflicts_for_changed_deleted_and_duplicate_owned_groups_without_touching_others()
{
    let merge = plan_merge("{}", HELPER_CMD).unwrap();
    let mut edited: Value = serde_json::from_str(merge.merged_config.as_str()).unwrap();

    // 1. SessionStart: user modified timeout in our owned group -> MissingOrModified conflict, group kept.
    edited["hooks"]["SessionStart"][0]["hooks"][0]["timeout"] = json!(5);
    edited["hooks"]["SessionStart"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "hooks": [{ "type": "command", "command": "unrelated-keep" }] }));

    // 2. SessionEnd: user deleted our owned group -> MissingOrModified conflict.
    edited["hooks"]["SessionEnd"] = json!([
        { "hooks": [{ "type": "command", "command": "user-end-keep" }] }
    ]);

    // 3. UserPromptSubmit: user duplicated our exact owned group -> DuplicateOwnedGroup conflict, both kept.
    let dup_group = edited["hooks"]["UserPromptSubmit"][0].clone();
    edited["hooks"]["UserPromptSubmit"]
        .as_array_mut()
        .unwrap()
        .push(dup_group);

    // 4. Stop: untouched -> cleanly removed.

    let revert = plan_revert(&serde_json::to_string(&edited).unwrap(), &merge.manifest).unwrap();
    assert_eq!(revert.preview.total_removed_groups, 1);
    assert_eq!(revert.preview.total_conflicts, 3);
    assert_eq!(
        revert.conflicts,
        vec![
            EventConflict {
                event: "SessionStart",
                reason: ConflictReason::MissingOrModified,
            },
            EventConflict {
                event: "SessionEnd",
                reason: ConflictReason::MissingOrModified,
            },
            EventConflict {
                event: "UserPromptSubmit",
                reason: ConflictReason::DuplicateOwnedGroup,
            },
        ]
    );
    assert_eq!(revert.preview.events[3].action, RevertAction::RemovedGroup);

    let after: Value = serde_json::from_str(revert.reverted_config.as_str()).unwrap();
    // Modified SessionStart group + unrelated group are both preserved untouched.
    assert_eq!(
        after["hooks"]["SessionStart"],
        edited["hooks"]["SessionStart"]
    );
    // SessionEnd unrelated group preserved.
    assert_eq!(after["hooks"]["SessionEnd"], edited["hooks"]["SessionEnd"]);
    // Duplicate UserPromptSubmit groups both preserved.
    assert_eq!(
        after["hooks"]["UserPromptSubmit"],
        edited["hooks"]["UserPromptSubmit"]
    );
    // Stop owned group was uniquely matched and removed.
    assert_eq!(after["hooks"]["Stop"], json!([]));
}

#[test]
fn malformed_and_oversized_inputs_and_command_controls_are_rejected_with_sanitized_errors() {
    // Oversized config (> 1 MiB)
    let oversized_config = " ".repeat(MAX_CONFIG_BYTES + 1);
    assert_eq!(
        plan_merge(&oversized_config, HELPER_CMD).err(),
        Some(PlanError::ConfigTooLarge)
    );

    // Malformed JSON containing sensitive user content must not leak in error display
    let secret_json = r#"{"secret_token": "sk-live-99999", broken"#;
    let err = plan_merge(secret_json, HELPER_CMD).err().unwrap();
    assert_eq!(err, PlanError::InvalidConfigJson);
    assert!(!err.to_string().contains("sk-live-99999"));

    // Wrong root / hooks / event / group / handler types
    assert_eq!(
        plan_merge("[]", HELPER_CMD).err(),
        Some(PlanError::RootNotObject)
    );
    assert_eq!(
        plan_merge(r#"{"hooks": []}"#, HELPER_CMD).err(),
        Some(PlanError::HooksNotObject)
    );
    assert_eq!(
        plan_merge(r#"{"hooks": {"SessionStart": {}}}"#, HELPER_CMD).err(),
        Some(PlanError::EventHooksNotArray("SessionStart"))
    );
    assert_eq!(
        plan_merge(r#"{"hooks": {"SessionStart": ["bad"]}}"#, HELPER_CMD).err(),
        Some(PlanError::GroupNotObject("SessionStart"))
    );
    assert_eq!(
        plan_merge(
            r#"{"hooks": {"SessionStart": [{"hooks": {}}]}}"#,
            HELPER_CMD
        )
        .err(),
        Some(PlanError::HandlersNotArray("SessionStart"))
    );
    assert_eq!(
        plan_merge(
            r#"{"hooks": {"SessionStart": [{"hooks": [123]}]}}"#,
            HELPER_CMD
        )
        .err(),
        Some(PlanError::HandlerNotObject("SessionStart"))
    );

    // Command validation: empty, oversized, leading/trailing whitespace, control characters
    assert_eq!(validate_command(""), Err(PlanError::EmptyCommand));
    let long_cmd = "a".repeat(MAX_COMMAND_BYTES + 1);
    assert_eq!(validate_command(&long_cmd), Err(PlanError::CommandTooLarge));
    assert_eq!(
        validate_command(" leading"),
        Err(PlanError::InvalidCommandCharacters)
    );
    assert_eq!(
        validate_command("trailing "),
        Err(PlanError::InvalidCommandCharacters)
    );
    assert_eq!(
        validate_command("cmd\nwith-newline"),
        Err(PlanError::InvalidCommandCharacters)
    );
    assert_eq!(
        validate_command("cmd\0with-null"),
        Err(PlanError::InvalidCommandCharacters)
    );
    assert_eq!(
        validate_command("cmd\twith-tab"),
        Err(PlanError::InvalidCommandCharacters)
    );

    // Redacted preview must never contain raw config or command strings
    let merge = plan_merge(r#"{"secret": "top-secret-value"}"#, HELPER_CMD).unwrap();
    let preview_debug = format!("{:?}", merge.preview);
    assert!(!preview_debug.contains("top-secret-value"));
    assert!(!preview_debug.contains(HELPER_CMD));
    assert_eq!(merge.preview.events.len(), 4);
    assert!(merge
        .preview
        .events
        .iter()
        .all(|e| e.action == MergeAction::AddedGroup && e.added_groups == 1));
}
