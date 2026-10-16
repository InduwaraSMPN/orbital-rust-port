//! Shared UI plumbing for the GPUI stack: theme tokens resolved from the live
//! settings, GPUI text measurement (used to size the popups before they open —
//! see `PORT_STATUS.md` for why GPUI cannot measure a window after the fact),
//! simple dialogs over `rfd`, and the Win32 helpers the port needs that GPUI
//! does not expose.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;

use gpui::{
    App, Font, FontWeight, Pixels, SharedString, StyleRefinement, Window, div, prelude::*, px, rgba,
};

use crate::settings::AppSettings;
use crate::theme;

/// The active theme table, refreshed whenever the resolved theme name changes —
///
/// exactly the two calls the source makes in `ApplyTheme` +
/// `ApplySystemAccentResources`.
static ACTIVE_THEME: Mutex<(String, theme::ThemeColors)> = Mutex::new((
    String::new(),
    theme::ThemeColors {
        entries: [theme::ThemeEntry { key: "", value: 0 }; 29],
    },
));

/// The theme table for `resolved_theme`, with the DWM accent applied.
pub fn active_theme(resolved_theme: &str) -> theme::ThemeColors {
    let mut guard = match ACTIVE_THEME.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.0 != resolved_theme {
        let mut colors = theme::colors(resolved_theme);
        // C# `App.ApplySystemAccentResources` — the runtime DWM accent overrides the
        // five accent resources of the active theme.
        if let Some((accent, soft)) = theme::accent_with_soft() {
            let set = |colors: &mut theme::ThemeColors, key: &str, value: u32| {
                if let Some(entry) = colors.entries.iter_mut().find(|entry| entry.key == key) {
                    entry.value = value;
                }
            };
            set(&mut colors, "AccentVi", accent);
            set(&mut colors, "AccentCy", soft);
            set(&mut colors, "AccentBrush", accent);
            set(&mut colors, "AccentViColor", accent);
            set(&mut colors, "AccentCyColor", soft);
        }
        guard.0 = resolved_theme.to_string();
        guard.1 = colors;
    }
    guard.1
}

/// The theme name resolved from a setting — `System` asks Windows.
pub fn resolved_theme_name(settings: &AppSettings) -> String {
    if settings.theme == "System" {
        resolve_system_theme()
    } else {
        settings.theme.clone()
    }
}

/// C# `App.ResolveTheme("System")` → `GetSystemTheme` (registry, Dark fallback).
pub fn resolve_system_theme() -> String {
    if crate::platform::registry::apps_use_light_theme() == Some(true) {
        "Light".to_string()
    } else {
        "Dark".to_string()
    }
}

/// A theme resource as `0xAARRGGBB`, exactly as the XAML declares it.
pub fn theme_argb(resolved_theme: &str, key: &str) -> u32 {
    active_theme(resolved_theme)
        .get(key)
        .or_else(|| theme::colors("Dark").get(key))
        .unwrap_or(0xFF00_0000)
}

/// GPUI color for a theme resource key of the resolved theme.
pub fn theme_color(resolved_theme: &str, key: &str) -> gpui::Rgba {
    rgba(argb_to_rgba(theme_argb(resolved_theme, key)))
}

/// `gpui::rgba` takes `0xRRGGBBAA`; theme values are stored `0xAARRGGBB`, exactly as
/// the XAML declares them — so the conversion rotates the alpha to the low byte.
fn argb_to_rgba(argb: u32) -> u32 {
    let a = (argb >> 24) & 0xFF;
    let rgb = argb & 0x00FF_FFFF;
    (rgb << 8) | a
}

// ── Text measurement ────────────────────────────────────────────────────────

/// The UI font family the WPF target declares (`App.xaml` Window style) — a
/// fallback chain whose primary is the Windows 11 system font; Segoe UI is the
/// fallback for machines without it.
pub const UI_FONT_FAMILY: &str = "Segoe UI Variable";
/// The MDL2 font the source resolves private-use glyphs to.
pub const ICON_FONT_FAMILY: &str = "Segoe MDL2 Assets";

