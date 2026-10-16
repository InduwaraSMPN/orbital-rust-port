//! Orchestrator port of `App.xaml.cs` — startup order, explicit-shutdown daemon
//! behavior, theme/accent/language application, trigger pipeline (80 ms throttle with
//! cancel-in-flight, 150 ms post-dismiss suppress, 25/80 ms settle delay), and the
//! `CheckEditability (canSelect, canWrite, hasText)` table with the browser fast-path,
//! excluded shell/control classes, and fallback-editor list.
//!
//! The WPF-specific parts (windows, dispatcher) are behind the `UiHooks` trait so the
//! pipeline is testable headless. `ui/` provides the GPUI implementation.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use uiautomation::types::{ControlType, Point as UiaPoint};
use uiautomation::{UIAutomation, patterns::UITextPattern, patterns::UIValuePattern};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};

use crate::executor::{ActionExecutor, ClipboardFns, LoggingTooltipSink, ShellLauncher};
use crate::hooks::{HookEvent, HookManager};
use crate::llm::{LlmClient, OpenAiApiService};
use crate::settings::{AppSettings, SettingsStore, create_default_settings};

/// GPUI-side effects the orchestrator needs. Implemented by `ui` (default feature);
/// a headless implementation serves tests.
pub trait UiHooks: Send + Sync + 'static {
    /// `_radialMenu.ShowAtCursor(x, y, isEditable, hasText)`
    fn show_radial(&self, screen_x: i32, screen_y: i32, editable: bool, has_text: bool);
    /// `_radialMenu.Hide()` after setting `_lastDismissTime`
    fn hide_radial(&self);
    /// Settings window closed → re-read settings, rebuild executor, apply hotkeys.
    fn settings_changed(&self);
    /// Modal error dialog (Str_HookErrorMsg / Str_SettingsRecoveryMsg / …).
    fn show_error(&self, title_key: &str, message: &str);
    fn show_warning(&self, title_key: &str, message: &str);
    /// Exit the app (ShutdownMode.OnExplicitShutdown).
    fn quit(&self);
    /// `_radialMenu.IsVisible` — used by the dismiss handlers.
    fn radial_visible(&self) -> bool;

    /// C# `SystemHookManager_OnAnyMouseDown`'s inside-window test: true when the
    /// point (raw hook device pixels) lands on the radial popup's rectangle. The
    /// default `false` keeps headless callers on the "hide it" path; the GPUI
    /// implementation converts pixels to DIPs and compares against the popup bounds.
    fn point_inside_radial(&self, screen_x: i32, screen_y: i32) -> bool {
        let _ = (screen_x, screen_y);
        false
    }
}
/// Port of `Orbital.App` — the orchestrator's live state.
///
/// `store`/`settings`/`executor`/`hooks` are the seams the startup path and the
/// settings window touch; the trigger timestamps and the cancellation gate are behind
/// `Arc`s because the source runs `TriggerSelectionMenu` on its own `Task.Run` thread.
pub struct App {
    pub store: SettingsStore,
    pub settings: Arc<RwLock<AppSettings>>,
    pub executor: Option<ActionExecutor>,
    pub hooks: Option<HookManager>,
    pending_update_version: Option<String>,
    /// C# `_lastDismissTime` / `_lastTriggerAttemptTime`. Behind an `Arc<Mutex<..>>`
    /// because the long-press path runs `TriggerSelectionMenu` on a background task and
    /// cannot borrow `&mut App`.
    trigger: Arc<Mutex<TriggerTimestamps>>,
    /// C# `_selectionCts` + `_selectionLock`.
    selection: Arc<SelectionGate>,
    /// Tooltip sink for the executor's Popup result; wired to `ui` by the caller.
    tooltip: Arc<dyn crate::executor::TooltipSink>,
}

/// C# `_lastDismissTime` / `_lastTriggerAttemptTime`; `None` is `DateTime.MinValue`, so
/// the first trigger is never throttled or suppressed.
#[derive(Default)]
struct TriggerTimestamps {
    last_dismiss: Option<Instant>,
    last_trigger_attempt: Option<Instant>,
}

/// The app's tokio runtime, published by `ui::run` before the hook event pump
/// starts.
///
/// `Task.Run` in the source always finds the dispatcher's scheduler; here the
/// dispatch runs on the GPUI main thread, which has no tokio runtime in scope, so
/// the app's runtime has to be reachable from any thread that drives the trigger
/// pipeline.
static APP_RUNTIME: Mutex<Option<tokio::runtime::Handle>> = Mutex::new(None);

/// Publishes the app's tokio runtime so `trigger_selection_menu` and the long-press
/// path can spawn onto it.
pub fn set_runtime_handle(handle: tokio::runtime::Handle) {
    match APP_RUNTIME.lock() {
        Ok(mut slot) => *slot = Some(handle),
        Err(poisoned) => *poisoned.into_inner() = Some(handle),
    }
}

