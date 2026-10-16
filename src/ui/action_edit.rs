//! ActionEditDialog port — GPUI modal: name / icon / prompt + `{text}` hint /
//! output-mode / display-mode / requires-selection / clean-output, preset menu from
//! `action_presets`, `\uE8C8`-literal parse via `char::from_u32`, Tag-based enum
//! persistence (result_action string), name-required guard.
//!
//! The source's `ContextMenu` of presets is a modal popup; the port renders the same
//! 17 presets as an inline panel toggled by the load-preset button.
//! `PORT_STATUS.md` (2026-10-12, workstream D) records that as a visual-chrome diff.

use std::sync::Arc;
use std::sync::RwLock;

use gpui::{
    App, Context, Entity, Render, StyleRefinement, Window, WindowHandle, div, prelude::*, px, size,
};

use crate::action_type::ActionType;
use crate::i18n;
use crate::settings::{ActionProfile, AppSettings, ButtonDisplayMode, SettingsStore};

use super::bridge::MainThreadBridge;
use super::common;

/// C# `ActionEditDialog` window size.
const DIALOG_WIDTH: f32 = 480.0;
const DIALOG_HEIGHT: f32 = 460.0;

struct EditShared {
    bridge: Arc<MainThreadBridge>,
    settings: Arc<RwLock<AppSettings>>,
    store: SettingsStore,
}

/// Where a committed profile goes, matching `AddAction_Click` vs `EditAction_Click`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EditOrigin {
    /// Append to the list.
    Add,
    /// Replace the action at this index.
    Edit(usize),
}

pub struct ActionEditDialog {
    inner: Arc<EditShared>,
}

/// The dialog's form state. `profile` is what `Result` becomes on OK.
pub(crate) struct ActionEditView {
    shared: Arc<EditShared>,
    origin: EditOrigin,
    theme_name: String,

    name: Entity<gpui_kit::component::input::InputState>,
    icon: Entity<gpui_kit::component::input::InputState>,
    prompt: Entity<gpui_kit::component::input::TextareaState>,

    /// C# `ActionTypeExtensions.FromString(selectedActionString)` — stored as the
    /// serialized string, exactly like `profile.ResultAction`.
    result_action: &'static str,
    display_mode: ButtonDisplayMode,
    requires_selection: bool,
    clean_output: bool,

    preset_open: bool,
}

impl ActionEditDialog {
    pub(crate) fn new(
        bridge: Arc<MainThreadBridge>,
        settings: Arc<RwLock<AppSettings>>,
        store: SettingsStore,
    ) -> Self {
        Self {
            inner: Arc::new(EditShared {
                bridge,
                settings,
                store,
            }),
        }
    }

    /// C# `new ActionEditDialog { Owner = this }` (+ `ShowDialog`).
    pub fn open(&self, action: Option<crate::settings::ActionProfile>) {
        self.open_with(action, EditOrigin::Add);
    }

    pub(crate) fn open_with(&self, action: Option<ActionProfile>, origin: EditOrigin) {
        let inner = Arc::clone(&self.inner);
        let bridge = Arc::clone(&inner.bridge);
        bridge.post(Box::new(move |cx| {
            inner.show_modal(cx, action, origin);
        }));
    }

    /// C# `Result = new ActionProfile()` — a blank profile for the "Add" case, whose
    /// defaults are the ones `SettingsManager` writes for a new action.
    fn blank_profile() -> ActionProfile {
        ActionProfile {
            name: String::new(),
            icon: String::new(),
            prompt_format: String::new(),
            result_action: "Popup".to_string(),
            requires_selection: None,
            display_mode: ButtonDisplayMode::TextAndIcon,
            clean_output: false,
        }
    }
}

