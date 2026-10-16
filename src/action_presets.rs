//! ActionPresets port of `ActionPresets.cs` — the 17-entry template library,
//! never auto-added to the user's action list. Prompts are verbatim; the `\uE8xx`
//! MDL2 glyphs map to Lucide icon names through `lucide_name` (flagged visual diff).

/// One built-in template, matching the C# `ActionProfile` literals in ActionPresets.cs.
#[derive(Debug, Clone, PartialEq)]
pub struct ActionPreset {
    pub name: &'static str,
    /// MDL2 glyph (PUA) as written in the C# source, e.g. "\u{E8C8}".
    pub icon: &'static str,
    pub prompt_format: &'static str,
    pub result_action: &'static str,
    pub requires_selection: Option<bool>,
    pub clean_output: bool,
}

pub fn all() -> Vec<ActionPreset> {
    vec![
        // ── Utility (no LLM) ─────────────────────────────────────────────────
        ActionPreset {
            name: "Copy",
            icon: "\u{E8C8}",
            prompt_format: "",
            result_action: "DirectCopy",
            requires_selection: Some(true),
            clean_output: false,
        },
        ActionPreset {
            name: "Cut",
            icon: "\u{E8C6}",
            prompt_format: "",
            result_action: "Cut",
            requires_selection: Some(true),
            clean_output: false,
        },
        ActionPreset {
            name: "Paste",
            icon: "\u{E77F}",
            prompt_format: "",
            result_action: "Paste",
            requires_selection: Some(false),
            clean_output: false,
        },
        ActionPreset {
            name: "Select All",
            icon: "\u{E8B3}",
            prompt_format: "",
            result_action: "SelectAll",
            requires_selection: Some(false),
            clean_output: false,
        },
        ActionPreset {
            name: "Search",
            icon: "\u{E721}",
            prompt_format: "",
            result_action: "Browser",
            requires_selection: Some(true),
            clean_output: false,
        },
        // ── Translation ───────────────────────────────────────────────────────
        ActionPreset {
            name: "Translate to Korean",
            icon: "\u{E8C1}",
            prompt_format: "Translate the following to Korean organically: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Translate to English",
            icon: "\u{E8C1}",
            prompt_format: "Translate the following to English naturally: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Translate to Japanese",
            icon: "\u{E8C1}",
            prompt_format: "Translate the following to Japanese naturally: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        // ── Writing ───────────────────────────────────────────────────────────
        ActionPreset {
            name: "Polish",
            icon: "\u{E70F}",
            prompt_format: "Correct grammar and make this sound professional: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Formal",
            icon: "\u{E8D4}",
            prompt_format: "Rewrite the following in a formal, professional tone: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Casual",
            icon: "\u{E76E}",
            prompt_format: "Rewrite the following in a casual, friendly tone: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Shorter",
            icon: "\u{E8A4}",
            prompt_format: "Make the following more concise without losing meaning: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        ActionPreset {
            name: "Bullet Points",
            icon: "\u{E8FD}",
            prompt_format: "Convert the following into a clear bullet-point list: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
        // ── Analysis (Popup) ──────────────────────────────────────────────────
        ActionPreset {
            name: "Summarize",
            icon: "\u{E7C3}",
            prompt_format: "Summarize the following in 3 lines: {text}",
            result_action: "Popup",
            requires_selection: Some(true),
            clean_output: false,
        },
        ActionPreset {
            name: "Explain",
            icon: "\u{E82D}",
            prompt_format: "Explain the following in simple terms: {text}",
            result_action: "Popup",
            requires_selection: Some(true),
            clean_output: false,
        },
        ActionPreset {
            name: "ELI5",
            icon: "\u{E899}",
            prompt_format: "Explain the following as if I'm 5 years old: {text}",
            result_action: "Popup",
            requires_selection: Some(true),
            clean_output: false,
        },
        ActionPreset {
            name: "Fix Code",
            icon: "\u{E943}",
            prompt_format: "Find and fix bugs in the following code, then return only the corrected code: {text}",
            result_action: "Replace",
            requires_selection: Some(true),
            clean_output: true,
        },
    ]
}

/// Maps an MDL2 glyph (`\uE8xx`) to a Lucide icon name.
/// Returns None for glyphs with no Lucide equivalent (icon-font fallback in UI instead).
pub fn lucide_name(icon: &str) -> Option<&'static str> {
    match icon {
        "\u{E8C8}" => Some("copy"),
        "\u{E8C6}" => Some("scissors"),
        "\u{E77F}" => Some("clipboard-paste"),
        "\u{E8B3}" => Some("text-select-input"),
        "\u{E721}" => Some("search"),
        "\u{E8C1}" => Some("languages"),
        "\u{E70F}" => Some("sparkles"),
        "\u{E8D4}" => Some("briefcase"),
        "\u{E76E}" => Some("message-circle"),
        "\u{E8A4}" => Some("minimize-2"),
        "\u{E8FD}" => Some("list"),
        "\u{E7C3}" => Some("file-text"),
        "\u{E82D}" => Some("lightbulb"),
        "\u{E899}" => Some("baby"),
        "\u{E943}" => Some("bug"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every preset icon must be present in `lucide_name`'s table — the UI would
    /// otherwise fall back to the MDL2 font for a glyph that has a Lucide twin.
    #[test]
    fn every_preset_icon_has_a_lucide_name() {
        for preset in all() {
            assert!(
                lucide_name(preset.icon).is_some(),
                "no Lucide mapping for icon of preset '{}'",
                preset.name
            );
        }
    }

    #[test]
    fn all_returns_the_17_presets_in_source_order() {
        let presets = all();

        assert_eq!(presets.len(), 17);
        assert_eq!(
            presets.iter().map(|p| p.name).collect::<Vec<_>>(),
            vec![
                "Copy",
                "Cut",
                "Paste",
                "Select All",
                "Search",
                "Translate to Korean",
                "Translate to English",
                "Translate to Japanese",
                "Polish",
                "Formal",
                "Casual",
                "Shorter",
                "Bullet Points",
                "Summarize",
                "Explain",
                "ELI5",
                "Fix Code",
            ]
        );
    }

    #[test]
    fn prompts_and_flags_are_verbatim() {
        let presets = all();

        let by_name = |name: &str| -> ActionPreset {
            presets
                .iter()
                .find(|p| p.name == name)
                .cloned()
                .expect("preset present")
        };

        let copy = by_name("Copy");
        assert_eq!(copy.icon, "\u{E8C8}");
        assert_eq!(copy.prompt_format, "");
        assert_eq!(copy.result_action, "DirectCopy");
        assert_eq!(copy.requires_selection, Some(true));
        assert!(!copy.clean_output);

        let translate = by_name("Translate to Korean");
        assert_eq!(
            translate.prompt_format,
            "Translate the following to Korean organically: {text}"
        );
        assert_eq!(translate.result_action, "Replace");
        assert_eq!(translate.requires_selection, Some(true));
        assert!(translate.clean_output);

        let fix_code = by_name("Fix Code");
        assert_eq!(
            fix_code.prompt_format,
            "Find and fix bugs in the following code, then return only the corrected code: {text}"
        );
        assert_eq!(fix_code.result_action, "Replace");

        let eli5 = by_name("ELI5");
        assert_eq!(
            eli5.prompt_format,
            "Explain the following as if I'm 5 years old: {text}"
        );
        assert_eq!(eli5.result_action, "Popup");
        assert_eq!(eli5.requires_selection, Some(true));
    }

    #[test]
    fn presets_use_only_known_result_actions() {
        for preset in all() {
            assert!(
                crate::action_type::ActionType::try_from_str(preset.result_action).is_some(),
                "preset '{}' has unknown ResultAction '{}'",
                preset.name,
                preset.result_action
            );
        }
    }

    #[test]
    fn lucide_name_maps_the_mdl2_glyphs_used_by_the_presets() {
        assert_eq!(lucide_name("\u{E8C8}"), Some("copy"));
        assert_eq!(lucide_name("\u{E8C6}"), Some("scissors"));
        assert_eq!(lucide_name("\u{E77F}"), Some("clipboard-paste"));
        assert_eq!(lucide_name("\u{E8B3}"), Some("text-select-input"));
        assert_eq!(lucide_name("\u{E721}"), Some("search"));
        assert_eq!(lucide_name("\u{E8C1}"), Some("languages"));
        assert_eq!(lucide_name("\u{E70F}"), Some("sparkles"));
        assert_eq!(lucide_name("\u{E8D4}"), Some("briefcase"));
        assert_eq!(lucide_name("\u{E76E}"), Some("message-circle"));
        assert_eq!(lucide_name("\u{E8A4}"), Some("minimize-2"));
        assert_eq!(lucide_name("\u{E8FD}"), Some("list"));
        assert_eq!(lucide_name("\u{E7C3}"), Some("file-text"));
        assert_eq!(lucide_name("\u{E82D}"), Some("lightbulb"));
        assert_eq!(lucide_name("\u{E899}"), Some("baby"));
        assert_eq!(lucide_name("\u{E943}"), Some("bug"));
    }

    #[test]
    fn lucide_name_returns_none_for_unknown_glyphs() {
        assert_eq!(lucide_name(""), None);
        assert_eq!(lucide_name("not a glyph"), None);
        assert_eq!(lucide_name("\u{E8C9}"), None);
    }
}