/// C# `RadialMenuWindow.ResolveIconFont` — MDL2 for private-use glyphs, emoji font
/// otherwise.
pub fn icon_font_family(icon: &str) -> &'static str {
    if icon.chars().any(|c| ('\u{E000}'..='\u{F8FF}').contains(&c)) {
        ICON_FONT_FAMILY
    } else {
        "Segoe UI Emoji"
    }
}

/// Measures a single line of text in logical (DIP) pixels.
///
/// `TextSystem` only offers per-glyph advances (the shaped-line API lives on
/// `WindowTextSystem`, which needs a `Window`), so a string is the sum of its
/// characters' advance widths — the quantity DirectWrite reports for a run
/// without contextual substitutions.
pub fn text_width(cx: &App, text: &str, font_size: f32, family: &str) -> Pixels {
    if text.is_empty() {
        return px(0.0);
    }

    let font = Font {
        family: family.into(),
        features: Default::default(),
        // C# `FontFamily="Segoe UI Variable, Segoe UI"` — Segoe UI is the fallback.
        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec![
            "Segoe UI".to_string(),
        ])),
        weight: FontWeight::default(),
        style: Default::default(),
    };
    let font_id = cx.text_system().resolve_font(&font);
    let text_system = cx.text_system();
    let mut width = px(0.0);
    for character in text.chars() {
        width += text_system.layout_width(font_id, px(font_size), character);
    }
    width
}

/// Greedy word-wrap of `text` into lines no wider than `max_width`, standing in for
/// WPF `TextWrapping="Wrap"` on the popups.
pub fn wrap_lines(cx: &App, text: &str, font_size: f32, max_width: Pixels) -> Vec<String> {
    let family = UI_FONT_FAMILY;
    let mut lines = Vec::new();

    for paragraph in text.split('\n') {
        if paragraph.is_empty() {
            lines.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut current_width = px(0.0);
        let space = text_width(cx, " ", font_size, family);

        for word in paragraph.split(' ') {
            let word_width = text_width(cx, word, font_size, family);
            let candidate = if current.is_empty() {
                word_width
            } else {
                current_width + space + word_width
            };

            if !current.is_empty() && candidate > max_width {
                lines.push(std::mem::take(&mut current));
                current_width = word_width;
                current.push_str(word);
            } else {
                if !current.is_empty() {
                    current.push(' ');
                    current_width += space;
                }
                current.push_str(word);
                current_width += word_width;
            }
        }
        lines.push(current);
    }

    lines
}

// ── Dialogs ────────────────────────────────────────────────────────────────

/// C# `MessageBox.Show(message, title, OK, icon)`.
pub fn message_box(title_key: &str, message: &str, level: rfd::MessageLevel) {
    rfd::MessageDialog::new()
        .set_title(crate::i18n::get(title_key))
        .set_description(message)
        .set_level(level)
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

/// C# `MessageBox.Show(..., OK, Error)`.
pub fn error_box(title_key: &str, message: &str) {
    message_box(title_key, message, rfd::MessageLevel::Error);
}

/// C# `MessageBox.Show(..., OK, Warning)`.
pub fn warning_box(title_key: &str, message: &str) {
    message_box(title_key, message, rfd::MessageLevel::Warning);
}

/// C# `MessageBox.Show(..., OK, Information)`.
pub fn info_box(title_key: &str, message: &str) {
    message_box(title_key, message, rfd::MessageLevel::Info);
}

/// C# `MessageBox.Show(..., YesNo, Question)` — `true` means Yes.
pub fn yes_no_box(title_key: &str, message: &str) -> bool {
    rfd::MessageDialog::new()
        .set_title(crate::i18n::get(title_key))
        .set_description(message)
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNo)
        .show()
        == rfd::MessageDialogResult::Yes
}

/// C# `MessageBox.Show(..., YesNoCancel, Question)` mapped onto the source's
/// `Replace` / `Merge` / `Abort` decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportModeChoice {
    /// Yes — replace the existing actions.
    Replace,
    /// No — merge, skipping duplicates by name.
    Merge,
    /// Cancel — abort the import.
    Abort,
}