impl EditShared {
    /// Opens the dialog window. C# `new ActionEditDialog { Owner = this }.ShowDialog()`.
    fn show_modal(
        self: &Arc<Self>,
        cx: &mut App,
        action: Option<ActionProfile>,
        origin: EditOrigin,
    ) {
        let settings = common::settings_snapshot(&self.settings);
        let theme_name = common::resolved_theme_name(&settings);
        // C# `public ActionProfile Result { get; private set; } = new ActionProfile();`
        let existing = action.unwrap_or_else(ActionEditDialog::blank_profile);

        let title = i18n::get("Str_EditAction");
        let options = common::dialog_window_options(
            gpui::Bounds::centered(None, size(px(DIALOG_WIDTH), px(DIALOG_HEIGHT)), cx),
            Some(title.clone()),
        );

        let shared = Arc::clone(self);
        let window = match cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                let name = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(existing.name.clone())
                });
                let icon = cx.new(|cx| {
                    gpui_kit::component::input::InputState::new(window, cx)
                        .default_value(existing.icon.clone())
                });
                let prompt = cx.new(|cx| {
                    gpui_kit::component::input::TextareaState::new(window, cx)
                        .default_value(existing.prompt_format.clone())
                });

                ActionEditView {
                    shared: Arc::clone(&shared),
                    origin,
                    theme_name: theme_name.clone(),
                    name,
                    icon,
                    prompt,
                    result_action: ActionType::from_str(&existing.result_action).as_str(),
                    display_mode: existing.display_mode,
                    requires_selection: crate::settings::is_selection_required(&existing),
                    clean_output: existing.clean_output,
                    preset_open: false,
                }
            })
        }) {
            Ok(window) => window,
            Err(error) => {
                tracing::error!(%error, "could not open the action edit dialog");
                return;
            }
        };

        register_edit_window(window);
        common::apply_window_backdrop(
            &title,
            theme_name != "Light",
            crate::backdrop::WindowBackdropKind::TransientWindow,
        );
    }
}

impl ActionEditView {
    fn settings(&self) -> Arc<RwLock<AppSettings>> {
        Arc::clone(&self.shared.settings)
    }

    fn save(&self) {
        let settings = common::settings_snapshot(&self.shared.settings);
        if let Err(error) = self.shared.store.save(&settings) {
            tracing::warn!(%error, "could not save settings from the action edit dialog");
        }
    }

    fn input_value(state: &Entity<gpui_kit::component::input::InputState>, cx: &App) -> String {
        state.read(cx).text().to_string()
    }

    fn value_prompt(state: &Entity<gpui_kit::component::input::TextareaState>, cx: &App) -> String {
        state.read(cx).text().to_string()
    }

