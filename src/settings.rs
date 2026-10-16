//! Settings port of `SettingsManager.cs` (+ `platform::dpapi`, `platform::registry`).
//! `%APPDATA%/Orbital/settings.json`, legacy `Orbit→Orbital` migration, `.corrupt.bak`
//! recovery, autoClose<1 → 20, import merge/replace with duplicate/unknown validation.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::action_type::ActionType;
use crate::placement::PopupPlacementMode;

/// Newtonsoft default-enum semantics: discriminant 0 (`TextAndIcon`), numeric wire
/// format (no StringEnumConverter in the source). Missing or null → TextAndIcon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonDisplayMode {
    TextAndIcon,
    TextOnly,
    IconOnly,
}

// The frozen stub keeps this `Default` impl hand-written next to the enum
// declaration (so `#[derive(Default)]` + `#[default]` is deliberately not used).
#[allow(clippy::derivable_impls)]
impl Default for ButtonDisplayMode {
    fn default() -> Self {
        Self::TextAndIcon
    }
}

/// Newtonsoft acceptance, same as `PopupPlacementMode`: any integer loads (out-of-range
/// values have no switch on the executor path, so they land on the discriminant-0
/// member), JSON strings match member names case-insensitively (numeric strings too),
/// and an unknown name is the source's corrupt-recovery path.
impl<'de> Deserialize<'de> for ButtonDisplayMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
            return Ok(Self::TextAndIcon);
        };
        match value {
            serde_json::Value::Number(number) => Ok(match number.as_i64() {
                Some(1) => Self::TextOnly,
                Some(2) => Self::IconOnly,
                _ => Self::TextAndIcon,
            }),
            serde_json::Value::String(raw) => {
                let raw = raw.trim();
                if let Ok(number) = raw.parse::<i64>() {
                    return Ok(match number {
                        1 => Self::TextOnly,
                        2 => Self::IconOnly,
                        _ => Self::TextAndIcon,
                    });
                }
                match raw.to_ascii_lowercase().as_str() {
                    "textandicon" => Ok(Self::TextAndIcon),
                    "textonly" => Ok(Self::TextOnly),
                    "icononly" => Ok(Self::IconOnly),
                    other => Err(D::Error::custom(format!(
                        "Unknown ButtonDisplayMode name {other:?}"
                    ))),
                }
            }
            other => Err(D::Error::custom(format!(
                "Invalid ButtonDisplayMode value {other}"
            ))),
        }
    }
}

impl Serialize for ButtonDisplayMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            Self::TextAndIcon => 0,
            Self::TextOnly => 1,
            Self::IconOnly => 2,
        })
    }
}

/// Port of `ActionProfile`. JSON wire format is identical to the C# class:
/// PascalCase property names (serde renames), `ResultAction` stays a string.
///
/// An explicit JSON `null` coerces to the property's initializer default, exactly
/// like Newtonsoft, which leaves the C# property at `string.Empty` /
/// `ActionType.Popup.ToSerializedString()` instead of failing the load.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ActionProfile {
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub name: String,
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub icon: String,
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub prompt_format: String,
    #[serde(
        default = "default_result_action",
        deserialize_with = "de_result_action"
    )]
    pub result_action: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_selection: Option<bool>,
    #[serde(default)]
    pub display_mode: ButtonDisplayMode,
    #[serde(default)]
    pub clean_output: bool,
}

