//! Action execution port of `Services/ActionExecutorService.cs` — gating guard,
//! utility dispatch (Browser/DirectCopy/Cut/Paste/SimulateKey), LLM dispatch
//! (Replace/Copy/Popup), selection-bounds via UIA rect union for Popup anchors,
//! sound after every path. Injected out-ports (Clipboard/Launcher/TooltipSink)
//! replace the statics; the source's `Dispatcher.Invoke` for tooltip show becomes
//! the TooltipSink call.

use std::sync::Arc;
use std::time::Duration;

use uiautomation::patterns::UITextRange;
use uiautomation::{UIAutomation, patterns::UITextPattern, types::Point as UiaPoint};
use windows::Win32::Foundation::POINT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, SAFEARRAY};
use windows::Win32::System::Ole::{
    SafeArrayDestroy, SafeArrayGetElement, SafeArrayGetLBound, SafeArrayGetUBound,
    SafeArrayGetVartype,
};
use windows::Win32::System::Variant::VT_R8;
use windows::Win32::UI::Accessibility::IUIAutomationTextRange;

use crate::action_type::ActionType;
use crate::llm::LlmClient;
use crate::placement::{AnchorContext, Point, Rect};
use crate::settings::{ActionProfile, AppSettings, action_type_of, is_selection_required};
use crate::sound;

/// Port of the ClipboardHelper calls the executor makes.
pub trait ClipboardPort: Send + Sync {
    fn get_selected_text(&self) -> String;
    fn copy_to_clipboard(&self, text: &str);
    fn replace_selected_text(&self, text: &str);
    fn delete_selected_text(&self);
    fn simulate_key(&self, key_name: &str);
    fn simulate_paste(&self);
}

/// Port of `Process.Start(UseShellExecute = true)` for the Browser action.
pub trait Launcher: Send + Sync {
    fn open_url(&self, url: &str);
}

/// Port of `new ResultTooltipWindow(result, anchors).Show()` on the dispatcher.
pub trait TooltipSink: Send + Sync {
    fn show_result(&self, result: String, anchors: AnchorContext);
}

/// Default ports over the free functions — constructed by `app` at startup.
pub struct ClipboardFns;
impl ClipboardPort for ClipboardFns {
    fn get_selected_text(&self) -> String {
        crate::clipboard::get_selected_text()
    }
    fn copy_to_clipboard(&self, text: &str) {
        crate::clipboard::copy_to_clipboard(text);
    }
    fn replace_selected_text(&self, text: &str) {
        crate::clipboard::replace_selected_text(text);
    }
    fn delete_selected_text(&self) {
        crate::clipboard::delete_selected_text();
    }
    fn simulate_key(&self, key_name: &str) {
        crate::clipboard::simulate_key(key_name);
    }
    fn simulate_paste(&self) {
        crate::clipboard::simulate_paste();
    }
}

/// `Process.Start(UseShellExecute = true)` equivalent.
pub struct ShellLauncher;
impl Launcher for ShellLauncher {
    fn open_url(&self, url: &str) {
        let _ = open::that(url);
    }
}

/// Placeholder sink used until `ui` wires a real tooltip window.
///
/// The result is never traced — it is arbitrary selected text the user acted on, so
/// only its length is reported. `ui` (workstream D) provides the real implementation
/// through `App::set_tooltip`.
pub struct LoggingTooltipSink;
impl TooltipSink for LoggingTooltipSink {
    fn show_result(&self, result: String, anchors: AnchorContext) {
        tracing::debug!(
            result_len = result.len(),
            cursor_dip = ?anchors.cursor_dip,
            has_selection = anchors.selection_dip.is_some(),
            has_action_bar = anchors.action_bar_dip.is_some(),
            "result tooltip requested (no tooltip window is wired)"
        );
    }
}

/// Port of `ActionExecutorService` — one action execution behind injected out-ports,
/// so the clipboard/launcher/tooltip the source reaches through its statics can be
/// recorded in tests.
pub struct ActionExecutor {
    llm: Option<Arc<dyn LlmClient>>,
    settings: Arc<std::sync::RwLock<AppSettings>>,
    clipboard: Arc<dyn ClipboardPort>,
    launcher: Arc<dyn Launcher>,
    tooltip: Arc<dyn TooltipSink>,
}

