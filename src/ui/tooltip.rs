//! ResultTooltipWindow port — GPUI popup. Anchors DPI-normalized (px→DIP) then
//! `placement::calculate`; auto-close timer from settings; copy button writes the
//! clipboard; close stops the timer.
//!
//! The window's width is the XAML's `Width="380"`; its height is measured (the source
//! gets it from `SizeToContent` + `ActualHeight`). `PORT_STATUS.md` (2026-10-12,
//! workstream D) records the measurement as a target-side decision; the placement
//! math and the auto-close semantics are the source's.
//!
//! C# `ResultText` is a read-only `TextBox` with `TextWrapping="Wrap"`,
//! `FontSize="13"`, `MaxHeight="320"` and `VerticalScrollBarVisibility="Auto"`.
//! The body renders as gpui-kit's read-only `Textarea` at the measured body height
//! (capped at `TEXT_MAX_HEIGHT`), so a long result scrolls inside the element — the
//! `Auto` scrollbar — and the text stays selectable, like the source.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use gpui::{
    App, Bounds, Context, Entity, Pixels, Render, Window, WindowHandle, div, prelude::*, px, size,
};

use crate::placement::{self, AnchorContext, Point, PopupPlacementMode, Rect, Size};
use crate::settings::AppSettings;

use super::bridge::MainThreadBridge;
use super::common::{self, tip};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

struct TooltipShared {
    bridge: Arc<MainThreadBridge>,
    settings: Arc<RwLock<AppSettings>>,
    popup: Mutex<Option<WindowHandle<TooltipView>>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

/// The tooltip controller. One instance lives for the whole app, like the source's
/// `new ResultTooltipWindow(...).Show()` per result.
#[derive(Clone)]
pub struct ResultTooltip {
    inner: Arc<TooltipShared>,
}

struct TooltipView {
    result: String,
    theme_name: String,
    /// C# `_autoCloseTimer` — cleared when the close button or the timer closes the
    /// window, so a second close is a no-op.
    auto_close: Option<Arc<AtomicBool>>,
    /// C# `ResultText` — the read-only result text as a gpui-kit `TextareaState`.
    result_text: Entity<gpui_kit::component::input::TextareaState>,
    /// C# `TextBox.MaxHeight="320"`-capped body height, so the element scrolls
    /// internally instead of being clipped.
    result_height: Pixels,
}

impl ResultTooltip {
    pub(crate) fn new(
        bridge: Arc<MainThreadBridge>,
        settings: Arc<RwLock<AppSettings>>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            inner: Arc::new(TooltipShared {
                bridge,
                settings,
                popup: Mutex::new(None),
                runtime,
            }),
        }
    }

    /// C# `new ResultTooltipWindow(result, anchors).Show()`
    pub fn show(&self, result: String, anchors: crate::placement::AnchorContext) {
        let inner = Arc::clone(&self.inner);
        let bridge = Arc::clone(&inner.bridge);
        bridge.post(Box::new(move |cx| {
            inner.show_on_main_thread(cx, result, anchors);
        }));
    }

    /// C# `Close()` from the close button, the auto-close tick, and any re-show.
    pub(crate) fn close(&self) {
        let inner = Arc::clone(&self.inner);
        let bridge = Arc::clone(&inner.bridge);
        bridge.post(Box::new(move |cx| {
            inner.close_on_main_thread(cx);
        }));
    }
}

