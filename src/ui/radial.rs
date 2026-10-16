//! RadialMenuWindow port — GPUI `WindowKind::PopUp` overlay. Buttons filtered by
//! editable/hasText (Disabled actions hidden), icon-font fallback for non-Lucide
//! glyphs, center-above-cursor + clamp + below-fallback placement, SelectAll
//! re-show flow, LLM-missing guard, outside-click + focus-loss + pill-background
//! hide.
//!
//! GPUI fixes a window's bounds at creation and offers no measure-then-move path,
//! so the pill is measured through GPUI's own text system before the window opens.
//! `PORT_STATUS.md` (2026-10-12, workstream D) records that as a target-side
//! decision; the placement math itself is the source's, run on the measured size.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Pixels, Render, StyleRefinement, Window, WindowHandle, div, prelude::*,
    px, size,
};
use windows::Win32::Foundation::HWND;
use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, SetForegroundWindow};

use crate::action_presets::lucide_name;
use crate::action_type::ActionType;
use crate::clipboard;
use crate::executor::ActionExecutor;
use crate::i18n;
use crate::placement::{Rect, Size};
use crate::settings::{
    ActionProfile, AppSettings, ButtonDisplayMode, SettingsStore, action_type_of,
    is_selection_required,
};
use crate::sound;

use super::ResultTooltip;
use super::bridge::MainThreadBridge;
use super::common::{self, bar};
use super::tooltip;

/// The popup's bounds in DIPs (C# `Left`/`Top`/`ActualWidth`/`ActualHeight`) plus the
/// DPI scale they were placed at, so the executor can report device pixels.
#[derive(Debug, Clone, Copy)]
pub(crate) struct RadialWindowMetrics {
    pub(crate) dip: Rect,
    scale: f64,
}

/// C# `_radialMenu` window state: whether it is on screen.
#[derive(Default)]
struct RadialState {
    visible: bool,
}

/// Everything the main-thread window job and the entity share.
struct RadialShared {
    bridge: Arc<MainThreadBridge>,
    settings: Arc<RwLock<AppSettings>>,
    /// C# `_radialMenu`'s executor — `UpdateActionExecutor` replaces it wholesale.
    executor: Mutex<Option<Arc<ActionExecutor>>>,
    state: Mutex<RadialState>,
    /// The open popup, so `Hide()` can close it from any thread's job.
    popup: Mutex<Option<WindowHandle<RadialMenuView>>>,
    /// The popup's placement, published for the executor's `action_bar_bounds`.
    window: Mutex<Option<RadialWindowMetrics>>,
    /// The popup the error path reports through (C# `new ResultTooltipWindow(...)`).
    tooltip: ResultTooltip,
    runtime: Arc<tokio::runtime::Runtime>,
    /// The window that owned the foreground while the pill was opening (the target
    /// of every action). C#'s `ShowActivated="False"` keeps that window activated
    /// through a click on the pill; GPUI's `PopUp` window answers `WM_MOUSEACTIVATE`
    /// with `MA_ACTIVATE` (no `WS_EX_NOACTIVATE`), so the click hands the pill the
    /// foreground and the executor's injected Ctrl+C / Ctrl+V / Delete keys would
    /// land on the pill instead of the target. `on_action_click` restores it.
    target_hwnd: Mutex<Option<isize>>,
}

/// The controller for the radial menu window. Cloned into the `UiHooks`
/// implementation and into the shared state the click handlers read; every clone is
/// a handle over the same `Arc`, so there is only ever one menu.
#[derive(Clone)]
pub struct RadialMenu {
    shared: Arc<RadialShared>,
}