impl ActionExecutor {
    /// C# constructor — `None` llm mirrors `new ActionExecutorService(null)`.
    pub fn new(
        llm: Option<Arc<dyn LlmClient>>,
        settings: Arc<std::sync::RwLock<AppSettings>>,
        clipboard: Arc<dyn ClipboardPort>,
        launcher: Arc<dyn Launcher>,
        tooltip: Arc<dyn TooltipSink>,
    ) -> Self {
        Self {
            llm,
            settings,
            clipboard,
            launcher,
            tooltip,
        }
    }

    /// C# `HasLlmService` — `_llmService != null`.
    pub fn has_llm_service(&self) -> bool {
        self.llm.is_some()
    }

    /// C# `ExecuteAsync` — `Err("API key is not configured for LLM actions.")` mirrors
    /// the source's `InvalidOperationException` message when an LLM action has no client.
    pub async fn execute(
        &self,
        action: &ActionProfile,
        selected_text: &str,
        action_bar_bounds: Option<Rect>,
    ) -> Result<(), String> {
        if is_selection_required(action) && selected_text.is_empty() {
            // Defensive runtime guard
            return Ok(());
        }

        // Utility actions that never call the LLM — the source's first switch.
        match action_type_of(action) {
            ActionType::Browser => {
                let url = format!("{SEARCH_BASE}{}", escape_data_string(selected_text));
                self.launcher.open_url(&url);
                self.play_action_sound();
                return Ok(());
            }

            ActionType::DirectCopy => {
                self.clipboard.copy_to_clipboard(selected_text);
                self.play_action_sound();
                return Ok(());
            }

            ActionType::Cut => {
                self.clipboard.copy_to_clipboard(selected_text);
                self.clipboard.delete_selected_text();
                self.play_action_sound();
                return Ok(());
            }

            ActionType::Paste => {
                // C# `Thread.Sleep(150)` — allow focus to return to the target window
                // after the popup hides. A blocking sleep would stall the runtime.
                tokio::time::sleep(Duration::from_millis(150)).await;
                self.clipboard.simulate_paste();
                self.play_action_sound();
                return Ok(());
            }

            ActionType::SimulateKey => {
                // C# `action.PromptFormat ?? string.Empty`
                self.clipboard.simulate_key(&action.prompt_format);
                self.play_action_sound();
                return Ok(());
            }

            // Replace / Copy / Popup fall through to the LLM path below.
            ActionType::Replace | ActionType::Copy | ActionType::Popup => {}
            // SelectAll has neither a utility case nor an LLM case in the source, so it
            // falls through to the LLM path and finishes in its `default:` popup arm.
            ActionType::SelectAll => {}
        }

        // LLM actions — the source's second switch.
        let Some(llm) = self.llm.as_ref() else {
            return Err("API key is not configured for LLM actions.".to_string());
        };

        let prompt = action.prompt_format.replace("{text}", selected_text);
        let system_prompt = if action.clean_output {
            Some(CLEAN_OUTPUT_SYSTEM_PROMPT)
        } else {
            None
        };
        let result = llm
            .call_api(&prompt, system_prompt)
            .await
            .map_err(|error| error.to_string())?;

        match action_type_of(action) {
            ActionType::Replace => {
                self.clipboard.replace_selected_text(&result);
            }

            ActionType::Copy => {
                self.clipboard.copy_to_clipboard(&result);
            }

            ActionType::Popup | ActionType::SelectAll => {
                let cursor = crate::platform::win32::cursor_pos();
                let anchors = AnchorContext {
                    cursor_dip: Point::new(f64::from(cursor.x), f64::from(cursor.y)),
                    selection_dip: try_get_selection_bounds_at_cursor_dip(cursor),
                    action_bar_dip: action_bar_bounds,
                };
                self.tooltip.show_result(result, anchors);
            }

            // Utility actions returned above; the LLM switch only sees these three.
            ActionType::Browser
            | ActionType::DirectCopy
            | ActionType::Cut
            | ActionType::Paste
            | ActionType::SimulateKey => {}
        }

        self.play_action_sound();
        Ok(())
    }