/// Port of `AppSettings` — same PascalCase wire format and same defaults.
///
/// An explicit JSON `null` on a scalar field coerces to that field's default,
/// mirroring Newtonsoft's coverage: `"ApiBaseUrl": null` leaves the property at
/// `string.Empty`, which the source's downstream `string.IsNullOrWhiteSpace` check
/// in `RebuildActionExecutor` then substitutes with the same default. A missing key
/// still takes `#[serde(default)]`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AppSettings {
    #[serde(default, deserialize_with = "de_string_or_empty")]
    pub encrypted_api_key: String,
    #[serde(default = "default_api_base_url", deserialize_with = "de_api_base_url")]
    pub api_base_url: String,
    #[serde(default = "default_model_name", deserialize_with = "de_model_name")]
    pub model_name: String,
    #[serde(default)]
    pub run_at_startup: bool,
    #[serde(default = "default_theme", deserialize_with = "de_theme")]
    pub theme: String,
    #[serde(default = "default_language", deserialize_with = "de_language")]
    pub language: String,
    #[serde(default, deserialize_with = "de_actions_or_empty")]
    pub actions: Vec<ActionProfile>,
    #[serde(default, deserialize_with = "de_u32_or_zero")]
    pub hotkey_modifiers: u32,
    #[serde(default, deserialize_with = "de_u32_or_zero")]
    pub hotkey_virtual_key: u32,
    #[serde(default = "default_true")]
    pub enable_drag_trigger: bool,
    #[serde(default = "default_true")]
    pub enable_double_click_trigger: bool,
    #[serde(default = "default_true")]
    pub enable_long_press_trigger: bool,
    #[serde(default = "default_true")]
    pub enable_keyboard_selection_trigger: bool,
    #[serde(default = "default_true")]
    pub sound_enabled: bool,
    #[serde(default)]
    pub popup_placement: PopupPlacementMode,
    #[serde(default = "default_true")]
    pub popup_auto_close_enabled: bool,
    #[serde(
        default = "default_popup_auto_close_seconds",
        deserialize_with = "de_popup_auto_close_seconds"
    )]
    pub popup_auto_close_seconds: i32,
}

// — Newtonsoft null-coercion helpers ———————————————————————————————————————————
// Each helper is the null arm of one field's `#[serde(default)]`: JSON `null`
// deserializes to `Option::None`, which maps back to the same value a missing key
// would have produced. Anything else is the ordinary serde path.

/// `string` field whose initializer default is `string.Empty`.
fn de_string_or_empty<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

/// C# `ActionProfile.ResultAction` initializer — `ActionType.Popup`.
fn de_result_action<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_result_action))
}

/// C# `AppSettings.ApiBaseUrl` initializer.
fn de_api_base_url<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_api_base_url))
}

/// C# `AppSettings.ModelName` initializer.
fn de_model_name<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_model_name))
}

/// C# `AppSettings.Theme` initializer.
fn de_theme<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_theme))
}

/// C# `AppSettings.Language` initializer.
fn de_language<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_else(default_language))
}

/// C# `AppSettings.Actions` initializer — `new List<ActionProfile>()`.
fn de_actions_or_empty<'de, D>(deserializer: D) -> Result<Vec<ActionProfile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<Vec<ActionProfile>>::deserialize(deserializer)?.unwrap_or_default())
}

/// C# `AppSettings.HotkeyModifiers` / `HotkeyVirtualKey` initializer — 0.
fn de_u32_or_zero<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<u32>::deserialize(deserializer)?.unwrap_or_default())
}

/// C# `AppSettings.PopupAutoCloseSeconds` initializer — 20 (`NormalizeSettings`
/// re-applies the same 20 for any value < 1).
fn de_popup_auto_close_seconds<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Option::<i32>::deserialize(deserializer)?.unwrap_or_else(default_popup_auto_close_seconds))
}

/// Parsed typed `ActionType` (C# `ActionProfile.ActionType` — `FromString` fallback Popup).
pub fn action_type_of(profile: &ActionProfile) -> ActionType {
    ActionType::from_str(&profile.result_action)
}

/// C# `ActionProfile.IsSelectionRequired` — `RequiresSelection ?? (≠ Paste && ≠ SimulateKey)`.
pub fn is_selection_required(profile: &ActionProfile) -> bool {
    match profile.requires_selection {
        Some(explicit) => explicit,
        None => {
            let action_type = action_type_of(profile);
            action_type != ActionType::Paste && action_type != ActionType::SimulateKey
        }
    }
}

pub fn create_default_settings() -> AppSettings {
    AppSettings {
        encrypted_api_key: String::new(),
        api_base_url: default_api_base_url(),
        model_name: default_model_name(),
        run_at_startup: false,
        theme: default_theme(),
        language: default_language(),
        actions: default_actions(),
        hotkey_modifiers: 0,
        hotkey_virtual_key: 0,
        enable_drag_trigger: true,
        enable_double_click_trigger: true,
        enable_long_press_trigger: true,
        enable_keyboard_selection_trigger: true,
        sound_enabled: true,
        popup_placement: PopupPlacementMode::BottomRight,
        popup_auto_close_enabled: true,
        popup_auto_close_seconds: default_popup_auto_close_seconds(),
    }
}