/// One button as `PopulateBarButtons` renders it.
struct RadialButton {
    action: ActionProfile,
    show_icon: bool,
    show_text: bool,
    /// C# `action.Icon` rendered as raw text when no Lucide mapping exists.
    icon_fallback: Option<(String, &'static str)>,
    icon_size: f32,
}

/// The pill: one GPUI window, one root entity.
struct RadialMenuView {
    shared: Arc<RadialShared>,
    buttons: Vec<RadialButton>,
    theme_name: String,
    /// The controller, so the view can re-show the menu (C# the SelectAll branch).
    menu: RadialMenu,
    /// Set by a button's click handler so the pill's own listener can reproduce C#'s
    /// "did this click start on a Button ancestor?" test and stay out of the way.
    button_clicked: bool,
}

/// Recovers a poisoned lock — every lock in this port is held only for scalar
/// writes, so the data behind a poisoned lock is still valid.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl RadialMenu {
    /// Builds the controller. Called once by `ui::run`; the public surface stays the
    /// frozen `show_at_cursor` / `hide` / `is_visible`.
    pub(crate) fn new(
        bridge: Arc<MainThreadBridge>,
        settings: Arc<RwLock<AppSettings>>,
        tooltip: ResultTooltip,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            shared: Arc::new(RadialShared {
                bridge,
                settings,
                executor: Mutex::new(None),
                state: Mutex::new(RadialState::default()),
                popup: Mutex::new(None),
                window: Mutex::new(None),
                tooltip,
                runtime,
                target_hwnd: Mutex::new(None),
            }),
        }
    }

    /// C# `_radialMenu.UpdateActionExecutor(executor)` — `_actionExecutor = executor`
    /// wholesale, `None` included: a removed API key must drop the stored executor so
    /// the LLM guard reports "API key is not configured" again, exactly like the source.
    pub(crate) fn update_action_executor(&self, executor: Option<Arc<ActionExecutor>>) {
        *lock(&self.shared.executor) = executor;
    }

    /// `ShowAtCursor` — DIP conversion via platform::dpi then radial geometry.
    pub fn show_at_cursor(&self, screen_x: i32, screen_y: i32, editable: bool, has_text: bool) {
        let shared = Arc::clone(&self.shared);
        lock(&shared.state).visible = true;
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            shared.open_at(cx, screen_x, screen_y, editable, has_text);
        }));
    }

    pub fn hide(&self) {
        let shared = Arc::clone(&self.shared);
        let bridge = Arc::clone(&shared.bridge);
        bridge.post(Box::new(move |cx| {
            shared.close_window(cx);
        }));
    }

    pub fn is_visible(&self) -> bool {
        lock(&self.shared.state).visible
    }

    /// C# `ActionButton_Click`'s SelectAll branch: `await Task.Delay(100)`,
    /// `SimulateSelectAll()`, `await Task.Delay(150)`, then re-show the bar at the
    /// cursor with `isEditable: true, hasText: true`.
    fn show_at_cursor_deferred(&self) {
        let menu = self.clone();
        let runtime = Arc::clone(&self.shared.runtime);
        runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            clipboard::simulate_select_all();
            tokio::time::sleep(Duration::from_millis(150)).await;
            let point = crate::platform::win32::cursor_pos();
            menu.show_at_cursor(point.x, point.y, true, true);
        });
    }
}

