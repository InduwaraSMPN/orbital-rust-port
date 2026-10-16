//! Orbital — Rust port of the C#/WPF original (`orbital-main/`).
//!
//! Layout follows `PORT_PLAN.md` §2. Pure logic and platform modules are
//! feature-independent; the GPUI UI stack is behind the default `ui` feature so
//! `cargo check --no-default-features` can verify logic/platform/services quickly.

pub mod action_presets;
pub mod action_type;
pub mod app;
pub mod backdrop;
pub mod clipboard;
pub mod executor;
pub mod hooks;
pub mod i18n;
pub mod llm;
pub mod placement;
pub mod platform;
pub mod release_flow;
pub mod settings;
pub mod sound;
pub mod theme;

#[cfg(feature = "ui")]
pub mod tray;

#[cfg(feature = "ui")]
pub mod ui;

/// Program.cs entry: Velopack-hook ordering, STA init, then the GPUI app run.
/// Implemented across `app`/`ui` (workstream C/D).
#[cfg(feature = "ui")]
pub fn start_app() {
    ui::run();
}

pub use placement::{
    AnchorContext, Point, PopupPlacementMode, Rect, Size, calculate_position, clamp_to_working_area,
};
pub use settings::{ActionProfile, AppSettings, SettingsStore, create_default_settings};
