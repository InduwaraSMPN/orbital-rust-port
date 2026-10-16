//! SettingsWindow port — GPUI + gpui-kit components: provider table
//! (openai 4 / openrouter 14 `:free` / custom editable), model/URL/key + last-4
//! status, hotkey capture + pack bits, 2×2 triggers, actions table with
//! up/down/delete/edit/import (merge or replace dialog)/export, version + GitHub
//! link, release-check line.
//!
//! Every field mutates the shared `AppSettings` and saves immediately, exactly like
//! `SettingsManager.CurrentSettings` + `SaveSettings()` in the code-behind.

use std::sync::Arc;
use std::sync::RwLock;

use gpui::{App, Context, Entity, Render, Window, WindowHandle, div, prelude::*, px, size};

use crate::i18n;
use crate::placement::PopupPlacementMode;
use crate::settings::{ActionProfile, AppSettings, SettingsStore, action_type_of};

use super::action_edit::{ActionEditDialog, EditOrigin};
use super::bridge::MainThreadBridge;
use super::common;
use super::tooltip;

/// C# `SettingsWindow.Providers`, in declaration order.
const PROVIDERS: &[(&str, &str, &[&str])] = &[
    (
        "openai",
        "https://api.openai.com/v1",
        &["gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "gpt-3.5-turbo"],
    ),
    (
        "openrouter",
        "https://openrouter.ai/api/v1",
        &[
            // Free-model auto routing (default)
            "openrouter/free",
            // Paid
            "openai/gpt-4o-mini",
            "openai/gpt-4o",
            "anthropic/claude-3.5-sonnet",
            "anthropic/claude-3-haiku",
            "google/gemini-flash-1.5",
            // Free (:free tag)
            "meta-llama/llama-3.1-8b-instruct:free",
            "meta-llama/llama-3.3-70b-instruct:free",
            "deepseek/deepseek-r1:free",
            "deepseek/deepseek-chat-v3-0324:free",
            "google/gemini-2.0-flash-exp:free",
            "qwen/qwen2.5-vl-72b-instruct:free",
        ],
    ),
    ("custom", "", &[]),
];

/// C# `Providers[tag]` label key.
fn provider_label_key(tag: &str) -> &'static str {
    match tag {
        "openrouter" => "Str_ProviderOpenRouter",
        "custom" => "Str_ProviderCustom",
        _ => "Str_ProviderOpenAI",
    }
}

fn provider_url(tag: &str) -> String {
    PROVIDERS
        .iter()
        .find(|(name, _, _)| *name == tag)
        .map(|(_, url, _)| (*url).to_string())
        .unwrap_or_default()
}

fn provider_models(tag: &str) -> Vec<String> {
    PROVIDERS
        .iter()
        .find(|(name, _, _)| *name == tag)
        .map(|(_, _, models)| models.iter().map(|model| (*model).to_string()).collect())
        .unwrap_or_default()
}

/// C# `SettingsWindow.Providers[tag]` label key.
struct SettingsWindowShared {
    bridge: Arc<MainThreadBridge>,
    store: SettingsStore,
    settings: Arc<RwLock<AppSettings>>,
}

pub struct SettingsWindow {
    inner: Arc<SettingsWindowShared>,
}

/// The modal window's form state.
pub(crate) struct SettingsView {
    shared: Arc<SettingsWindowShared>,
    pub(crate) theme_name: String,
    dialog: ActionEditDialog,

    // API panel
    provider: String,
    base_url: Entity<gpui_kit::component::input::InputState>,
    model: Entity<gpui_kit::component::input::InputState>,
    api_key: Entity<gpui_kit::component::input::InputState>,
    api_key_status: String,
    auto_close_seconds: Entity<gpui_kit::component::input::InputState>,

    // Hotkey
    hotkey_ctrl: bool,
    hotkey_alt: bool,
    hotkey_shift: bool,
    hotkey_key: Entity<gpui_kit::component::input::InputState>,
    hotkey_focus: gpui::FocusHandle,

    /// GPUI cancels a subscription when its handle is dropped, so the input
    /// subscriptions the C# XAML event handlers need are kept here for as long
    /// as the view lives.
    #[allow(dead_code)]
    subscriptions: Vec<gpui::Subscription>,

    // Actions
    actions: Vec<ActionProfile>,
    selected: Option<usize>,

    // Version / update line
    version_label: String,
    update_available: Option<String>,
    update_url: Option<String>,
}

impl SettingsWindow {
    /// Builds the controller. `ui::run` creates it once and hands it to the tray and
    /// to the `UiHooks` implementation.
    pub(crate) fn new(
        bridge: Arc<MainThreadBridge>,
        store: SettingsStore,
        settings: Arc<RwLock<AppSettings>>,
    ) -> Self {
        Self {
            inner: Arc::new(SettingsWindowShared {
                bridge,
                store,
                settings,
            }),
        }
    }

    /// C# `win.ShowDialog()` — modal; on close the app re-reads settings.
    pub fn open(&self) {
        let inner = Arc::clone(&self.inner);
        let bridge = Arc::clone(&inner.bridge);
        bridge.post(Box::new(move |cx| {
            inner.show_modal(cx);
        }));
    }
}