/// The eight actions `SettingsManager.CreateDefaultSettings` ships with.
fn default_actions() -> Vec<ActionProfile> {
    fn action(
        name: &str,
        icon: &str,
        prompt_format: &str,
        result_action: &str,
        requires_selection: Option<bool>,
        clean_output: bool,
    ) -> ActionProfile {
        ActionProfile {
            name: name.to_string(),
            icon: icon.to_string(),
            prompt_format: prompt_format.to_string(),
            result_action: result_action.to_string(),
            requires_selection,
            display_mode: ButtonDisplayMode::TextAndIcon,
            clean_output,
        }
    }

    vec![
        action("Copy", "\u{E8C8}", "", "DirectCopy", Some(true), false),
        action("Cut", "\u{E8C6}", "", "Cut", Some(true), false),
        action("Paste", "\u{E77F}", "", "Paste", Some(false), false),
        action(
            "Select All",
            "\u{E8B3}",
            "",
            "SelectAll",
            Some(false),
            false,
        ),
        action(
            "Translate",
            "\u{E8C1}",
            "Translate the following to Korean organically: {text}",
            "Replace",
            Some(true),
            true,
        ),
        action(
            "Summarize",
            "\u{E7C3}",
            "Summarize the following in 3 lines: {text}",
            "Popup",
            Some(true),
            false,
        ),
        action(
            "Polish",
            "\u{E70F}",
            "Correct grammar and make this sound professional: {text}",
            "Replace",
            Some(true),
            true,
        ),
        action("Search", "\u{E721}", "", "Browser", Some(true), false),
    ]
}

/// Result of `load`: settings plus whether a corrupt file was recovered (backup written).
#[derive(Debug)]
pub struct LoadOutcome {
    pub settings: AppSettings,
    pub recovered: bool,
}

/// Owns the settings file path; all file IO goes through it so tests can point at tempdirs.
#[derive(Debug, Clone)]
pub struct SettingsStore {
    path: PathBuf,
}

impl SettingsStore {
    /// `%APPDATA%/Orbital/settings.json`.
    pub fn at_default_path() -> Self {
        Self {
            path: default_settings_path(),
        }
    }

