//! System tray port of the WinForms NotifyIcon usage in App.xaml.cs — themed icon
//! pick (light/dark asset with fallback), tooltip string, menu
//! (Settings / separator / Restart(+version when an update is pending) / separator /
//! Exit), rebuild on language change, balloon tip for downloaded updates.
//! Windows backend is `Shell_NotifyIconW` via the `gpui-tray` crate (see plan §3).

use std::sync::Arc;
use std::sync::Mutex;

use gpui::{App, Global, Image, ImageFormat};

use crate::i18n;
use crate::ui::bridge::MainThreadBridge;

const ICON_DARK: &[u8] = include_bytes!("../assets/orbit_logo_dark.ico");
const ICON_LIGHT: &[u8] = include_bytes!("../assets/orbit_logo_light.ico");
const ICON_BASE: &[u8] = include_bytes!("../assets/orbit_logo.ico");

/// Cloned into the shared state and the release-check task; the inner handle is an
/// Arc, so a clone is a referential copy, not a second tray icon.
#[derive(Clone)]
pub struct Tray {
    shared: Arc<TrayShared>,
}

pub struct TrayCallbacks {
    pub on_settings: Box<dyn Fn() + Send + Sync + 'static>,
    pub on_restart: Box<dyn Fn() + Send + Sync + 'static>,
    pub on_exit: Box<dyn Fn() + Send + Sync + 'static>,
}

struct TrayShared {
    bridge: Arc<MainThreadBridge>,
    callbacks: Arc<TrayCallbacks>,
    /// C# `_notifyIcon.Text`, refreshed with the active language.
    tooltip: Mutex<String>,
    /// C# `_pendingUpdate.TargetFullRelease.Version`, when one is downloaded.
    pending_version: Mutex<Option<String>>,
    /// The resolved theme the icon is drawn for.
    resolved_theme: Mutex<String>,
}

/// The native tray handle. `gpui_tray::Tray` is bound to the thread that built it,
/// so it lives behind a GPUI global (main-thread only) rather than inside the shared
/// handle every other thread carries.
#[derive(Default)]
pub(crate) struct TrayGlobal {
    handle: Option<gpui_tray::Tray>,
}

impl Global for TrayGlobal {}

impl Tray {
    pub fn new(callbacks: TrayCallbacks) -> Self {
        let bridge = MainThreadBridge::global().unwrap_or_else(|| {
            tracing::warn!("main-thread bridge is missing; tray effects will be dropped");
            MainThreadBridge::uninstalled()
        });

        let shared = Arc::new(TrayShared {
            bridge,
            callbacks: Arc::new(callbacks),
            tooltip: Mutex::new(i18n::get("Str_TrayTooltip")),
            pending_version: Mutex::new(None),
            resolved_theme: Mutex::new(crate::ui::common::resolve_system_theme()),
        });

        // C# `InitializeTrayIcon` creates the icon on the dispatcher thread.
        let creation = Arc::clone(&shared);
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            creation.create(cx);
        }));

        Self { shared }
    }

    /// The menu's callbacks, reachable from the GPUI action handlers.
    pub(crate) fn callbacks(&self) -> Arc<TrayCallbacks> {
        Arc::clone(&self.shared.callbacks)
    }

    /// `RebuildTrayMenu` — re-reads localized strings and the pending-update label.
    pub fn rebuild_menu(&self, pending_version: Option<&str>) {
        *lock(&self.shared.pending_version) = pending_version.map(str::to_string);
        *lock(&self.shared.tooltip) = i18n::get("Str_TrayTooltip");

        let shared = Arc::clone(&self.shared);
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            shared.apply_tooltip(cx);
            shared.refresh_menu(cx);
        }));
    }

    /// `UpdateTrayIcon` — light/dark themed icon with base-asset fallback.
    pub fn update_icon(&self, resolved_theme: &str) {
        *lock(&self.shared.resolved_theme) = resolved_theme.to_string();

        let shared = Arc::clone(&self.shared);
        let resolved = resolved_theme.to_string();
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            shared.set_icon(cx, &resolved);
        }));
    }

    /// `ShowBalloonTip(8000, ...)`.
    ///
    /// gpui-tray exposes no balloon API (its Windows backend owns the
    /// `Shell_NotifyIconW` handle), so the tip is delivered as a Windows toast
    /// notification through GPUI's own `SystemNotification`. `PORT_STATUS.md`
    /// (2026-10-12, workstream D) records the mechanism difference.
    pub fn show_balloon(&self, title: &str, message: &str) {
        let title = title.to_string();
        let message = message.to_string();

        let shared = Arc::clone(&self.shared);
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            cx.show_system_notification(gpui::SystemNotification {
                tag: "orbital-update".into(),
                title: title.into(),
                body: message.into(),
                actions: Vec::new(),
            });
        }));
    }
}