    /// C# `SoundHelper.PlayActionSound()` — gated on `SettingsEnabled`, read live from
    /// the shared settings, exactly like the source's `SettingsManager.CurrentSettings`.
    fn play_action_sound(&self) {
        let sound_enabled = read_setting(&self.settings, |settings| settings.sound_enabled);
        sound::play_action(sound_enabled);
    }
}

/// Reads one flag out of the shared settings without holding the lock across an await.
fn read_setting(
    settings: &std::sync::RwLock<AppSettings>,
    pick: impl Fn(&AppSettings) -> bool,
) -> bool {
    match settings.read() {
        Ok(guard) => pick(&guard),
        // A poisoned lock must not take the app down; the last written value is read.
        Err(poisoned) => pick(&poisoned.into_inner()),
    }
}

/// C# `_llmService.CallApiAsync` system prompt for `CleanOutput`.
const CLEAN_OUTPUT_SYSTEM_PROMPT: &str = "Return only the processed result. Do not include any explanation, preamble, commentary, or extra text.";

/// C# `$"https://www.google.com/search?q={Uri.EscapeDataString(selectedText)}"`.
const SEARCH_BASE: &str = "https://www.google.com/search?q=";

/// C# `Uri.EscapeDataString` — RFC 3986 unreserved characters pass through untouched
/// (upper-case hex percent-encoding of every other byte of the UTF-8 form).
///
/// No percent-encoding crate is a dependency of this crate, so the escape is written
/// out here. The `.NET` implementation escapes exactly the characters that are not
/// unreserved per RFC 3986, which is what this reproduces.
fn escape_data_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            escaped.push(byte as char);
        } else {
            escaped.push('%');
            escaped.push(hex_digit(byte >> 4));
            escaped.push(hex_digit(byte & 0x0F));
        }
    }
    escaped
}

fn hex_digit(value: u8) -> char {
    match value {
        0..=9 => (b'0' + value) as char,
        _ => (b'A' + (value - 10)) as char,
    }
}

/// C# `TryGetSelectionBoundsAtCursorDip(Point cursorDip)` — the union of the first
/// text range's bounding rectangles under the cursor, or null on any failure.
///
/// `Cursor.Position` is reported by WinForms in **device pixels** even though the
/// source's local is named `cursorDip`; the numbers are passed through unchanged and
/// flagged in `PORT_STATUS.md`. `AutomationElement.FromPoint` takes physical screen
/// coordinates, so the same values are used for both calls in the source.
fn try_get_selection_bounds_at_cursor_dip(cursor: POINT) -> Option<Rect> {
    let automation = create_automation()?;
    let element = automation
        .element_from_point(UiaPoint::new(cursor.x, cursor.y))
        .ok()?;
    let pattern = element.get_pattern::<UITextPattern>().ok()?;
    let selection = pattern.get_selection().ok()?;
    let range = selection.into_iter().next()?;
    let rects = text_range_bounding_rectangles(&range);

    union_selection_rects(&rects)
}

/// Raw `[left, top, width, height]` quadruples, the layout `GetBoundingRectangles`
/// fills for each rectangle of a text range.
type RawRect = (f64, f64, f64, f64);

/// C# rect union of `TextPatternRange.GetBoundingRectangles()` — rectangles with a
/// non-positive width or height are skipped, `double.MaxValue` stays as the "nothing
/// was accumulated" sentinel, and the result is `new Rect((minX,minY),(maxX,maxY))`.
fn union_selection_rects(rects: &[RawRect]) -> Option<Rect> {
    let mut min_x = f64::MAX;
    let mut min_y = f64::MAX;
    let mut max_x = f64::MIN;
    let mut max_y = f64::MIN;

    for rect in rects {
        let (x, y, width, height) = *rect;
        if width <= 0.0 || height <= 0.0 {
            continue;
        }

        min_x = min_x.min(x);
        min_y = min_y.min(y);
        max_x = max_x.max(x + width);
        max_y = max_y.max(y + height);
    }

    if min_x == f64::MAX {
        return None;
    }

    Some(Rect::new(min_x, min_y, max_x, max_y))
}