impl SettingsWindowShared {
    fn show_modal(self: &Arc<Self>, cx: &mut App) {
        let settings = common::settings_snapshot(&self.settings);
        let theme_name = common::resolved_theme_name(&settings);
        let dialog = ActionEditDialog::new(
            Arc::clone(&self.bridge),
            Arc::clone(&self.settings),
            self.store.clone(),
        );

        let title = "Orbital Settings".to_string();
        let options = common::dialog_window_options(
            gpui::Bounds::centered(None, size(px(700.0), px(720.0)), cx),
            Some(title.clone()),
        );

        let shared = Arc::clone(self);
        let window = match cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let base_url = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(settings.api_base_url.clone())
                });
                // C# `SettingsWindow.xaml.cs:234` — the base URL is read-only unless
                // the provider is `custom`.
                base_url.update(cx, |state, cx| {
                    state.set_readonly(detect_provider(&settings.api_base_url) != "custom", cx);
                });
                let model = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(settings.model_name.clone())
                });
                let api_key = cx.new(|cx| {
                    let mut state = gpui_kit::component::input::InputState::new(window, cx);
                    state.set_masked(true, window, cx);
                    state
                });
                let auto_close_seconds = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(settings.popup_auto_close_seconds.to_string())
                });
                let hotkey_key = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(vk_to_label(settings.hotkey_virtual_key))
                });

                // C# `PopupAutoCloseSecondsBox_LostFocus` — GPUI's text input has no
                // lost-focus event, so the two `InputEvent`s that end an edit are the
                // commit point.
                let auto_close_subscription = cx.subscribe(
                    &auto_close_seconds,
                    |view: &mut SettingsView, _, event, cx| match event {
                        gpui_kit::component::input::InputEvent::PressEnter { .. }
                        | gpui_kit::component::input::InputEvent::Blur => {
                            view.commit_auto_close_seconds(cx);
                        }
                        _ => {}
                    },
                );

                // C# `ApiKeyBox_KeyDown` — Enter runs `SaveApiKey_Click`.
                let api_key_subscription =
                    cx.subscribe(&api_key, |view: &mut SettingsView, _, event, cx| {
                        if let gpui_kit::component::input::InputEvent::PressEnter { .. } = event {
                            view.save_api_key_on_enter(cx);
                        }
                    });

                SettingsView {
                    shared: Arc::clone(&shared),
                    theme_name: theme_name.clone(),
                    dialog,
                    provider: detect_provider(&settings.api_base_url),
                    base_url,
                    model,
                    api_key,
                    api_key_status: api_key_status(&settings, &shared.store),
                    auto_close_seconds,
                    hotkey_ctrl: settings.hotkey_modifiers & 0x02 != 0,
                    hotkey_alt: settings.hotkey_modifiers & 0x01 != 0,
                    hotkey_shift: settings.hotkey_modifiers & 0x04 != 0,
                    hotkey_key,
                    hotkey_focus: cx.focus_handle(),
                    subscriptions: vec![auto_close_subscription, api_key_subscription],
                    actions: settings.actions.clone(),
                    selected: None,
                    version_label: format!("v{}", env!("CARGO_PKG_VERSION")),
                    update_available: None,
                    update_url: None,
                }
            })
        }) {
            Ok(window) => window,
            Err(error) => {
                tracing::error!(%error, "could not open the settings window");
                return;
            }
        };

        register_settings_window(window);
        // C# `OnSourceInitialized` → `WindowBackdropHelper.TryApply`.
        common::apply_window_backdrop(
            &title,
            theme_name != "Light",
            crate::backdrop::WindowBackdropKind::MainWindow,
        );

        // C# `_ = CheckForUpdateAsync();`
        self.spawn_update_check();
    }

    /// C# `SettingsWindow.CheckForUpdateAsync`.
    ///
    /// The check runs on the app's long-lived runtime: dropping a locally built one
    /// aborts the task it just spawned, so the port pulls the runtime the startup path
    /// owns out of `UiState` instead.
    fn spawn_update_check(&self) {
        let version = format!("v{}", env!("CARGO_PKG_VERSION"));

        super::post(move |cx| {
            let runtime = Arc::clone(&cx.global::<super::UiState>().shared.runtime);
            runtime.spawn(async move {
                // rustls with the `rustls-no-provider` feature has no process-level
                // default provider, so one must be installed before the first HTTPS
                // call in a fresh process.
                crate::llm::ensure_tls_provider();

                let client = match reqwest::Client::builder()
                    .user_agent("Orbital-UpdateCheck/1.0")
                    .timeout(std::time::Duration::from_secs(30))
                    .build()
                {
                    Ok(client) => client,
                    Err(_) => return,
                };

                let response = client
                    .get("https://api.github.com/repos/CrowKing63/Orbital/releases/latest")
                    .send()
                    .await;
                let Ok(response) = response else { return };
                if !response.status().is_success() {
                    return;
                }
                let body = response.text().await.unwrap_or_default();
                let parsed: serde_json::Value = match serde_json::from_str(&body) {
                    Ok(value) => value,
                    Err(_) => return,
                };
                let tag = parsed
                    .get("tag_name")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .trim_start_matches('v')
                    .to_string();
                let url = parsed
                    .get("html_url")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();

                // C# `Version.TryParse` + `remote > local`.
                let (Some(remote), Some(local)) = (parse_version(&tag), parse_version(&version))
                else {
                    return;
                };
                if remote <= local {
                    return;
                }

                super::post(move |cx| {
                    if let Some(handle) = current_settings_window() {
                        let tag = tag.clone();
                        let url = url.clone();
                        let _ = handle.update(cx, |view: &mut SettingsView, _, cx| {
                            view.update_available = Some(tag);
                            view.update_url = Some(url);
                            cx.notify();
                        });
                    }
                });
            });
        });
    }
}

/// C# `Version.TryParse` — (major, minor, build).
fn parse_version(raw: &str) -> Option<(u64, u64, u64)> {
    let raw = raw.trim().trim_start_matches('v');
    let mut parts = raw.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let build = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, build))
}

impl SettingsView {
    fn settings(&self) -> Arc<RwLock<AppSettings>> {
        Arc::clone(&self.shared.settings)
    }

    fn save(&self) {
        let settings = common::settings_snapshot(&self.shared.settings);
        if let Err(error) = self.shared.store.save(&settings) {
            tracing::warn!(%error, "could not save settings from the settings window");
        }
    }

    fn input_value(state: &Entity<gpui_kit::component::input::InputState>, cx: &App) -> String {
        state.read(cx).text().to_string()
    }

    fn set_input(
        state: &Entity<gpui_kit::component::input::InputState>,
        value: impl Into<gpui::SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let value = value.into();
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn provider_models(&self) -> Vec<String> {
        provider_models(&self.provider)
    }

    /// C# `SaveApiKey_Click` — base URL, model, then the key when one was typed.
    fn save_api_key(&mut self, window: &mut Window, cx: &mut App) {
        let base_url = Self::input_value(&self.base_url, cx);
        let model = Self::input_value(&self.model, cx);
        let key = Self::input_value(&self.api_key, cx);

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            let base_url = base_url.trim();
            if !base_url.is_empty() {
                settings.api_base_url = base_url.to_string();
            }
            let model = model.trim();
            if !model.is_empty() {
                settings.model_name = model.to_string();
            }
        }

        if !key.trim().is_empty() {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            SettingsStore::set_api_key(&mut settings, &key);
            self.api_key_status = api_key_status(&settings, &self.shared.store);
            drop(settings);
            Self::set_input(&self.api_key, "", window, cx);
        }

        self.save();
    }

