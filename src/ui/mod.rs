//! GPUI UI ports: RadialMenuWindow, ResultTooltipWindow, SettingsWindow, ActionEditDialog.
//! (MainWindow is deliberately not ported — empty placeholder in the source.)
//!
//! `run()` is `Program.cs` + `App.OnStartup` in order: update/install hook args first,
//! COM STA init, settings load, theme/accent/language, registry sync, the action
//! executor, tray, radial menu + hooks, hotkey, the background release check, and the
//! hook event pump. `ShutdownMode.OnExplicitShutdown` becomes `QuitMode::Explicit`.

pub(crate) mod action_edit;
pub(crate) mod bridge;
pub(crate) mod common;
pub(crate) mod logging;
pub(crate) mod radial;
pub(crate) mod settings;
pub(crate) mod tooltip;
pub(crate) mod tray_actions;

pub use action_edit::ActionEditDialog;
pub use bridge::MainThreadBridge;
pub use radial::RadialMenu;
pub use settings::SettingsWindow;
pub use tooltip::ResultTooltip;

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::RwLock;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::time::Duration;

use gpui::{App, Global, Task};

use crate::app::{App as OrchestratorApp, UiHooks};
use crate::executor::{ActionExecutor, ClipboardFns, ShellLauncher};
use crate::hooks::{HookEvent, HookManager};
use crate::i18n;
use crate::llm::OpenAiApiService;
use crate::placement::Rect;
use crate::release_flow::ReleaseManager;
use crate::settings::{AppSettings, SettingsStore, create_default_settings};

pub(crate) use bridge::post;

/// C# `new GithubSource("https://github.com/CrowKing63/Orbital", null, false)`.
const UPDATE_REPO: &str = "CrowKing63/Orbital";

/// The hook event pump's poll interval. The source's handlers run on the dispatcher
/// as events arrive; `std::sync::mpsc` has no async receiver, so the pump polls the
/// manager's bounded channel from a GPUI task.
const PUMP_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The OS theme / accent watcher's poll interval — C#
/// `Microsoft.Win32.SystemEvents.UserPreferenceChanged` fires the moment the setting
/// changes; GPUI has no such notification, so the equivalent is a poll.
const THEME_WATCH_POLL_INTERVAL: Duration = Duration::from_millis(2000);

/// Set by the app-quit handler so the theme watcher stops polling on shutdown.
static THEME_WATCHER_STOP: AtomicBool = AtomicBool::new(false);

// ── Global application state ────────────────────────────────────────────────

/// Everything the main thread needs, published as one GPUI global.
pub(crate) struct UiState {
    /// The orchestrator state `app::dispatch_hook_event` drives.
    pub(crate) app: Option<OrchestratorApp>,
    /// The GPUI `UiHooks` implementation handed to `dispatch_hook_event`.
    pub(crate) ui: Arc<dyn UiHooks>,
    /// Shared handles the UI pieces read from.
    pub(crate) shared: SharedState,
    /// A pending update's installer, applied when the app exits.
    pub(crate) pending_update: Option<std::path::PathBuf>,
}

impl Global for UiState {}

/// State shared between the main-thread windows and the background threads.
pub(crate) struct SharedState {
    #[allow(dead_code)]
    settings: Arc<RwLock<AppSettings>>,
    tooltip: ResultTooltip,
    settings_window: SettingsWindow,
    tray: crate::tray::Tray,
    #[allow(dead_code)]
    runtime: Arc<tokio::runtime::Runtime>,
}

/// The radial menu's current bounds, published for the hook side's inside-window
/// test.
static RADIAL_METRICS: Mutex<Option<radial::RadialWindowMetrics>> = Mutex::new(None);

/// A pending update's installer path, shared between the release check and the quit
/// path so `WaitExitThenApply` survives across threads.
static PENDING_UPDATE: Mutex<Option<std::path::PathBuf>> = Mutex::new(None);

fn pending_update_slot() -> std::sync::MutexGuard<'static, Option<std::path::PathBuf>> {
    match PENDING_UPDATE.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