    pub fn with_path(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// C# `LoadSettings`: legacy migrate → parse → normalize → or corrupt-backup recovery.
    pub fn load(&self) -> std::io::Result<LoadOutcome> {
        migrate_from_legacy_path();

        match std::fs::read_to_string(&self.path) {
            Ok(json) => match serde_json::from_str::<AppSettings>(&json) {
                Ok(mut settings) => {
                    normalize_settings(&mut settings);
                    tracing::debug!(path = %self.path.display(), "settings loaded");
                    Ok(LoadOutcome {
                        settings,
                        recovered: false,
                    })
                }
                Err(err) => {
                    tracing::warn!(
                        path = %self.path.display(),
                        error = %err,
                        "settings JSON could not be parsed; backing up and starting fresh"
                    );
                    self.recover_from_corrupt_file()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                let settings = create_default_settings();
                self.save(&settings)?;
                Ok(LoadOutcome {
                    settings,
                    recovered: false,
                })
            }
            Err(err) => {
                tracing::warn!(
                    path = %self.path.display(),
                    error = %err,
                    "settings file could not be read; backing up and starting fresh"
                );
                self.recover_from_corrupt_file()
            }
        }
    }

    /// C# `SaveSettings`: creates dir, writes indented JSON.
    pub fn save(&self, settings: &AppSettings) -> std::io::Result<()> {
        let dir = match self.path.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir,
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "Unable to resolve settings directory.",
                ));
            }
        };

        if !dir.exists() {
            std::fs::create_dir_all(dir)?;
        }

        let json = serde_json::to_string_pretty(settings)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;
        // Newtonsoft's `JsonTextWriter` indents with `Environment.NewLine`, which is
        // CRLF on Windows — keep the artifact byte-compatible with the source's file.
        let json = json.replace('\n', "\r\n");
        std::fs::write(&self.path, json)
    }

    /// C# `SetApiKey` — DPAPI CurrentUser + Base64.
    pub fn set_api_key(settings: &mut AppSettings, raw_key: &str) {
        if raw_key.is_empty() {
            settings.encrypted_api_key = String::new();
            return;
        }

        match dpapi_protect(raw_key.as_bytes()) {
            Ok(encrypted) => settings.encrypted_api_key = base64_encode(&encrypted),
            Err(err) => {
                // The source lets a ProtectedData.Protect failure escape as an
                // exception; the frozen `()` signature cannot, so the key is simply
                // not stored. Never logged: the payload is a secret.
                tracing::warn!("DPAPI protect failed; API key not stored: {err}");
                settings.encrypted_api_key = String::new();
            }
        }
    }

    /// C# `GetApiKey` — Base64 + DPAPI, empty string on any failure.
    pub fn get_api_key(settings: &AppSettings) -> String {
        if settings.encrypted_api_key.is_empty() {
            return String::new();
        }

        let Some(encrypted) = base64_decode(&settings.encrypted_api_key) else {
            return String::new();
        };
        let Ok(decrypted) = dpapi_unprotect(&encrypted) else {
            return String::new();
        };

        // C# `Encoding.UTF8.GetString` replaces invalid sequences rather than failing.
        String::from_utf8_lossy(&decrypted).into_owned()
    }

    /// C# `ExportActionPack` — error string on failure, like the source returns.
    pub fn export_actions(settings: &AppSettings, file_path: &Path) -> Result<(), String> {
        let json = match serde_json::to_string_pretty(&settings.actions) {
            Ok(json) => json,
            Err(err) => return Err(format!("Export failed: {err}")),
        };

        match std::fs::write(file_path, json) {
            Ok(()) => Ok(()),
            Err(err) => Err(format!("Export failed: {err}")),
        }
    }

    /// C# corrupt-file recovery: best-effort move to `.corrupt.bak`, then defaults
    /// written back to the original path (`recovered == true` afterwards).
    fn recover_from_corrupt_file(&self) -> std::io::Result<LoadOutcome> {
        let backup = corrupt_backup_path(&self.path);

        // Both steps are best-effort in the source (`catch { }`).
        if backup.exists() {
            let _ = std::fs::remove_file(&backup);
        }
        let _ = std::fs::rename(&self.path, &backup);

        let settings = create_default_settings();
        self.save(&settings)?;

        Ok(LoadOutcome {
            settings,
            recovered: true,
        })
    }

    /// C# `ImportActionPack` — validation errors returned as strings, settings untouched on error.
    pub fn import_actions(
        &self,
        file_path: &Path,
        replace_existing: bool,
        settings: &mut AppSettings,
    ) -> Result<(), String> {
        if !file_path.exists() {
            return Err("File not found.".to_string());
        }

        let json = match std::fs::read_to_string(file_path) {
            Ok(json) => json,
            Err(err) => return Err(format!("Import failed: {err}")),
        };

        // C# deserializes into a nullable list: a JSON `null` is "no valid actions".
        let parsed: Option<Vec<ActionProfile>> = match serde_json::from_str(&json) {
            Ok(parsed) => parsed,
            Err(err) => return Err(format!("Invalid JSON format: {err}")),
        };
        let mut imported = match parsed {
            Some(imported) if !imported.is_empty() => imported,
            _ => return Err("No valid actions found in file.".to_string()),
        };

        // Validate first: nothing is mutated until every action passes.
        let mut imported_names: HashSet<String> = HashSet::with_capacity(imported.len());
        for action in imported.iter_mut() {
            if let Some(error) = validate_imported_action(action) {
                return Err(error);
            }

            action.name = action.name.trim().to_string();

            // `HashSet.Add` returns false for a name already seen (case-insensitive).
            if !imported_names.insert(normalize_name(&action.name)) {
                return Err(format!(
                    "Duplicate action name '{}' found in import file.",
                    action.name
                ));
            }
        }

        if replace_existing {
            settings.actions = imported;
        } else {
            // Merge: add imported actions, skip duplicates by name
            // (StringComparer.OrdinalIgnoreCase).
            let mut existing_names: HashSet<String> = settings
                .actions
                .iter()
                .map(|a| normalize_name(&a.name))
                .collect();

            for action in imported {
                if existing_names.insert(normalize_name(&action.name)) {
                    settings.actions.push(action);
                }
            }
        }

        self.save(settings)
            .map_err(|err| format!("Import failed: {err}"))
    }
}