    /// C# `ApiKeyBox_KeyDown` — Enter runs `SaveApiKey_Click`.
    ///
    /// GPUI's input publishes `InputEvent::PressEnter` instead of a key-down hook, and
    /// `save_api_key` needs a `&mut Window`, which a subscription closure does not
    /// have. The write is therefore deferred to the end of the effect cycle, when the
    /// window is back on the app and reachable through its handle.
    fn save_api_key_on_enter(&mut self, cx: &mut Context<SettingsView>) {
        cx.defer(move |cx| {
            let Some(handle) = current_settings_window() else {
                return;
            };
            if let Err(error) = handle.update(cx, |view: &mut SettingsView, window, cx| {
                view.save_api_key(window, &mut *cx);
            }) {
                tracing::debug!(%error, "the API-key save could not run from the Enter key");
            }
        });
    }

    /// C# `PopupAutoCloseSecondsBox_LostFocus` → `TryApplyPopupAutoCloseSeconds`.
    ///
    /// The source parses the box on focus loss, keeps the value when it is an int
    /// `>= 1` and reverts the box otherwise; GPUI's input has no lost-focus event, so
    /// `PressEnter` and `Blur` — the two `InputEvent`s that end an edit — carry it.
    fn commit_auto_close_seconds(&mut self, cx: &mut Context<SettingsView>) {
        let raw = Self::input_value(&self.auto_close_seconds, cx);
        let Ok(seconds) = raw.trim().parse::<i32>() else {
            self.revert_auto_close_seconds(cx);
            return;
        };
        if seconds < 1 {
            self.revert_auto_close_seconds(cx);
            return;
        }

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            settings.popup_auto_close_seconds = seconds;
        }
        self.save();
        tracing::debug!(seconds, "applied the popup auto-close seconds");
    }

    /// C# the revert in `PopupAutoCloseSecondsBox_LostFocus`:
    /// `PopupAutoCloseSecondsBox.Text = CurrentSettings.PopupAutoCloseSeconds.ToString()`.
    fn revert_auto_close_seconds(&self, cx: &mut Context<SettingsView>) {
        let stored = common::settings_snapshot(&self.settings())
            .popup_auto_close_seconds
            .to_string();
        let entity = self.auto_close_seconds.clone();
        cx.defer(move |cx| {
            let Some(handle) = current_settings_window() else {
                return;
            };
            if let Err(error) = handle.update(cx, |_: &mut SettingsView, window, cx| {
                SettingsView::set_input(&entity, stored, window, cx);
            }) {
                tracing::debug!(%error, "the auto-close seconds box could not be reverted");
            }
        });
    }

    /// C# `SaveHotkeySettings`.
    ///
    /// Every mutation point (the three modifier checkboxes, the key capture and the
    /// clear button) ends here, and the source's method ends with
    /// `App.ApplyHotkeySettings()` — the live manager is pushed on every edit, not only
    /// when the window closes.
    fn save_hotkey(&mut self, _window: &mut Window, cx: &mut App) {
        let mut modifiers = 0u32;
        if self.hotkey_ctrl {
            modifiers |= 0x02;
        }
        if self.hotkey_alt {
            modifiers |= 0x01;
        }
        if self.hotkey_shift {
            modifiers |= 0x04;
        }
        let virtual_key = self.captured_vk(cx);

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            settings.hotkey_modifiers = modifiers;
            settings.hotkey_virtual_key = virtual_key;
        }
        self.save();
        self.push_hotkey();
    }

    /// C# `App.ApplyHotkeySettings` — push the stored hotkey into the live hook
    /// manager. The manager lives in the main-thread global, so the push is marshalled
    /// through the bridge.
    fn push_hotkey(&self) {
        super::post(move |cx| {
            let state = cx.global::<super::UiState>();
            let snapshot = common::settings_snapshot(&state.shared.settings);
            if let Some(hooks) = state.app.as_ref().and_then(|app| app.hooks.as_ref()) {
                hooks.set_hotkey(snapshot.hotkey_modifiers, snapshot.hotkey_virtual_key);
            }
        });
    }

    fn captured_vk(&self, cx: &App) -> u32 {
        let label = Self::input_value(&self.hotkey_key, cx);
        vk_from_label(&label).unwrap_or(0)
    }

    /// C# `UpdateHotkeyHint`.
    fn hotkey_hint(&self, cx: &App) -> String {
        let vk = self.captured_vk(cx);
        if vk == 0 {
            return i18n::get("Str_HotkeyNone");
        }
        let mut parts = Vec::new();
        if self.hotkey_ctrl {
            parts.push("Ctrl".to_string());
        }
        if self.hotkey_alt {
            parts.push("Alt".to_string());
        }
        if self.hotkey_shift {
            parts.push("Shift".to_string());
        }
        parts.push(vk_to_label(vk));
        i18n::get_fmt("Str_HotkeyActive", &[&parts.join(" + ")])
    }

    /// C# `HotkeyKeyBox_PreviewKeyDown`.
    fn capture_hotkey_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut App,
    ) {
        if event.keystroke.key == "backspace" || event.keystroke.key == "delete" {
            Self::set_input(&self.hotkey_key, "", window, cx);
            self.save_hotkey(window, cx);
            return;
        }

        // C# ignores standalone modifiers and the Windows keys.
        let modifiers = &event.keystroke.modifiers;
        if modifiers.control || modifiers.alt || modifiers.platform {
            return;
        }

        if let Some(vk) = vk_from_key(&event.keystroke.key) {
            Self::set_input(&self.hotkey_key, vk_to_label(vk), window, cx);
            self.save_hotkey(window, cx);
        }
    }

    // ── Actions table ────────────────────────────────────────────────────────

    /// C# `RefreshActionList`.
    fn refresh_actions(&mut self) {
        let settings = common::settings_snapshot(&self.shared.settings);
        self.actions = settings.actions.clone();
    }

    /// C# `MoveActionUp_Click` / `MoveActionDown_Click`.
    fn move_action(&mut self, delta: isize, _cx: &mut App) {
        let Some(selected) = self.selected else {
            return;
        };
        let target = selected as isize + delta;
        if target < 0 || target as usize >= self.actions.len() {
            return;
        }
        let target = target as usize;

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            let action = settings.actions.remove(selected);
            settings.actions.insert(target, action);
        }
        self.save();
        self.refresh_actions();
        self.selected = Some(target);
    }

    /// C# `DeleteAction_Click`.
    fn delete_action(&mut self, _cx: &mut App) {
        let Some(selected) = self.selected else {
            return;
        };
        let Some(action) = self.actions.get(selected) else {
            return;
        };
        // C# MessageBox.Show(Str_DeleteActionConfirm{name}, Str_AppTitle, YesNo, Question)
        if !common::yes_no_box(
            "Str_AppTitle",
            &i18n::get_fmt("Str_DeleteActionConfirm", &[&action.name]),
        ) {
            return;
        }

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            settings.actions.remove(selected);
        }
        self.save();
        self.refresh_actions();
        self.selected = None;
    }

    /// C# `ExportActions_Click`.
    fn export_actions(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Orbital Action Pack (*.json)", &["json"])
            .add_filter("All Files (*.*)", &["*"])
            .set_file_name("orbital-actions.json")
            .set_title("Export Action Pack")
            .save_file();
        let Some(path) = picked else {
            return;
        };

        let settings = common::settings_snapshot(&self.shared.settings);
        match SettingsStore::export_actions(&settings, &path) {
            Ok(()) => common::info_box(
                "Str_AppTitle",
                &i18n::get_fmt("Str_ExportSuccess", &[&path.display().to_string()]),
            ),
            Err(error) => common::error_box("Str_ExportFailed", &error),
        }
    }

    /// C# `ImportActions_Click`.
    fn import_actions(&mut self) {
        let picked = rfd::FileDialog::new()
            .add_filter("Orbital Action Pack (*.json)", &["json"])
            .add_filter("All Files (*.*)", &["*"])
            .set_title("Import Action Pack")
            .pick_file();
        let Some(path) = picked else {
            return;
        };

        let replace = match common::import_mode_box() {
            common::ImportModeChoice::Replace => true,
            common::ImportModeChoice::Merge => false,
            common::ImportModeChoice::Abort => return,
        };

        let settings = self.settings();
        let mut settings = common::settings_write(&settings);
        match self
            .shared
            .store
            .import_actions(&path, replace, &mut settings)
        {
            Ok(()) => {
                drop(settings);
                self.refresh_actions();
                // C# `replaceExisting ? Str_ImportSuccessReplace : Str_ImportSuccessMerge`
                let key = if replace {
                    "Str_ImportSuccessReplace"
                } else {
                    "Str_ImportSuccessMerge"
                };
                common::info_box("Str_AppTitle", &i18n::get(key));
            }
            Err(error) => common::error_box("Str_ImportFailed", &error),
        }
    }

    /// C# `SaveAndClose_Click` followed by the `OpenSettingsWindow` rebuild sequence.
    fn save_and_close(&mut self, window: &mut Window, cx: &mut App) {
        self.save_api_key(window, cx);
        self.save();
        if let Some(handle) = current_settings_window() {
            clear_settings_window();
            let _ = handle.update(cx, |_: &mut SettingsView, window: &mut Window, _| {
                window.remove_window();
            });
        }
        // C# RebuildActionExecutor(); ApplyHotkeySettings(); _radialMenu.UpdateActionExecutor(...)
        super::rebuild_after_settings(cx);
    }
}

