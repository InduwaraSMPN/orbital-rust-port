//! Settings vectors ported 1:1 from `orbital-main/Orbit.Tests/SettingsManagerTests.cs`,
//! plus characterization of the import/export validation messages.
//!
//! `tempfile` replaces the C# `IsolatedSettingsScope`, and `SettingsStore::with_path`
//! replaces `OverrideConfigPathForTesting`.

use std::fs;
use std::path::{Path, PathBuf};

use orbital::placement::PopupPlacementMode;
use orbital::settings::{
    ActionProfile, AppSettings, ButtonDisplayMode, SettingsStore, create_default_settings,
    is_selection_required,
};

/// One isolated settings directory + the store pointed at it.
struct IsolatedScope {
    root: tempfile::TempDir,
}

impl IsolatedScope {
    fn new() -> Self {
        Self {
            root: tempfile::tempdir().expect("temp dir"),
        }
    }

    fn config_path(&self) -> PathBuf {
        self.root.path().join("settings.json")
    }

    fn import_path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn store(&self) -> SettingsStore {
        SettingsStore::with_path(self.config_path())
    }
}

/// Serializes an action list the way the C# test does (Newtonsoft, indented).
fn write_actions(path: &Path, actions: &[ActionProfile]) {
    let json = serde_json::to_string_pretty(actions).expect("serialize actions");
    fs::write(path, json).expect("write actions");
}

fn default_action(name: &str, prompt_format: &str, result_action: &str) -> ActionProfile {
    ActionProfile {
        name: name.to_string(),
        icon: String::new(),
        prompt_format: prompt_format.to_string(),
        result_action: result_action.to_string(),
        requires_selection: None,
        display_mode: ButtonDisplayMode::TextAndIcon,
        clean_output: false,
    }
}

// ── 1. CreateDefaultSettings_ReturnsValidSettings ────────────────────────────

#[test]
fn create_default_settings_returns_valid_settings() {
    let settings = create_default_settings();

    assert_eq!(settings.api_base_url, "https://api.openai.com/v1");
    assert_eq!(settings.model_name, "gpt-4o-mini");
    assert_eq!(settings.popup_placement, PopupPlacementMode::BottomRight);
    assert!(settings.popup_auto_close_enabled);
    assert_eq!(settings.popup_auto_close_seconds, 20);
}

// ── 2. DefaultSettings_ContainsExpectedActions ───────────────────────────────

#[test]
fn default_settings_contains_expected_actions() {
    let settings = create_default_settings();

    let names: Vec<&str> = settings.actions.iter().map(|a| a.name.as_str()).collect();
    for expected in [
        "Copy",
        "Cut",
        "Paste",
        "Translate",
        "Summarize",
        "Polish",
        "Search",
    ] {
        assert!(
            names.contains(&expected),
            "default actions are missing '{expected}'"
        );
    }
}

// ── 3. DefaultSettings_LocalActionsHaveCorrectConfiguration ──────────────────

#[test]
fn default_settings_local_actions_have_correct_configuration() {
    let settings = create_default_settings();

    let find = |name: &str| -> ActionProfile {
        settings
            .actions
            .iter()
            .find(|a| a.name == name)
            .cloned()
            .expect("action present")
    };

    let copy = find("Copy");
    assert_eq!(copy.result_action, "DirectCopy");
    assert!(is_selection_required(&copy));

    let cut = find("Cut");
    assert_eq!(cut.result_action, "Cut");
    assert!(is_selection_required(&cut));

    let paste = find("Paste");
    assert_eq!(paste.result_action, "Paste");
    assert!(!is_selection_required(&paste));
}

// ── 4. ApiKey_EncryptionRoundTrip_PreservesValue ─────────────────────────────

#[test]
fn api_key_encryption_round_trip_preserves_value() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let test_key = "sk-test-key-12345";

    SettingsStore::set_api_key(&mut settings, test_key);
    let retrieved = SettingsStore::get_api_key(&settings);

    assert_eq!(retrieved, test_key);
}