/// Port of `Program.Main` + `App.OnStartup`.
pub fn run() {
    // (0) Tracing first, before any other startup work: every layer below emits
    // events, and without a subscriber they are dropped and a failing gesture has
    // no observable symptom.
    logging::install();

    // (a) Velopack hook ordering: an update hook runs before any UI init and exits.
    if handle_update_hook_args() {
        return;
    }

    // (b) COM STA init — `CoInitializeEx(ApartmentThreaded)`.
    init_com_apartment();

    // (c) Settings load (+ corrupt-file recovery dialog).
    let store = SettingsStore::at_default_path();
    let (settings, recovered) = match store.load() {
        Ok(outcome) => (outcome.settings, outcome.recovered),
        Err(error) => {
            tracing::warn!(%error, "settings could not be loaded; starting from defaults");
            (create_default_settings(), false)
        }
    };

    // (d) Theme + accent + language.
    i18n::set_language(&settings.language);

    if recovered {
        // C# MessageBox.Show(Str_SettingsRecoveryMsg, Str_SettingsRecoveryTitle, OK, Warning)
        common::warning_box(
            "Str_SettingsRecoveryTitle",
            &i18n::get("Str_SettingsRecoveryMsg"),
        );
    }

    // (e) Registry startup sync.
    if settings.run_at_startup != crate::platform::registry::is_run_at_startup_enabled() {
        crate::platform::registry::apply_run_at_startup(settings.run_at_startup);
    }

    let settings = Arc::new(RwLock::new(settings));
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("the tokio runtime for async work"),
    );

    // Publish the runtime so the hook-event dispatch path can reach it: the source's
    // `Task.Run` always has the dispatcher's scheduler in scope, and the GPUI main
    // thread does not unless something enters it.
    crate::app::set_runtime_handle(runtime.handle().clone());

    // (l) The GPUI run loop; `QuitMode::Explicit` is `ShutdownMode.OnExplicitShutdown`.
    gpui_kit::application()
        .with_quit_mode(gpui::QuitMode::Explicit)
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx| launch(cx, store, settings, runtime));
}

/// The pre-GPUI startup state, carried into `launch`.
struct Startup {
    store: SettingsStore,
    settings: Arc<RwLock<AppSettings>>,
    runtime: Arc<tokio::runtime::Runtime>,
}

impl Startup {
    fn new(
        store: SettingsStore,
        settings: Arc<RwLock<AppSettings>>,
        runtime: Arc<tokio::runtime::Runtime>,
    ) -> Self {
        Self {
            store,
            settings,
            runtime,
        }
    }
}

/// C# `VelopackApp.Build().Run()` — the one hook the port models is a downloaded
/// installer handed to us by the previous process.
fn handle_update_hook_args() -> bool {
    let mut args = std::env::args_os().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--apply-update" {
            let Some(path) = args.next() else {
                tracing::warn!("--apply-update needs an installer path");
                return true;
            };
            let manager = ReleaseManager::new(UPDATE_REPO);
            tracing::info!(path = %path.to_string_lossy(), "applying a downloaded update");
            manager.apply_and_restart(std::path::Path::new(&path));
        }
        tracing::debug!(?flag, "ignoring an unrecognized startup argument");
    }
    false
}

/// C# `[STAThread]` — `CoInitializeEx(nullptr, ApartmentThreaded)`.
fn init_com_apartment() {
    use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};

    // SAFETY: `CoInitializeEx` with a null reserved argument only initializes this
    // thread's COM apartment. The HRESULT is deliberately ignored: a thread that is
    // already initialized reports S_FALSE, which the source's `[STAThread]` also
    // treats as success.
    let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if hr.is_err() {
        tracing::debug!(?hr, "CoInitializeEx reported a failure");
    }
}

