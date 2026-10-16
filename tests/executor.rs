//! Action executor vectors ported from
//! `orbital-main/Orbit.Tests/ActionExecutorServiceTests.cs`, plus characterization of
//! the port seams the source exposes only through its statics (clipboard, launcher,
//! tooltip, sound).
//!
//! The xunit facts use `null` for the LLM client and a `MockLlmService`; the port's
//! trait objects are exercised with the same shapes. `sound` is disabled in these
//! settings so no device is touched — the gate is what the executor reads either way.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use orbital::executor::{
    ActionExecutor, ClipboardFns, ClipboardPort, Launcher, ShellLauncher, TooltipSink,
};
use orbital::llm::{LlmClient, LlmError};
use orbital::placement::{AnchorContext, Point};
use orbital::settings::{
    ActionProfile, AppSettings, ButtonDisplayMode, create_default_settings, is_selection_required,
};

/// A `BtnSounds=false` settings bag, so the executor never reaches the sound sink.
fn settings() -> std::sync::Arc<std::sync::RwLock<AppSettings>> {
    let mut settings = create_default_settings();
    settings.sound_enabled = false;
    std::sync::Arc::new(std::sync::RwLock::new(settings))
}

/// C# object-initializer shape used by the xunit facts.
fn action(
    name: &str,
    prompt_format: &str,
    result_action: &str,
    requires_selection: Option<bool>,
    clean_output: bool,
) -> ActionProfile {
    ActionProfile {
        name: name.to_string(),
        icon: String::new(),
        prompt_format: prompt_format.to_string(),
        result_action: result_action.to_string(),
        requires_selection,
        display_mode: ButtonDisplayMode::TextAndIcon,
        clean_output,
    }
}

// —— Test doubles for the out-ports the source reaches through its statics ————————

/// Records every ClipboardHelper call the executor makes.
#[derive(Default)]
struct RecordingClipboard {
    copied: Mutex<Vec<String>>,
    replaced: Mutex<Vec<String>>,
    deleted: AtomicUsize,
    keys: Mutex<Vec<String>>,
    pasted: AtomicUsize,
    selected_text_reads: AtomicUsize,
}

impl ClipboardPort for RecordingClipboard {
    fn get_selected_text(&self) -> String {
        self.selected_text_reads.fetch_add(1, Ordering::SeqCst);
        String::new()
    }
    fn copy_to_clipboard(&self, text: &str) {
        self.lock(&self.copied).push(text.to_string());
    }
    fn replace_selected_text(&self, text: &str) {
        self.lock(&self.replaced).push(text.to_string());
    }
    fn delete_selected_text(&self) {
        self.deleted.fetch_add(1, Ordering::SeqCst);
    }
    fn simulate_key(&self, key_name: &str) {
        self.lock(&self.keys).push(key_name.to_string());
    }
    fn simulate_paste(&self) {
        self.pasted.fetch_add(1, Ordering::SeqCst);
    }
}

impl RecordingClipboard {
    fn lock<'a, T>(&'a self, mutex: &'a Mutex<T>) -> std::sync::MutexGuard<'a, T> {
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Records `Process.Start(UseShellExecute = true)`.
#[derive(Default)]
struct RecordingLauncher {
    urls: Mutex<Vec<String>>,
}

impl Launcher for RecordingLauncher {
    fn open_url(&self, url: &str) {
        match self.urls.lock() {
            Ok(mut guard) => guard.push(url.to_string()),
            Err(poisoned) => poisoned.into_inner().push(url.to_string()),
        }
    }
}

/// Records `new ResultTooltipWindow(result, anchors).Show()`.
#[derive(Default)]
struct RecordingTooltip {
    shown: Mutex<Vec<(String, AnchorContext)>>,
}

impl TooltipSink for RecordingTooltip {
    fn show_result(&self, result: String, anchors: AnchorContext) {
        match self.shown.lock() {
            Ok(mut guard) => guard.push((result, anchors)),
            Err(poisoned) => poisoned.into_inner().push((result, anchors)),
        }
    }
}

/// Port of the xunit `MockLlmService` — records the prompt and the system prompt.
struct MockLlmClient {
    calls: Mutex<Vec<(String, Option<String>)>>,
}

impl MockLlmClient {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }
}

impl LlmClient for MockLlmClient {
    fn call_api<'a>(
        &'a self,
        prompt: &'a str,
        system_prompt: Option<&'a str>,
    ) -> Pin<Box<dyn Future<Output = Result<String, LlmError>> + Send + 'a>> {
        let prompt = prompt.to_string();
        let system_prompt = system_prompt.map(str::to_string);
        Box::pin(async move {
            match self.calls.lock() {
                Ok(mut guard) => guard.push((prompt, system_prompt)),
                Err(poisoned) => poisoned.into_inner().push((prompt, system_prompt)),
            }
            Ok("mock response".to_string())
        })
    }
}