/// C# `DetectProvider`.
fn detect_provider(url: &str) -> String {
    if url.contains("openrouter.ai") {
        "openrouter".to_string()
    } else if url.contains("openai.com") {
        "openai".to_string()
    } else {
        "custom".to_string()
    }
}

/// C# `string.IsNullOrEmpty(key) ? Str_ApiKeyNotConfigured : Str_ApiKeySaved{last4}`.
fn api_key_status(settings: &AppSettings, _store: &SettingsStore) -> String {
    let key = SettingsStore::get_api_key(settings);
    if key.is_empty() {
        i18n::get("Str_ApiKeyNotConfigured")
    } else {
        let last4 = if key.len() >= 4 {
            &key[key.len() - 4..]
        } else {
            key.as_str()
        };
        i18n::get_fmt("Str_ApiKeySaved", &[last4])
    }
}

/// C# `VkToLabel` — Space/Enter/Esc/Tab get dedicated labels, Windows key names
/// stay in English.
fn vk_to_label(vk: u32) -> String {
    match vk {
        0 => String::new(),
        0x20 => "Space".to_string(),
        0x0D => "Enter".to_string(),
        0x1B => "Esc".to_string(),
        0x09 => "Tab".to_string(),
        other => key_name(other),
    }
}

/// C# `KeyInterop.VirtualKeyFromKey` for the keys the hotkey capture accepts.
fn vk_from_key(key: &str) -> Option<u32> {
    // GPUI reports the letter of a keypress lowercase (`Key.A` is "Key.A" in WPF but
    // "a" here), so a single ASCII character is upper-cased the way the WPF
    // `VirtualKeyFromKey` mapping does for letters, and used as-is for digits.
    if let Some(ch) = single_char(key)
        && ch.is_ascii_alphanumeric()
    {
        return Some(u32::from(ch.to_ascii_uppercase()));
    }

    let vk = match key.to_ascii_lowercase().as_str() {
        "space" => 0x20,
        "enter" | "return" => 0x0D,
        "escape" => 0x1B,
        "tab" => 0x09,
        "backspace" => 0x08,
        "delete" => 0x2E,
        "insert" => 0x2D,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        "left" => 0x25,
        "up" => 0x26,
        "right" => 0x27,
        "down" => 0x28,
        "f1" => 0x70,
        "f2" => 0x71,
        "f3" => 0x72,
        "f4" => 0x73,
        "f5" => 0x74,
        "f6" => 0x75,
        "f7" => 0x76,
        "f8" => 0x77,
        "f9" => 0x78,
        "f10" => 0x79,
        "f11" => 0x7A,
        "f12" => 0x7B,
        _ => return None,
    };
    Some(vk)
}

fn single_char(key: &str) -> Option<char> {
    let mut chars = key.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

/// C# `KeyInterop.KeyFromVirtualKey(...).ToString()`.
fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => ((vk as u8) as char).to_string(),
        0x70..=0x7B => format!("F{}", vk - 0x6F),
        _ => format!("VK_{vk:02X}"),
    }
}

/// C# `VkToLabel`'s inverse, used when reading the captured key back out.
fn vk_from_label(label: &str) -> Option<u32> {
    match label {
        "" => Some(0),
        "Space" => Some(0x20),
        "Enter" => Some(0x0D),
        "Esc" => Some(0x1B),
        "Tab" => Some(0x09),
        other => match single_char(other) {
            Some(ch) if ch.is_ascii_uppercase() || ch.is_ascii_digit() => Some(u32::from(ch)),
            _ => vk_from_key(other),
        },
    }
}