    fn set(
        state: &Entity<gpui_kit::component::input::InputState>,
        value: impl Into<gpui::SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let value = value.into();
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    fn set_prompt(
        state: &Entity<gpui_kit::component::input::TextareaState>,
        value: impl Into<gpui::SharedString>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let value = value.into();
        state.update(cx, |state, cx| state.set_value(value, window, cx));
    }

    /// C# `LoadPreset_Click` — fills every field from the preset.
    fn load_preset(&mut self, index: usize, window: &mut Window, cx: &mut App) {
        let presets = crate::action_presets::all();
        let Some(preset) = presets.get(index) else {
            return;
        };

        Self::set(&self.name, preset.name, window, cx);
        Self::set(&self.icon, preset.icon, window, cx);
        Self::set_prompt(&self.prompt, preset.prompt_format, window, cx);
        self.result_action = ActionType::from_str(preset.result_action).as_str();
        self.display_mode = ButtonDisplayMode::TextAndIcon;
        self.requires_selection = preset
            .requires_selection
            .unwrap_or_else(|| default_requires_selection(self.result_action));
        self.clean_output = preset.clean_output;
        self.preset_open = false;
    }

    /// C# `Ok_Click` — name guard, icon literal parse, then the result.
    fn commit(&mut self, window: &mut Window, cx: &mut App) {
        let name = Self::input_value(&self.name, cx);
        if name.trim().is_empty() {
            // C# MessageBox.Show(Str_ButtonNameRequired, Str_AppTitle, OK, Warning)
            common::warning_box("Str_AppTitle", &i18n::get("Str_ButtonNameRequired"));
            return;
        }

        let profile = ActionProfile {
            // C# `Name = NameBox.Text.Trim()`
            name: name.trim().to_string(),
            // C# `Icon = ParseIconInput(IconBox.Text.Trim())`
            icon: parse_icon_input(Self::input_value(&self.icon, cx).trim()),
            prompt_format: Self::value_prompt(&self.prompt, cx),
            result_action: self.result_action.to_string(),
            requires_selection: Some(self.requires_selection),
            display_mode: self.display_mode,
            clean_output: self.clean_output,
        };

        {
            let settings = self.settings();
            let mut settings = common::settings_write(&settings);
            match self.origin {
                EditOrigin::Add => settings.actions.push(profile),
                EditOrigin::Edit(index) => {
                    if let Some(slot) = settings.actions.get_mut(index) {
                        *slot = profile;
                    }
                }
            }
        }
        self.save();
        self.close_window(window, cx);
    }

    fn close_window(&self, _window: &mut Window, cx: &mut App) {
        if let Some(window) = current_edit_window() {
            clear_edit_window();
            let _ = window.update(cx, |_: &mut ActionEditView, window: &mut Window, _| {
                window.remove_window();
            });
        }
    }
}

/// C# `ActionProfile.IsSelectionRequired`'s default when `RequiresSelection` is
/// null — `!= Paste && != SimulateKey`.
fn default_requires_selection(result_action: &str) -> bool {
    let action_type = ActionType::from_str(result_action);
    action_type != ActionType::Paste && action_type != ActionType::SimulateKey
}

/// C# `ActionEditDialog.ParseIconInput` — `\uE8C8` / `U+E8C8` typed as text becomes
/// the character it names.
fn parse_icon_input(input: &str) -> String {
    if input.trim().is_empty() {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len());
    let mut index = 0usize;

    while index < input.len() {
        let rest = &input[index..];
        // C# `Regex.Replace` recognizes `\u`, `\U`, `U+` and `u+` as the marker, all
        // two characters long.
        let marker_len = if rest.starts_with("\\u")
            || rest.starts_with("\\U")
            || rest.starts_with("U+")
            || rest.starts_with("u+")
        {
            2
        } else {
            0
        };

        if marker_len == 0 {
            // Copy one whole character (the source's `Regex.Replace` copies every
            // non-match verbatim).
            let character = rest.chars().next().unwrap_or('\u{FFFD}');
            out.push(character);
            index += character.len_utf8();
            continue;
        }

        let digits = &rest[marker_len..];
        let taken = digits
            .chars()
            .take_while(|c| c.is_ascii_hexdigit())
            .take(5)
            .count();
        if !(4..=5).contains(&taken) {
            out.push_str(&rest[..marker_len]);
            index += marker_len;
            continue;
        }

        let hex = &digits[..taken];
        match u32::from_str_radix(hex, 16).ok().and_then(char::from_u32) {
            // C# `char.ConvertFromUtf32(...)` — a scalar, not a surrogate pair.
            Some(character) => {
                out.push(character);
                index += marker_len + taken;
            }
            None => {
                out.push_str(&rest[..marker_len]);
                index += marker_len;
            }
        }
    }

    out
}

impl Render for ActionEditView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = self.theme_name.clone();
        let mut root = common::window_root(&theme_name).p(px(24.0)).gap(px(12.0));