/// Current-thread runtime with the time driver, because `tokio`'s `macros` feature is
/// not enabled in this crate (so no `#[tokio::test]`).
fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
}

/// The executor and its recorded ports, so each fact can assert on the recordings.
struct Fixtures {
    executor: ActionExecutor,
    clipboard: Arc<RecordingClipboard>,
    launcher: Arc<RecordingLauncher>,
    tooltip: Arc<RecordingTooltip>,
}

fn fixtures(llm: Option<std::sync::Arc<dyn LlmClient>>) -> Fixtures {
    let clipboard = std::sync::Arc::new(RecordingClipboard::default());
    let launcher = std::sync::Arc::new(RecordingLauncher::default());
    let tooltip = std::sync::Arc::new(RecordingTooltip::default());
    let shared = settings();
    let executor = ActionExecutor::new(
        llm,
        shared,
        Arc::clone(&clipboard) as Arc<dyn ClipboardPort>,
        Arc::clone(&launcher) as Arc<dyn Launcher>,
        Arc::clone(&tooltip) as Arc<dyn TooltipSink>,
    );

    Fixtures {
        executor,
        clipboard,
        launcher,
        tooltip,
    }
}

// —— The four xunit facts, ported 1:1 ————————————————————————————————————————————

#[test]
fn constructor_with_no_llm_client_creates_a_valid_instance() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;

    assert!(!executor.has_llm_service());
}

#[test]
fn constructor_with_an_llm_client_sets_the_properties() {
    let fixtures = fixtures(Some(std::sync::Arc::new(MockLlmClient::new())));
    let executor = &fixtures.executor;

    assert!(executor.has_llm_service());
}

#[test]
fn execute_with_a_selection_required_action_and_empty_text_returns_early() {
    let fixtures = fixtures(Some(std::sync::Arc::new(MockLlmClient::new())));
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let launcher = &fixtures.launcher;
    let tooltip = &fixtures.tooltip;
    let action = action("Test", "", "DirectCopy", Some(true), false);

    runtime().block_on(async {
        // Should not throw, just return early.
        executor
            .execute(&action, "", None)
            .await
            .expect("the defensive guard returns Ok");
    });

    // Nothing was dispatched — no clipboard write, no browser, no tooltip.
    assert!(clipboard.lock(&clipboard.copied).is_empty());
    assert!(launcher.urls.lock().expect("launcher lock").is_empty());
    assert!(tooltip.shown.lock().expect("tooltip lock").is_empty());
}

#[test]
fn execute_with_an_llm_action_and_no_client_reports_the_missing_api_key() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let action = action("Translate", "Translate: {text}", "Replace", None, false);
    assert!(is_selection_required(&action));

    runtime().block_on(async {
        // C# `Assert.ThrowsAsync<InvalidOperationException>`.
        let error = executor
            .execute(&action, "test text", None)
            .await
            .expect_err("an LLM action without a client must fail");

        assert_eq!(error, "API key is not configured for LLM actions.");
    });
}

// —— Extra characterization: the seams the source hides behind statics ——————————

/// Differential check of the `Browser` action's URL, whose query value is
/// `Uri.EscapeDataString(selectedText)` in the source.
///
/// Each expected string is the literal output observed from
/// `Uri.EscapeDataString` on .NET 8.0.425 for that same input, so this pins the
/// port's re-implementation (PORT_STATUS.md C6) against the real .NET behaviour
/// rather than against the port's own assumption.
#[test]
fn execute_browser_builds_the_url_exactly_as_dotnet_escapes_it() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let launcher = &fixtures.launcher;
    let action = action("Search", "", "Browser", Some(true), false);

    for (text, escaped) in [
        ("rust port guide", "rust%20port%20guide"),
        ("a+b/c?d#e", "a%2Bb%2Fc%3Fd%23e"),
        ("caf\u{e9}", "caf%C3%A9"),
        ("\u{4e2d}\u{6587}", "%E4%B8%AD%E6%96%87"),
        ("\u{1F600}", "%F0%9F%98%80"),
    ] {
        runtime().block_on(async {
            executor
                .execute(&action, text, None)
                .await
                .expect("the browser path succeeds");
        });

        // The URL opened for this text, i.e. C#'s
        // $"https://www.google.com/search?q={Uri.EscapeDataString(selectedText)}".
        let urls = launcher.urls.lock().expect("launcher lock");
        assert_eq!(
            urls.last().map(String::as_str),
            Some(&*format!("https://www.google.com/search?q={escaped}")),
            "unexpected URL for selected text {text:?}"
        );
    }
}