// ── 5. ApiKey_EmptyString_HandledCorrectly ───────────────────────────────────

#[test]
fn api_key_empty_string_handled_correctly() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    SettingsStore::set_api_key(&mut settings, "");
    let retrieved = SettingsStore::get_api_key(&settings);

    assert_eq!(retrieved, "");
    assert_eq!(settings.encrypted_api_key, "");
}

// ── 6. ApiKey_NullString_HandledCorrectly ────────────────────────────────────

#[test]
fn api_key_null_string_handled_correctly() {
    // C# passes `null!` — the Rust signature has no null, so an empty string is
    // the same input.
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    SettingsStore::set_api_key(&mut settings, "");
    let retrieved = SettingsStore::get_api_key(&settings);

    assert_eq!(retrieved, "");
}

// ── 7. LoadSettings_WithCorruptedJson_RecoversAndCreatesBackup_InIsolatedPath ─

#[test]
fn load_settings_with_corrupted_json_recovers_and_creates_backup_in_isolated_path() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();
    fs::write(&config, "{ invalid json").expect("write corrupt settings");

    let outcome = scope.store().load().expect("load");

    assert!(outcome.recovered);
    assert!(config.exists());
    assert!(PathBuf::from(format!("{}.corrupt.bak", config.display())).exists());
    assert!(outcome.settings.actions.iter().any(|a| a.name == "Copy"));
}

// ── 8. ImportActionPack_WithUnknownResultAction_ReturnsValidationError_… ──────

#[test]
fn import_action_pack_with_unknown_result_action_returns_validation_error_and_does_not_mutate_settings()
 {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;
    let before = settings.clone();

    let import_path = scope.import_path("invalid-actions.json");
    write_actions(
        &import_path,
        &[default_action("Broken Action", "", "NotARealAction")],
    );

    let error = store
        .import_actions(&import_path, false, &mut settings)
        .expect_err("import must fail");

    assert!(error.contains("Unknown ResultAction"), "error was: {error}");
    assert!(!settings.actions.iter().any(|a| a.name == "Broken Action"));
    assert_eq!(settings, before, "settings must not be mutated on error");
}

// ── 9. ImportActionPack_RejectsDuplicateNamesFromImportedFile ────────────────

#[test]
fn import_action_pack_rejects_duplicate_names_from_imported_file() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let import_path = scope.import_path("duplicate-actions.json");
    write_actions(
        &import_path,
        &[
            default_action("Quick Search", "", "Browser"),
            default_action("Quick Search", "", "Browser"),
        ],
    );

    let error = store
        .import_actions(&import_path, false, &mut settings)
        .expect_err("import must fail");

    assert!(
        error.contains("Duplicate action name"),
        "error was: {error}"
    );
    assert!(
        !settings.actions.iter().any(|a| a.name == "Quick Search"),
        "settings must not be mutated on error"
    );
}

// ── 10. LoadSettings_WhenPopupPlacementMissing_DefaultsToBottomRight ─────────

#[test]
fn load_settings_when_popup_placement_missing_defaults_to_bottom_right() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let mut settings = create_default_settings();
    settings.popup_placement = PopupPlacementMode::NearCursor;
    let mut value = serde_json::to_value(&settings).expect("to value");
    value
        .as_object_mut()
        .expect("object")
        .remove("PopupPlacement");
    fs::write(
        &config,
        serde_json::to_string_pretty(&value).expect("serialize"),
    )
    .expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert_eq!(
        outcome.settings.popup_placement,
        PopupPlacementMode::BottomRight
    );
}

// ── Extra characterization: Newtonsoft null-coercion of explicit JSON nulls ──

// C# Newtonsoft coerces an explicit `null` to each property's initializer default
// (and the source's `RebuildActionExecutor` re-substitutes the same default through
// `string.IsNullOrWhiteSpace`), so a null body must load normally instead of taking
// the corrupt-recovery path.

