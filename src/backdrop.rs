//! Window backdrop port of `Services/WindowBackdropHelper.cs` — DwmSetWindowAttribute
//! attrs 20 (immersive dark) / 1029 (mica, pre-22621) / 38 (system backdrop, ≥22621),
//! kinds 2 (main) / 3 (transient), version gate at 10.0.22000 / 10.0.22621, swallow-all.

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Dwm::{
    DWMWA_SYSTEMBACKDROP_TYPE, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWINDOWATTRIBUTE,
    DwmSetWindowAttribute,
};
use windows::Win32::System::Registry::{
    HKEY, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_SZ, REG_VALUE_TYPE, RegCloseKey,
    RegOpenKeyExW, RegQueryValueExW,
};
use windows::core::PCWSTR;

/// C# `DWMWA_MICA_EFFECT` = 1029. This attribute id is not part of the public Windows
/// metadata the `windows` crate ships, so the raw id is kept as a local constant.
const DWMWA_MICA_EFFECT: DWMWINDOWATTRIBUTE = DWMWINDOWATTRIBUTE(1029i32);

/// C# `DWMSBT_MAINWINDOW`.
const DWMSBT_MAINWINDOW: i32 = 2;
/// C# `DWMSBT_TRANSIENTWINDOW`.
const DWMSBT_TRANSIENTWINDOW: i32 = 3;

/// First Windows 11 build: `DWMWA_MICA_EFFECT` starts applying here.
const MICA_BUILD: u32 = 22000;
/// Windows 11 22H2 build: `DWMWA_SYSTEMBACKDROP_TYPE` replaces the mica attribute.
const SYSTEMBACKDROP_BUILD: u32 = 22621;

/// C# `WindowBackdropKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowBackdropKind {
    MainWindow,
    TransientWindow,
}

/// C# `WindowBackdropHelper.TryApply` — cosmetic; never fails loudly.
///
/// Every DWM or registry failure is swallowed, exactly like the source's `catch`,
/// because the backdrop only affects rendering cosmetics. The raw HWND hidden by the
/// source's `new WindowInteropHelper(window).Handle` is supplied directly by the caller.
pub fn try_apply(hwnd: isize, use_dark_mode: bool, kind: WindowBackdropKind) {
    let _ = try_apply_inner(hwnd, use_dark_mode, kind);
}

/// C# `WindowBackdropHelper.TryApply` body.
fn try_apply_inner(
    hwnd: isize,
    use_dark_mode: bool,
    kind: WindowBackdropKind,
) -> windows::core::Result<()> {
    let window: HWND = HWND(hwnd as *mut std::ffi::c_void);
    if window.0.is_null() {
        return Ok(());
    }

    // C# `int dark = useDarkMode ? 1 : 0; DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, ...)`.
    // The result is deliberately not propagated: `WindowBackdropHelper.TryApply` wraps
    // every DWM call in one try/catch ("Backdrop is cosmetic. Ignore failures and
    // keep default rendering"), so a failed dark-mode set must not skip the backdrop
    // step below the way an early `?` would.
    let dark: i32 = i32::from(use_dark_mode);
    if let Err(err) = set_attribute(window, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark) {
        tracing::trace!(%err, "immersive dark mode attribute not applied");
    }

    // C# `if (!OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22000)) return;`
    if !is_windows_build_at_least(MICA_BUILD) {
        return Ok(());
    }

    let backdrop_type = match kind {
        WindowBackdropKind::MainWindow => DWMSBT_MAINWINDOW,
        WindowBackdropKind::TransientWindow => DWMSBT_TRANSIENTWINDOW,
    };

    // C# `if (OperatingSystem.IsWindowsVersionAtLeast(10, 0, 22621)) { SYSTEMBACKDROP_TYPE; return; }`
    if is_windows_build_at_least(SYSTEMBACKDROP_BUILD) {
        set_attribute(window, DWMWA_SYSTEMBACKDROP_TYPE, &backdrop_type)?;
        return Ok(());
    }

    // C# `int mica = 1; DwmSetWindowAttribute(hwnd, DWMWA_MICA_EFFECT, ...)`
    let mica: i32 = 1;
    set_attribute(window, DWMWA_MICA_EFFECT, &mica)?;
    Ok(())
}