pub fn import_mode_box() -> ImportModeChoice {
    match rfd::MessageDialog::new()
        .set_title(crate::i18n::get("Str_ImportModeTitle"))
        .set_description(crate::i18n::get("Str_ImportModeMsg"))
        .set_level(rfd::MessageLevel::Info)
        .set_buttons(rfd::MessageButtons::YesNoCancel)
        .show()
    {
        rfd::MessageDialogResult::Yes => ImportModeChoice::Replace,
        rfd::MessageDialogResult::No => ImportModeChoice::Merge,
        _ => ImportModeChoice::Abort,
    }
}

/// C# `Process.Start(new ProcessStartInfo { FileName = url, UseShellExecute = true })`.
pub fn open_url(url: &str) {
    if let Err(error) = open::that(url) {
        tracing::debug!(%error, url, "could not open the URL in the shell");
    }
}

// ── Window helpers ──────────────────────────────────────────────────────────

/// C# `WindowBackdropHelper.TryApply` — the DWM backdrop of the settings and edit
/// windows. GPUI does not publish a window's HWND, so it is resolved from the
/// window title this stack sets on those windows (best-effort, cosmetic only).
pub fn apply_window_backdrop(
    title: &str,
    use_dark_mode: bool,
    kind: crate::backdrop::WindowBackdropKind,
) {
    if let Some(hwnd) = find_own_window_by_title(title) {
        crate::backdrop::try_apply(hwnd, use_dark_mode, kind);
    }
}

/// Finds a top-level window of this process whose title matches.
fn find_own_window_by_title(title: &str) -> Option<isize> {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId,
        IsWindowVisible,
    };

    let wanted = title.encode_utf16().collect::<Vec<u16>>();

    /// The state the enumeration carries; only the main thread ever reads it back.
    struct Probe {
        wanted: Vec<u16>,
        wanted_chars: usize,
        own_pid: u32,
        found: Option<isize>,
    }

    unsafe extern "system" fn probe(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
        // SAFETY: `lparam` is the address of the `Probe` that outlives this call.
        let state = unsafe { &mut *(lparam.0 as *mut Probe) };
        if state.found.is_some() {
            return windows::core::BOOL(1);
        }
        // SAFETY: `hwnd` is a live window handle supplied by the enumeration.
        if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return windows::core::BOOL(1);
        }

        let mut pid = 0u32;
        // SAFETY: `pid` is a live out slot and `hwnd` is valid.
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        if pid != state.own_pid {
            return windows::core::BOOL(1);
        }

        // SAFETY: `GetWindowTextLengthW` only reads the window's title.
        let length = unsafe { GetWindowTextLengthW(hwnd) };
        if length <= 0 || length as usize != state.wanted_chars {
            return windows::core::BOOL(1);
        }

        let mut buffer = vec![0u16; length as usize + 1];
        // SAFETY: `buffer` is a live out buffer sized from the reported length.
        let copied = unsafe { GetWindowTextW(hwnd, &mut buffer) };
        if copied as usize != state.wanted_chars {
            return windows::core::BOOL(1);
        }
        if buffer[..copied as usize] == state.wanted[..] {
            state.found = Some(hwnd.0 as isize);
        }
        windows::core::BOOL(1)
    }

    let mut state = Probe {
        wanted,
        wanted_chars: title.encode_utf16().count(),
        own_pid: std::process::id(),
        found: None,
    };

    // SAFETY: `probe` is a `'static` fn item of this image and `state` outlives the
    // call; EnumWindows only reads the callback's return value.
    let _ = unsafe { EnumWindows(Some(probe), LPARAM(&raw mut state as isize)) };
    state.found
}

// ── Popup chrome ────────────────────────────────────────────────────────────

/// Radial-menu geometry, read from `RadialMenuWindow.xaml` (button style, pill
/// border, and the `PopulateBarButtons` separator border).
pub mod bar {
    pub const BUTTON_HEIGHT: f32 = 36.0;
    pub const BUTTON_MIN_WIDTH: f32 = 44.0;
    pub const BUTTON_PADDING_X: f32 = 10.0;
    pub const SEPARATOR_WIDTH: f32 = 1.0;
    pub const PILL_PADDING_X: f32 = 6.0;
    pub const PILL_PADDING_Y: f32 = 4.0;
    pub const PILL_BORDER: f32 = 1.0;
    pub const ICON_TEXT_GAP: f32 = 8.0;
    pub const ICON_SIZE_WITH_TEXT: f32 = 14.0;
    pub const ICON_SIZE_ALONE: f32 = 16.0;
}