impl RadialShared {
    /// C# `ShowAtCursor`: filter the buttons, measure the pill, place it, open it.
    fn open_at(
        self: &Arc<Self>,
        cx: &mut App,
        screen_x: i32,
        screen_y: i32,
        editable: bool,
        has_text: bool,
    ) {
        // A live popup is replaced rather than reused: `Show()` on an already open
        // WPF window just re-positions it, and the port has no move to call.
        self.close_window(cx);

        let settings = common::settings_snapshot(&self.settings);
        let buttons = populate_bar_buttons(&settings, editable, has_text);
        // C# `ShowAtCursor` always shows the pill and plays the sound, even when
        // `PopulateBarButtons` added no buttons (its early return is inside the
        // populate method, and its result is ignored by `ShowAtCursor`).
        tracing::debug!(
            screen_x,
            screen_y,
            editable,
            has_text,
            buttons = buttons.len(),
            "opening the radial menu"
        );

        let theme_name = common::resolved_theme_name(&settings);
        let pill = pill_size(cx, &buttons);

        let scale = crate::platform::dpi::scale_for_point(screen_x, screen_y);
        let (cursor_x, cursor_y) =
            crate::platform::dpi::to_dip(f64::from(screen_x), f64::from(screen_y), scale);
        let (work, display_id) = work_area_dip(cx, cursor_x, cursor_y);

        // C# `left = cursorX - ActualWidth / 2; top = cursorY - ActualHeight - 8;`
        let mut left = cursor_x - pill.width / 2.0;
        let mut top = cursor_y - pill.height - 8.0;

        // C# `left = Math.Max(workAreaLeft + 4, Math.Min(left, workAreaRight - ActualWidth - 4));`
        left = (work.left + 4.0).max(left.min(work.right - pill.width - 4.0));
        // C# `if (top < workAreaTop + 4) top = cursorY + 8;` — below-fallback.
        if top < work.top + 4.0 {
            top = cursor_y + 8.0;
        }

        let bounds = Bounds::new(
            gpui::point(px(left as f32), px(top as f32)),
            size(px(pill.width as f32), px(pill.height as f32)),
        );

        // C# `ShowActivated="False"` — show without stealing focus.
        // The display the pill was placed on must be named too: GPUI validates the
        // requested bounds against the display a window is created on, and with
        // `None` it always picks the primary one.
        let options = common::popup_window_options(bounds, true, false, display_id);
        let theme_name_for_view = theme_name.clone();
        let menu = RadialMenu {
            shared: Arc::clone(self),
        };
        let shared_ref = Arc::clone(self);
        match cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                RadialMenuView::new(menu, shared_ref, buttons, theme_name_for_view, window, cx)
            })
        }) {
            Ok(window) => {
                lock(&self.popup).replace(window);
                let metrics = RadialWindowMetrics {
                    dip: Rect::new(left, top, left + pill.width, top + pill.height),
                    scale,
                };
                lock(&self.window).replace(metrics);
                super::publish_radial_metrics(Some(metrics));
                tracing::info!(
                    left,
                    top,
                    width = pill.width,
                    height = pill.height,
                    "the radial menu window is open"
                );
                // The pill was shown without activation (`focus: false`), so the
                // window that owned the foreground is still the action target.
                // Remember it: a click on the pill activates this (GPUI `PopUp`)
                // window, and the button handler must hand the foreground back
                // before the executor injects Ctrl+C / Ctrl+V / Delete into it.
                *lock(&self.target_hwnd) = foreground_window();
                // `close_window` above replaced any live popup and cleared the                // visible flag with it; the source's `Show()` leaves a shown window
                // reporting `IsVisible == true`, so restore it now that the popup is
                // actually on screen. Without this, every later event handler's
                // `IsVisible` test reads false and the menu can never be dismissed.
                lock(&self.state).visible = true;
            }
            Err(error) => {
                tracing::warn!(%error, "could not open the radial menu window");
                lock(&self.state).visible = false;
                return;
            }
        }

        // C# `SoundHelper.PlayPopupSound();`
        sound::play_popup(settings.sound_enabled);
    }

    /// C# `Hide()` — closes the popup and clears the shared state.
    fn close_window(self: &Arc<Self>, cx: &mut App) {
        if let Some(handle) = lock(&self.popup).take() {
            let _ = handle.update(cx, |_: &mut RadialMenuView, window: &mut Window, _| {
                window.remove_window();
            });
        }
        tracing::debug!("closing the radial menu window");
        self.reset_state();
    }

    /// Clears the visibility/placement state without touching a window.
    fn reset_state(&self) {
        lock(&self.state).visible = false;
        lock(&self.window).take();
        super::publish_radial_metrics(None);
    }

    fn executor(&self) -> Option<Arc<ActionExecutor>> {
        lock(&self.executor).clone()
    }

    /// C# `new Rect(Left * toDeviceX, Top * toDeviceY, ActualWidth * toDeviceX,
    /// ActualHeight * toDeviceY)` — the window's DIP bounds converted back to device
    /// pixels (`Rect`'s C# constructor is x/y/width/height, ours is edges).
    fn action_bar_bounds(&self) -> Option<Rect> {
        let guard = lock(&self.window);
        let metrics = (*guard)?;
        let scale = metrics.scale;
        Some(Rect::new(
            metrics.dip.left * scale,
            metrics.dip.top * scale,
            metrics.dip.right * scale,
            metrics.dip.bottom * scale,
        ))
    }

    /// C# `Window_Deactivated`: a 100 ms grace period so a transient focus steal does
    /// not dismiss the popup, then hide if the window is still inactive.
    fn schedule_deactivate_dismiss(self: &Arc<Self>) {
        let shared = Arc::clone(self);
        self.runtime.spawn(async move {
            tokio::time::sleep(Duration::from_millis(100)).await;
            let bridge = Arc::clone(&shared.bridge);
            bridge.post(Box::new(move |cx| {
                let handle = lock(&shared.popup).as_ref().copied();
                let active = handle
                    .map(|handle| handle.is_active(cx).unwrap_or(false))
                    .unwrap_or(false);
                tracing::debug!(active, "the radial menu dismiss timer elapsed");
                if !active {
                    shared.close_window(cx);
                }
            }));
        });
    }

    /// Hands the foreground back to the action target after a pill click took it.
    ///
    /// C#'s `ShowActivated="False"` means the source's radial window is never
    /// activated, so the target keeps the foreground across the click and the
    /// executor's `SendInput` keystrokes reach it. GPUI's `PopUp` window activates
    /// on `WM_MOUSEACTIVATE`, so the port has to undo that itself: restoring the
    /// window recorded while the pill opened (the click handler runs while this
    /// process owns the foreground, which is what `SetForegroundWindow` requires).
    fn restore_target_foreground(&self) {
        let Some(target) = lock(&self.target_hwnd).take() else {
            return;
        };
        // SAFETY: the handle came from `GetForegroundWindow` in this process's
        // session and is only re-activated, never dereferenced.
        let current = unsafe { GetForegroundWindow() };
        if current.0 as isize != target {
            // SAFETY: `SetForegroundWindow` takes a window handle by value.
            let restored = unsafe { SetForegroundWindow(HWND(target as *mut std::ffi::c_void)) };
            tracing::debug!(
                target = target,
                ?restored,
                "restored the foreground to the action target"
            );
        }
    }
}