        // Title row + load preset.
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .w_full()
                .child(
                    div()
                        .text_size(px(16.0))
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_color(common::theme_color(&theme_name, "TitleFgBrush"))
                        .child(i18n::get("Str_EditAction")),
                )
                .child(div().flex().flex_row().items_center().gap(px(8.0)).child(
                    common::action_button(
                        "load-preset",
                        &theme_name,
                        i18n::get("Str_LoadPreset"),
                        cx.listener(|view, _, _, _| {
                            view.preset_open = !view.preset_open;
                        }),
                    ),
                )),
        );

        if self.preset_open {
            let mut list = div()
                .id("preset-list")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .max_h(px(180.0))
                .overflow_y_scroll()
                .p(px(8.0))
                .rounded(px(8.0))
                .bg(common::theme_color(&theme_name, "PanelBgBrush"))
                .border_1()
                .border_color(common::theme_color(&theme_name, "PanelBorderBrush"));
            let theme_name = theme_name.clone();
            for (index, preset) in crate::action_presets::all().into_iter().enumerate() {
                let theme_name = theme_name.clone();
                let hover_bg = common::theme_color(&theme_name, "BgHover");
                list = list.child(
                    div()
                        .id(("preset-item", index))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(8.0))
                        .py(px(6.0))
                        .rounded(px(5.0))
                        .text_sm()
                        .cursor_pointer()
                        .text_color(common::theme_color(&theme_name, "TextHigh"))
                        .child(
                            div()
                                .w(px(20.0))
                                .text_color(common::theme_color(&theme_name, "AccentBrush"))
                                .child(preset.icon.to_string()),
                        )
                        .child(preset.name.to_string())
                        .hover(move |style: StyleRefinement| style.bg(hover_bg))
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.load_preset(index, window, &mut *cx);
                        })),
                );
            }
            root = root.child(list);
        }

        // Name + icon — the C# field panel (`Padding="16"`, 3*/1* column split).
        root = root.child(
            common::panel_tight(&theme_name).child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(12.0))
                    .items_start()
                    .child(
                        div()
                            .flex_grow(3.0)
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(common::field_label(
                                &theme_name,
                                i18n::get("Str_ButtonName"),
                            ))
                            .child(gpui_kit::component::input::Input::new(&self.name)),
                    )
                    .child(
                        div()
                            .flex_grow(1.0)
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .child(common::field_label(&theme_name, i18n::get("Str_IconHint")))
                            .child(
                                div()
                                    .text_center()
                                    .child(gpui_kit::component::input::Input::new(&self.icon)),
                            ),
                    ),
            ),
        );

        // Prompt — the C# field panel.
        root = root.child(
            common::panel_tight(&theme_name).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap(px(6.0))
                            .child(common::field_label(&theme_name, i18n::get("Str_Prompt")))
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(common::theme_color(&theme_name, "TextLow"))
                                    .child(i18n::get("Str_PromptHint")),
                            ),
                    )
                    .child(
                        div()
                            .h(px(90.0))
                            .child(gpui_kit::component::input::Textarea::new(&self.prompt)),
                    ),
            ),
        );

        // Output mode.
        let action_options: Vec<(&'static str, String)> = [
            ("Replace", "Str_ActionReplace"),
            ("Copy", "Str_ActionCopy"),
            ("Popup", "Str_ActionPopup"),
            ("Browser", "Str_ActionBrowser"),
            ("DirectCopy", "Str_ActionDirectCopy"),
            ("Cut", "Str_ActionCut"),
            ("Paste", "Str_ActionPaste"),
            ("SimulateKey", "Str_ActionSimulateKey"),
            ("SelectAll", "Str_ActionSelectAll"),
        ]
        .iter()
        .map(|(tag, key)| (*tag, i18n::get(key)))
        .collect();

        // Display mode.
        let display_options: Vec<(ButtonDisplayMode, String)> = [
            (ButtonDisplayMode::TextAndIcon, "Str_DisplayTextAndIcon"),
            (ButtonDisplayMode::TextOnly, "Str_DisplayTextOnly"),
            (ButtonDisplayMode::IconOnly, "Str_DisplayIconOnly"),
        ]
        .iter()
        .map(|(mode, key)| (*mode, i18n::get(key)))
        .collect();

        // Output mode, display mode and the two checkboxes — the C# third field
        // panel (`ActionEditDialog.xaml:79-119`).
        root = root.child(
            common::panel_tight(&theme_name).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(common::field_label(
                        &theme_name,
                        i18n::get("Str_OutputMode"),
                    ))
                    .child(common::chip_row(
                        "output-mode-chips",
                        &theme_name,
                        &action_options,
                        self.result_action,
                        cx.listener(|view, tag: &&str, _, _| {
                            view.result_action = tag;
                        }),
                    )),
            ),
        );

        root = root.child(
            common::panel_tight(&theme_name).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(common::field_label(
                        &theme_name,
                        i18n::get("Str_ButtonDisplay"),
                    ))
                    .child(common::chip_row(
                        "display-mode-chips",
                        &theme_name,
                        &display_options,
                        self.display_mode,
                        cx.listener(|view, mode: &ButtonDisplayMode, _, _| {
                            view.display_mode = *mode;
                        }),
                    )),
            ),
        );

        root = root.child(
            common::panel_tight(&theme_name).child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(common::checkbox_row(
                        "requires-selection",
                        i18n::get("Str_RequiresSelection"),
                        self.requires_selection,
                        11.0,
                        cx.listener(|view, checked, _, _| {
                            view.requires_selection = *checked;
                        }),
                    ))
                    .child(common::checkbox_row(
                        "clean-output",
                        i18n::get("Str_CleanOutput"),
                        self.clean_output,
                        11.0,
                        cx.listener(|view, checked, _, _| {
                            view.clean_output = *checked;
                        }),
                    )),
            ),
        );

        // Buttons — C# Save then Cancel, 82 px each, `Margin="0,14,0,0"`.
        root = root.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(8.0))
                .mt(px(14.0))
                .child(div().w(px(82.0)).child(common::primary_button(
                    "edit-save",
                    &theme_name,
                    i18n::get("Str_Save"),
                    cx.listener(|view, _, window, cx| {
                        view.commit(window, &mut *cx);
                    }),
                )))
                .child(div().w(px(82.0)).child(common::action_button(
                    "edit-cancel",
                    &theme_name,
                    i18n::get("Str_Cancel"),
                    cx.listener(|view, _, window, cx| {
                        view.close_window(window, &mut *cx);
                    }),
                ))),
        );

        root
    }
}