/// Result-tooltip geometry, read from `ResultTooltipWindow.xaml`.
pub mod tip {
    pub const WINDOW_WIDTH: f32 = 380.0;
    pub const HEADER_PADDING_X: f32 = 14.0;
    pub const HEADER_PADDING_Y: f32 = 10.0;
    pub const BRAND_FONT_SIZE: f32 = 12.0;
    pub const BRAND_DOT: f32 = 8.0;
    pub const SEPARATOR_HEIGHT: f32 = 1.0;
    pub const BODY_PADDING_LEFT: f32 = 16.0;
    pub const BODY_PADDING_RIGHT: f32 = 16.0;
    pub const BODY_PADDING_TOP: f32 = 14.0;
    pub const BODY_PADDING_BOTTOM: f32 = 16.0;
    pub const TEXT_FONT_SIZE: f32 = 13.0;
    pub const TEXT_MAX_HEIGHT: f32 = 320.0;
    pub const COPY_BUTTON_MARGIN_TOP: f32 = 12.0;
    pub const COPY_BUTTON_PADDING_X: f32 = 12.0;
    pub const COPY_BUTTON_PADDING_Y: f32 = 6.0;
    pub const COPY_BUTTON_FONT_SIZE: f32 = 11.0;
    pub const COPY_ICON_SIZE: f32 = 13.0;
    pub const COPY_ICON_GAP: f32 = 8.0;
    pub const BORDER: f32 = 1.0;
}

/// WPF's default line height for a `TextBox` of `font_size` — the font's line
/// spacing, which Segoe UI reports as about 1.33 em.
pub fn text_line_height(font_size: f32) -> f32 {
    font_size * 1.33
}

// ── Small styled controls ───────────────────────────────────────────────────
// The WPF `App.xaml` ControlTemplates map one-to-one onto these `div()` styles;
// gpui-kit's `Button` and `Checkbox` supply the primitives that need real
// interactivity.

/// A panel used by the settings and edit windows. C# `SettingsWindow.xaml` pads the
/// API panel `18,16` and the other five `18,14`; `panel` is the common case and
/// `panel_with_padding` the API panel's.
pub fn panel(resolved_theme: &str) -> gpui::Div {
    panel_with_padding(resolved_theme, 14.0)
}

/// `panel` with an explicit vertical padding.
pub fn panel_with_padding(resolved_theme: &str, vertical: f32) -> gpui::Div {
    div()
        .bg(theme_color(resolved_theme, "PanelBgBrush"))
        .rounded(px(12.0))
        .border_1()
        .border_color(theme_color(resolved_theme, "PanelBorderBrush"))
        .px(px(18.0))
        .py(px(vertical))
        .flex()
        .flex_col()
}

/// A uniformly padded panel (`Padding="16"`, as the ActionEditDialog field panels
/// use).
pub fn panel_tight(resolved_theme: &str) -> gpui::Div {
    panel_with_padding(resolved_theme, 16.0)
}

/// Section header text (`FontSize=11 SemiBold LabelFgBrush`).
pub fn section_header(resolved_theme: &str, text: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .text_size(px(11.0))
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(theme_color(resolved_theme, "LabelFgBrush"))
        .child(text.into())
}

/// Field label text (`FontSize=11 LabelFgBrush`).
pub fn field_label(resolved_theme: &str, text: impl Into<gpui::SharedString>) -> gpui::Div {
    div()
        .text_size(px(11.0))
        .text_color(theme_color(resolved_theme, "LabelFgBrush"))
        .child(text.into())
}

