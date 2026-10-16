//! Registry port of the Microsoft.Win32 usage in SettingsManager.cs + App.xaml.cs —
//! HKCU Run value = quoted exe path (autostart), Personalize AppsUseLightTheme
//! (System theme resolution). All best-effort, never fatal.

use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_DWORD, REG_SZ, RegCloseKey, RegDeleteValueW,
    RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
use windows::core::PCWSTR;

/// C# `RunRegistryKey`.
const RUN_KEY: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
/// C# `RunRegistryValueName`.
const RUN_VALUE_NAME: &str = "Orbital";
/// C# `GetSystemTheme` key path.
const PERSONALIZE_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize";
/// C# `"AppsUseLightTheme"`.
const APPS_USE_LIGHT_THEME_VALUE: &str = "AppsUseLightTheme";

/// C# `ApplyStartupRegistry` — best-effort, every failure is swallowed exactly like
/// the source's `catch { /* non-fatal: startup registry is best-effort */ }`.
pub fn apply_run_at_startup(enable: bool) {
    let _ = apply_run_at_startup_inner(enable);
}

/// C# `IsStartupRegistryEnabled` — `false` when the key or value is unreadable.
pub fn is_run_at_startup_enabled() -> bool {
    match open_subkey(RUN_KEY, KEY_READ) {
        Some(key) => {
            let present = query_value_exists(&key, RUN_VALUE_NAME);
            close_key(&key);
            present
        }
        None => false,
    }
}

/// C# `GetSystemTheme` — `AppsUseLightTheme`; None when unreadable (→ Dark).
///
/// Returning `Option` keeps the C# `is int v && v == 1 ? "Light" : "Dark"` decision
/// (and its unreadable-key fallback) in the caller.
pub fn apps_use_light_theme() -> Option<bool> {
    let key = open_subkey(PERSONALIZE_KEY, KEY_READ)?;
    let light = query_dword(&key, APPS_USE_LIGHT_THEME_VALUE);
    close_key(&key);
    light.map(|value| value == 1)
}

/// C# `ApplyStartupRegistry` body.
fn apply_run_at_startup_inner(enable: bool) -> windows::core::Result<()> {
    let key = match open_subkey(RUN_KEY, KEY_WRITE) {
        Some(key) => key,
        None => return Ok(()),
    };

    let result = if enable {
        // C# `Environment.ProcessPath ?? ... ?? string.Empty` — an empty path is a
        // silent no-op, because `if (string.IsNullOrEmpty(exePath)) return;`.
        match std::env::current_exe() {
            Ok(exe_path) => {
                // C# `key.SetValue(RunRegistryValueName, $"\"{exePath}\"")` — REG_SZ.
                let value = format!("\"{}\"", exe_path.display());
                let encoded: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
                set_unicode_value(&key, RUN_VALUE_NAME, &encoded)
            }
            Err(error) => {
                tracing::debug!(%error, "could not resolve the current executable path for run-at-startup");
                Ok(())
            }
        }
    } else {
        // C# `if (key.GetValue(RunRegistryValueName) != null) key.DeleteValue(..., false);`
        if query_value_exists(&key, RUN_VALUE_NAME) {
            delete_value(&key, RUN_VALUE_NAME)
        } else {
            Ok(())
        }
    };

    close_key(&key);
    result
}

/// C# `Registry.CurrentUser.OpenSubKey(path, writable)` — `None` when the key is absent.
fn open_subkey(
    path: &str,
    access: windows::Win32::System::Registry::REG_SAM_FLAGS,
) -> Option<HKEY> {
    let encoded: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
    let mut key = HKEY::default();
    // SAFETY: `encoded` is a NUL-terminated UTF-16 path and `key` is a live out slot.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(encoded.as_ptr()),
            None,
            access,
            &mut key,
        )
    };
    if status.is_ok() { Some(key) } else { None }
}

fn close_key(key: &HKEY) {
    if key.0.is_null() {
        return;
    }
    // SAFETY: `key` is either an open HKCU subkey or null; the null case was handled above.
    unsafe {
        let _ = RegCloseKey(*key);
    }
}

/// C# `key.GetValue(name) != null`.
fn query_value_exists(key: &HKEY, name: &str) -> bool {
    let encoded: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut capacity: u32 = 0;
    // A `None` data pointer asks only for the size, which is enough to tell whether the
    // value exists.
    // SAFETY: `encoded` is a NUL-terminated UTF-16 value name and `capacity` is a live
    // out slot; no data buffer is written.
    let status = unsafe {
        RegQueryValueExW(
            *key,
            PCWSTR(encoded.as_ptr()),
            None,
            None,
            None,
            Some(&mut capacity),
        )
    };
    status.is_ok()
}

/// Read the value as a DWORD. `None` when it is missing or not a DWORD.
fn query_dword(key: &HKEY, name: &str) -> Option<u32> {
    let encoded: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let mut value: u32 = 0;
    let mut capacity: u32 = size_of_u32();
    let mut kind = windows::Win32::System::Registry::REG_VALUE_TYPE::default();
    // SAFETY: `encoded` is a NUL-terminated UTF-16 value name; `value`/`capacity`
    // describe a 4-byte buffer and `kind` is a live out slot.
    let status = unsafe {
        RegQueryValueExW(
            *key,
            PCWSTR(encoded.as_ptr()),
            None,
            Some(&mut kind),
            Some((&raw mut value).cast()),
            Some(&mut capacity),
        )
    };
    if status.is_ok() && kind == REG_DWORD && capacity == size_of_u32() {
        Some(value)
    } else {
        None
    }
}

/// C# `key.SetValue(name, quoted exe path)` — an REG_SZ value.
fn set_unicode_value(key: &HKEY, name: &str, value: &[u16]) -> windows::core::Result<()> {
    let encoded: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `encoded` is a NUL-terminated UTF-16 value name and `value` is a live
    // UTF-16 buffer whose byte length is passed through unchanged.
    let status = unsafe {
        RegSetValueExW(
            *key,
            PCWSTR(encoded.as_ptr()),
            None,
            REG_SZ,
            Some(std::slice::from_raw_parts(
                value.as_ptr().cast::<u8>(),
                std::mem::size_of_val(value),
            )),
        )
    };
    status.ok()
}

/// C# `key.DeleteValue(name, throwOnMissingValue: false)`.
fn delete_value(key: &HKEY, name: &str) -> windows::core::Result<()> {
    let encoded: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: `encoded` is a NUL-terminated UTF-16 value name owned by this frame.
    let status = unsafe { RegDeleteValueW(*key, PCWSTR(encoded.as_ptr())) };
    status.ok()
}

fn size_of_u32() -> u32 {
    std::mem::size_of::<u32>() as u32
}