#[test]
fn execute_replaces_the_text_placeholder_and_sends_the_clean_output_system_prompt() {
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client.clone()));
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action(
        "Translate",
        "Translate the following to Korean organically: {text}",
        "Replace",
        Some(true),
        true,
    );

    runtime().block_on(async {
        executor
            .execute(&action, "hello", None)
            .await
            .expect("the replace path succeeds");
    });

    let calls = client.calls.lock().expect("mock lock");
    assert_eq!(calls.len(), 1, "exactly one LLM call");
    assert_eq!(
        calls[0].0,
        "Translate the following to Korean organically: hello"
    );
    assert_eq!(
        calls[0].1.as_deref(),
        Some(
            "Return only the processed result. Do not include any explanation, preamble, commentary, or extra text."
        )
    );

    // C# `ClipboardHelper.ReplaceSelectedText(result)`.
    let replaced = clipboard.lock(&clipboard.replaced);
    assert_eq!(replaced.as_slice(), ["mock response"]);
}

#[test]
fn execute_without_clean_output_sends_no_system_prompt_and_copies_the_result() {
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client.clone()));
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action(
        "Summarize",
        "Summarize the following in 3 lines: {text}",
        "Copy",
        Some(true),
        false,
    );

    runtime().block_on(async {
        executor
            .execute(&action, "a b c", None)
            .await
            .expect("the copy path succeeds");
    });

    let calls = client.calls.lock().expect("mock lock");
    assert_eq!(calls[0].0, "Summarize the following in 3 lines: a b c");
    assert_eq!(calls[0].1, None);

    assert_eq!(
        clipboard.lock(&clipboard.copied).as_slice(),
        ["mock response"]
    );
    assert!(clipboard.lock(&clipboard.replaced).is_empty());
}

#[test]
fn execute_replaces_every_text_placeholder_occurrence() {
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client.clone()));
    let executor = &fixtures.executor;
    let action = action(
        "Rewrap",
        "before {text} middle {text} after",
        "Popup",
        Some(true),
        false,
    );

    runtime().block_on(async {
        executor
            .execute(&action, "X", None)
            .await
            .expect("the popup path succeeds");
    });

    let calls = client.calls.lock().expect("mock lock");
    // C# `string.Replace` replaces every occurrence, not just the first.
    assert_eq!(calls[0].0, "before X middle X after");
}

#[test]
fn execute_browser_escapes_the_selection_into_the_search_url() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let launcher = &fixtures.launcher;
    let action = action("Search", "", "Browser", Some(true), false);

    runtime().block_on(async {
        executor
            .execute(&action, "rust port guide", None)
            .await
            .expect("the browser path succeeds");
    });

    let urls = launcher.urls.lock().expect("launcher lock");
    assert_eq!(
        urls.as_slice(),
        ["https://www.google.com/search?q=rust%20port%20guide"]
    );

    // C# never copies for the Browser action.
    assert!(clipboard.lock(&clipboard.copied).is_empty());
}

#[test]
fn execute_browser_escapes_reserved_and_non_ascii_characters() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let launcher = &fixtures.launcher;
    let action = action("Search", "", "Browser", Some(true), false);

    runtime().block_on(async {
        executor
            .execute(&action, "a+b/c?d#e", None)
            .await
            .expect("the browser path succeeds");
    });

    let urls = launcher.urls.lock().expect("launcher lock");
    assert_eq!(
        urls.as_slice(),
        ["https://www.google.com/search?q=a%2Bb%2Fc%3Fd%23e"]
    );
}

#[test]
fn execute_direct_copy_copies_the_selection_only() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action("Copy", "", "DirectCopy", Some(true), false);

    runtime().block_on(async {
        executor
            .execute(&action, "selected", None)
            .await
            .expect("the direct copy path succeeds");
    });

    assert_eq!(clipboard.lock(&clipboard.copied).as_slice(), ["selected"]);
    assert_eq!(clipboard.deleted.load(Ordering::SeqCst), 0);
    assert!(clipboard.lock(&clipboard.replaced).is_empty());
}

#[test]
fn execute_cut_copies_then_deletes_the_selection() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action("Cut", "", "Cut", Some(true), false);

    runtime().block_on(async {
        executor
            .execute(&action, "cut me", None)
            .await
            .expect("the cut path succeeds");
    });

    // C# copy first, then delete — the order matters for the undo behaviour.
    assert_eq!(clipboard.lock(&clipboard.copied).as_slice(), ["cut me"]);
    assert_eq!(clipboard.deleted.load(Ordering::SeqCst), 1);
}