#[test]
fn explicit_json_nulls_load_with_field_defaults_instead_of_recovering() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let body = r#"{
  "ApiBaseUrl": null,
  "ModelName": null,
  "Theme": null,
  "Language": null,
  "HotkeyModifiers": null,
  "HotkeyVirtualKey": null,
  "PopupAutoCloseSeconds": null,
  "Actions": null
}"#;
    fs::write(&config, body).expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert!(!outcome.recovered);
    let backup = PathBuf::from(format!("{}.corrupt.bak", config.display()));
    assert!(!backup.exists(), "no corrupt backup must be written");
    assert_eq!(outcome.settings.api_base_url, "https://api.openai.com/v1");
    assert_eq!(outcome.settings.model_name, "gpt-4o-mini");
    assert_eq!(outcome.settings.theme, "Dark");
    assert_eq!(outcome.settings.language, "en");
    assert_eq!(outcome.settings.hotkey_modifiers, 0);
    assert_eq!(outcome.settings.hotkey_virtual_key, 0);
    // `NormalizeSettings` (autoClose < 1 → 20) covers a null auto-close too.
    assert_eq!(outcome.settings.popup_auto_close_seconds, 20);
    assert!(outcome.settings.actions.is_empty());
}

#[test]
fn explicit_json_nulls_in_one_action_load_with_profile_defaults() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let body = r#"{
  "Actions": [
    {
      "Name": null,
      "Icon": null,
      "PromptFormat": null,
      "ResultAction": null
    }
  ]
}"#;
    fs::write(&config, body).expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert!(!outcome.recovered);
    let action = outcome.settings.actions.first().expect("one action");
    assert_eq!(action.name, "");
    assert_eq!(action.icon, "");
    assert_eq!(action.prompt_format, "");
    assert_eq!(action.result_action, "Popup");
}

#[test]
fn out_of_range_popup_placement_loads_as_near_cursor_without_recovering() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let mut settings = create_default_settings();
    settings.popup_placement = PopupPlacementMode::NearSelection;
    let mut value = serde_json::to_value(&settings).expect("to value");
    value["PopupPlacement"] = serde_json::json!(7);
    fs::write(
        &config,
        serde_json::to_string_pretty(&value).expect("serialize"),
    )
    .expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert!(!outcome.recovered);
    assert_eq!(
        outcome.settings.popup_placement,
        PopupPlacementMode::NearCursor
    );
}

// Newtonsoft also accepts enum member names (case-insensitively) in a hand-edited
// settings file; the numeric fields stay numeric, the named forms must load.

#[test]
fn enum_member_names_and_out_of_range_values_load_without_recovering() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let settings = create_default_settings();
    let mut value = serde_json::to_value(&settings).expect("to value");
    value["PopupPlacement"] = serde_json::json!("NearSelection");
    value["Actions"][0]["DisplayMode"] = serde_json::json!("IconOnly");
    value["Actions"][1]["DisplayMode"] = serde_json::json!(9);
    fs::write(
        &config,
        serde_json::to_string_pretty(&value).expect("serialize"),
    )
    .expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert!(!outcome.recovered);
    assert_eq!(
        outcome.settings.popup_placement,
        PopupPlacementMode::NearSelection
    );
    assert_eq!(
        outcome.settings.actions[0].display_mode,
        ButtonDisplayMode::IconOnly
    );
    // An out-of-range display mode has no executor switch; discriminant 0 loads.
    assert_eq!(
        outcome.settings.actions[1].display_mode,
        ButtonDisplayMode::TextAndIcon
    );
}

// ── 11. SaveAndLoadSettings_PreservesPopupAutoCloseSettings ─────────────────

#[test]
fn save_and_load_settings_preserves_popup_auto_close_settings() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    settings.popup_auto_close_enabled = false;
    settings.popup_auto_close_seconds = 60;
    store.save(&settings).expect("save");

    let reloaded = store.load().expect("load").settings;

    assert!(!reloaded.popup_auto_close_enabled);
    assert_eq!(reloaded.popup_auto_close_seconds, 60);
}

