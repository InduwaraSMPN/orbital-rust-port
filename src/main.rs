//! Entry point — a thin `#![windows_subsystem]` wrapper around `orbital::ui::run()`.
//!
//! The startup ordering of `Program.cs` + `App.OnStartup` (update/install hook args
//! first, COM STA init, settings/tray/hooks/radial menu startup, then the
//! `ShutdownMode.OnExplicitShutdown` daemon behavior) lives in `ui::run()`, reached
//! through `orbital::start_app`; nothing about it is duplicated here.

// `windows_subsystem` keeps the console window away in release; the windows-manifest
// feature of gpui handles the application manifest.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    #[cfg(feature = "ui")]
    orbital::start_app();

    #[cfg(not(feature = "ui"))]
    eprintln!("orbital: built without the `ui` feature; nothing to run.");
}