/// `IUIAutomationTextRange.GetBoundingRectangles` as `[left, top, width, height]`
/// quadruples.
///
/// The `uiautomation` crate does not wrap that COM method, so the interface the
/// crate's `UITextRange` already exposes is called directly. The returned safe array
/// is a flat 1-D `VT_R8` array holding four doubles per rectangle — which is what the
/// .NET interop assembly marshals to `double[]`.
fn text_range_bounding_rectangles(range: &UITextRange) -> Vec<RawRect> {
    let raw: &IUIAutomationTextRange = range.as_ref();
    // SAFETY: `GetBoundingRectangles` only writes the returned array pointer, and
    // `raw` is a live COM interface obtained from the crate's own wrapper.
    let Ok(array) = (unsafe { raw.GetBoundingRectangles() }) else {
        return Vec::new();
    };
    if array.is_null() {
        return Vec::new();
    }

    let rects = safe_array_doubles(array)
        .as_chunks::<4>()
        .0
        .iter()
        .map(|quadruple| (quadruple[0], quadruple[1], quadruple[2], quadruple[3]))
        .collect();

    // SAFETY: `array` was returned by `GetBoundingRectangles` above and is still
    // unreleased; the safe array owns its own storage, so `SafeArrayDestroy` is the
    // matching release call. `SAFEARRAY` is used as the C pointer it is.
    let _ = unsafe { SafeArrayDestroy(array.cast()) };
    rects
}

/// Reads a 1-D `VT_R8` safe array into a `Vec<f64>`; anything else yields an empty
/// vector (the source's `(double[])rects` cast would throw instead).
///
/// The element type is checked first because `SafeArrayGetElement` copies
/// `sizeof(element)` bytes into the destination, and only `VT_R8` is 8 bytes.
fn safe_array_doubles(array: *const SAFEARRAY) -> Vec<f64> {
    // SAFETY: `array` is the live safe array returned by UIA above.
    let bounds = unsafe {
        let Ok(vartype) = SafeArrayGetVartype(array) else {
            return Vec::new();
        };
        if vartype != VT_R8 {
            return Vec::new();
        }
        let Ok(lower) = SafeArrayGetLBound(array, 1) else {
            return Vec::new();
        };
        let Ok(upper) = SafeArrayGetUBound(array, 1) else {
            return Vec::new();
        };
        (lower, upper)
    };
    let (lower, upper) = bounds;
    if upper < lower {
        return Vec::new();
    }

    let count = (upper - lower + 1) as usize;
    let mut values = Vec::with_capacity(count);
    for index in lower..=upper {
        let mut value: f64 = 0.0;
        // SAFETY: `index` is inside the array bounds and `value` is a live `f64`
        // whose address is passed as the element-sized destination for `VT_R8`.
        if unsafe { SafeArrayGetElement(array, &index, std::ptr::addr_of_mut!(value).cast()) }
            .is_err()
        {
            return Vec::new();
        }
        values.push(value);
    }
    values
}