impl Render for SettingsView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = self.theme_name.clone();

        let mut root = common::window_root(&theme_name).p(px(24.0)).gap(px(18.0));

        // ── Title ────────────────────────────────────────────────────────────
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(10.0))
                .child(
                    div()
                        .size(px(9.0))
                        .rounded_full()
                        .bg(common::theme_color(&theme_name, "AccentCyColor")),
                )
                .child(
                    div()
                        .text_size(px(20.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(common::theme_color(&theme_name, "TitleFgBrush"))
                        .child(i18n::get("Str_AppTitle")),
                )
                .child(
                    div()
                        .text_size(px(15.0))
                        .text_color(common::theme_color(&theme_name, "SubtitleFgBrush"))
                        .child(i18n::get("Str_Settings")),
                ),
        );

        // ── API Settings panel ───────────────────────────────────────────────
        root = root.child(render_api_panel(self, &theme_name, cx));

        // ── General panel ────────────────────────────────────────────────────
        root = root.child(render_general_panel(self, &theme_name, cx));

        // ── Appearance panel ─────────────────────────────────────────────────
        root = root.child(render_appearance_panel(self, &theme_name, cx));

        // ── Keyboard shortcut panel ──────────────────────────────────────────
        root = root.child(render_hotkey_panel(self, &theme_name, cx));

        // ── Triggers panel ───────────────────────────────────────────────────
        root = root.child(render_triggers_panel(self, &theme_name, cx));

        // ── Actions panel ────────────────────────────────────────────────────
        root = root.child(render_actions_panel(self, &theme_name, cx));

        // ── Bottom buttons ───────────────────────────────────────────────────
        root = root.child(render_bottom_buttons(self, &theme_name, cx));

        root
    }
}

fn render_api_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let mut panel = common::panel_with_padding(theme_name, 16.0).gap(px(12.0));
    panel = panel.child(common::section_header(
        theme_name,
        i18n::get("Str_ApiSettings"),
    ));

    // Provider chips (C# ProviderBox, matched by Tag).
    let provider_options: Vec<(String, String)> = PROVIDERS
        .iter()
        .map(|(tag, _, _)| ((*tag).to_string(), i18n::get(provider_label_key(tag))))
        .collect();
    panel = panel.child(
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(common::field_label(theme_name, i18n::get("Str_Provider")))
            .child(common::chip_row(
                "provider-chips",
                theme_name,
                &provider_options,
                view.provider.clone(),
                cx.listener(|view, tag: &String, window, cx| {
                    // C# ProviderBox_SelectionChanged.
                    view.provider = tag.clone();
                    let models = view.provider_models();
                    let url = provider_url(&view.provider);

                    {
                        let settings = view.settings();
                        let mut settings = common::settings_write(&settings);
                        if view.provider != "custom" {
                            settings.api_base_url = url.clone();
                        }
                    }
                    view.save();

                    if view.provider != "custom" {
                        SettingsView::set_input(&view.base_url, url, window, cx);
                    }
                    // C# `BaseUrlBox.IsReadOnly = (providerTag != "custom")`.
                    view.base_url.update(cx, |state, cx| {
                        state.set_readonly(view.provider != "custom", cx);
                    });
                    // C# `UpdateModelList(tag, info.Models.Length > 0 ? info.Models[0] : prevModel)`
                    let next = models
                        .first()
                        .cloned()
                        .unwrap_or_else(|| SettingsView::input_value(&view.model, cx));
                    SettingsView::set_input(&view.model, next, window, cx);
                }),
            )),
    );

    // Base URL + model fields side by side.
    let base_url_field = common::labeled(
        theme_name,
        i18n::get("Str_ApiBaseUrl"),
        div().child(gpui_kit::component::input::Input::new(&view.base_url)),
    );
    let model_options: Vec<(String, String)> = view
        .provider_models()
        .iter()
        .map(|model| (model.clone(), model.clone()))
        .collect();
    let model_field = common::labeled(
        theme_name,
        i18n::get("Str_Model"),
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(gpui_kit::component::input::Input::new(&view.model))
            .child(common::chip_row(
                "model-chips",
                theme_name,
                &model_options,
                SettingsView::input_value(&view.model, cx),
                cx.listener(|view, model: &String, window, cx| {
                    SettingsView::set_input(&view.model, model.clone(), window, cx);
                }),
            )),
    );

    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .gap(px(12.0))
            .items_start()
            .child(div().flex_1().child(base_url_field))
            .child(div().flex_1().child(model_field)),
    );

    // API key + Save, then the status line.
    panel = panel.child(
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(common::field_label(theme_name, i18n::get("Str_ApiKey")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_1()
                            .child(gpui_kit::component::input::Input::new(&view.api_key)),
                    )
                    .child(div().w(px(72.0)).ml(px(8.0)).child(common::primary_button(
                        "save-api-key",
                        theme_name,
                        i18n::get("Str_Save"),
                        cx.listener(|view, _, window, cx| {
                            view.save_api_key(window, &mut *cx);
                        }),
                    ))),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(common::theme_color(theme_name, "LabelFgBrush"))
                    .child(view.api_key_status.clone()),
            ),
    );

    panel
}

