//! Shared Win32 + platform helpers (window class, cursor pos, process name, DPI,
//! DPAPI, registry autostart) used by hooks/clipboard/app/ui.

pub mod win32;

#[cfg(target_os = "windows")]
pub mod dpapi;

#[cfg(target_os = "windows")]
pub mod registry;

#[cfg(target_os = "windows")]
pub mod dpi;