/// Build a UIAutomation client, initializing COM on the calling thread when needed.
///
/// Mirrors the helper of the same name in `clipboard.rs`: `UIAutomation::new()`
/// hard-fails with `RPC_E_CHANGED_MODE` when the thread already lives in an STA
/// apartment, so COM is initialized tolerantly and the object is built directly.
fn create_automation() -> Option<UIAutomation> {
    // SAFETY: `CoInitializeEx` is per-thread bookkeeping and takes no pointers.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    UIAutomation::new_direct().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with_sound(enabled: bool) -> Arc<std::sync::RwLock<AppSettings>> {
        let mut settings = crate::settings::create_default_settings();
        settings.sound_enabled = enabled;
        Arc::new(std::sync::RwLock::new(settings))
    }

    // C# `Uri.EscapeDataString`.

    #[test]
    fn escape_data_string_keeps_the_rfc3986_unreserved_set() {
        assert_eq!(escape_data_string("Az09-._~"), "Az09-._~");
    }

    #[test]
    fn escape_data_string_upper_cases_the_percent_encoding() {
        assert_eq!(escape_data_string(" "), "%20");
        assert_eq!(escape_data_string("a+b"), "a%2Bb");
        assert_eq!(escape_data_string("?"), "%3F");
        assert_eq!(escape_data_string("&"), "%26");
        assert_eq!(escape_data_string("#"), "%23");
    }

    #[test]
    fn escape_data_string_percent_encodes_utf8_bytes() {
        // Written as an escape so the source of the bytes stays visible on any console.
        let hangul = char::from_u32(0xD55C).expect("valid scalar");
        assert_eq!(escape_data_string(&hangul.to_string()), "%ED%95%9C");
        assert_eq!(escape_data_string("caf\u{e9}"), "caf%C3%A9");
    }

    #[test]
    fn escape_data_string_escapes_the_reserved_sub_delims() {
        // `!`, `(`, `)` and `*` are not unreserved per RFC 3986.
        assert_eq!(escape_data_string("!()*"), "%21%28%29%2A");
    }

    /// Differential check against the real .NET 8 `Uri.EscapeDataString`.
    ///
    /// PORT_STATUS.md C6 flagged this re-implementation as "could not be verified
    /// against the source" because no .NET SDK was on the machine. One now is
    /// (.NET 8.0.425), and `Uri.EscapeDataString` was run over every printable
    /// ASCII code point and over the non-ASCII / control vectors below; the
    /// pass-through set it emits is exactly `A-Z a-z 0-9 - . _ ~`, and every
    /// other byte becomes an upper-case `%XX` of its UTF-8 form. This test pins
    /// that observed table.
    #[test]
    fn escape_data_string_matches_dotnet_for_every_printable_ascii_code_point() {
        for code in 0x20u8..=0x7E {
            // A lone printable ASCII byte is always valid UTF-8.
            let text = char::from(code).to_string();
            let expected =
                if code.is_ascii_alphanumeric() || matches!(code, b'-' | b'.' | b'_' | b'~') {
                    text.clone()
                } else {
                    format!("%{code:02X}")
                };
            assert_eq!(
                escape_data_string(&text),
                expected,
                "code point {code:#04X}"
            );
        }
    }

    #[test]
    fn escape_data_string_matches_dotnet_for_non_ascii_and_control_input() {
        // Each value below is the literal output observed from
        // `Uri.EscapeDataString` on .NET 8.0.425 for that same input.
        assert_eq!(escape_data_string("caf\u{e9}"), "caf%C3%A9");
        assert_eq!(escape_data_string("\u{4e2d}\u{6587}"), "%E4%B8%AD%E6%96%87");
        assert_eq!(escape_data_string("\u{1F600}"), "%F0%9F%98%80");
        assert_eq!(escape_data_string("\u{7}"), "%07");
        assert_eq!(escape_data_string("\u{FFFD}"), "%EF%BF%BD");
        assert_eq!(escape_data_string(""), "");
    }

    // C# rect union of GetBoundingRectangles.

    #[test]
    fn union_selection_rects_unions_every_line_rectangle() {
        let union =
            union_selection_rects(&[(100.0, 100.0, 50.0, 20.0), (100.0, 130.0, 40.0, 20.0)])
                .expect("a union");

        assert_eq!(union, Rect::new(100.0, 100.0, 150.0, 150.0));
    }

    #[test]
    fn union_selection_rects_skips_degenerate_rectangles() {
        let union = union_selection_rects(&[
            (0.0, 0.0, 0.0, 30.0),
            (0.0, 0.0, 30.0, 0.0),
            (10.0, 20.0, 30.0, 40.0),
        ])
        .expect("a union");

        assert_eq!(union, Rect::new(10.0, 20.0, 40.0, 60.0));
    }

    #[test]
    fn union_selection_rects_returns_none_when_every_rectangle_is_skipped() {
        assert!(union_selection_rects(&[]).is_none());
        assert!(union_selection_rects(&[(1.0, 2.0, 0.0, 5.0)]).is_none());
        assert!(union_selection_rects(&[(1.0, 2.0, -3.0, -4.0)]).is_none());
    }

    #[test]
    fn read_setting_returns_the_live_flag() {
        let settings = settings_with_sound(false);
        assert!(!read_setting(&settings, |settings| settings.sound_enabled));

        settings.write().expect("write").sound_enabled = true;
        assert!(read_setting(&settings, |settings| settings.sound_enabled));
    }
}