fn render_general_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let settings = common::settings_snapshot(&view.shared.settings);
    let mut panel = common::panel(theme_name).gap(px(10.0));
    panel = panel.child(common::section_header(theme_name, i18n::get("Str_General")));

    panel = panel.child(common::checkbox_row(
        "run-at-startup",
        i18n::get("Str_RunAtStartup"),
        settings.run_at_startup,
        12.0,
        cx.listener(|view, checked, _, _cx| {
            let settings = view.settings();
            let mut settings = common::settings_write(&settings);
            settings.run_at_startup = *checked;
            drop(settings);
            crate::platform::registry::apply_run_at_startup(*checked);
            view.save();
        }),
    ));

    panel = panel.child(common::checkbox_row(
        "sound-enabled",
        i18n::get("Str_EnableSoundEffects"),
        settings.sound_enabled,
        12.0,
        cx.listener(|view, checked, _, _cx| {
            let settings = view.settings();
            let mut settings = common::settings_write(&settings);
            settings.sound_enabled = *checked;
            drop(settings);
            view.save();
        }),
    ));

    let placement_options: Vec<(PopupPlacementMode, String)> = tooltip::placement_modes()
        .iter()
        .map(|(mode, key)| (*mode, i18n::get(key)))
        .collect();
    panel = panel.child(
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(common::field_label(
                theme_name,
                i18n::get("Str_PopupPosition"),
            ))
            .child(common::chip_row(
                "popup-position-chips",
                theme_name,
                &placement_options,
                settings.popup_placement,
                cx.listener(|view, mode: &PopupPlacementMode, _, _cx| {
                    let settings = view.settings();
                    let mut settings = common::settings_write(&settings);
                    settings.popup_placement = *mode;
                    drop(settings);
                    view.save();
                }),
            )),
    );

    panel = panel.child(common::checkbox_row(
        "popup-auto-close",
        i18n::get("Str_PopupAutoClose"),
        settings.popup_auto_close_enabled,
        12.0,
        cx.listener(|view, checked, _, _cx| {
            let settings = view.settings();
            let mut settings = common::settings_write(&settings);
            settings.popup_auto_close_enabled = *checked;
            drop(settings);
            view.save();
        }),
    ));

    // C# `UpdatePopupAutoCloseControls`: the seconds box is only usable while the
    // auto-close checkbox is checked, and keeps showing the stored value, dimmed.
    panel = panel.child(
        div()
            .flex()
            .flex_col()
            .gap(px(5.0))
            .child(common::field_label(
                theme_name,
                i18n::get("Str_PopupAutoCloseSeconds"),
            ))
            .child(match settings.popup_auto_close_enabled {
                true => div()
                    .w(px(120.0))
                    .child(gpui_kit::component::input::Input::new(
                        &view.auto_close_seconds,
                    ))
                    .into_any_element(),
                false => div()
                    .w(px(120.0))
                    .px(px(8.0))
                    .py(px(6.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(common::theme_color(theme_name, "Border"))
                    .bg(common::theme_color(theme_name, "BgElement"))
                    .text_color(common::theme_color(theme_name, "TextLow"))
                    .text_size(px(12.0))
                    .child(settings.popup_auto_close_seconds.to_string())
                    .into_any_element(),
            }),
    );

    panel
}

fn render_appearance_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let settings = common::settings_snapshot(&view.shared.settings);
    let mut panel = common::panel(theme_name).gap(px(10.0));
    panel = panel.child(common::section_header(
        theme_name,
        i18n::get("Str_Appearance"),
    ));

    let theme_options: Vec<(String, String)> = ["Dark", "Light", "System"]
        .iter()
        .map(|name| {
            (
                (*name).to_string(),
                match *name {
                    "Dark" => i18n::get("Str_ThemeDark"),
                    "Light" => i18n::get("Str_ThemeLight"),
                    _ => i18n::get("Str_ThemeSystem"),
                },
            )
        })
        .collect();
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .w(px(70.0))
                    .text_size(px(12.0))
                    .text_color(common::theme_color(theme_name, "SubtitleFgBrush"))
                    .child(i18n::get("Str_Theme")),
            )
            .child(common::chip_row(
                "theme-chips",
                theme_name,
                &theme_options,
                settings.theme.clone(),
                cx.listener(|view, tag: &String, _, cx| {
                    let settings = view.settings();
                    let mut settings = common::settings_write(&settings);
                    settings.theme = tag.clone();
                    drop(settings);
                    view.save();
                    // C# App.ApplyTheme(tag) — resolves System against Windows and
                    // re-renders every window with the new theme.
                    let snapshot = common::settings_snapshot(&view.settings());
                    view.theme_name = common::resolved_theme_name(&snapshot);
                    cx.notify();
                    // C# `ApplyTheme`'s last line is `UpdateTrayIcon(resolved)` — the
                    // tray icon is not theme-resource bound, so it is pushed here.
                    super::post(move |cx| {
                        let state = cx.global::<super::UiState>();
                        let theme = common::resolved_theme_name(&common::settings_snapshot(
                            &state.shared.settings,
                        ));
                        state.shared.tray.update_icon(&theme);
                    });
                }),
            )),
    );

    let language_options: Vec<(String, String)> = i18n::supported_languages()
        .iter()
        .map(|code| ((*code).to_string(), language_label(code)))
        .collect();
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.0))
            .child(
                div()
                    .w(px(70.0))
                    .text_size(px(12.0))
                    .text_color(common::theme_color(theme_name, "SubtitleFgBrush"))
                    .child(i18n::get("Str_Language")),
            )
            .child(common::chip_row(
                "language-chips",
                theme_name,
                &language_options,
                i18n::current_language().to_string(),
                cx.listener(|view, tag: &String, _, cx| {
                    let settings = view.settings();
                    let mut settings = common::settings_write(&settings);
                    settings.language = tag.clone();
                    drop(settings);
                    i18n::set_language(tag);
                    view.save();
                    // C# re-reads the key status in the new language.
                    let settings = common::settings_snapshot(&view.settings());
                    view.api_key_status = api_key_status(&settings, &view.shared.store);
                    // C# `App.ApplyLanguage` swaps the merged resource dictionary, so
                    // every DynamicResource-bound string in this window follows it.
                    cx.notify();
                    // C# `ApplyLanguage` also calls `RebuildTrayMenu()` — WinForms menu
                    // items are not resource-bound, and the port's menu is a native one.
                    super::post(move |cx| {
                        let state = cx.global::<super::UiState>();
                        let version = state
                            .app
                            .as_ref()
                            .and_then(|app| app.pending_update_version().map(str::to_string));
                        state.shared.tray.rebuild_menu(version.as_deref());
                    });
                }),
            )),
    );

    panel
}

/// The language names in `SettingsWindow.xaml`'s LanguageBox, which are not
/// localized (they are the languages' own names).
fn language_label(code: &str) -> String {
    match code {
        "en" => "English".to_string(),
        "ko" => "\u{D55C}\u{AD6D}\u{C5B4}".to_string(),
        "ja" => "\u{65E5}\u{672C}\u{8A9E}".to_string(),
        "zh" => "\u{4E2D}\u{6587}\u{FF08}\u{7B80}\u{4F53}\u{FF09}".to_string(),
        "es" => "Espa\u{F1}ol".to_string(),
        "fr" => "Fran\u{E7}ais".to_string(),
        "de" => "Deutsch".to_string(),
        "pt" => "Portugu\u{EA}s".to_string(),
        "ru" => "\u{420}\u{443}\u{441}\u{441}\u{43A}\u{438}\u{439}".to_string(),
        "it" => "Italiano".to_string(),
        other => other.to_string(),
    }
}