/// The foreground window's HWND as an `isize`, or `None` when the desktop has it.
fn foreground_window() -> Option<isize> {
    // SAFETY: `GetForegroundWindow` takes no arguments and only reads thread state.
    let hwnd = unsafe { GetForegroundWindow() };
    (!hwnd.0.is_null()).then_some(hwnd.0 as isize)
}

impl RadialMenuView {
    /// Builds the view and wires C# `Window_Deactivated`.
    fn new(
        menu: RadialMenu,
        shared: Arc<RadialShared>,
        buttons: Vec<RadialButton>,
        theme_name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let view = Self {
            shared: Arc::clone(&shared),
            buttons,
            theme_name,
            menu,
            button_clicked: false,
        };
        let shared_for_dismiss = Arc::clone(&shared);
        let _activation = cx.observe_window_activation(window, move |_view, window, _cx| {
            if window.is_window_active() {
                return;
            }
            tracing::debug!("the radial menu window lost activation; arming the dismiss timer");
            shared_for_dismiss.schedule_deactivate_dismiss();
        });
        view
    }

    /// C# `PillBorder_MouseDown` — a click that did not land on a button ancestor
    /// dismisses the popup.
    ///
    /// The source walks the visual tree from `e.OriginalSource` and returns early
    /// when it finds a `Button`, so a button click reaches only its own handler.
    /// GPUI hands a child's click to the parent's `on_click` listener as well, so the
    /// equivalent exclusion is done with a flag the button handlers set before the
    /// parent's listener runs (a child's handler is registered first and therefore
    /// fires first, mirroring the visual-tree walk).
    ///
    /// The close goes through the bridge rather than `close_window(cx)` directly:
    /// this handler runs while the pill window is already on GPUI's window update
    /// stack, and `update_window_erased` silently skips a window that is being
    /// updated (it is taken out of the slot for the dispatch), so the direct call
    /// would leave the window on screen forever.
    fn on_pill_click(&mut self, _: &gpui::ClickEvent, _: &mut Window, _cx: &mut Context<Self>) {
        if self.button_clicked {
            // A button handled this click; the source's visual-tree walk would have
            // returned early here.
            self.button_clicked = false;
            return;
        }
        self.menu.hide();
    }