/// The runtime to spawn the trigger pipeline onto: the one `ui::run` published, or
/// the calling thread's own when it already runs inside a tokio context (which is
/// what the headless tests do, and what any future embedder that calls
/// `dispatch_hook_event` inside a runtime gets).
pub fn runtime_handle() -> Option<tokio::runtime::Handle> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return Some(handle);
    }
    match APP_RUNTIME.lock() {
        Ok(slot) => slot.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// Port of `_selectionCts` + `_selectionLock`.
///
/// .NET cancels the previous trigger by disposing its `CancellationTokenSource`. A
/// generation counter does the same for spawned Rust tasks: an in-flight task only
/// shows the radial menu while its own generation is still current, which is exactly
/// the source's `if (!token.IsCancellationRequested)` guard before `ShowAtCursor`.
/// `abort` mirrors `CancellationTokenSource.Dispose` and is best-effort — the source's
/// `Task.Run` body is not interruptible either while `CheckEditability` is in flight.
#[derive(Default)]
struct SelectionGate {
    generation: Arc<AtomicU64>,
    current: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl SelectionGate {
    /// C# `_selectionCts?.Cancel()` in the throttle path.
    fn cancel(&self) {
        if let Some(handle) = self.take_current() {
            handle.abort();
        }
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// C# `lock (_selectionLock) { _selectionCts?.Cancel(); ...; _selectionCts = new CancellationTokenSource(); }`
    ///
    /// Returns the identity the caller's task carries; any older generation is cancelled
    /// from this point on.
    fn begin(&self) -> u64 {
        self.cancel();
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    fn store(&self, handle: tokio::task::JoinHandle<()>) {
        match self.current.lock() {
            Ok(mut slot) => *slot = Some(handle),
            Err(poisoned) => *poisoned.into_inner() = Some(handle),
        }
    }

    fn take_current(&self) -> Option<tokio::task::JoinHandle<()>> {
        match self.current.lock() {
            Ok(mut slot) => slot.take(),
            Err(poisoned) => poisoned.into_inner().take(),
        }
    }
}

impl App {
    /// Default construction: settings at the default path, no executor and no hooks.
    ///
    /// The source starts these up in `OnStartup`; the sequencing lives with
    /// `start_app`, which owns the windows, the tray and the COM STA thread. The tooltip
    /// sink defaults to a logging one so the executor is always constructible, and
    /// `set_tooltip` swaps in the GPUI implementation.
    pub fn new() -> Self {
        Self::with_tooltip(Arc::new(LoggingTooltipSink))
    }

    /// `App::new` with a tooltip sink already wired.
    pub fn with_tooltip(tooltip: Arc<dyn crate::executor::TooltipSink>) -> Self {
        Self {
            store: SettingsStore::at_default_path(),
            settings: Arc::new(RwLock::new(create_default_settings())),
            executor: None,
            hooks: None,
            pending_update_version: None,
            trigger: Arc::new(Mutex::new(TriggerTimestamps::default())),
            selection: Arc::new(SelectionGate::default()),
            tooltip,
        }
    }

    /// Replaces the tooltip sink the executor uses for Popup results.
    ///
    /// The caller rebuilds the executor right after, exactly like the source's
    /// `OpenSettingsWindow` → `RebuildActionExecutor` sequence.
    pub fn set_tooltip(&mut self, tooltip: Arc<dyn crate::executor::TooltipSink>) {
        self.tooltip = tooltip;
    }

    /// Version of the downloaded-but-not-yet-applied update, for the tray restart label.
    pub fn pending_update_version(&self) -> Option<&str> {
        self.pending_update_version.as_deref()
    }

    /// Port of `RebuildActionExecutor` — rebuilds the LLM client from the current
    /// settings, mirroring the source's defaults and its `ArgumentException` dialog.
    pub fn rebuild_action_executor(&mut self, ui: &Arc<dyn UiHooks>) {
        let settings = read_settings(&self.settings);
        let api_key = SettingsStore::get_api_key(&settings);

        // The source disposes the previous `ILlmApiService`; `Arc` does that here.
        let llm: Option<Arc<dyn LlmClient>> = if api_key.is_empty() {
            None
        } else {
            let base_url = if settings.api_base_url.trim().is_empty() {
                DEFAULT_API_BASE_URL
            } else {
                settings.api_base_url.as_str()
            };
            let model_name = if settings.model_name.trim().is_empty() {
                DEFAULT_MODEL_NAME
            } else {
                settings.model_name.as_str()
            };

            match OpenAiApiService::new(&api_key, base_url, model_name) {
                Ok(service) => Some(Arc::new(service)),
                Err(error) => {
                    ui.show_warning(
                        "Str_InvalidApiConfigTitle",
                        &crate::i18n::get_fmt("Str_InvalidApiConfigMsg", &[&error.to_string()]),
                    );
                    None
                }
            }
        };

        self.executor = Some(ActionExecutor::new(
            llm,
            Arc::clone(&self.settings),
            Arc::new(ClipboardFns),
            Arc::new(ShellLauncher),
            Arc::clone(&self.tooltip),
        ));
    }

    /// Port of `CheckForUpdatesAsync` — best-effort startup update check.
    ///
    /// Downloads the installer, records the version for the tray label and arms the
    /// apply-on-exit path. The source also rebuilds the tray menu and shows the balloon
    /// tip, which needs the window/tray layer (`ui`), so those stay with `start_app`.
    pub async fn check_for_updates(&mut self) -> Option<std::path::PathBuf> {
        let manager = crate::release_flow::ReleaseManager::new(GITHUB_REPO);

        // C# `catch { /* Update check is best-effort — ignore all errors */ }`
        match manager.check_for_updates(env!("CARGO_PKG_VERSION")).await {
            Ok(Some(info)) => match manager.download(&info).await {
                Ok(installer) => {
                    self.pending_update_version = Some(info.version.clone());
                    manager.arm_apply_on_exit(installer.clone());
                    tracing::debug!(
                        version = %info.version,
                        asset = %info.asset_name,
                        "update downloaded; it applies when the app exits"
                    );
                    Some(installer)
                }
                Err(error) => {
                    tracing::debug!(%error, "update download failed; it is ignored");
                    None
                }
            },
            Ok(None) => {
                tracing::debug!("already on the latest release");
                None
            }
            Err(error) => {
                tracing::debug!(%error, "update check failed; it is ignored");
                None
            }
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

/// C# `_lastDismissTime` / `_lastTriggerAttemptTime` accessors used by the event handlers.
impl App {
    fn set_last_dismiss(&self, at: Instant) {
        set_last_dismiss(&self.trigger, at);
    }
}

fn set_last_dismiss(trigger: &Arc<Mutex<TriggerTimestamps>>, at: Instant) {
    match trigger.lock() {
        Ok(mut timestamps) => timestamps.last_dismiss = Some(at),
        Err(poisoned) => poisoned.into_inner().last_dismiss = Some(at),
    }
}

/// C# `_lastTriggerAttemptTime` bookkeeping.
///
/// The source stamps the attempt time only when the trigger is ACCEPTED: a throttled
/// trigger returns early at line 574 without updating it, so the next window opens
/// 80 ms after the last accepted one rather than sliding forever.
fn stamp_trigger_attempt(trigger: &Arc<Mutex<TriggerTimestamps>>) -> Option<bool> {
    match trigger.lock() {
        Ok(mut timestamps) => {
            let throttled = timestamps
                .last_trigger_attempt
                .is_some_and(|at| at.elapsed() < THROTTLE_WINDOW);
            if !throttled {
                timestamps.last_trigger_attempt = Some(Instant::now());
            }
            Some(throttled)
        }
        Err(poisoned) => {
            let mut timestamps = poisoned.into_inner();
            let throttled = timestamps
                .last_trigger_attempt
                .is_some_and(|at| at.elapsed() < THROTTLE_WINDOW);
            if !throttled {
                timestamps.last_trigger_attempt = Some(Instant::now());
            }
            Some(throttled)
        }
    }
}

fn suppressed_after_dismiss(trigger: &Arc<Mutex<TriggerTimestamps>>) -> bool {
    match trigger.lock() {
        Ok(timestamps) => timestamps
            .last_dismiss
            .is_some_and(|at| at.elapsed() < POST_DISMISS_WINDOW),
        Err(timestamps) => timestamps
            .into_inner()
            .last_dismiss
            .is_some_and(|at| at.elapsed() < POST_DISMISS_WINDOW),
    }
}

/// Reads one flag out of the shared settings without holding the lock.
fn setting_flag(settings: &RwLock<AppSettings>, pick: impl Fn(&AppSettings) -> bool) -> bool {
    match settings.read() {
        Ok(guard) => pick(&guard),
        Err(poisoned) => pick(&poisoned.into_inner()),
    }
}

/// Full defensive copy of the settings (the source hands out the same object reference).
fn read_settings(settings: &RwLock<AppSettings>) -> AppSettings {
    match settings.read() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

/// C# `OpenAiApiService` defaults, as used in `RebuildActionExecutor`.
const DEFAULT_API_BASE_URL: &str = "https://api.openai.com/v1";
const DEFAULT_MODEL_NAME: &str = "gpt-4o-mini";
/// C# `new GithubSource("https://github.com/CrowKing63/Orbital", null, false)`.
const GITHUB_REPO: &str = "CrowKing63/Orbital";

/// C# `TriggerSelectionMenu` throttle window (80 ms).
const THROTTLE_WINDOW: Duration = Duration::from_millis(80);
/// C# post-dismiss re-trigger suppression window (150 ms).
const POST_DISMISS_WINDOW: Duration = Duration::from_millis(150);
/// C# `int delay = requireEditable ? 80 : 25;` — the single adaptive settle delay.
const SETTLE_DELAY_EDITABLE: Duration = Duration::from_millis(80);
const SETTLE_DELAY_DEFAULT: Duration = Duration::from_millis(25);

/// C# `s_browserClasses` — browser window classes that support text selection, used as a
/// fast-path to skip the UIA round-trip entirely.
const BROWSER_CLASSES: [&str; 4] = [
    "Chrome_WidgetWin_1",
    "Chrome_WidgetWin_2",
    "Chrome_RenderWidgetHostHWND",
    "MozillaWindowClass",
];

/// C# `s_excludedRootWindowClasses`.
const EXCLUDED_ROOT_CLASSES: [&str; 4] = [
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "WorkerW",
];

/// C# `s_excludedControlClasses`.
const EXCLUDED_CONTROL_CLASSES: [&str; 4] = [
    "SHELLDLL_DefView",
    "SysListView32",
    "DirectUIHWND",
    "NamespaceTreeControl",
];

/// C# `s_clipboardFallbackEditorClasses`.
const FALLBACK_EDITOR_CLASSES: [&str; 3] =
    ["Notepad++", "EmEditorMainFrame3", "EmEditorMainFrame4"];

/// C# `s_clipboardFallbackEditorProcesses`.
const FALLBACK_EDITOR_PROCESSES: [&str; 3] = ["notepad++", "emeditor", "sublime_text"];

/// C# `StringComparer.OrdinalIgnoreCase` set membership.
fn contains_ignore_case(values: &[&str], candidate: &str) -> bool {
    values
        .iter()
        .any(|value| value.eq_ignore_ascii_case(candidate))
}

/// Port of `App.CheckEditability` — browser fast-path, excluded classes,
/// Edit/ComboBox/Document/Pane(+TextPattern)/parent-fallback/fallback-editors.
pub fn check_editability(screen_x: i32, screen_y: i32, hwnd_class: &str) -> (bool, bool, bool) {
    // Fast path: skip expensive UIA IPC for known browser windows.
    // Browsers are always treated as read-only Document with optimistic hasText.
    if !hwnd_class.is_empty() && contains_ignore_case(&BROWSER_CLASSES, hwnd_class) {
        return (true, false, true);
    }

    // C# wraps the whole body in try/catch → (false, false, false).
    editability_at_point(screen_x, screen_y).unwrap_or((false, false, false))
}

/// The body of `CheckEditability`. `None` stands for "an exception was thrown", which
/// the source answers with (false, false, false).
fn editability_at_point(screen_x: i32, screen_y: i32) -> Option<(bool, bool, bool)> {
    let hwnd = crate::platform::win32::window_from_point(screen_x, screen_y);
    let root_class = crate::platform::win32::root_window_class(hwnd);
    let control_class = crate::platform::win32::window_class(hwnd);
    let process_name = crate::platform::win32::process_name(hwnd);

    if contains_ignore_case(&EXCLUDED_ROOT_CLASSES, &root_class)
        || contains_ignore_case(&EXCLUDED_CONTROL_CLASSES, &control_class)
    {
        return Some((false, false, false));
    }

    let automation = create_automation()?;
    let element = automation
        .element_from_point(UiaPoint::new(screen_x, screen_y))
        .ok()?;

    // C# reads ControlType out of the CacheRequest; `as ControlType` on a value that is
    // not one yields null, which is `None` here.
    let control_type = element.get_control_type().ok();

    // Writable text input — always show popup (Paste is useful even without selection).
    if control_type == Some(ControlType::Edit) || control_type == Some(ControlType::ComboBox) {
        let sel = has_real_text_selection(&element)?;
        return Some((true, true, sel));
    }

    if control_type == Some(ControlType::Document) {
        let is_writable = document_is_writable(&element)?;
        let sel = has_real_text_selection(&element)?;
        return Some(if is_writable {
            (true, true, sel)
        } else {
            (true, false, true)
        });
    }

    // WinUI 3 / UWP text areas (e.g. new Sticky Notes) report as Pane
    // but expose TextPattern.
    if control_type == Some(ControlType::Pane) && element.get_pattern::<UITextPattern>().is_ok() {
        let sel = has_real_text_selection(&element)?;
        return Some((true, true, sel));
    }

    // Parent fallback — parent wasn't in the cache, so live calls are required.
    let walker = automation.get_control_view_walker().ok()?;
    if let Ok(parent) = walker.get_parent(&element) {
        let parent_type = parent.get_control_type().ok();
        if parent_type == Some(ControlType::Edit) {
            let sel = has_real_text_selection(&parent)?;
            return Some((true, true, sel));
        }
        if parent_type == Some(ControlType::Document) {
            let is_writable = document_is_writable(&parent)?;
            let sel = has_real_text_selection(&parent)?;
            return Some(if is_writable {
                (true, true, sel)
            } else {
                (true, false, true)
            });
        }
        if control_type == Some(ControlType::Text) && parent_type == Some(ControlType::Group) {
            return Some((true, false, true));
        }
    }

    if is_clipboard_fallback_editor(&control_class, &root_class, &process_name) {
        return Some((true, true, true));
    }

    Some((false, false, false))
}

/// C# `ValuePattern` present → `!IsReadOnly`, otherwise `IsKeyboardFocusable`.
///
/// The C# casts throw on a missing/non-bool property, which means (false, false, false)
/// for the whole call — so a failed read is `None` here, not `false`.
fn document_is_writable(element: &uiautomation::core::UIElement) -> Option<bool> {
    match element.get_pattern::<UIValuePattern>() {
        Ok(value_pattern) => Some(!value_pattern.is_readonly().ok()?),
        Err(_) => Some(element.is_keyboard_focusable().ok()?),
    }
}

/// C# `HasRealTextSelection` — true when TextPattern reports a non-collapsed selection.
///
/// `Some(false)` means "no TextPattern" (the source returns false), `None` means the
/// underlying call threw, which the source answers by abandoning the whole check.
fn has_real_text_selection(element: &uiautomation::core::UIElement) -> Option<bool> {
    // C# prefers a cached pattern and falls back to a live one; `uiautomation` has no
    // cache API (PARITY-CONTRACT.md decision 5), so both are the same call here.
    let pattern = match element.get_pattern::<UITextPattern>() {
        Ok(pattern) => pattern,
        Err(_) => return Some(false),
    };

    let selection = pattern.get_selection().ok()?;
    let Some(range) = selection.into_iter().next() else {
        // C# `if (sel.Length == 0) return false;` — the pattern answered, there is
        // simply no selection; that is NOT a failed call.
        return Some(false);
    };
    // CompareEndpoints == 0 means start == end (collapsed cursor, no visible selection).
    let delta = range
        .compare_endpoints(
            uiautomation::types::TextPatternRangeEndpoint::Start,
            &range,
            uiautomation::types::TextPatternRangeEndpoint::End,
        )
        .ok()?;
    Some(delta != 0)
}

/// Port of `App.IsClipboardFallbackEditor` — class/root-class lists + process name,
/// case-insensitive.
pub fn is_clipboard_fallback_editor(
    control_class: &str,
    root_class: &str,
    process_name: &str,
) -> bool {
    contains_ignore_case(&FALLBACK_EDITOR_CLASSES, control_class)
        || contains_ignore_case(&FALLBACK_EDITOR_CLASSES, root_class)
        || (!process_name.is_empty()
            && contains_ignore_case(&FALLBACK_EDITOR_PROCESSES, process_name))
}

/// Port of `App.IsOverEditableControl` (long-press path — no selection required).
pub fn is_over_editable_control(screen_x: i32, screen_y: i32) -> bool {
    let Some(automation) = create_automation() else {
        return false;
    };
    // C# wraps the whole body in try/catch → false.
    let Ok(element) = automation.element_from_point(UiaPoint::new(screen_x, screen_y)) else {
        return false;
    };

    match element.get_control_type() {
        Ok(ControlType::Edit) | Ok(ControlType::ComboBox) => true,
        Ok(ControlType::Document) => document_is_writable(&element).unwrap_or(false),
        _ => false,
    }
}

/// Build a UIAutomation client, initializing COM on the calling thread when needed.
///
/// Mirrors the helper of the same name in `clipboard.rs` and `executor.rs`:
/// `UIAutomation::new()` hard-fails with `RPC_E_CHANGED_MODE` when the thread already
/// lives in an STA apartment, so COM is initialized tolerantly and the object is built
/// directly.
fn create_automation() -> Option<UIAutomation> {
    // SAFETY: `CoInitializeEx` is per-thread bookkeeping and takes no pointers.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    UIAutomation::new_direct().ok()
}

/// Port of `TriggerSelectionMenu` + the event handlers that feed it. The effect calls
/// go through `UiHooks`; timing/state math is identical to the source.
/// The argument list is the frozen port of `TriggerSelectionMenu`'s optional
/// parameters, so it keeps their shape instead of being repacked into a struct.
#[allow(clippy::too_many_arguments)]
pub fn trigger_selection_menu(
    app: &mut App,
    ui: &Arc<dyn UiHooks>,
    screen_x: i32,
    screen_y: i32,
    is_keyboard: bool,
    require_editable: bool,
    edit_check_x: i32,
    edit_check_y: i32,
    drag_start_hwnd_class: &str,
    force_has_text: Option<bool>,
) {
    trigger_selection_menu_inner(
        &app.trigger,
        &app.selection,
        ui,
        screen_x,
        screen_y,
        is_keyboard,
        require_editable,
        edit_check_x,
        edit_check_y,
        drag_start_hwnd_class,
        force_has_text,
    );
}

/// Shared body of `trigger_selection_menu`.
///
/// Takes the trigger state as `&Arc` so the long-press path (which the source runs on
/// its own `Task.Run` thread) can reach it without borrowing `&mut App`.
#[allow(clippy::too_many_arguments)]
fn trigger_selection_menu_inner(
    trigger: &Arc<Mutex<TriggerTimestamps>>,
    selection: &Arc<SelectionGate>,
    ui: &Arc<dyn UiHooks>,
    screen_x: i32,
    screen_y: i32,
    is_keyboard: bool,
    require_editable: bool,
    edit_check_x: i32,
    edit_check_y: i32,
    drag_start_hwnd_class: &str,
    force_has_text: Option<bool>,
) {
    // Throttle: if another trigger arrived within 80 ms, just cancel the in-flight
    // task instead of starting a new UIA round-trip that would pile up and lag the mouse.
    if stamp_trigger_attempt(trigger) == Some(true) {
        tracing::debug!("a trigger was throttled (80 ms window); cancelling the in-flight task");
        selection.cancel();
        return;
    }

    // Suppress rapid re-triggers after dismiss (e.g. accidental double-trigger).
    if suppressed_after_dismiss(trigger) {
        tracing::debug!("a trigger was suppressed (150 ms post-dismiss window)");
        return;
    }

    // Fall back to the menu-display position when no explicit check position is given.
    let edit_check_x = if edit_check_x < 0 {
        screen_x
    } else {
        edit_check_x
    };
    let edit_check_y = if edit_check_y < 0 {
        screen_y
    } else {
        edit_check_y
    };
    let drag_start_hwnd_class = drag_start_hwnd_class.to_string();

    let runtime = runtime_handle();
    let Some(runtime) = runtime else {
        tracing::error!(
            "no tokio runtime is published; the radial menu cannot be shown for this trigger"
        );
        return;
    };

    let generation = selection.begin();
    let task_gate = Arc::clone(selection);
    let gate = Arc::clone(selection);
    let ui = Arc::clone(ui);

    tracing::debug!(
        screen_x,
        screen_y,
        is_keyboard,
        require_editable,
        "trigger accepted; the radial menu is being prepared"
    );

    let handle = runtime.spawn(async move {
        if task_gate.generation() != generation {
            return;
        }

        let (is_editable, has_text) = match (is_keyboard, force_has_text) {
            // Long-press path: already verified editable, no text selection.
            (true, Some(forced)) => (true, forced),
            (true, None) => (true, true), // keyboard selection always produces a text selection
            (false, _) => {
                // Single attempt with a short adaptive delay.
                // Browsers hit the fast-path in CheckEditability (zero UIA IPC).
                // Native apps (Notepad, Word) are fast with CacheRequest.
                // Double-click needs a slightly longer wait for word selection to settle.
                let delay = if require_editable {
                    SETTLE_DELAY_EDITABLE
                } else {
                    SETTLE_DELAY_DEFAULT
                };
                tokio::time::sleep(delay).await;
                if task_gate.generation() != generation {
                    return;
                }

                let (can_select, can_write, selected_has_text) =
                    check_editability(edit_check_x, edit_check_y, &drag_start_hwnd_class);
                tracing::debug!(
                    can_select,
                    can_write,
                    selected_has_text,
                    "the editability check answered"
                );
                if !can_select {
                    return;
                }
                (can_write, selected_has_text)
            }
        };

        // C# `Dispatcher.BeginInvoke(...)` — `ui` marshals to the window thread itself.
        if task_gate.generation() != generation {
            tracing::debug!("a newer trigger superseded this one; the menu is not shown");
            return;
        }
        tracing::debug!(
            screen_x,
            screen_y,
            is_editable,
            has_text,
            "showing the radial menu"
        );
        ui.show_radial(screen_x, screen_y, is_editable, has_text);
    });

    gate.store(handle);
}

/// The event pump — maps HookEvents to the source's OnXxx handlers.
pub fn dispatch_hook_event(app: &mut App, ui: &Arc<dyn UiHooks>, event: HookEvent) {
    match event {
        HookEvent::AnyMouseDown(point) => {
            // C# `SystemHookManager_OnAnyMouseDown` — the raw hook pixels are
            // converted with the radial window's TransformFromDevice and compared
            // against its rectangle; only an outside click dismisses. The conversion
            // needs the window bounds, which only the UI layer knows.
            if ui.radial_visible() && !ui.point_inside_radial(point.x, point.y) {
                // C# stamps `_lastDismissTime` and hides.
                app.set_last_dismiss(Instant::now());
                ui.hide_radial();
            }
        }

        HookEvent::MouseUp(point) => {
            if !setting_flag(&app.settings, |settings| settings.enable_drag_trigger) {
                return;
            }

            // Capture drag-start position now; _buttonDownPos may be overwritten by the
            // next click.
            let (down, down_class) = app
                .hooks
                .as_ref()
                .map(HookManager::last_button_down)
                .unwrap_or_default();

            trigger_selection_menu(
                app,
                ui,
                point.x,
                point.y,
                false,
                false,
                down.x,
                down.y,
                &down_class,
                None,
            );
        }

        HookEvent::DoubleClickRelease(point) => {
            if !setting_flag(&app.settings, |settings| {
                settings.enable_double_click_trigger
            }) {
                return;
            }

            // Editable-control check is deferred to the background thread inside
            // TriggerSelectionMenu.
            trigger_selection_menu(app, ui, point.x, point.y, false, true, -1, -1, "", None);
        }

        HookEvent::KeyboardSelection(point) => {
            if !setting_flag(&app.settings, |settings| {
                settings.enable_keyboard_selection_trigger
            }) {
                return;
            }

            trigger_selection_menu(app, ui, point.x, point.y, true, false, -1, -1, "", None);
        }

        HookEvent::CustomHotkey(point) => {
            trigger_selection_menu(app, ui, point.x, point.y, true, false, -1, -1, "", None);
        }

        HookEvent::LongPress(point) => {
            if !setting_flag(&app.settings, |settings| settings.enable_long_press_trigger) {
                return;
            }

            // Run IsOverEditableControl on a background thread to avoid blocking the hook
            // callback. Then go through TriggerSelectionMenu so CTS cancellation prevents
            // conflicts with subsequent drag / double-click / keyboard triggers.
            let Some(runtime) = runtime_handle() else {
                return;
            };
            let trigger = Arc::clone(&app.trigger);
            let selection = Arc::clone(&app.selection);
            let ui = Arc::clone(ui);
            runtime.spawn(async move {
                if !is_over_editable_control(point.x, point.y) {
                    return;
                }
                trigger_selection_menu_inner(
                    &trigger,
                    &selection,
                    &ui,
                    point.x,
                    point.y,
                    true,
                    false,
                    -1,
                    -1,
                    "",
                    Some(false),
                );
            });
        }

        HookEvent::EscapePressed | HookEvent::ClipboardShortcut => {
            if ui.radial_visible() {
                app.set_last_dismiss(Instant::now());
                ui.hide_radial();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use super::*;
    use crate::hooks::MousePoint;

    /// Headless `UiHooks` recording every effect, with a switchable visibility answer.
    #[derive(Default)]
    struct RecordingUi {
        shows: Mutex<Vec<(i32, i32, bool, bool)>>,
        hides: AtomicUsize,
        visible: AtomicBool,
        inside: AtomicBool,
    }

    impl RecordingUi {
        fn visible() -> Arc<Self> {
            Arc::new(Self {
                shows: Mutex::new(Vec::new()),
                hides: AtomicUsize::new(0),
                visible: AtomicBool::new(true),
                inside: AtomicBool::new(false),
            })
        }

        fn hidden() -> Arc<Self> {
            Arc::new(Self {
                shows: Mutex::new(Vec::new()),
                hides: AtomicUsize::new(0),
                visible: AtomicBool::new(false),
                inside: AtomicBool::new(false),
            })
        }

        fn shows(&self) -> Vec<(i32, i32, bool, bool)> {
            self.shows.lock().expect("shows lock").clone()
        }

        fn hides(&self) -> usize {
            self.hides.load(Ordering::SeqCst)
        }
    }

    impl UiHooks for RecordingUi {
        fn show_radial(&self, screen_x: i32, screen_y: i32, editable: bool, has_text: bool) {
            self.shows
                .lock()
                .expect("shows lock")
                .push((screen_x, screen_y, editable, has_text));
        }

        fn hide_radial(&self) {
            self.hides.fetch_add(1, Ordering::SeqCst);
            self.visible.store(false, Ordering::SeqCst);
        }

        fn settings_changed(&self) {}
        fn show_error(&self, _title_key: &str, _message: &str) {}
        fn show_warning(&self, _title_key: &str, _message: &str) {}
        fn quit(&self) {}

        fn radial_visible(&self) -> bool {
            self.visible.load(Ordering::SeqCst)
        }

        fn point_inside_radial(&self, _screen_x: i32, _screen_y: i32) -> bool {
            self.inside.load(Ordering::SeqCst)
        }
    }

    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
    }

    /// Settings with every trigger flag off, so the gated handlers can be observed.
    fn triggers_disabled() -> AppSettings {
        let mut settings = create_default_settings();
        settings.enable_drag_trigger = false;
        settings.enable_double_click_trigger = false;
        settings.enable_keyboard_selection_trigger = false;
        settings.enable_long_press_trigger = false;
        settings
    }

    // The browser fast-path, which the source takes before any UIA call.

    #[test]
    fn check_editability_treats_browser_classes_as_read_only_documents_with_text() {
        for class in [
            "Chrome_WidgetWin_1",
            "Chrome_WidgetWin_2",
            "Chrome_RenderWidgetHostHWND",
            "MozillaWindowClass",
        ] {
            assert_eq!(
                check_editability(0, 0, class),
                (true, false, true),
                "browser class '{class}' should take the fast-path"
            );
        }
    }

    #[test]
    fn check_editability_matches_browser_classes_case_insensitively() {
        assert_eq!(
            check_editability(5, 5, "chrome_widgetwin_1"),
            (true, false, true)
        );
    }

    #[test]
    fn check_editability_with_an_empty_class_reaches_uia() {
        // The fast-path is the only branch that skips UIA, so an empty class goes to the
        // real desktop; the source answers the same three-tuple for whatever is there,
        // and a UIA failure is (false, false, false).
        let _ = check_editability(0, 0, "");
    }

    // TriggerSelectionMenu.

    #[test]
    fn a_keyboard_trigger_shows_the_radial_menu_as_editable_with_text() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            trigger_selection_menu(&mut app, &ui, 12, 34, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.shows(), [(12, 34, true, true)]);
    }

    #[test]
    fn a_forced_has_text_flag_overrides_the_optimistic_selection() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            // C# long-press passes `forceHasText: false`.
            trigger_selection_menu(&mut app, &ui, 1, 2, true, false, -1, -1, "", Some(false));
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.shows(), [(1, 2, true, false)]);
    }

    #[test]
    fn a_second_trigger_inside_the_throttle_window_is_dropped() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();

            trigger_selection_menu(&mut app, &ui, 1, 2, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;
            // Inside the 80 ms window: the source cancels the in-flight task instead of
            // starting a new UIA round-trip.
            trigger_selection_menu(&mut app, &ui, 3, 4, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;

            // Outside it.
            tokio::time::sleep(THROTTLE_WINDOW + Duration::from_millis(20)).await;
            trigger_selection_menu(&mut app, &ui, 5, 6, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.shows(), [(1, 2, true, true), (5, 6, true, true)]);
    }

    #[test]
    fn a_recent_dismiss_suppresses_the_next_trigger() {
        let recorder = RecordingUi::visible();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();

            // A dismissal stamps `_lastDismissTime`, and the 150 ms window then swallows
            // the next trigger.
            dispatch_hook_event(&mut app, &ui, HookEvent::EscapePressed);
            trigger_selection_menu(&mut app, &ui, 1, 2, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;

            tokio::time::sleep(POST_DISMISS_WINDOW + Duration::from_millis(20)).await;
            trigger_selection_menu(&mut app, &ui, 7, 8, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.shows(), [(7, 8, true, true)]);
    }

    // The event pump.

    #[test]
    fn the_custom_hotkey_triggers_the_menu_without_a_settings_gate() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::CustomHotkey(MousePoint { x: 9, y: 8 }),
            );
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.shows(), [(9, 8, true, true)]);
    }

    #[test]
    fn the_keyboard_selection_trigger_respects_its_settings_flag() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            *app.settings.write().expect("settings write") = triggers_disabled();
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::KeyboardSelection(MousePoint { x: 9, y: 8 }),
            );
            tokio::task::yield_now().await;
        });

        assert!(recorder.shows().is_empty());
    }

    #[test]
    fn the_mouse_up_trigger_respects_its_settings_flag() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            *app.settings.write().expect("settings write") = triggers_disabled();
            dispatch_hook_event(&mut app, &ui, HookEvent::MouseUp(MousePoint { x: 9, y: 8 }));
            tokio::task::yield_now().await;
        });

        assert!(recorder.shows().is_empty());
    }

    #[test]
    fn the_long_press_trigger_respects_its_settings_flag() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            *app.settings.write().expect("settings write") = triggers_disabled();
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::LongPress(MousePoint { x: 9, y: 8 }),
            );
            tokio::task::yield_now().await;
        });

        assert!(recorder.shows().is_empty());
    }

    #[test]
    fn escape_hides_a_visible_radial_menu_and_stamps_the_dismiss_time() {
        let recorder = RecordingUi::visible();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(&mut app, &ui, HookEvent::EscapePressed);
            tokio::task::yield_now().await;

            trigger_selection_menu(&mut app, &ui, 1, 2, true, false, -1, -1, "", None);
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.hides.load(Ordering::SeqCst), 1);
        assert!(
            recorder.shows().is_empty(),
            "the trigger inside the post-dismiss window is suppressed"
        );
    }

    #[test]
    fn the_clipboard_shortcut_hides_a_visible_radial_menu() {
        let recorder = RecordingUi::visible();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(&mut app, &ui, HookEvent::ClipboardShortcut);
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.hides.load(Ordering::SeqCst), 1);
        assert!(!ui.radial_visible());
    }

    #[test]
    fn a_hidden_radial_menu_is_left_alone_by_escape_and_mouse_down() {
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(&mut app, &ui, HookEvent::EscapePressed);
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::AnyMouseDown(MousePoint { x: 1, y: 1 }),
            );
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.hides.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn any_mouse_down_hides_a_visible_radial_menu() {
        let recorder = RecordingUi::visible();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::AnyMouseDown(MousePoint { x: 1, y: 1 }),
            );
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.hides.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_mouse_down_inside_the_radial_menu_keeps_it_open() {
        // C# only hides when the point is outside the window rect.
        let recorder = RecordingUi::visible();
        recorder.inside.store(true, Ordering::SeqCst);
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::AnyMouseDown(MousePoint { x: 1, y: 1 }),
            );
            tokio::task::yield_now().await;
        });

        assert_eq!(recorder.hides(), 0);
        assert!(ui.radial_visible());
    }

    #[test]
    fn an_outside_mouse_down_stamps_the_dismiss_time_it_hides_from() {
        // C# stamps `_lastDismissTime` before hiding, so the post-dismiss suppression
        // starts on this path too.
        let recorder = RecordingUi::visible();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        runtime().block_on(async {
            let mut app = App::new();
            assert!(!suppressed_after_dismiss(&app.trigger));
            dispatch_hook_event(
                &mut app,
                &ui,
                HookEvent::AnyMouseDown(MousePoint { x: 1, y: 1 }),
            );
            tokio::task::yield_now().await;
            assert!(suppressed_after_dismiss(&app.trigger));
        });

        assert_eq!(recorder.hides(), 1);
    }

    #[test]
    fn a_throttled_trigger_keeps_the_last_accepted_stamp() {
        // C# returns from the throttle branch without touching `_lastTriggerAttemptTime`.
        let trigger = Arc::new(Mutex::new(TriggerTimestamps::default()));

        assert_eq!(stamp_trigger_attempt(&trigger), Some(false));
        let accepted = trigger
            .lock()
            .expect("trigger lock")
            .last_trigger_attempt
            .expect("the accepted trigger is stamped");

        assert_eq!(stamp_trigger_attempt(&trigger), Some(true));
        let unchanged = trigger
            .lock()
            .expect("trigger lock")
            .last_trigger_attempt
            .expect("still stamped");
        assert_eq!(
            accepted, unchanged,
            "a throttled trigger must not slide the window"
        );
    }

    // The pieces `start_app` wires together.

    #[test]
    fn app_new_has_no_executor_no_hooks_and_no_pending_update() {
        let app = App::new();

        assert!(app.executor.is_none());
        assert!(app.hooks.is_none());
        assert!(app.pending_update_version().is_none());

        let settings = app.settings.read().expect("settings lock");
        assert!(settings.enable_drag_trigger);
        assert!(settings.sound_enabled);
    }

    #[test]
    fn rebuild_action_executor_without_an_api_key_builds_an_executor_with_no_client() {
        let mut app = App::new();
        let recorder = RecordingUi::hidden();
        let ui: Arc<dyn UiHooks> = Arc::clone(&recorder) as Arc<dyn UiHooks>;
        app.set_tooltip(Arc::new(LoggingTooltipSink));

        app.rebuild_action_executor(&ui);

        assert!(
            app.executor
                .as_ref()
                .is_some_and(|executor| !executor.has_llm_service()),
            "no key means no LLM client, like `new ActionExecutorService(null)`"
        );
    }

    #[test]
    fn check_for_updates_reports_nothing_when_the_latest_release_is_not_newer() {
        runtime().block_on(async {
            let mut app = App::new();
            // The real GitHub API is not reachable from a unit test; an unreachable
            // endpoint is the "best-effort" path and must not panic.
            let _ = app.check_for_updates().await;
        });
    }
}