impl TrayShared {
    /// C# `InitializeTrayIcon` body.
    fn create(self: &Arc<Self>, cx: &mut App) {
        if tray_global(cx).handle.is_some() {
            return;
        }

        let shared = Arc::clone(self);
        let menu_builder = move |cx: &mut App| shared.build_menu(cx);
        // C# `UpdateTrayIcon`'s last fallback is `SystemIcons.Application`; a stock
        // icon has no GPUI equivalent, so a fully transparent pixel stands in.
        let icon = self.load_icon(cx).unwrap_or_else(stock_icon);

        let tray = gpui_tray::Tray::builder()
            .icon(icon)
            .tooltip(self.tooltip())
            // C# `_notifyIcon.DoubleClick += (s, e) => Dispatcher.Invoke(OpenSettingsWindow)`
            .on_activate(crate::ui::tray_actions::TraySettings)
            .menu(menu_builder)
            .build(cx);

        match tray {
            Ok(tray) => tray_global_mut(cx).handle = Some(tray),
            Err(error) => tracing::warn!(%error, "could not create the system tray icon"),
        }
    }

    /// C# `RebuildTrayMenu`'s menu body.
    fn build_menu(&self, _cx: &App) -> Vec<gpui::MenuItem> {
        let pending = lock(&self.pending_version).clone();
        // C# `$"{Loc.Get("Str_TrayRestart")} (v{version})"`
        let restart_label = match &pending {
            Some(version) => format!("{} (v{})", i18n::get("Str_TrayRestart"), version),
            None => i18n::get("Str_TrayRestart"),
        };

        vec![
            gpui::MenuItem::action(
                i18n::get("Str_TraySettings"),
                crate::ui::tray_actions::TraySettings,
            ),
            gpui::MenuItem::separator(),
            gpui::MenuItem::action(restart_label, crate::ui::tray_actions::TrayRestart),
            gpui::MenuItem::separator(),
            gpui::MenuItem::action(i18n::get("Str_TrayExit"), crate::ui::tray_actions::TrayExit),
        ]
    }

    fn apply_tooltip(&self, cx: &mut App) {
        let tooltip = self.tooltip();
        if let Some(tray) = tray_handle(cx) {
            let _ = tray.set_tooltip(Some(tooltip), cx);
        }
    }

    fn refresh_menu(&self, cx: &mut App) {
        if let Some(tray) = tray_handle(cx) {
            let _ = tray.refresh_menu(cx);
        }
    }

    fn set_icon(&self, cx: &mut App, resolved_theme: &str) {
        // C# `UpdateTrayIcon`: themed asset first, base asset as the fallback.
        let themed = if resolved_theme == "Light" {
            ICON_LIGHT
        } else {
            ICON_DARK
        };
        let Some(icon) = load_gpui_icon(themed, cx).or_else(|| load_gpui_icon(ICON_BASE, cx))
        else {
            return;
        };
        if let Some(tray) = tray_handle(cx) {
            let _ = tray.set_icon(Some(icon), cx);
        }
    }

    fn tooltip(&self) -> String {
        lock(&self.tooltip).clone()
    }

    fn load_icon(&self, cx: &App) -> Option<gpui_tray::Icon> {
        let resolved = lock(&self.resolved_theme).clone();
        let themed = if resolved == "Light" {
            ICON_LIGHT
        } else {
            ICON_DARK
        };
        load_gpui_icon(themed, cx).or_else(|| load_gpui_icon(ICON_BASE, cx))
    }
}

/// C# `SystemIcons.Application` — the last-resort tray icon.
fn stock_icon() -> gpui_tray::Icon {
    gpui_tray::Icon::from_rgba(vec![0, 0, 0, 0], 1, 1).expect("a 1x1 icon is always valid")
}

/// Decodes an embedded `.ico` through GPUI's own image pipeline (`image` crate with
/// the `ico` feature) and normalizes it to the RGBA8 the native backends take.
fn load_gpui_icon(bytes: &[u8], cx: &App) -> Option<gpui_tray::Icon> {
    let image = Image::from_bytes(ImageFormat::Ico, bytes.to_vec());
    gpui_tray::Icon::from_gpui(&image, cx).ok()
}

fn tray_global(cx: &App) -> &TrayGlobal {
    cx.global::<TrayGlobal>()
}

fn tray_global_mut(cx: &mut App) -> &mut TrayGlobal {
    cx.global_mut::<TrayGlobal>()
}

/// A clone of the native tray, for callers that must not hold the global borrowed
/// while taking `&mut App`.
pub(crate) fn tray_handle(cx: &App) -> Option<gpui_tray::Tray> {
    tray_global(cx).handle.clone()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