impl TooltipShared {
    fn show_on_main_thread(self: &Arc<Self>, cx: &mut App, result: String, anchors: AnchorContext) {
        // A live tooltip is replaced, exactly like the source's fresh window per result.
        self.close_on_main_thread(cx);

        let settings = common::settings_snapshot(&self.settings);
        let theme_name = common::resolved_theme_name(&settings);
        let point = crate::platform::win32::cursor_pos();

        // C# `PresentationSource...TransformFromDevice` on the tooltip window — the
        // plan's DPI normalization of the device-pixel anchors the executor reports.
        let scale = crate::platform::dpi::scale_for_point(point.x, point.y);
        let (cursor_x, cursor_y) =
            crate::platform::dpi::to_dip(f64::from(point.x), f64::from(point.y), scale);
        // C# `Screen.FromPoint(cursorPosition).WorkingArea`, read at the tooltip's
        // load time exactly like the source.
        let _cursor_origin = gpui::point(px(cursor_x as f32), px(cursor_y as f32));

        let normalized = AnchorContext {
            cursor_dip: scale_anchor(anchors.cursor_dip, scale),
            selection_dip: anchors.selection_dip.map(|r| scale_rect(r, scale)),
            action_bar_dip: anchors.action_bar_dip.map(|r| scale_rect(r, scale)),
        };

        // C# `TextBox.MaxHeight="320"` — the body height the read-only text element
        // gets; `tooltip_size` reserves the same value, so the window is sized for
        // exactly what is rendered.
        let body_height = text_height(cx, &result);
        let popup_size = tooltip_size(body_height);
        let (work, display_id) = work_area_dip(cx, cursor_x, cursor_y);
        let position =
            placement::calculate_position(settings.popup_placement, normalized, work, popup_size);

        let bounds = Bounds::new(
            gpui::point(px(position.x as f32), px(position.y as f32)),
            size(px(popup_size.width as f32), px(popup_size.height as f32)),
        );

        // C# `if (SettingsManager.CurrentSettings.PopupAutoCloseEnabled) { ... }`
        let auto_close = if settings.popup_auto_close_enabled {
            let flag = Arc::new(AtomicBool::new(false));
            let inner = Arc::clone(self);
            let cancel = Arc::clone(&flag);
            let seconds = i64::from(settings.popup_auto_close_seconds);
            self.runtime.spawn(async move {
                if seconds > 0 {
                    tokio::time::sleep(Duration::from_secs(seconds as u64)).await;
                }
                // C# `_autoCloseTimer.Stop(); Close();`
                if !cancel.load(Ordering::SeqCst) {
                    let tooltip = Arc::new(ResultTooltip { inner });
                    tooltip.close();
                }
            });
            Some(flag)
        } else {
            None
        };

        let options = common::popup_window_options(bounds, true, false, display_id);
        match cx.open_window(options, |window, cx| {
            cx.new(|cx| {
                // C# `<TextBox x:Name="ResultText" ... IsReadOnly="True"
                // TextWrapping="Wrap" FontSize="13" MaxHeight="320"
                // VerticalScrollBarVisibility="Auto"/>`
                let result_text = cx.new(|cx| {
                    gpui_kit::component::input::TextareaState::new(window, cx)
                        .soft_wrap(true)
                        .default_value(result.clone())
                });
                TooltipView {
                    result,
                    theme_name,
                    auto_close,
                    result_text,
                    result_height: px(body_height),
                }
            })
        }) {
            Ok(window) => {
                lock(&self.popup).replace(window);
            }
            Err(error) => tracing::warn!(%error, "could not open the result tooltip window"),
        }
    }

    fn close_on_main_thread(&self, cx: &mut App) {
        if let Some(handle) = lock(&self.popup).take() {
            let _ = handle.update(cx, |view: &mut TooltipView, window: &mut Window, _| {
                // Cancelling the auto-close task first is what `Stop()` does before `Close()`.
                if let Some(flag) = &view.auto_close {
                    flag.store(true, Ordering::SeqCst);
                }
                window.remove_window();
            });
        }
    }
}