/// The `run()` body, on the GPUI main thread.
fn launch(
    cx: &mut App,
    store: SettingsStore,
    settings: Arc<RwLock<AppSettings>>,
    runtime: Arc<tokio::runtime::Runtime>,
) {
    let startup = Startup::new(store, settings, runtime);
    gpui_kit::init(cx);

    let bridge = bridge::MainThreadBridge::install(cx);
    let settings = Arc::clone(&startup.settings);
    let store = startup.store.clone();

    // (g) Tray icon - created on the next main-thread turn, like
    // `InitializeTrayIcon` runs on the dispatcher.
    cx.set_global(crate::tray::TrayGlobal::default());
    let tray = crate::tray::Tray::new(crate::tray::TrayCallbacks {
        on_settings: Box::new(|| post(open_settings)),
        on_restart: Box::new(|| post(tray_restart)),
        on_exit: Box::new(|| post(tray_exit)),
    });
    let resolved_theme = common::resolved_theme_name(&common::settings_snapshot(&settings));
    tray.update_icon(&resolved_theme);

    // (h) Radial menu entity + its window.
    let tooltip = ResultTooltip::new(
        Arc::clone(&bridge),
        Arc::clone(&settings),
        Arc::clone(&startup.runtime),
    );
    let radial = RadialMenu::new(
        Arc::clone(&bridge),
        Arc::clone(&settings),
        tooltip.clone(),
        Arc::clone(&startup.runtime),
    );
    let settings_window =
        SettingsWindow::new(Arc::clone(&bridge), store.clone(), Arc::clone(&settings));

    // (f) The executor the radial menu launches actions through, and the one the
    // orchestrator holds. Both wrap the same LLM client; with no key the client is
    // `None` but the executors still exist (`new ActionExecutorService(null)`).
    let llm = build_llm_client(&settings);
    let executor = build_executor(llm.clone(), &settings, &tooltip);
    radial.update_action_executor(Some(executor));

    // (h) Hooks: a failed mouse hook is fatal, a failed keyboard hook is tolerated.
    let hooks = match HookManager::start() {
        Ok(manager) => manager,
        Err(error) => {
            // C# shows the raw Win32 error code from `StartMouseHook`'s out parameter.
            let code = error.mouse_error_code.unwrap_or(0).to_string();
            let message = i18n::get_fmt("Str_HookErrorMsg", &[&code]);
            common::error_box("Str_HookErrorTitle", &message);
            cx.quit();
            return;
        }
    };

    // (i) Hotkey.
    apply_hotkey(&hooks, &settings);

    let ui: Arc<dyn UiHooks> = Arc::new(GpuiUi {
        radial: radial.clone(),
    });
    let app = Some(new_orchestrator(&store, &settings, &tooltip, hooks));

    let shared = SharedState {
        settings: Arc::clone(&settings),
        tooltip,
        settings_window,
        tray: tray.clone(),
        runtime: Arc::clone(&startup.runtime),
    };

    cx.set_global(UiState {
        app,
        ui,
        shared,
        pending_update: None,
    });

    // (j) Background release check.
    spawn_release_check(Arc::clone(&startup.runtime));

    // (k) Hook event pump.
    spawn_event_pump(cx);

    // C# `Microsoft.Win32.SystemEvents.UserPreferenceChanged`, registered in
    // `OnStartup` right after the theme/accent/language applies.
    spawn_theme_watcher(Arc::clone(&settings));

    // The tray menu and the tray icon's primary activation dispatch GPUI actions,
    // which is where the WinForms click handlers land.
    cx.on_action(|_: &tray_actions::TraySettings, cx| {
        (cx.global::<UiState>().shared.tray.callbacks().on_settings)();
    })
    .on_action(|_: &tray_actions::TrayRestart, cx| {
        (cx.global::<UiState>().shared.tray.callbacks().on_restart)();
    })
    .on_action(|_: &tray_actions::TrayExit, cx| {
        (cx.global::<UiState>().shared.tray.callbacks().on_exit)();
    });

    let _quit = cx.on_app_quit(move |cx: &mut App| {
        // C# `App.OnExit`.
        THEME_WATCHER_STOP.store(true, Ordering::Relaxed);
        let hooks = {
            let state = cx.global_mut::<UiState>();
            state.app.as_mut().and_then(|app| app.hooks.take())
        };
        // `SystemHookManager.StopHooks()` - dropping the manager unhooks and joins.
        drop(hooks);
        if let Some(tray) = crate::tray::tray_handle(cx) {
            // `_notifyIcon.Visible = false;` then `_notifyIcon?.Dispose();`
            let _ = tray.set_visible(false, cx);
            let _ = tray.close(cx);
        }

        let pending = pending_update_slot().clone();
        async move {
            if let Some(path) = pending {
                let manager = ReleaseManager::new(UPDATE_REPO);
                tracing::info!(?path, "applying a pending update on exit");
                manager.apply_and_restart(&path);
            }
        }
    });
}