    /// C# `ActionButton_Click`.
    fn on_action_click(
        &mut self,
        index: usize,
        _: &gpui::ClickEvent,
        window: &mut Window,
        _cx: &mut App,
    ) {
        let _ = window;
        let Some(button) = self.buttons.get(index) else {
            return;
        };
        let action = button.action.clone();

        // This click belongs to the button, so the pill's own listener must not
        // dismiss the popup for it a second time (C# `PillBorder_MouseDown`'s
        // Button-ancestor walk).
        self.button_clicked = true;

        // C# reads the window metrics after `Hide()` (WPF keeps Left/Top/ActualWidth
        // alive); the port's `close_window` clears them, so the same values are
        // captured first.
        let action_bar_bounds = self.shared.action_bar_bounds();

        // C# `Hide();` — posted through the bridge: this handler runs while the pill
        // window is on GPUI's update stack, and a direct `close_window` would be a
        // silent no-op (`update_window_erased` skips a window that is mid-update),
        // leaving the pill on screen.
        self.menu.hide();

        // The click on the pill activated it (GPUI's `PopUp` answers
        // `WM_MOUSEACTIVATE` with `MA_ACTIVATE`), unlike the source's
        // `ShowActivated="False"` window. Give the target window the foreground
        // back before any action runs: the executor's injected Ctrl+C / Ctrl+V /
        // Delete keystrokes must reach the target, not the closing pill.
        self.shared.restore_target_foreground();

        // Select All: simulate Ctrl+A, then re-show the action bar with full selection.
        if action_type_of(&action) == ActionType::SelectAll {
            self.menu.show_at_cursor_deferred();
            return;
        }

        let requires_llm = action_type_of(&action).requires_llm();
        let executor = self.shared.executor();
        if requires_llm {
            // C# `ActionButton_Click`'s guard reads the API key, not the executor:
            // the source always has an `ActionExecutorService` (with a null LLM
            // service when no key is configured) and shows the message box here.
            let snapshot = common::settings_snapshot(&self.shared.settings);
            if SettingsStore::get_api_key(&snapshot).is_empty() {
                // C# MessageBox.Show(Str_ApiKeyNotConfiguredDetail, Str_AppTitle, OK, Information).
                // The modal runs on a helper thread: an `rfd` dialog shown inside the
                // GPUI event dispatch pumps a nested message loop, and a foreground
                // task that wakes inside it re-borrows the app (`RefCell already
                // borrowed`), which aborts the process through the window proc.
                std::thread::spawn(|| {
                    common::info_box("Str_AppTitle", &i18n::get("Str_ApiKeyNotConfiguredDetail"));
                });
                return;
            }
        }
        let Some(executor) = executor else {
            return;
        };

        let selection_required = is_selection_required(&action);
        let tooltip_sink = self.shared.tooltip.clone();
        let runtime = Arc::clone(&self.shared.runtime);
        runtime.spawn(async move {
            let mut text = String::new();
            if selection_required {
                // C# `await Task.Delay(120); text = ClipboardHelper.GetSelectedText();`
                tokio::time::sleep(Duration::from_millis(120)).await;
                text = clipboard::get_selected_text();
                if text.trim().is_empty() {
                    // C# `if (string.IsNullOrWhiteSpace(text)) return;` — abort silently.
                    return;
                }
            }

            if let Err(message) = executor.execute(&action, &text, action_bar_bounds).await {
                // C# `catch (Exception ex)` → `new ResultTooltipWindow(Str_ErrorOccurred{ex.Message}, anchors)`
                let anchors = tooltip::cursor_anchors();
                let title = i18n::get_fmt("Str_ErrorOccurred", &[&message]);
                tooltip_sink.show(title, anchors);
            }
        });
    }
}