impl Render for TooltipView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme_name = self.theme_name.clone();
        let shell = common::theme_color(&theme_name, "PillBgBrush");
        let border = common::theme_color(&theme_name, "PillBorderBrush");
        let header_bg = common::theme_color(&theme_name, "BgPanel");
        let title_fg = common::theme_color(&theme_name, "TitleFgBrush");
        let text_fg = common::theme_color(&theme_name, "TextHigh");
        let low_fg = common::theme_color(&theme_name, "TextLow");
        let mid_fg = common::theme_color(&theme_name, "TextMid");
        let element_bg = common::theme_color(&theme_name, "BgElement");
        let element_border = common::theme_color(&theme_name, "Border");
        let hover_bg = common::theme_color(&theme_name, "BgHover");
        let accent = common::theme_color(&theme_name, "AccentViColor");
        let accent_cy = common::theme_color(&theme_name, "AccentCyColor");

        // Header: brand dot (gradient) + name, then the close button on the right.
        let header = div()
            .flex()
            .flex_row()
            .items_center()
            .justify_between()
            .bg(header_bg)
            .px(px(tip::HEADER_PADDING_X))
            .py(px(tip::HEADER_PADDING_Y))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .child(div().size(px(tip::BRAND_DOT)).rounded_full().bg(accent_cy))
                    .child(
                        div()
                            .text_size(px(tip::BRAND_FONT_SIZE))
                            .font_weight(gpui::FontWeight::BOLD)
                            .text_color(title_fg)
                            .child(crate::i18n::get("Str_AppTitle")),
                    ),
            )
            .child(
                div()
                    .id("tooltip-close")
                    .size(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(6.0))
                    .text_size(px(tip::BRAND_FONT_SIZE))
                    .text_color(low_fg)
                    .cursor_pointer()
                    .child("\u{2715}")
                    .hover(move |style: gpui::StyleRefinement| {
                        style.bg(hover_bg).text_color(title_fg)
                    })
                    .on_click(cx.listener(|view, _, window, cx| {
                        if let Some(flag) = &view.auto_close {
                            flag.store(true, Ordering::SeqCst);
                        }
                        let _ = cx;
                        window.remove_window();
                    })),
            );

        // Separator across the accent at half opacity (C# `Opacity="0.5"`).
        let separator = div()
            .h(px(tip::SEPARATOR_HEIGHT))
            .bg(accent)
            .opacity(0.5)
            .flex_shrink_0();

        // Body: the read-only result text (C# `ResultText`).
        let result_text = gpui_kit::component::input::Textarea::new(&self.result_text)
            .readonly(true)
            .appearance(false)
            .bordered(false)
            .h(self.result_height)
            .text_size(px(tip::TEXT_FONT_SIZE))
            .line_height(px(common::text_line_height(tip::TEXT_FONT_SIZE)))
            .text_color(text_fg);
        let body = div()
            .flex()
            .flex_col()
            .px(px(tip::BODY_PADDING_LEFT))
            .pt(px(tip::BODY_PADDING_TOP))
            .pb(px(tip::BODY_PADDING_BOTTOM))
            .child(result_text)
            .child(
                div()
                    .id("tooltip-copy")
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .gap(px(tip::COPY_ICON_GAP))
                    .mt(px(tip::COPY_BUTTON_MARGIN_TOP))
                    .px(px(tip::COPY_BUTTON_PADDING_X))
                    .py(px(tip::COPY_BUTTON_PADDING_Y))
                    .rounded(px(7.0))
                    .border_1()
                    .border_color(element_border)
                    .bg(element_bg)
                    .text_size(px(tip::COPY_BUTTON_FONT_SIZE))
                    .text_color(mid_fg)
                    .cursor_pointer()
                    .child(
                        gpui::svg()
                            .path("icons/copy.svg")
                            .size(px(tip::COPY_ICON_SIZE))
                            .text_color(accent),
                    )
                    .child(crate::i18n::get("Str_CopyToClipboard"))
                    .hover(move |style: gpui::StyleRefinement| {
                        style.bg(hover_bg).border_color(accent).text_color(title_fg)
                    })
                    .on_click(cx.listener(|view, _, _, cx| {
                        // C# `Clipboard.SetText(ResultText.Text);`
                        crate::clipboard::copy_to_clipboard(&view.result);
                        let _ = cx;
                    })),
            );

        div()
            .id("result-tooltip")
            .size_full()
            .rounded(px(14.0))
            .bg(shell)
            .border_1()
            .border_color(border)
            .flex()
            .flex_col()
            .font_family(common::UI_FONT_FAMILY)
            .child(header)
            .child(separator)
            .child(body)
    }
}

/// C# `OnLoaded`'s anchor scaling (device px → DIP).
fn scale_anchor(point: Point, scale: f64) -> Point {
    if scale == 0.0 {
        point
    } else {
        Point::new(point.x / scale, point.y / scale)
    }
}

/// C# `new Rect(selectionPx.X * dpiX, selectionPx.Y * dpiY, selectionPx.Width * dpiX, selectionPx.Height * dpiY)`
/// with `dpiX = 96/dpi = 1/scale`: device pixels → DIPs, so every edge divides by the
/// scale factor (WPF's `Rect` is left/top/width/height; ours is edges).
fn scale_rect(rect: Rect, scale: f64) -> Rect {
    if scale == 0.0 {
        rect
    } else {
        Rect::new(
            rect.left / scale,
            rect.top / scale,
            rect.right / scale,
            rect.bottom / scale,
        )
    }
}