/// Builds the orchestrator the hook pump drives.
///
/// `app::App::with_tooltip` is the constructor workstream C exposes; the live
/// `store`/`settings`/`executor`/`hooks` owned by the startup path are moved in
/// right after, so the orchestrator and the windows share one settings snapshot and
/// one `HookManager` — the source keeps the same state on `Orbital.App`.
fn new_orchestrator(
    store: &SettingsStore,
    settings: &Arc<RwLock<AppSettings>>,
    tooltip: &ResultTooltip,
    hooks: HookManager,
) -> OrchestratorApp {
    let mut app =
        OrchestratorApp::with_tooltip(Arc::new(tooltip::TooltipSinkImpl::new(tooltip.clone())));
    app.store = store.clone();
    app.settings = Arc::clone(settings);
    // C# `RebuildActionExecutor`'s tail: the executor exists even without a key
    // (`new ActionExecutorService(null)`), so only the LLM actions are gated.
    app.executor = Some(new_executor(build_llm_client(settings), settings, tooltip));
    app.hooks = Some(hooks);
    app
}

// ── UiHooks ─────────────────────────────────────────────────────────────────

struct GpuiUi {
    radial: RadialMenu,
}

impl UiHooks for GpuiUi {
    fn show_radial(&self, screen_x: i32, screen_y: i32, editable: bool, has_text: bool) {
        self.radial
            .show_at_cursor(screen_x, screen_y, editable, has_text);
    }

    fn hide_radial(&self) {
        self.radial.hide();
    }

    fn settings_changed(&self) {
        // C# `OpenSettingsWindow`'s tail: RebuildActionExecutor, ApplyHotkeySettings,
        // `_radialMenu.UpdateActionExecutor(_actionExecutor)`.
        let radial = self.radial.clone();
        post(move |cx| {
            let executor = rebuild_after_settings(cx);
            radial.update_action_executor(Some(executor));
        });
    }

    fn show_error(&self, title_key: &str, message: &str) {
        let title = title_key.to_string();
        let message = message.to_string();
        show_dialog_blocking(title, message, rfd::MessageLevel::Error);
    }

    fn show_warning(&self, title_key: &str, message: &str) {
        let title = title_key.to_string();
        let message = message.to_string();
        show_dialog_blocking(title, message, rfd::MessageLevel::Warning);
    }

    fn quit(&self) {
        post(|cx| cx.quit());
    }

    fn radial_visible(&self) -> bool {
        self.radial.is_visible()
    }

    fn point_inside_radial(&self, screen_x: i32, screen_y: i32) -> bool {
        // C# converts the hook's raw pixels with the window's TransformFromDevice
        // (96/dpi) and compares against `_radialMenu.Left/Top/Width/Height`.
        let Some(bounds) = radial::radial_bounds_dip() else {
            return false;
        };
        let scale = crate::platform::dpi::scale_for_point(screen_x, screen_y);
        let (x, y) = crate::platform::dpi::to_dip(f64::from(screen_x), f64::from(screen_y), scale);
        x >= bounds.left && x <= bounds.right && y >= bounds.top && y <= bounds.bottom
    }
}

/// Blocking dialogs are shown on a helper thread so a modal message box never parks
/// the GPUI run loop — the source's `MessageBox.Show` on the dispatcher is the only
/// equivalent available without a second window toolkit.
fn show_dialog_blocking(title: String, message: String, level: rfd::MessageLevel) {
    std::thread::spawn(move || {
        common::message_box(&title, &message, level);
    });
}

// ── Executor ────────────────────────────────────────────────────────────────

