//! Action-gating vectors ported 1:1 from `orbital-main/Orbit.Tests/ActionGatingTests.cs`.
//! The unit of parity is `ActionProfile.IsSelectionRequired` (Rust:
//! `orbital::settings::is_selection_required`).

use orbital::settings::{ActionProfile, ButtonDisplayMode, is_selection_required};

/// C# object-initializer shape used by the xunit facts.
fn action(name: &str, prompt_format: &str, result_action: &str) -> ActionProfile {
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

fn action_with_selection(
    name: &str,
    prompt_format: &str,
    result_action: &str,
    requires_selection: Option<bool>,
) -> ActionProfile {
    ActionProfile {
        requires_selection,
        ..action(name, prompt_format, result_action)
    }
}

#[test]
fn action_profile_is_selection_required_defaults_to_true_for_non_paste_actions() {
    let action = action("Translate", "Translate: {text}", "Replace");

    assert!(is_selection_required(&action));
}

#[test]
fn action_profile_is_selection_required_defaults_to_false_for_paste_action() {
    let action = action("Paste", "", "Paste");

    assert!(!is_selection_required(&action));
}

#[test]
fn action_profile_is_selection_required_respects_explicit_value() {
    let action = action_with_selection("Custom", "", "Popup", Some(false));

    assert!(!is_selection_required(&action));
}

#[test]
fn action_profile_is_selection_required_explicit_true_overrides_default() {
    let action = action_with_selection("CustomPaste", "", "Paste", Some(true));

    assert!(is_selection_required(&action));
}

#[test]
fn action_profile_is_selection_required_defaults_to_true_for_common_actions() {
    // [InlineData] arguments of the C# theory.
    for result_action in ["Browser", "DirectCopy", "Cut", "Replace", "Copy"] {
        let action = action("Test", "", result_action);

        assert!(
            is_selection_required(&action),
            "ResultAction '{result_action}' should default to requiring a selection"
        );
    }
}

// Extra characterization of the same property beyond the xunit file: the C#
// default is `!= Paste && != SimulateKey`, so every other action (including
// unknown ones, which `ActionType.FromString` maps to Popup) requires a selection.

#[test]
fn action_profile_is_selection_required_defaults_to_false_for_simulate_key() {
    let action = action("Paste Key", "", "SimulateKey");

    assert!(!is_selection_required(&action));
}

#[test]
fn action_profile_is_selection_required_defaults_to_true_for_unknown_result_action() {
    // `FromString` falls back to Popup → selection required.
    let action = action("Broken", "", "NotARealAction");

    assert!(is_selection_required(&action));
}