/// C# `ValidateImportedAction`.
fn validate_imported_action(action: &ActionProfile) -> Option<String> {
    if action.name.trim().is_empty() {
        return Some("Invalid action: Name is required.".to_string());
    }

    if action.result_action.trim().is_empty() {
        return Some(format!(
            "Invalid action '{}': ResultAction is required.",
            action.name
        ));
    }

    if ActionType::try_from_str(&action.result_action).is_none() {
        return Some(format!(
            "Invalid action '{}': Unknown ResultAction '{}'.",
            action.name, action.result_action
        ));
    }

    None
}

/// `StringComparer.OrdinalIgnoreCase` — invariant-uppercase both sides.
fn normalize_name(name: &str) -> String {
    name.to_uppercase()
}

/// C# `NormalizeSettings`.
fn normalize_settings(settings: &mut AppSettings) {
    if settings.popup_auto_close_seconds < 1 {
        settings.popup_auto_close_seconds = 20;
    }
}

/// C# `MigrateFromLegacyPath` — renames `%APPDATA%/Orbit` to `%APPDATA%/Orbital`
/// when the new directory does not exist yet.
fn migrate_from_legacy_path() {
    let base = config_dir();
    let old_dir = base.join("Orbit");
    let new_dir = base.join("Orbital");

    if !new_dir.exists() && old_dir.exists() {
        // Directory.Move failures are non-fatal in the source ("will start fresh").
        match std::fs::rename(&old_dir, &new_dir) {
            Ok(()) => tracing::info!("migrated legacy settings directory to Orbital"),
            Err(err) => tracing::debug!(error = %err, "legacy settings migration skipped"),
        }
    }
}

/// C# backup path: `settings.json` → `settings.json.corrupt.bak`.
fn corrupt_backup_path(path: &Path) -> PathBuf {
    let mut backup = path.as_os_str().to_os_string();
    backup.push(".corrupt.bak");
    PathBuf::from(backup)
}

/// `%APPDATA%` (`Environment.SpecialFolder.ApplicationData`).
fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_default()
}

/// `%APPDATA%/Orbital/settings.json`.
fn default_settings_path() -> PathBuf {
    config_dir().join("Orbital").join("settings.json")
}

/// DPAPI CurrentUser round-trip. Windows-only: off-Windows builds keep the module
/// surface but never store a key.
#[cfg(target_os = "windows")]
fn dpapi_protect(data: &[u8]) -> std::io::Result<Vec<u8>> {
    crate::platform::dpapi::protect(data)
}

#[cfg(target_os = "windows")]
fn dpapi_unprotect(data: &[u8]) -> std::io::Result<Vec<u8>> {
    crate::platform::dpapi::unprotect(data)
}

#[cfg(not(target_os = "windows"))]
fn dpapi_protect(_data: &[u8]) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other("DPAPI is only available on Windows"))
}

#[cfg(not(target_os = "windows"))]
fn dpapi_unprotect(_data: &[u8]) -> std::io::Result<Vec<u8>> {
    Err(std::io::Error::other("DPAPI is only available on Windows"))
}

// ── Tiny Base64 ─────────────────────────────────────────────────────────────
// The engine's `base64` crate is not a dependency of this crate, so the subset
// `Convert.ToBase64String` / `Convert.FromBase64String` need for the DPAPI blob
// is implemented here: standard alphabet, `=` padding, and anything else is a
// hard failure (which `GetApiKey` turns into an empty string, like the source's
// `catch { return string.Empty; }`).

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let quantum = (b0 << 16) | (b1 << 8) | b2;

        out.push(BASE64_ALPHABET[(quantum >> 18) as usize & 0x3F] as char);
        out.push(BASE64_ALPHABET[(quantum >> 12) as usize & 0x3F] as char);
        out.push(match chunk.len() {
            1 => '=',
            _ => BASE64_ALPHABET[(quantum >> 6) as usize & 0x3F] as char,
        });
        out.push(match chunk.len() {
            1 | 2 => '=',
            _ => BASE64_ALPHABET[quantum as usize & 0x3F] as char,
        });
    }

    out
}