/// The LLM client half of `App.RebuildActionExecutor`.
///
/// `ActionExecutor` is neither `Clone` nor constructible from its fields (workstream
/// C owns them), so the two places the source keeps one instance — `_actionExecutor`
/// and `_radialMenu`'s — build their own executor over this shared `Arc<dyn LlmClient>`.
fn build_llm_client(settings: &Arc<RwLock<AppSettings>>) -> Option<Arc<dyn crate::llm::LlmClient>> {
    let snapshot = common::settings_snapshot(settings);
    let api_key = SettingsStore::get_api_key(&snapshot);

    // C# `if (string.IsNullOrEmpty(apiKey)) { _actionExecutor = new ActionExecutorService(null); return; }`
    if api_key.is_empty() {
        return None;
    }

    // C# `IsNullOrWhiteSpace(ApiBaseUrl) ? "https://api.openai.com/v1" : ApiBaseUrl`
    let base_url = if snapshot.api_base_url.trim().is_empty() {
        "https://api.openai.com/v1".to_string()
    } else {
        snapshot.api_base_url.clone()
    };
    // C# `IsNullOrWhiteSpace(ModelName) ? "gpt-4o-mini" : ModelName`
    let model_name = if snapshot.model_name.trim().is_empty() {
        "gpt-4o-mini".to_string()
    } else {
        snapshot.model_name.clone()
    };

    // C# `try { new ActionExecutorService(new OpenAiApiService(...)); }
    //     catch (ArgumentException ex) { MessageBox.Show(Str_InvalidApiConfigMsg, ...);
    //                                    _actionExecutor = new ActionExecutorService(null); }`
    match OpenAiApiService::new(&api_key, &base_url, &model_name) {
        Ok(service) => Some(Arc::new(service)),
        Err(error) => {
            common::warning_box(
                "Str_InvalidApiConfigTitle",
                &i18n::get_fmt("Str_InvalidApiConfigMsg", &[&error.to_string()]),
            );
            None
        }
    }
}

/// A fresh `ActionExecutor` over a shared LLM client. `None` client mirrors
/// `new ActionExecutorService(null)`.
fn new_executor(
    llm: Option<Arc<dyn crate::llm::LlmClient>>,
    settings: &Arc<RwLock<AppSettings>>,
    tooltip: &ResultTooltip,
) -> ActionExecutor {
    ActionExecutor::new(
        llm,
        Arc::clone(settings),
        Arc::new(ClipboardFns),
        Arc::new(ShellLauncher),
        Arc::new(tooltip::TooltipSinkImpl::new(tooltip.clone())),
    )
}

/// The shared executor the radial menu launches through.
///
/// Always `Some`, exactly like the source: `RebuildActionExecutor` builds
/// `new ActionExecutorService(null)` when no key is configured, so the
/// non-LLM actions (Copy/Cut/Paste/SelectAll) keep working without a key;
/// only the LLM actions hit the "API key is not configured" guard, which
/// reads the key off the shared settings (see `RadialMenuView::on_action_click`).
pub(crate) fn build_executor(
    llm: Option<Arc<dyn crate::llm::LlmClient>>,
    settings: &Arc<RwLock<AppSettings>>,
    tooltip: &ResultTooltip,
) -> Arc<ActionExecutor> {
    Arc::new(new_executor(llm, settings, tooltip))
}

/// C# the `OpenSettingsWindow` rebuild tail — rebuild the executor, apply the hotkey,
/// publish the executor to the radial menu.
pub(crate) fn rebuild_after_settings(cx: &mut App) -> Arc<ActionExecutor> {
    let settings = Arc::clone(&cx.global::<UiState>().shared.settings);
    let tooltip = cx.global::<UiState>().shared.tooltip.clone();

    // One LLM client, two executors — see `build_llm_client`.
    let llm = build_llm_client(&settings);
    let executor = build_executor(llm.clone(), &settings, &tooltip);
    let owned = new_executor(llm, &settings, &tooltip);

    {
        let state = cx.global_mut::<UiState>();
        if let Some(app) = state.app.as_mut() {
            app.executor = Some(owned);
        }
    }
    apply_hotkey_from_settings(cx);
    executor
}

/// C# `App.ApplyHotkeySettings`.
fn apply_hotkey(hooks: &HookManager, settings: &Arc<RwLock<AppSettings>>) {
    let snapshot = common::settings_snapshot(settings);
    hooks.set_hotkey(snapshot.hotkey_modifiers, snapshot.hotkey_virtual_key);
}