// ── 12. LoadSettings_WhenPopupAutoCloseFieldsMissing_DefaultsArePreserved ───

#[test]
fn load_settings_when_popup_auto_close_fields_missing_defaults_are_preserved() {
    let scope = IsolatedScope::new();
    let config = scope.config_path();

    let mut settings = create_default_settings();
    settings.popup_auto_close_enabled = false;
    settings.popup_auto_close_seconds = 45;
    let mut value = serde_json::to_value(&settings).expect("to value");
    let object = value.as_object_mut().expect("object");
    object.remove("PopupAutoCloseEnabled");
    object.remove("PopupAutoCloseSeconds");
    fs::write(
        &config,
        serde_json::to_string_pretty(&value).expect("serialize"),
    )
    .expect("write settings");

    let outcome = scope.store().load().expect("load");

    assert!(outcome.settings.popup_auto_close_enabled);
    assert_eq!(outcome.settings.popup_auto_close_seconds, 20);
}

// ── Extra characterization: the exact validation messages of the source ──────

#[test]
fn import_actions_reports_every_validation_message_verbatim() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let cases: Vec<(Vec<ActionProfile>, &str)> = vec![
        // Name is required.
        (
            vec![default_action("", "", "Popup")],
            "Invalid action: Name is required.",
        ),
        // ResultAction is required.
        (
            vec![default_action("Blank Action", "", "")],
            "Invalid action 'Blank Action': ResultAction is required.",
        ),
        // Unknown ResultAction.
        (
            vec![default_action("Odd Action", "", "NotARealAction")],
            "Invalid action 'Odd Action': Unknown ResultAction 'NotARealAction'.",
        ),
        // Duplicate name (case-insensitive, ordinal).
        (
            vec![
                default_action("Same Name", "", "Popup"),
                default_action("same name", "", "Popup"),
            ],
            "Duplicate action name 'same name' found in import file.",
        ),
    ];

    for (actions, expected) in cases {
        let path = scope.import_path("import.json");
        write_actions(&path, &actions);

        let error = store
            .import_actions(&path, false, &mut settings)
            .expect_err("import must fail");

        assert_eq!(error, expected);
    }
}

#[test]
fn import_actions_reports_file_not_found_for_a_missing_file() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let error = store
        .import_actions(
            &scope.import_path("does-not-exist.json"),
            false,
            &mut settings,
        )
        .expect_err("import must fail");

    assert_eq!(error, "File not found.");
}

#[test]
fn import_actions_reports_no_valid_actions_for_an_empty_or_null_payload() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    for payload in ["[]", "null"] {
        let path = scope.import_path("empty.json");
        fs::write(&path, payload).expect("write payload");

        let error = store
            .import_actions(&path, false, &mut settings)
            .expect_err("import must fail");

        assert_eq!(error, "No valid actions found in file.");
    }
}

#[test]
fn import_actions_reports_invalid_json_for_a_broken_payload() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let path = scope.import_path("broken.json");
    fs::write(&path, "{ not json").expect("write payload");

    let error = store
        .import_actions(&path, false, &mut settings)
        .expect_err("import must fail");

    assert!(
        error.starts_with("Invalid JSON format:"),
        "error was: {error}"
    );
}

#[test]
fn import_actions_trims_names_and_replaces_or_merges() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;
    let default_count = settings.actions.len();

    let import_path = scope.import_path("actions.json");
    write_actions(
        &import_path,
        &[
            default_action("  Trimmed  ", "", "Popup"),
            default_action("Copy", "", "Popup"),
            default_action("select all", "", "SelectAll"),
        ],
    );

    // Merge: "Copy" and "select all" collide with defaults (ordinal-ignore-case),
    // so only the trimmed new action is appended.
    store
        .import_actions(&import_path, false, &mut settings)
        .expect("merge import");
    assert_eq!(settings.actions.len(), default_count + 1);
    assert!(
        settings.actions.iter().any(|a| a.name == "Trimmed"),
        "imported name must be trimmed"
    );

    // Replace: the default list is dropped entirely.
    store
        .import_actions(&import_path, true, &mut settings)
        .expect("replace import");
    let names: Vec<&str> = settings.actions.iter().map(|a| a.name.as_str()).collect();
    assert_eq!(names, vec!["Trimmed", "Copy", "select all"]);
}