fn render_hotkey_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let mut panel = common::panel(theme_name).gap(px(10.0));
    panel = panel.child(common::section_header(
        theme_name,
        i18n::get("Str_KeyboardShortcut"),
    ));
    panel = panel.child(
        div()
            .text_size(px(11.0))
            .text_color(common::theme_color(theme_name, "SubtitleFgBrush"))
            .child(i18n::get("Str_HotkeyDescription")),
    );

    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .child(common::checkbox_row(
            "hotkey-ctrl",
            "Ctrl",
            view.hotkey_ctrl,
            12.0,
            cx.listener(|view, checked, window, cx| {
                view.hotkey_ctrl = *checked;
                view.save_hotkey(window, &mut *cx);
            }),
        ))
        .child(common::checkbox_row(
            "hotkey-alt",
            "Alt",
            view.hotkey_alt,
            12.0,
            cx.listener(|view, checked, window, cx| {
                view.hotkey_alt = *checked;
                view.save_hotkey(window, &mut *cx);
            }),
        ))
        .child(common::checkbox_row(
            "hotkey-shift",
            "Shift",
            view.hotkey_shift,
            12.0,
            cx.listener(|view, checked, window, cx| {
                view.hotkey_shift = *checked;
                view.save_hotkey(window, &mut *cx);
            }),
        ))
        .child(
            div()
                .text_size(px(12.0))
                .text_color(common::theme_color(theme_name, "SubtitleFgBrush"))
                .child("+"),
        );

    // The capture box: focused on click, swallows the next non-modifier key.
    let hotkey_focus = view.hotkey_focus.clone();
    row = row.child(
        div()
            .id("hotkey-key-box")
            .w(px(80.0))
            .track_focus(&hotkey_focus)
            .on_click(cx.listener(|view, _, window, cx| {
                view.hotkey_focus.focus(window, &mut *cx);
            }))
            .on_key_down(cx.listener(|view, event, window, cx| {
                view.capture_hotkey_key(event, window, &mut *cx);
            }))
            .child(gpui_kit::component::input::Input::new(&view.hotkey_key)),
    );
    row = row.child(div().w(px(56.0)).child(common::action_button(
        "hotkey-clear",
        theme_name,
        i18n::get("Str_Clear"),
        cx.listener(|view, _, window, cx| {
            SettingsView::set_input(&view.hotkey_key, "", window, cx);
            view.save_hotkey(window, &mut *cx);
        }),
    )));

    panel = panel.child(row);
    panel = panel.child(
        div()
            .text_size(px(10.0))
            .text_color(common::theme_color(theme_name, "TextLow"))
            .child(view.hotkey_hint(cx)),
    );

    panel
}

fn render_triggers_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let settings = common::settings_snapshot(&view.shared.settings);
    let mut panel = common::panel(theme_name).gap(px(6.0));
    panel = panel.child(common::section_header(
        theme_name,
        i18n::get("Str_PopupTriggers"),
    ));

    // C# the 2×2 trigger checkbox grid, in XAML order: row 0 Drag | DoubleClick,
    // row 1 LongPress | Keyboard.
    let triggers: [(bool, &str); 4] = [
        (settings.enable_drag_trigger, "Str_TriggerDrag"),
        (
            settings.enable_double_click_trigger,
            "Str_TriggerDoubleClick",
        ),
        (settings.enable_long_press_trigger, "Str_TriggerLongPress"),
        (
            settings.enable_keyboard_selection_trigger,
            "Str_TriggerKeyboard",
        ),
    ];

    let mut grid = div().flex().flex_col().gap(px(6.0));
    for pair in [0usize, 2] {
        let mut row = div().flex().flex_row().gap(px(6.0));
        for offset in 0..2 {
            let index = pair + offset;
            let (flag, key) = triggers[index];
            row = row.child(common::checkbox_row(
                ("trigger-check", index),
                i18n::get(key),
                flag,
                12.0,
                cx.listener(move |view, checked, _, _cx| {
                    let settings = view.settings();
                    let mut settings = common::settings_write(&settings);
                    match index {
                        0 => settings.enable_drag_trigger = *checked,
                        1 => settings.enable_double_click_trigger = *checked,
                        2 => settings.enable_long_press_trigger = *checked,
                        _ => settings.enable_keyboard_selection_trigger = *checked,
                    }
                    drop(settings);
                    view.save();
                }),
            ));
        }
        grid = grid.child(row);
    }

    panel = panel.child(grid);
    panel
}

