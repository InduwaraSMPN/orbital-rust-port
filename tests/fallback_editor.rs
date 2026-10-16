//! Clipboard fallback editor vectors ported from
//! `orbital-main/Orbit.Tests/ClipboardFallbackEditorTests.cs`.
//! The unit of parity is `App.IsClipboardFallbackEditor` (Rust:
//! `orbital::app::is_clipboard_fallback_editor`).

use orbital::app::is_clipboard_fallback_editor;

/// `[InlineData("", "", "notepad++")]`, `"emeditor"`, `"sublime_text"`.
#[test]
fn returns_true_for_the_fallback_editor_process_names() {
    for process_name in ["notepad++", "emeditor", "sublime_text"] {
        assert!(
            is_clipboard_fallback_editor("", "", process_name),
            "process '{process_name}' should be a fallback editor"
        );
    }
}

/// `[InlineData("Notepad++", "", "")]`.
#[test]
fn returns_true_for_the_fallback_editor_control_class() {
    assert!(is_clipboard_fallback_editor("Notepad++", "", ""));
}

/// `[InlineData("", "EmEditorMainFrame3", "")]` and `"EmEditorMainFrame4"`.
#[test]
fn returns_true_for_the_fallback_editor_root_classes() {
    for root_class in ["EmEditorMainFrame3", "EmEditorMainFrame4"] {
        assert!(
            is_clipboard_fallback_editor("", root_class, ""),
            "root class '{root_class}' should be a fallback editor"
        );
    }
}

#[test]
fn returns_false_for_an_unknown_editor() {
    assert!(!is_clipboard_fallback_editor(
        "CustomClass",
        "CustomRoot",
        "unknown_editor"
    ));
}

#[test]
fn matches_process_names_case_insensitively() {
    assert!(is_clipboard_fallback_editor("", "", "Sublime_Text"));
}

// Extra characterization beyond the xunit file: every lookup in the source is
// OrdinalIgnoreCase, so the class matches are case-insensitive too.

#[test]
fn matches_classes_case_insensitively() {
    assert!(is_clipboard_fallback_editor("notepad++", "", ""));
    assert!(is_clipboard_fallback_editor("NOTEPAD++", "", ""));
    assert!(is_clipboard_fallback_editor("", "emeditormainframe3", ""));
    assert!(is_clipboard_fallback_editor("", "EMEDITORMAINFRAME4", ""));
}

/// C# guards the process lookup with `!string.IsNullOrEmpty(processName)`, so an empty
/// (or blank-but-non-empty) name never matches.
#[test]
fn an_empty_process_name_never_matches_the_process_list() {
    assert!(!is_clipboard_fallback_editor("", "", ""));
    assert!(!is_clipboard_fallback_editor("", "", " "));
}