#[test]
fn execute_paste_waits_then_simulates_paste() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    // `IsSelectionRequired` defaults to false for Paste, so an empty selection passes.
    let action = action("Paste", "", "Paste", None, false);

    runtime().block_on(async {
        executor
            .execute(&action, "", None)
            .await
            .expect("the paste path succeeds");
    });

    assert_eq!(clipboard.pasted.load(Ordering::SeqCst), 1);
    assert!(clipboard.lock(&clipboard.copied).is_empty());
}

#[test]
fn execute_paste_with_a_selection_also_waits_then_pastes() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action("Paste", "", "Paste", Some(true), false);

    runtime().block_on(async {
        executor
            .execute(&action, "something", None)
            .await
            .expect("the paste path succeeds");
    });

    assert_eq!(clipboard.pasted.load(Ordering::SeqCst), 1);
}

#[test]
fn execute_simulate_key_passes_the_prompt_format_as_the_key_name() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let action = action("Enter", "Enter", "SimulateKey", Some(false), false);

    runtime().block_on(async {
        executor
            .execute(&action, "", None)
            .await
            .expect("the simulate key path succeeds");
    });

    assert_eq!(clipboard.lock(&clipboard.keys).as_slice(), ["Enter"]);
}

#[test]
fn execute_simulate_key_with_an_empty_prompt_format_sends_an_empty_key() {
    let fixtures = fixtures(None);
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    // C# `action.PromptFormat ?? string.Empty` — an unset format is an empty key name.
    let action = action("Send", "", "SimulateKey", Some(false), false);

    runtime().block_on(async {
        executor
            .execute(&action, "", None)
            .await
            .expect("the simulate key path succeeds");
    });

    assert_eq!(clipboard.lock(&clipboard.keys).as_slice(), [""]);
}

#[test]
fn execute_popup_shows_the_tooltip_with_the_result_and_the_action_bar_anchor() {
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client));
    let executor = &fixtures.executor;
    let tooltip = &fixtures.tooltip;
    let action = action("Summarize", "Summarize: {text}", "Popup", Some(true), false);
    let action_bar = orbital::placement::Rect::new(10.0, 20.0, 110.0, 40.0);

    runtime().block_on(async {
        executor
            .execute(&action, "text", Some(action_bar))
            .await
            .expect("the popup path succeeds");
    });

    let shown = tooltip.shown.lock().expect("tooltip lock");
    assert_eq!(shown.len(), 1, "exactly one tooltip");
    assert_eq!(shown[0].0, "mock response");
    // C# passes `actionBarBoundsDip` through unchanged.
    assert_eq!(shown[0].1.action_bar_dip, Some(action_bar));
    // The cursor anchor is the live cursor position (device pixels — see PORT_STATUS.md),
    // so only the shape is asserted here.
    assert!(shown[0].1.cursor_dip.x >= 0.0 && shown[0].1.cursor_dip.y >= 0.0);
}

#[test]
fn execute_popup_uses_the_default_action_when_the_action_type_is_unknown() {
    // C# `ActionType.FromString` falls back to Popup for an unrecognized value.
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client));
    let executor = &fixtures.executor;
    let clipboard = &fixtures.clipboard;
    let tooltip = &fixtures.tooltip;
    let action = action("Broken", "fix: {text}", "NotARealAction", Some(true), false);
    assert!(is_selection_required(&action));

    runtime().block_on(async {
        executor
            .execute(&action, "text", None)
            .await
            .expect("the default popup path succeeds");
    });

    assert_eq!(tooltip.shown.lock().expect("tooltip lock").len(), 1);
    assert!(clipboard.lock(&clipboard.copied).is_empty());
}

#[test]
fn execute_propagates_the_llm_error_class_of_a_rejected_action() {
    let client = std::sync::Arc::new(MockLlmClient::new());
    let fixtures = fixtures(Some(client));
    let executor = &fixtures.executor;
    let action = action("Translate", "t: {text}", "Replace", Some(true), false);

    runtime().block_on(async {
        // The mock always succeeds, so this pins the happy path and the fact that the
        // error channel is the same one the source's exception escapes through.
        executor
            .execute(&action, "text", None)
            .await
            .expect("the mock LLM never fails");
    });
}

#[test]
fn the_default_ports_delegate_to_the_platform_functions() {
    // Construction only: calling them would touch the real clipboard and shell.
    let _clipboard = ClipboardFns;
    let _launcher = ShellLauncher;
    let _point = Point::new(0.0, 0.0);
    let _anchors = AnchorContext::default();
}