/// `App.xaml` default Button: `BgElement` surface, `Border` edge, radius 8,
/// padding 14,8, hover `BgHover`, pressed `BgDeep`.
pub fn action_button(
    id: impl Into<gpui::ElementId>,
    resolved_theme: &str,
    label: impl Into<gpui::SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(theme_color(resolved_theme, "BgElement"))
        .border_1()
        .border_color(theme_color(resolved_theme, "Border"))
        .text_base()
        .text_color(theme_color(resolved_theme, "TextHigh"))
        .cursor_pointer()
        .child(label.into())
        .hover(move |style: StyleRefinement| {
            style
                .bg(theme_color(resolved_theme, "BgHover"))
                .border_color(theme_color(resolved_theme, "Border"))
        })
        .active(move |style: StyleRefinement| style.bg(theme_color(resolved_theme, "BgDeep")))
        .on_click(on_click)
}

/// `PrimaryButton` style: accent background, white foreground.
pub fn primary_button(
    id: impl Into<gpui::ElementId>,
    resolved_theme: &str,
    label: impl Into<gpui::SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(theme_color(resolved_theme, "AccentVi"))
        .text_base()
        .font_weight(gpui::FontWeight::SEMIBOLD)
        .text_color(white())
        .cursor_pointer()
        .child(label.into())
        .on_click(on_click)
}

/// `DangerButton` style: danger foreground and border.
pub fn danger_button(
    id: impl Into<gpui::ElementId>,
    resolved_theme: &str,
    label: impl Into<gpui::SharedString>,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .px(px(14.0))
        .py(px(8.0))
        .rounded(px(8.0))
        .bg(theme_color(resolved_theme, "BgElement"))
        .border_1()
        .border_color(theme_color(resolved_theme, "Danger"))
        .text_base()
        .text_color(theme_color(resolved_theme, "Danger"))
        .cursor_pointer()
        .child(label.into())
        .hover(move |style: StyleRefinement| style.bg(theme_color(resolved_theme, "BgHover")))
        .on_click(on_click)
}

fn white() -> gpui::Rgba {
    gpui::Rgba {
        r: 1.0,
        g: 1.0,
        b: 1.0,
        a: 1.0,
    }
}

/// A checkbox row — `CheckBox` with a `TextMid` label. `label_size` is the XAML
/// `FontSize` of the row's `Content` (12 in the settings window, 11 in the edit
/// dialog); gpui-kit's `Checkbox` labels itself at `text_sm` (14 px), so the size is
/// forced on the wrapping row.
pub fn checkbox_row(
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    checked: bool,
    label_size: f32,
    on_change: impl Fn(&bool, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .text_size(px(label_size))
        .child(
            gpui_kit::component::checkbox::Checkbox::new(id)
                .label(label.into())
                .checked(checked)
                .on_change(on_change),
        )
}

/// A horizontal row of selectable chips, standing in for the WPF `ComboBox`
/// (same option set, same localized labels, same tag identity).
pub fn chip_row<T: Clone + PartialEq + Send + 'static>(
    id: &'static str,
    resolved_theme: &str,
    options: &[(T, String)],
    selected: T,
    on_select: impl Fn(&T, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let on_select = std::rc::Rc::new(on_select);
    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .flex_wrap()
        .gap(px(6.0))
        .items_center();

    for (index, (value, label)) in options.iter().enumerate() {
        let on_select = std::rc::Rc::clone(&on_select);
        let value = value.clone();
        let is_selected = value == selected;
        row = row.child(chip(
            ("chip", index),
            resolved_theme,
            label.clone(),
            is_selected,
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                on_select(&value, window, cx);
            },
        ));
    }

    row
}

fn chip(
    id: impl Into<gpui::ElementId>,
    resolved_theme: &str,
    label: String,
    selected: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> gpui::Stateful<gpui::Div> {
    let accent = theme_color(resolved_theme, "AccentVi");
    let mut chip = div()
        .id(id)
        .flex()
        .items_center()
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .border_1()
        .text_sm()
        .cursor_pointer()
        .child(label);

    if selected {
        chip = chip.bg(accent).border_color(accent).text_color(white());
    } else {
        chip = chip
            .bg(theme_color(resolved_theme, "BgElement"))
            .border_color(theme_color(resolved_theme, "Border"))
            .text_color(theme_color(resolved_theme, "TextMid"));
    }

    chip.on_click(on_click)
}

/// A labelled column: label above the control.
pub fn labeled(
    resolved_theme: &str,
    label: impl Into<SharedString>,
    child: impl gpui::IntoElement,
) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(5.0))
        .child(field_label(resolved_theme, label))
        .child(child)
}