/// The dialog window that is currently open, so `close_window` can reach it.
static EDIT_WINDOW: std::sync::Mutex<Option<WindowHandle<ActionEditView>>> =
    std::sync::Mutex::new(None);

fn register_edit_window(window: WindowHandle<ActionEditView>) {
    if let Ok(mut slot) = EDIT_WINDOW.lock() {
        *slot = Some(window);
    }
}

fn clear_edit_window() {
    if let Ok(mut slot) = EDIT_WINDOW.lock() {
        *slot = None;
    }
}

fn current_edit_window() -> Option<WindowHandle<ActionEditView>> {
    *EDIT_WINDOW.lock().ok()?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_icon_input_handles_the_typed_hex_forms() {
        // C# `Regex.Replace(input, @"(?:\\u|U\+)([0-9a-fA-F]{4,5})", ...)`.
        assert_eq!(parse_icon_input("\\uE8C8"), "\u{E8C8}");
        assert_eq!(parse_icon_input("U+E8C8"), "\u{E8C8}");
        assert_eq!(parse_icon_input("u+E7C3"), "\u{E7C3}");
        assert_eq!(parse_icon_input("\\uE8C8\\uE7C3"), "\u{E8C8}\u{E7C3}");
        assert_eq!(parse_icon_input("plain"), "plain");
        assert_eq!(parse_icon_input(""), "");
        assert_eq!(parse_icon_input("   "), "   ");
    }

    #[test]
    fn parse_icon_input_rejects_short_and_long_hex_runs() {
        // Fewer than four digits is not a match, so the text is kept verbatim.
        assert_eq!(parse_icon_input("\\uE8"), "\\uE8");
        // The C# regex `[0-9a-fA-F]{4,5}` is greedy: a fifth hex digit is consumed,
        // which this port reproduces exactly (source behavior, bugs included).
        assert_eq!(parse_icon_input("\\uE8C812"), "\u{E8C81}2");
    }

    #[test]
    fn default_requires_selection_matches_the_source_table() {
        assert!(default_requires_selection("Replace"));
        assert!(default_requires_selection("Popup"));
        assert!(default_requires_selection("Cut"));
        assert!(!default_requires_selection("Paste"));
        assert!(!default_requires_selection("SimulateKey"));
    }
}