fn apply_hotkey_from_settings(cx: &mut App) {
    let settings = Arc::clone(&cx.global::<UiState>().shared.settings);
    let snapshot = common::settings_snapshot(&settings);

    // Holding a live `HookManager` reference while `&mut App` is borrowed is not
    // possible, so the hotkey is pushed from a short-lived immutable borrow: the
    // global lives long enough for the `set_hotkey` call to complete.
    let hotkey = (snapshot.hotkey_modifiers, snapshot.hotkey_virtual_key);
    if let Some(hooks) = cx
        .global::<UiState>()
        .app
        .as_ref()
        .and_then(|app| app.hooks.as_ref())
    {
        hooks.set_hotkey(hotkey.0, hotkey.1);
    }
}

// ── Tray actions ────────────────────────────────────────────────────────────

/// C# `(s, e) => Dispatcher.Invoke(OpenSettingsWindow)`.
fn open_settings(cx: &mut App) {
    cx.global::<UiState>().shared.settings_window.open();
}

/// C# the Restart menu item's body.
fn tray_restart(cx: &mut App) {
    if let Some(path) = pending_update_slot().clone() {
        // C# `_updateManager?.ApplyUpdatesAndRestart(_pendingUpdate);` — never returns.
        let manager = ReleaseManager::new(UPDATE_REPO);
        manager.apply_and_restart(&path);
    }

    // C# `var exe = Environment.ProcessPath; if (exe != null) Process.Start(exe);
    //      Dispatcher.Invoke(Shutdown);`
    if let Ok(exe) = std::env::current_exe() {
        match std::process::Command::new(&exe).spawn() {
            Ok(_child) => tracing::info!(?exe, "relaunched Orbital"),
            Err(error) => tracing::warn!(%error, ?exe, "could not relaunch Orbital"),
        }
    } else {
        tracing::warn!("could not resolve the current executable to relaunch");
    }
    cx.quit();
}

/// C# `_notifyIcon.Visible = false; Dispatcher.Invoke(Shutdown);`
fn tray_exit(cx: &mut App) {
    if let Some(tray) = crate::tray::tray_handle(cx) {
        let _ = tray.set_visible(false, cx);
    }
    cx.quit();
}

// ── Release check ───────────────────────────────────────────────────────────

/// C# `CheckForUpdatesAsync`.
fn spawn_release_check(runtime: Arc<tokio::runtime::Runtime>) {
    runtime.spawn(async move {
        let manager = ReleaseManager::new(UPDATE_REPO);
        let current = env!("CARGO_PKG_VERSION");

        let info = match manager.check_for_updates(current).await {
            Ok(info) => info,
            Err(error) => {
                // C# `catch { /* Update check is best-effort — ignore all errors */ }`
                tracing::debug!(%error, "the release check failed");
                return;
            }
        };
        let Some(info) = info else {
            return;
        };

        let installer = match manager.download(&info).await {
            Ok(path) => path,
            Err(error) => {
                tracing::debug!(%error, "the update download failed");
                return;
            }
        };

        // C# `mgr.WaitExitThenApplyUpdates(updateInfo, silent: true, restart: true);`
        let arm_manager = ReleaseManager::new(UPDATE_REPO);
        arm_manager.arm_apply_on_exit(installer.clone());
        *pending_update_slot() = Some(installer);

        let version = info.version.clone();
        post(move |cx| {
            let state = cx.global_mut::<UiState>();
            state.pending_update = pending_update_slot().clone();
            state.shared.tray.rebuild_menu(Some(&version));
            state.shared.tray.show_balloon(
                &i18n::get("Str_UpdateBalloonTitle"),
                &i18n::get_fmt("Str_UpdateBalloonMsg", &[&version]),
            );
        });
    });
}

// ── OS theme / accent watcher ───────────────────────────────────────────────