impl Render for RadialMenuView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = self.theme_name.clone();
        let bg = common::theme_color(&theme_name, "PillBgBrush");
        let border = common::theme_color(&theme_name, "PillBorderBrush");
        let fg = common::theme_color(&theme_name, "BtnFgBrush");
        let hover_bg = common::theme_color(&theme_name, "BtnHoverBgBrush");
        let pressed_bg = common::theme_color(&theme_name, "BtnPressedBgBrush");
        let hover_fg = common::theme_color(&theme_name, "BtnHoverFgBrush");
        // C# `Color.FromArgb(0x28, 0x70, 0x80, 0xFF)` — the PopulateBarButtons separator
        // (A=0x28, RGB #7080FF), in `gpui::rgba`'s 0xRRGGBBAA byte order.
        let separator = gpui::rgba(0x70_80_FF_28);

        let mut pill = div()
            .id("radial-pill")
            .size_full()
            .flex()
            .flex_row()
            .items_center()
            .rounded(px(22.0))
            .bg(bg)
            .border_1()
            .border_color(border)
            .px(px(bar::PILL_PADDING_X))
            .py(px(bar::PILL_PADDING_Y))
            // C# `PillBorder_MouseDown`: a click on the pill that is not on a Button
            // ancestor calls `Hide()`. The source walks the visual tree and returns
            // early when a Button ancestor is found, so a button click reaches only
            // `ActionButton_Click` (which hides the window itself). GPUI delivers a
            // child's click to the parent's listener as well, so the check is done
            // here through a flag the button handlers set before the parent sees it.
            .on_click(cx.listener(Self::on_pill_click));

        for (index, button) in self.buttons.iter().enumerate() {
            if index > 0 {
                // C# separator before every button except the first visible one.
                pill = pill.child(
                    div()
                        .w(px(bar::SEPARATOR_WIDTH))
                        .h(px(18.0))
                        .bg(separator)
                        .flex_shrink_0(),
                );
            }

            pill = pill.child(render_button(
                index, button, fg, hover_bg, pressed_bg, hover_fg, cx,
            ));
        }

        pill
    }
}

fn render_button(
    index: usize,
    button: &RadialButton,
    fg: gpui::Rgba,
    hover_bg: gpui::Rgba,
    pressed_bg: gpui::Rgba,
    hover_fg: gpui::Rgba,
    cx: &mut Context<RadialMenuView>,
) -> impl IntoElement {
    let mut content = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(bar::ICON_TEXT_GAP));

    if button.show_icon {
        if let Some(icon) = lucide_name(&button.action.icon) {
            // MDL2 → Lucide mapping: the pre-approved visual difference of this port.
            content = content.child(
                gpui::svg()
                    .path(format!("icons/{icon}.svg"))
                    .size(px(button.icon_size))
                    .text_color(fg)
                    .flex_shrink_0(),
            );
        } else if let Some((glyph, family)) = &button.icon_fallback {
            content = content.child(
                div()
                    .font_family(*family)
                    .text_size(px(button.icon_size))
                    .flex_shrink_0()
                    .child(glyph.clone()),
            );
        }
    }

    if button.show_text {
        // C# `FontSize="11"` on the button's TextBlock (the GPUI default rem would
        // render 16 px).
        content = content.child(
            div()
                .flex_shrink_0()
                .text_size(px(11.0))
                .child(button.action.name.clone()),
        );
    }

    div()
        .id(("radial-button", index))
        .h(px(bar::BUTTON_HEIGHT))
        .min_w(px(bar::BUTTON_MIN_WIDTH))
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .rounded(px(16.0))
        .px(px(bar::BUTTON_PADDING_X))
        .text_color(fg)
        .cursor_pointer()
        .child(content)
        .hover(move |style: StyleRefinement| style.bg(hover_bg).text_color(hover_fg))
        .active(move |style: StyleRefinement| style.bg(pressed_bg))
        .on_click(cx.listener(move |view, event, window, cx| {
            view.on_action_click(index, event, window, cx);
        }))
}