fn base64_decode(input: &str) -> Option<Vec<u8>> {
    // .NET ignores ASCII white space inside Base64 strings.
    let filtered: Vec<u8> = input
        .bytes()
        .filter(|byte| !byte.is_ascii_whitespace())
        .collect();

    if !filtered.len().is_multiple_of(4) {
        return None;
    }
    if filtered.is_empty() {
        return Some(Vec::new());
    }

    let (quanta, remainder) = filtered.as_chunks::<4>();
    if !remainder.is_empty() {
        return None;
    }

    let mut out = Vec::with_capacity(filtered.len() / 4 * 3);
    let mut quanta = quanta.iter();
    let last = quanta.next_back();

    for quantum in quanta {
        decode_quantum(quantum, false, &mut out)?;
    }
    if let Some(quantum) = last {
        decode_quantum(quantum, true, &mut out)?;
    }

    Some(out)
}

fn decode_quantum(quantum_bytes: &[u8; 4], is_last: bool, out: &mut Vec<u8>) -> Option<()> {
    let mut values = [0u8; 4];
    let mut padding = 0usize;
    let mut seen_padding = false;

    for (index, byte) in quantum_bytes.iter().enumerate() {
        if *byte == b'=' {
            // Padding is only legal in the final quantum, and at most two of it.
            if !is_last || padding == 2 {
                return None;
            }
            seen_padding = true;
            padding += 1;
            continue;
        }
        if seen_padding {
            return None;
        }
        values[index] = decode_base64_char(*byte)?;
    }

    let quantum = ((values[0] as u32) << 18)
        | ((values[1] as u32) << 12)
        | ((values[2] as u32) << 6)
        | (values[3] as u32);

    match padding {
        0 => {
            out.push((quantum >> 16) as u8);
            out.push((quantum >> 8) as u8);
            out.push(quantum as u8);
        }
        1 => {
            out.push((quantum >> 16) as u8);
            out.push((quantum >> 8) as u8);
        }
        2 => out.push((quantum >> 16) as u8),
        _ => return None,
    }

    Some(())
}

fn decode_base64_char(byte: u8) -> Option<u8> {
    match byte {
        b'A'..=b'Z' => Some(byte - b'A'),
        b'a'..=b'z' => Some(byte - b'a' + 26),
        b'0'..=b'9' => Some(byte - b'0' + 52),
        b'+' => Some(62),
        b'/' => Some(63),
        _ => None,
    }
}

fn default_result_action() -> String {
    "Popup".to_string()
}
fn default_api_base_url() -> String {
    "https://api.openai.com/v1".to_string()
}
fn default_model_name() -> String {
    "gpt-4o-mini".to_string()
}
fn default_theme() -> String {
    "Dark".to_string()
}
fn default_language() -> String {
    "en".to_string()
}
fn default_true() -> bool {
    true
}
fn default_popup_auto_close_seconds() -> i32 {
    20
}