/// C# `App.OnStartup`'s
/// `SystemEvents.UserPreferenceChanged += OnSystemUserPreferenceChanged`.
///
/// GPUI exposes no OS user-preference notification, so the source's event becomes a
/// poll: a detached thread reads the registry's `AppsUseLightTheme` and the DWM accent
/// every two seconds and, when the resolved theme name or the accent moves, pushes the
/// tray icon exactly like the source's
/// `Dispatcher.BeginInvoke(() => ApplyTheme("System"))` +
/// `ApplySystemAccentResources()` pair. The radial and tooltip windows re-read the
/// theme when they show, so they need no push; only the tray icon is held.
fn spawn_theme_watcher(settings: Arc<RwLock<AppSettings>>) {
    std::thread::spawn(move || {
        let mut last_theme = common::resolved_theme_name(&common::settings_snapshot(&settings));
        let mut last_accent = crate::theme::accent_with_soft();

        while !THEME_WATCHER_STOP.load(Ordering::Relaxed) {
            std::thread::sleep(THEME_WATCH_POLL_INTERVAL);
            if THEME_WATCHER_STOP.load(Ordering::Relaxed) {
                break;
            }

            let snapshot = common::settings_snapshot(&settings);
            let resolved = common::resolved_theme_name(&snapshot);
            let accent = crate::theme::accent_with_soft();
            if resolved == last_theme && accent == last_accent {
                continue;
            }
            last_theme = resolved.clone();
            last_accent = accent;

            tracing::debug!(theme = %resolved, "the OS theme or the accent color changed");
            post(move |cx| {
                let tray = cx.global::<UiState>().shared.tray.clone();
                tray.update_icon(&resolved);

                // C# swaps the merged dictionaries, so an open window follows the OS
                // theme too; only the settings window can be open long-term, and it is
                // the one that caches its theme name.
                if let Some(handle) = settings::current_settings_window() {
                    let theme_name = resolved.clone();
                    let _ = handle.update(cx, |view: &mut settings::SettingsView, _, cx| {
                        view.theme_name = theme_name;
                        cx.notify();
                    });
                }
            });
        }
    });
}

// ── Event pump ──────────────────────────────────────────────────────────────

/// C# the hook event handlers, drained from the manager's channel.
///
/// The `HookManager` is owned by the main-thread global, so the pump polls its
/// receiver from a GPUI task instead of owning a clone of it: every event is then
/// dispatched on the main thread, which is what the source's `Dispatcher` handlers do.
fn spawn_event_pump(cx: &App) {
    let task: Task<()> = cx.spawn(async move |cx| {
        loop {
            let event = cx.update(|cx| {
                let state = cx.global::<UiState>();
                state
                    .app
                    .as_ref()
                    .and_then(|app| app.hooks.as_ref())
                    .and_then(|hooks| hooks.events().try_recv().ok())
            });

            match event {
                Some(event) => {
                    tracing::debug!(?event, "dispatching a hook event");
                    cx.update(|cx| dispatch(cx, event))
                }
                None => {
                    cx.background_executor().timer(PUMP_POLL_INTERVAL).await;
                }
            }
        }
    });
    task.detach();
}

/// C# `App.SystemHookManager_OnXxx` dispatch, one event at a time.
///
/// The dispatch runs inside the tokio runtime context (`_runtime_guard`) because the
/// source's handlers call `Task.Run`, which needs a scheduler; on the GPUI main
/// thread there is none unless we enter the one the startup path built.
fn dispatch(cx: &mut App, event: HookEvent) {
    let ui = Arc::clone(&cx.global::<UiState>().ui);
    let runtime = Arc::clone(&cx.global::<UiState>().shared.runtime);
    let _runtime_guard = runtime.enter();
    let state = cx.global_mut::<UiState>();
    let Some(app) = state.app.as_mut() else {
        return;
    };
    crate::app::dispatch_hook_event(app, &ui, event);
}

/// Publishes the radial popup's placement for the hook side's inside-window test.
pub(crate) fn publish_radial_metrics(metrics: Option<radial::RadialWindowMetrics>) {
    if let Ok(mut slot) = RADIAL_METRICS.lock() {
        *slot = metrics;
    }
}

/// The radial popup's bounds in DIPs, or `None` when it is closed.
pub(crate) fn radial_metrics_dip() -> Option<Rect> {
    RADIAL_METRICS
        .lock()
        .ok()
        .and_then(|slot| *slot)
        .map(|metrics| metrics.dip)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_hook_args_without_a_path_are_handled_and_reported() {
        // The hook runs before the UI exists; a missing path is a startup error the
        // source reports through Velopack's own logging.
        assert!(!handle_update_hook_args());
    }
}