/// C# `TextBox.MaxHeight="320"` — the wrapped text's height, capped at the XAML's
/// 320, measured with the source's own constants (`Width="380"`, body padding,
/// `FontSize="13"`). The window height and the text element both use this value.
fn text_height(cx: &App, result: &str) -> f32 {
    let text_width = tip::WINDOW_WIDTH - tip::BODY_PADDING_LEFT - tip::BODY_PADDING_RIGHT;
    let line_height = common::text_line_height(tip::TEXT_FONT_SIZE);
    let lines = common::wrap_lines(cx, result, tip::TEXT_FONT_SIZE, px(text_width));
    if result.is_empty() {
        0.0
    } else {
        (lines.len() as f32 * line_height).min(tip::TEXT_MAX_HEIGHT)
    }
}

/// Measures the tooltip with the XAML's constants: `Width="380"`,
/// `TextBox.MaxHeight="320"`, `TextWrapping="Wrap"`. `text_height` is the capped
/// body height `text_height` measured.
fn tooltip_size(text_height: f32) -> Size {
    let header_height = 2.0 * tip::HEADER_PADDING_Y
        + (tip::BRAND_FONT_SIZE * common::text_line_height(tip::BRAND_FONT_SIZE))
            .max(tip::BRAND_DOT);
    let copy_height = 2.0 * tip::COPY_BUTTON_PADDING_Y
        + tip::COPY_BUTTON_FONT_SIZE * common::text_line_height(tip::COPY_BUTTON_FONT_SIZE);

    let height = 2.0 * tip::BORDER
        + header_height
        + tip::SEPARATOR_HEIGHT
        + tip::BODY_PADDING_TOP
        + text_height
        + tip::COPY_BUTTON_MARGIN_TOP
        + copy_height
        + tip::BODY_PADDING_BOTTOM;

    Size::new(f64::from(tip::WINDOW_WIDTH), f64::from(height))
}

/// C# `Screen.FromPoint(cursorPosition).WorkingArea` in DIPs, plus the id of that
/// display so `open_window` places the popup on the right monitor.
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

    let primary = cx.primary_display();
    let rect = primary.as_ref().map_or_else(
        || Rect::new(0.0, 0.0, 0.0, 0.0),
        |display| dip(display.visible_bounds()),
    );
    (rect, primary.map(|display| display.id()))
}

/// The executor's tooltip port, so `ActionExecutor` can show results without knowing
/// about GPUI windows.
pub(crate) struct TooltipSinkImpl {
    tooltip: ResultTooltip,
}

impl TooltipSinkImpl {
    pub(crate) fn new(tooltip: ResultTooltip) -> Self {
        Self { tooltip }
    }
}

impl crate::executor::TooltipSink for TooltipSinkImpl {
    fn show_result(&self, result: String, anchors: AnchorContext) {
        self.tooltip.show(result, anchors);
    }
}

/// C# `PopupAnchorContext` for the radial error path.
pub(crate) fn cursor_anchors() -> AnchorContext {
    let point = crate::platform::win32::cursor_pos();
    AnchorContext {
        cursor_dip: Point::new(f64::from(point.x), f64::from(point.y)),
        selection_dip: None,
        action_bar_dip: None,
    }
}

/// The placement mode names, in the order `SettingsWindow.xaml` lists them.
pub(crate) fn placement_modes() -> [(PopupPlacementMode, &'static str); 4] {
    [
        (
            PopupPlacementMode::BottomRight,
            "Str_PopupPositionBottomRight",
        ),
        (
            PopupPlacementMode::NearCursor,
            "Str_PopupPositionNearCursor",
        ),
        (
            PopupPlacementMode::NearSelection,
            "Str_PopupPositionNearSelection",
        ),
        (
            PopupPlacementMode::NearActionBar,
            "Str_PopupPositionNearActionBar",
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_rect_divides_every_edge_by_the_scale_factor() {
        // C# converts device pixels to DIPs with 96/dpi = 1/scale.
        let scaled = scale_rect(Rect::new(100.0, 200.0, 300.0, 400.0), 2.0);
        assert_eq!(scaled, Rect::new(50.0, 100.0, 150.0, 200.0));

        // The size is preserved in DIP space — the direction matters for
        // near_selection_or_fallback, which rejects non-positive widths.
        let origin = scale_rect(Rect::new(0.0, 0.0, 100.0, 50.0), 2.0);
        assert_eq!(origin.width(), 50.0);
        assert_eq!(origin.height(), 25.0);
    }
}