#[test]
fn import_actions_persists_through_save_and_load() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    let import_path = scope.import_path("actions.json");
    write_actions(
        &import_path,
        &[default_action("Summarize Board", "", "Popup")],
    );
    store
        .import_actions(&import_path, false, &mut settings)
        .expect("import");

    let reloaded = store.load().expect("load").settings;
    assert!(reloaded.actions.iter().any(|a| a.name == "Summarize Board"));
    assert_eq!(reloaded, settings);
}

#[test]
fn export_actions_writes_the_current_action_list_indented() {
    let scope = IsolatedScope::new();
    let settings = create_default_settings();

    let export_path = scope.import_path("export.json");
    SettingsStore::export_actions(&settings, &export_path).expect("export");

    let json = fs::read_to_string(&export_path).expect("read export");
    let exported: Vec<ActionProfile> = serde_json::from_str(&json).expect("parse export");

    assert_eq!(exported, settings.actions);
    assert!(json.contains("\n  "), "export must be indented JSON");
    assert!(json.contains("\"Name\": \"Copy\""));
}

#[test]
fn export_actions_reports_a_failure_message_for_an_unwritable_path() {
    let settings = create_default_settings();

    let error =
        SettingsStore::export_actions(&settings, &PathBuf::from("no/such/directory/actions.json"))
            .expect_err("export must fail");

    assert!(error.starts_with("Export failed:"), "error was: {error}");
}

#[test]
fn load_without_an_existing_file_writes_defaults() {
    let scope = IsolatedScope::new();
    let store = scope.store();

    let outcome = store.load().expect("load");

    assert!(!outcome.recovered);
    assert!(scope.config_path().exists());
    assert_eq!(outcome.settings, create_default_settings());
    assert!(store.path().ends_with("settings.json"));
}

#[test]
fn at_default_path_points_at_appdata_orbital() {
    let store = SettingsStore::at_default_path();
    let path = store.path();

    assert!(
        path.ends_with("Orbital/settings.json") || path.ends_with("Orbital\\settings.json"),
        "unexpected default path: {}",
        path.display()
    );
}

#[test]
fn app_settings_round_trips_as_json() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let mut settings = store.load().expect("load").settings;

    settings.api_base_url = "https://openrouter.ai/api/v1".to_string();
    settings.model_name = "gpt-4o".to_string();
    settings.theme = "Light".to_string();
    settings.language = "ko".to_string();
    settings.popup_placement = PopupPlacementMode::NearSelection;
    settings.hotkey_modifiers = 0x02;
    settings.hotkey_virtual_key = 0x70;
    settings.run_at_startup = true;
    store.save(&settings).expect("save");

    let reloaded: AppSettings = store.load().expect("load").settings;

    assert_eq!(reloaded, settings);
    assert_eq!(reloaded.language, "ko");
    assert_eq!(reloaded.popup_placement, PopupPlacementMode::NearSelection);
}

#[test]
fn the_saved_file_uses_crlf_newlines_like_newtonsoft() {
    let scope = IsolatedScope::new();
    let store = scope.store();
    let settings = store.load().expect("load").settings;
    store.save(&settings).expect("save");

    // Newtonsoft's JsonTextWriter indents with Environment.NewLine = CRLF on Windows.
    let bytes = std::fs::read(store.path()).expect("read the saved file");
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count();
    let crlf = bytes.windows(2).filter(|pair| pair == b"\r\n").count();
    assert!(newlines > 0, "an indented artifact has newlines");
    assert_eq!(newlines, crlf, "every newline must be CRLF, not bare LF");
}