// `PopupPlacementMode` serde (numeric wire format, missing/null → BottomRight)
// is implemented in `placement.rs` next to the type.

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_round_trips_the_standard_alphabet() {
        for data in [
            Vec::new(),
            vec![0u8],
            vec![0u8, 0],
            vec![0u8, 0, 0],
            vec![0xFFu8, 0x00, 0x7F, 0x80, 0x01],
            (0u8..=255).collect::<Vec<u8>>(),
        ] {
            let encoded = base64_encode(&data);
            assert_eq!(base64_decode(&encoded), Some(data));
        }
    }

    #[test]
    fn base64_encode_matches_the_known_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn base64_decode_rejects_invalid_input() {
        assert_eq!(base64_decode("Zm9vYmF"), None); // bad length
        assert_eq!(base64_decode("Zm9*vg=="), None); // invalid character
        assert_eq!(base64_decode("Z==="), None); // too much padding
        assert_eq!(base64_decode("Z=9v"), None); // data after padding
    }

    #[test]
    fn base64_decode_ignores_whitespace() {
        assert_eq!(base64_decode("Zm9v\nYmFy"), base64_decode("Zm9vYmFy"));
        assert_eq!(base64_decode("Zm 9v\tYmFy"), Some(b"foobar".to_vec()));
    }

    #[test]
    fn default_settings_carry_the_eight_shipped_actions() {
        let settings = create_default_settings();
        let names: Vec<&str> = settings.actions.iter().map(|a| a.name.as_str()).collect();

        assert_eq!(names.len(), 8);
        assert_eq!(
            names,
            vec![
                "Copy",
                "Cut",
                "Paste",
                "Select All",
                "Translate",
                "Summarize",
                "Polish",
                "Search"
            ]
        );

        assert_eq!(settings.actions[0].icon, "\u{E8C8}");
        assert_eq!(settings.actions[0].result_action, "DirectCopy");
        assert_eq!(settings.actions[0].requires_selection, Some(true));
        assert_eq!(settings.actions[4].icon, "\u{E8C1}");
        assert!(settings.actions[4].clean_output);
        assert_eq!(settings.actions[6].icon, "\u{E70F}");
        assert_eq!(settings.actions[7].icon, "\u{E721}");
        assert_eq!(settings.actions[7].result_action, "Browser");

        assert_eq!(settings.theme, "Dark");
        assert_eq!(settings.language, "en");
        assert_eq!(settings.api_base_url, "https://api.openai.com/v1");
        assert_eq!(settings.model_name, "gpt-4o-mini");
        assert!(settings.sound_enabled);
        assert!(settings.popup_auto_close_enabled);
        assert_eq!(settings.popup_auto_close_seconds, 20);
        assert_eq!(settings.popup_placement, PopupPlacementMode::BottomRight);
    }

    #[test]
    fn normalize_settings_resets_a_non_positive_auto_close() {
        let mut settings = create_default_settings();

        settings.popup_auto_close_seconds = 0;
        normalize_settings(&mut settings);
        assert_eq!(settings.popup_auto_close_seconds, 20);

        settings.popup_auto_close_seconds = -5;
        normalize_settings(&mut settings);
        assert_eq!(settings.popup_auto_close_seconds, 20);

        settings.popup_auto_close_seconds = 60;
        normalize_settings(&mut settings);
        assert_eq!(settings.popup_auto_close_seconds, 60);
    }

    #[test]
    fn app_settings_json_uses_pascal_case_keys_in_declaration_order() {
        let settings = create_default_settings();
        let json = serde_json::to_string_pretty(&settings).expect("serialize");
        let value: serde_json::Value = serde_json::from_str(&json).expect("parse");

        // `serde_json::Value` sorts keys (BTreeMap), so the wire order is checked
        // against the serialized text, which keeps struct declaration order.
        const ORDER: &[&str] = &[
            "EncryptedApiKey",
            "ApiBaseUrl",
            "ModelName",
            "RunAtStartup",
            "Theme",
            "Language",
            "Actions",
            "HotkeyModifiers",
            "HotkeyVirtualKey",
            "EnableDragTrigger",
            "EnableDoubleClickTrigger",
            "EnableLongPressTrigger",
            "EnableKeyboardSelectionTrigger",
            "SoundEnabled",
            "PopupPlacement",
            "PopupAutoCloseEnabled",
            "PopupAutoCloseSeconds",
        ];

        let mut cursor = 0usize;
        for key in ORDER {
            let needle = format!("\"{key}\"");
            let found = json[cursor..]
                .find(&needle)
                .map(|offset| offset + cursor)
                .unwrap_or_else(|| panic!("key '{key}' missing or out of order"));
            cursor = found + needle.len();
        }

        assert_eq!(value["PopupPlacement"], 0);
        assert_eq!(value["Actions"][0]["Name"], "Copy");
        assert_eq!(value["Actions"][0]["ResultAction"], "DirectCopy");
        assert_eq!(value["Actions"][0]["Icon"], "\u{E8C8}");
    }
}