/// A row of the settings action list. `header` is the `GridViewColumnHeader` style
/// (`App.xaml` `GridViewColumnHeader`: padding 10,8, FontSize 11 SemiBold, TextLow,
/// `BgDeep` surface, 1 px bottom border); body rows keep the list styling.
pub fn table_row(
    id: impl Into<gpui::ElementId>,
    resolved_theme: &str,
    selected: bool,
    header: bool,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    cells: Vec<gpui::AnyElement>,
) -> gpui::Stateful<gpui::Div> {
    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(8.0))
        .px(px(10.0))
        .py(px(8.0))
        .rounded(px(6.0))
        .w_full()
        .text_color(theme_color(resolved_theme, "TextHigh"));

    if header {
        row = row
            .text_size(px(11.0))
            .font_weight(gpui::FontWeight::SEMIBOLD)
            .text_color(theme_color(resolved_theme, "TextLow"))
            .bg(theme_color(resolved_theme, "BgDeep"))
            .border_b_1()
            .border_color(theme_color(resolved_theme, "Border"));
    } else {
        row = row.text_sm().cursor_pointer();
        row = if selected {
            row.bg(theme_color(resolved_theme, "BgHover"))
        } else {
            row.bg(theme_color(resolved_theme, "BgElement"))
        };
    }

    row.children(cells).on_click(on_click)
}

/// Root container of a settings-style window: scrollable column with the window
/// background.
pub fn window_root(resolved_theme: &str) -> gpui::Stateful<gpui::Div> {
    div()
        .id("window-root")
        .size_full()
        .bg(theme_color(resolved_theme, "WindowBgBrush"))
        .text_color(theme_color(resolved_theme, "TextHigh"))
        .font_family(UI_FONT_FAMILY)
        .overflow_y_scroll()
        .flex()
        .flex_col()
}

/// `WindowOptions` for a modal dialog window (`ShowDialog` semantics).
pub fn dialog_window_options(
    bounds: gpui::Bounds<gpui::Pixels>,
    title: Option<String>,
) -> gpui::WindowOptions {
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: title.map(|title| gpui::TitlebarOptions {
            title: Some(title.into()),
            appears_transparent: true,
            traffic_light_position: Default::default(),
        }),
        kind: gpui::WindowKind::Dialog,
        focus: true,
        show: true,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        ..Default::default()
    }
}

/// `WindowOptions` for an overlay popup at `bounds`.
///
/// `display_id` matters: with `None`, GPUI resolves the window to the *primary*
/// display and then validates the requested bounds against that display
/// (`WindowsDisplay::check_given_bounds`). A pill placed on a secondary monitor
/// fails that check and GPUI silently replaces the bounds with its own
/// `default_bounds()` (a full-size window), so the caller must pass the display the
/// pill was placed on.
pub fn popup_window_options(
    bounds: gpui::Bounds<gpui::Pixels>,
    show: bool,
    focus: bool,
    display_id: Option<gpui::DisplayId>,
) -> gpui::WindowOptions {
    gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
        titlebar: None,
        focus,
        show,
        kind: gpui::WindowKind::PopUp,
        is_movable: false,
        is_resizable: false,
        is_minimizable: false,
        display_id,
        ..Default::default()
    }
}

// ── Settings access ────────────────────────────────────────────────────────

/// Reads the live settings snapshot without locking across an await.
pub fn settings_snapshot(settings: &Arc<RwLock<AppSettings>>) -> AppSettings {
    match settings.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Acquires a write lock on the settings, recovering a poisoned lock the way the
/// rest of the port does.
pub fn settings_write<'a>(
    settings: &'a Arc<RwLock<AppSettings>>,
) -> std::sync::RwLockWriteGuard<'a, AppSettings> {
    match settings.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}