fn render_actions_panel(
    view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    let mut panel = common::panel(theme_name).gap(px(10.0));

    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .w_full()
            .child(common::section_header(theme_name, i18n::get("Str_Actions")))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(8.0))
                    .child(div().w(px(76.0)).child(common::action_button(
                        "import-actions",
                        theme_name,
                        i18n::get("Str_Import"),
                        cx.listener(|view, _, _, _cx| {
                            view.import_actions();
                        }),
                    )))
                    .child(div().w(px(76.0)).child(common::action_button(
                        "export-actions",
                        theme_name,
                        i18n::get("Str_Export"),
                        cx.listener(|view, _, _, _cx| {
                            view.export_actions();
                        }),
                    )))
                    .child(div().w(px(76.0)).child(common::primary_button(
                        "add-action",
                        theme_name,
                        i18n::get("Str_AddAction"),
                        cx.listener(|view, _, _, _cx| {
                            view.dialog.open_with(None, EditOrigin::Add);
                        }),
                    ))),
            ),
    );

    let mut list = div().flex().flex_col().gap(px(4.0)).min_h(px(180.0));

    // C# `ActionsList`'s `GridView` column headers, in XAML order:
    // `Str_ColIcon` (40), `Str_ColName` (95), `Str_ColPrompt` (265), `Str_ColOutput` (98).
    let header = common::table_row(
        "actions-header",
        theme_name,
        false,
        true,
        |_: &gpui::ClickEvent, _, _| {},
        vec![
            div()
                .w(px(40.0))
                .text_center()
                .child(i18n::get("Str_ColIcon"))
                .into_any_element(),
            div()
                .w(px(95.0))
                .child(i18n::get("Str_ColName"))
                .into_any_element(),
            div()
                .flex_1()
                .child(i18n::get("Str_ColPrompt"))
                .into_any_element(),
            div()
                .w(px(98.0))
                .child(i18n::get("Str_ColOutput"))
                .into_any_element(),
        ],
    );
    list = list.child(header);

    for (index, action) in view.actions.clone().into_iter().enumerate() {
        let selected = view.selected == Some(index);
        let icon = action.icon.clone();
        let output = action_type_of(&action).as_str().to_string();

        let row = common::table_row(
            ("action-row", index),
            theme_name,
            selected,
            false,
            cx.listener(move |view, _, _, _cx| {
                view.selected = Some(index);
            }),
            vec![
                div()
                    .w(px(40.0))
                    .text_center()
                    .text_size(px(13.0))
                    .text_color(common::theme_color(theme_name, "AccentBrush"))
                    .child(icon)
                    .into_any_element(),
                div()
                    .w(px(95.0))
                    .child(action.name.clone())
                    .into_any_element(),
                div()
                    .flex_1()
                    .text_color(common::theme_color(theme_name, "TextMid"))
                    .child(action.prompt_format.clone())
                    .into_any_element(),
                div()
                    .w(px(98.0))
                    .text_color(common::theme_color(theme_name, "TextMid"))
                    .child(output)
                    .into_any_element(),
            ],
        );
        list = list.child(row);
    }
    panel = panel.child(list);

    // Info + GitHub link + version, exactly like the XAML's footer.
    let mut footer = div().flex().flex_col().gap(px(4.0)).mt(px(12.0));
    footer = footer.child(
        div()
            .text_size(px(10.0))
            .text_color(common::theme_color(theme_name, "TextLow"))
            .child(i18n::get("Str_PopupTriggerHint")),
    );
    footer = footer.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(6.0))
            .text_size(px(10.0))
            .text_color(common::theme_color(theme_name, "TextLow"))
            .child(view.version_label.clone())
            .child("·")
            .child(common::action_button(
                "github-link",
                theme_name,
                i18n::get("Str_GitHubLink"),
                cx.listener(|_, _, _, _cx| {
                    common::open_url("https://github.com/CrowKing63/Orbital");
                }),
            )),
    );
    if let Some(tag) = view.update_available.clone() {
        footer = footer.child(
            div()
                .id("update-link")
                .mt(px(3.0))
                .text_size(px(10.0))
                .text_color(common::theme_color(theme_name, "AccentBrush"))
                .cursor_pointer()
                .child(format!("\u{2191} New version available: {tag}"))
                .on_click(cx.listener(|view, _, _, _cx| {
                    let url = view
                        .update_url
                        .clone()
                        .unwrap_or_else(|| "https://github.com/CrowKing63/Orbital".to_string());
                    common::open_url(&url);
                })),
        );
    }
    panel = panel.child(footer);

    panel
}
fn render_bottom_buttons(
    _view: &mut SettingsView,
    theme_name: &str,
    cx: &mut Context<SettingsView>,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_end()
        .gap(px(8.0))
        .mt(px(12.0))
        .child(div().w(px(36.0)).child(common::action_button(
            "move-action-up",
            theme_name,
            "\u{2191}",
            cx.listener(|view, _, _, cx| {
                view.move_action(-1, &mut *cx);
            }),
        )))
        .child(div().w(px(36.0)).child(common::action_button(
            "move-action-down",
            theme_name,
            "\u{2193}",
            cx.listener(|view, _, _, cx| {
                view.move_action(1, &mut *cx);
            }),
        )))
        .child(div().w(px(76.0)).child(common::danger_button(
            "delete-action",
            theme_name,
            i18n::get("Str_Delete"),
            cx.listener(|view, _, _, cx| {
                view.delete_action(&mut *cx);
            }),
        )))
        .child(div().w(px(76.0)).child(common::action_button(
            "edit-action",
            theme_name,
            i18n::get("Str_Edit"),
            cx.listener(|view, _, _, _cx| {
                let Some(index) = view.selected else {
                    return;
                };
                let Some(action) = view.actions.get(index).cloned() else {
                    return;
                };
                view.dialog.open_with(Some(action), EditOrigin::Edit(index));
            }),
        )))
        .child(div().w(px(120.0)).child(common::primary_button(
            "save-and-close",
            theme_name,
            i18n::get("Str_SaveAndClose"),
            cx.listener(|view, _, window, cx| {
                view.save_and_close(window, &mut *cx);
            }),
        )))
}

/// The settings modal that is currently open, if any.
static SETTINGS_WINDOW: std::sync::Mutex<Option<WindowHandle<SettingsView>>> =
    std::sync::Mutex::new(None);

pub(crate) fn current_settings_window() -> Option<WindowHandle<SettingsView>> {
    *SETTINGS_WINDOW.lock().ok()?
}

fn register_settings_window(window: WindowHandle<SettingsView>) {
    if let Ok(mut slot) = SETTINGS_WINDOW.lock() {
        *slot = Some(window);
    }
}

fn clear_settings_window() {
    if let Ok(mut slot) = SETTINGS_WINDOW.lock() {
        *slot = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_provider_matches_the_source_order() {
        // C# checks openrouter first, then openai, else custom.
        assert_eq!(
            detect_provider("https://openrouter.ai/api/v1"),
            "openrouter"
        );
        assert_eq!(detect_provider("https://api.openai.com/v1"), "openai");
        assert_eq!(detect_provider("https://example.com/v1"), "custom");
    }

    #[test]
    fn provider_tables_carry_the_source_lists() {
        assert_eq!(
            provider_models("openai"),
            vec!["gpt-4o", "gpt-4o-mini", "gpt-4-turbo", "gpt-3.5-turbo"]
        );
        let openrouter = provider_models("openrouter");
        assert_eq!(openrouter.len(), 12);
        assert_eq!(openrouter[0], "openrouter/free");
        assert!(openrouter.contains(&"deepseek/deepseek-r1:free".to_string()));
        assert!(provider_models("custom").is_empty());
    }

    #[test]
    fn vk_labels_and_names_round_trip() {
        // C# `VkToLabel` keeps Windows key names in English, with four exceptions.
        assert_eq!(vk_to_label(0), "");
        assert_eq!(vk_to_label(0x20), "Space");
        assert_eq!(vk_to_label(0x0D), "Enter");
        assert_eq!(vk_to_label(0x1B), "Esc");
        assert_eq!(vk_to_label(0x09), "Tab");
        assert_eq!(vk_to_label(0x41), "A");
        assert_eq!(vk_to_label(0x70), "F1");

        assert_eq!(vk_from_key("a"), Some(0x41));
        assert_eq!(vk_from_key("f5"), Some(0x74));
        assert_eq!(vk_from_key("escape"), Some(0x1B));
        assert_eq!(vk_from_key("nonexistent"), None);
    }

    #[test]
    fn parse_version_compares_like_dotnet() {
        assert_eq!(parse_version("0.4.9"), Some((0, 4, 9)));
        assert_eq!(parse_version("1.2"), Some((1, 2, 0)));
        assert_eq!(parse_version("junk"), None);
        assert!(parse_version("0.5.0") > parse_version("0.4.9"));
    }

    #[test]
    fn language_labels_use_the_languages_own_names() {
        assert_eq!(language_label("en"), "English");
        assert_eq!(language_label("it"), "Italiano");
        let labels: Vec<String> = i18n::supported_languages()
            .iter()
            .map(|code| language_label(code))
            .collect();
        let unique: std::collections::HashSet<&String> = labels.iter().collect();
        assert_eq!(unique.len(), labels.len());
    }
}