/// C# `PopulateBarButtons`, including the source's separator-before-the-selection-check
/// ordering.
fn populate_bar_buttons(
    settings: &AppSettings,
    is_editable: bool,
    has_text: bool,
) -> Vec<RadialButton> {
    let mut buttons = Vec::new();

    for action in &settings.actions {
        // Hide actions that write to the target document when the target is read-only.
        // C# `Paste || Cut || SimulateKey || Replace`.
        let requires_write = matches!(
            action_type_of(action),
            ActionType::Paste | ActionType::Cut | ActionType::SimulateKey | ActionType::Replace
        );
        if requires_write && !is_editable {
            continue;
        }

        let show_icon =
            !action.icon.is_empty() && action.display_mode != ButtonDisplayMode::TextOnly;
        let show_text = action.display_mode != ButtonDisplayMode::IconOnly;
        let icon_size = if show_icon && show_text {
            bar::ICON_SIZE_WITH_TEXT
        } else {
            bar::ICON_SIZE_ALONE
        };

        // Hide actions that need a text selection when nothing is selected.
        // The separator for this button is still laid out, exactly as the source does.
        if is_selection_required(action) && !has_text {
            continue;
        }

        let icon_fallback = if show_icon && lucide_name(&action.icon).is_none() {
            Some((action.icon.clone(), common::icon_font_family(&action.icon)))
        } else {
            None
        };

        buttons.push(RadialButton {
            action: action.clone(),
            show_icon,
            show_text,
            icon_fallback,
            icon_size,
        });
    }

    buttons
}

/// Measures the pill in DIPs with the constants the XAML declares.
fn pill_size(cx: &App, buttons: &[RadialButton]) -> Size {
    let mut width = 2.0 * bar::PILL_BORDER + 2.0 * bar::PILL_PADDING_X;

    for (index, button) in buttons.iter().enumerate() {
        if index > 0 {
            width += bar::SEPARATOR_WIDTH;
        }

        let mut content = 0.0;
        if button.show_icon {
            content += if let Some((glyph, family)) = &button.icon_fallback {
                f32::from(common::text_width(cx, glyph, button.icon_size, family))
            } else {
                // The Lucide icon is drawn at the XAML's glyph size.
                button.icon_size
            };
        }
        if button.show_icon && button.show_text {
            content += bar::ICON_TEXT_GAP;
        }
        if button.show_text {
            content += f32::from(common::text_width(
                cx,
                &button.action.name,
                11.0,
                common::UI_FONT_FAMILY,
            ));
        }

        width += bar::BUTTON_MIN_WIDTH.max(content + 2.0 * bar::BUTTON_PADDING_X);
    }

    let height = bar::BUTTON_HEIGHT + 2.0 * bar::PILL_PADDING_Y + 2.0 * bar::PILL_BORDER;
    Size::new(f64::from(width), f64::from(height))
}

/// C# `Screen.FromPoint(...).WorkingArea` in DIPs, resolved through GPUI's display
/// list (which reports the taskbar-excluding visible bounds directly), plus the id of
/// that display so `open_window` can be told which monitor to place the window on.
fn work_area_dip(cx: &App, cursor_x: f64, cursor_y: f64) -> (Rect, Option<gpui::DisplayId>) {
    let dip = |bounds: gpui::Bounds<Pixels>| {
        Rect::new(
            f64::from(bounds.left()),
            f64::from(bounds.top()),
            f64::from(bounds.right()),
            f64::from(bounds.bottom()),
        )
    };

    for display in cx.displays() {
        let work = dip(display.visible_bounds());
        if cursor_x >= work.left
            && cursor_x < work.right
            && cursor_y >= work.top
            && cursor_y < work.bottom
        {
            return (work, Some(display.id()));
        }
    }

    // No display claimed the point; `Screen.FromPoint` falls back to the nearest one,
    // which for a point outside every monitor is reported by GPUI as the primary.
    let primary = cx.primary_display();
    let rect = primary.as_ref().map_or_else(
        || Rect::new(0.0, 0.0, 0.0, 0.0),
        |display| dip(display.visible_bounds()),
    );
    (rect, primary.map(|display| display.id()))
}

/// C# `_radialMenu`'s placement, for `App.SystemHookManager_OnAnyMouseDown`'s
/// inside-window test on the hook side (workstream C reads this).
pub(crate) fn radial_bounds_dip() -> Option<Rect> {
    super::radial_metrics_dip()
}
