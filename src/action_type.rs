//! ActionType port of `ActionType.cs` (pure logic, no dependencies).
//! 9 variants; `TryFromString` is exact-match after `Trim()` with `Popup` fallback;
//! `RequiresLlm` is true only for Replace/Copy/Popup.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ActionType {
    Replace,
    Copy,
    Popup,
    Browser,
    DirectCopy,
    Cut,
    Paste,
    SimulateKey,
    SelectAll,
}

impl ActionType {
    /// C# `ActionTypeExtensions.TryFromString` — exact match after Trim; None on no match.
    pub fn try_from_str(value: &str) -> Option<Self> {
        match value.trim() {
            "Replace" => Some(Self::Replace),
            "Copy" => Some(Self::Copy),
            "Popup" => Some(Self::Popup),
            "Browser" => Some(Self::Browser),
            "DirectCopy" => Some(Self::DirectCopy),
            "Cut" => Some(Self::Cut),
            "Paste" => Some(Self::Paste),
            "SimulateKey" => Some(Self::SimulateKey),
            "SelectAll" => Some(Self::SelectAll),
            _ => None,
        }
    }

    /// C# `FromString` — falls back to `Popup`.
    // Signature is frozen by the stub; the name mirrors the C# extension method.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(value: &str) -> Self {
        match Self::try_from_str(value) {
            Some(action_type) => action_type,
            None => Self::Popup,
        }
    }

    /// C# `ToSerializedString` — variant name exactly as Newtonsoft writes it.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Replace => "Replace",
            Self::Copy => "Copy",
            Self::Popup => "Popup",
            Self::Browser => "Browser",
            Self::DirectCopy => "DirectCopy",
            Self::Cut => "Cut",
            Self::Paste => "Paste",
            Self::SimulateKey => "SimulateKey",
            Self::SelectAll => "SelectAll",
        }
    }

    /// C# `RequiresLlm`.
    pub fn requires_llm(&self) -> bool {
        matches!(self, Self::Replace | Self::Copy | Self::Popup)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_from_str_matches_every_variant_name() {
        assert_eq!(
            ActionType::try_from_str("Replace"),
            Some(ActionType::Replace)
        );
        assert_eq!(ActionType::try_from_str("Copy"), Some(ActionType::Copy));
        assert_eq!(ActionType::try_from_str("Popup"), Some(ActionType::Popup));
        assert_eq!(
            ActionType::try_from_str("Browser"),
            Some(ActionType::Browser)
        );
        assert_eq!(
            ActionType::try_from_str("DirectCopy"),
            Some(ActionType::DirectCopy)
        );
        assert_eq!(ActionType::try_from_str("Cut"), Some(ActionType::Cut));
        assert_eq!(ActionType::try_from_str("Paste"), Some(ActionType::Paste));
        assert_eq!(
            ActionType::try_from_str("SimulateKey"),
            Some(ActionType::SimulateKey)
        );
        assert_eq!(
            ActionType::try_from_str("SelectAll"),
            Some(ActionType::SelectAll)
        );
    }

    #[test]
    fn try_from_str_trims_and_rejects_unknown() {
        assert_eq!(
            ActionType::try_from_str("  Paste\t"),
            Some(ActionType::Paste)
        );
        assert_eq!(ActionType::try_from_str("paste"), None);
        assert_eq!(ActionType::try_from_str(""), None);
        assert_eq!(ActionType::try_from_str("PasteAll"), None);
    }

    #[test]
    fn from_str_falls_back_to_popup() {
        assert_eq!(ActionType::from_str("Cut"), ActionType::Cut);
        assert_eq!(ActionType::from_str("nonsense"), ActionType::Popup);
        assert_eq!(ActionType::from_str(""), ActionType::Popup);
    }

    #[test]
    fn as_str_round_trips_through_try_from_str() {
        for action in [
            ActionType::Replace,
            ActionType::Copy,
            ActionType::Popup,
            ActionType::Browser,
            ActionType::DirectCopy,
            ActionType::Cut,
            ActionType::Paste,
            ActionType::SimulateKey,
            ActionType::SelectAll,
        ] {
            assert_eq!(ActionType::try_from_str(action.as_str()), Some(action));
        }
    }

    #[test]
    fn requires_llm_is_true_only_for_replace_copy_popup() {
        assert!(ActionType::Replace.requires_llm());
        assert!(ActionType::Copy.requires_llm());
        assert!(ActionType::Popup.requires_llm());

        assert!(!ActionType::Browser.requires_llm());
        assert!(!ActionType::DirectCopy.requires_llm());
        assert!(!ActionType::Cut.requires_llm());
        assert!(!ActionType::Paste.requires_llm());
        assert!(!ActionType::SimulateKey.requires_llm());
        assert!(!ActionType::SelectAll.requires_llm());
    }

    #[test]
    fn serde_uses_variant_name_strings() {
        let json = serde_json::to_string(&ActionType::SelectAll).expect("serialize");
        assert_eq!(json, "\"SelectAll\"");

        let back: ActionType = serde_json::from_str("\"DirectCopy\"").expect("deserialize");
        assert_eq!(back, ActionType::DirectCopy);
    }
}