/// One `DwmSetWindowAttribute(hwnd, attribute, &value, sizeof(int))` call.
fn set_attribute(
    window: HWND,
    attribute: DWMWINDOWATTRIBUTE,
    value: &i32,
) -> windows::core::Result<()> {
    // SAFETY: `value` is a live 4-byte integer and the byte-size argument matches it;
    // DWM only reads the window handle, and a failing call leaves `value` untouched.
    unsafe {
        DwmSetWindowAttribute(
            window,
            attribute,
            std::ptr::from_ref(value).cast(),
            std::mem::size_of::<i32>() as u32,
        )
    }
}

/// `OperatingSystem.IsWindowsVersionAtLeast(10, 0, <build>)`.
///
/// The version helper APIs in the `windows` crate consult the exe manifest rather
/// than the OS, so the build number is read from the registry instead — the same
/// source `RtlGetVersion`-based checks use, truthful on every Windows build without
/// needing a manifest to declare compatibility.
fn is_windows_build_at_least(build: u32) -> bool {
    current_build_number().is_some_and(|current| current >= build)
}

/// `CurrentBuildNumber` of the running OS; `None` when unreadable.
///
/// Read from `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` — the key that
/// actually carries the value; the `HKCU\...\CurrentVersion` tree does not.
/// Stock Windows stores the value as `REG_SZ` ("26100"), so the type and size are
/// learned first and the payload is then read in that type's own representation.
fn current_build_number() -> Option<u32> {
    const CURRENT_BUILD_SUBKEY: &str = "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion";
    const CURRENT_BUILD_VALUE: &str = "CurrentBuildNumber";

    let path: Vec<u16> = CURRENT_BUILD_SUBKEY
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value: Vec<u16> = CURRENT_BUILD_VALUE
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let value_name = PCWSTR(value.as_ptr());

    let mut key = HKEY::default();
    // SAFETY: `path` is a NUL-terminated UTF-16 subkey and `key` is a live out slot.
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(path.as_ptr()),
            None,
            KEY_READ,
            &mut key,
        )
    };
    if opened.is_err() {
        return None;
    }

    // Phase 1: ask only for the value's type and byte size, which decides how the
    // payload is read. A null data buffer writes nothing; `kind` is only an out
    // slot, so its initializer is never read.
    let mut kind = REG_VALUE_TYPE::default();
    let mut byte_len: u32 = 0;
    // SAFETY: `value_name` is a NUL-terminated UTF-16 value name; `kind`/`byte_len`
    // are live out slots and no data buffer is written.
    let queried = unsafe {
        RegQueryValueExW(
            key,
            value_name,
            None,
            Some(&mut kind),
            None,
            Some(&mut byte_len),
        )
    };

    let build = if queried.is_err() {
        None
    } else if kind == REG_DWORD {
        // Phase 2a: the original 4-byte read.
        let mut raw: u32 = 0;
        let mut capacity: u32 = std::mem::size_of::<u32>() as u32;
        // SAFETY: `value_name` is a NUL-terminated UTF-16 value name; `raw`/`capacity`
        // describe a 4-byte buffer.
        let status = unsafe {
            RegQueryValueExW(
                key,
                value_name,
                None,
                None,
                Some((&raw mut raw).cast()),
                Some(&mut capacity),
            )
        };
        (status.is_ok() && capacity == std::mem::size_of::<u32>() as u32).then_some(raw)
    } else if kind == REG_SZ && byte_len > 0 {
        // Phase 2b: stock Windows stores the build number as text, so read the
        // UTF-16 payload, drop the trailing NUL and parse the number.
        read_registry_string(key, value_name, byte_len)
            .and_then(|text| text.trim_matches('\0').trim().parse::<u32>().ok())
    } else {
        None
    };

    // SAFETY: `key` is an open registry handle returned by the call above.
    unsafe {
        let _ = RegCloseKey(key);
    }

    build
}

/// Phase 2b payload read: `byte_len` bytes of UTF-16 text at `name`.
fn read_registry_string(key: HKEY, name: PCWSTR, byte_len: u32) -> Option<String> {
    let mut buffer: Vec<u16> = vec![0; (byte_len as usize).div_ceil(2)];
    let mut capacity = byte_len;
    // SAFETY: `name` is a NUL-terminated UTF-16 value name; `buffer` holds
    // `capacity` writable bytes and is live for the call.
    let status = unsafe {
        RegQueryValueExW(
            key,
            name,
            None,
            None,
            Some(buffer.as_mut_ptr().cast()),
            Some(&mut capacity),
        )
    };
    if status.is_err() {
        return None;
    }

    // The kernel reports how many bytes it actually wrote; an unterminated value
    // simply yields fewer units than the buffer holds.
    let written = (capacity as usize / 2).min(buffer.len());
    String::from_utf16(&buffer[..written]).ok()
}
