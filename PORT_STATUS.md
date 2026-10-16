# Port status — deviations, source bugs and blocked items

Workstream B (platform Win32 / DPAPI / registry / DPI / clipboard / hooks / backdrop).
Every entry is something a contract owner must decide on; nothing here was chosen
silently. Date of record: **2026-10-12**.

## Windows-crate feature gaps (ALL RESOLVED — see the Integration pass, entries I1/I2)

The `windows` 0.62.2 feature list in `Cargo.toml` is missing two modules the port
needs. I could not edit `Cargo.toml`, so both are worked around inside my own files
using `windows::core::link!` (the same mechanism the `windows` crate itself uses).
Each wrapper is a one-line swap once the feature is on.

| # | Item | Missing feature | Where | Workaround |
|---|---|---|---|---|
| B1 | user32 clipboard ABI: `OpenClipboard`, `CloseClipboard`, `EmptyClipboard`, `EnumClipboardFormats`, `GetClipboardData`, `SetClipboardData`, `IsClipboardFormatAvailable` | `Win32_System_DataExchange` | `src/clipboard.rs` ("Raw clipboard access") | Each call is bound with `windows::core::link!` in the wrapper function, which is exactly how the crate's own bindings are produced. |
| B2 | `GetDpiForMonitor` / `MONITOR_DPI_TYPE` (`windows::Win32::UI::HiDpi`) | `Win32_UI_HiDpi` | `src/platform/dpi.rs` | `MonitorFromPoint` + `GetMonitorInfoW` for the monitor's `szDevice`, then `CreateDCW` + `GetDeviceCaps(LOGPIXELSX)` — the value `GetDpiForMonitor(MDT_DEFAULT)` reports for that same monitor. |

**Triage: source-wins / RESOLVED (I1, I2).** Adding both features to `Cargo.toml` would let
B1 and B2 drop their hand-rolled ABI. Both are listed as target-side mechanism
differences, not behavior differences: the observable result of each call is the same.
Note that the underlying Win32 functions are identical either way — only the binding
source changes.

## Deviations committed in this pass

| # | Item | Triage | Detail |
|---|---|---|---|
| B3 | `dpi::to_dip` divides by the scale factor | **source-wins** (fixed) | The scaffold's stub body was `(x * scale, y * scale)` with `scale_for_point` returning `dpi/96`, which is the inverse of what the source computes. C# `App.SystemHookManager_OnAnyMouseDown` uses `TransformFromDevice.M11`/`M22`, i.e. `96/dpi`, so DIP = `px * (96/dpi)` = `px / (dpi/96)`. Corrected to `x / scale` so the port matches the source; the doc comment on `scale_for_point` ("Scale factor (dpi/96.0)") is unchanged. |
| B4 | Long-press timer is a detached thread + armed flag / generation counter | **acceptable-difference: mechanism, same observable behavior** | C# uses `System.Threading.Timer` disposed on button-down/up. A `std::thread` cannot be cancelled, so the timer is *disarmed* instead: `armed` is cleared on button-up (the source's `Dispose`) and a monotonically increasing `generation` is stamped when a timer arms. A waking thread only takes effect when both still match its own values, so a cancelled or superseded timer is a no-op — the same observable result as a disposed `Timer` whose callback never runs. The generation never resets, because a reset would let an old timer alias a new one. The timer never blocks the hook proc (thread spawn, not `Thread.Sleep` on the hook thread). |
| B4b | `SetWindowsHookExW` error code captured on the pump thread | **source-wins** | C# `SetHook` reads `Marshal.GetLastWin32Error()` on the hook thread and returns it via `out errorCode`. `GetLastError` is thread-local, so the port stores the code in an `AtomicI32` on the pump thread at the failure site and `start` reads that. Reading it later from the caller's thread would return a stale value. |
| B5 | `OnDoubleClick` (down-time) event has no port variant | **out-of-scope** | C# `SystemHookManager.OnDoubleClick` fires on `WM_LBUTTONDOWN` and is documented "Kept for back-compat; prefer OnDoubleClickRelease". `App.xaml.cs` subscribes to `OnDoubleClickRelease` only (line 142), and the Rust `HookEvent` enum is FROZEN without a down-time variant. Nothing observable is lost; the double-click branch still sets `_isDoubleClick` and suppresses the long-press timer exactly as the source does. |
| B6 | `C# OnDoubleClick` trace log instead of an event | **source-wins** | Same event as B5. Rather than silently dropping the branch, `on_left_button_down` logs a `tracing::trace!` when a double click is detected so the branch is observable in traces. |
| B7 | `ReadLastError` is taken after `SetWindowsHookExW` returns | **acceptable-difference: mechanism, same observable behavior** | The C# reads `Marshal.GetLastWin32Error()` immediately after `SetWindowsHookEx`. The crate's `SetWindowsHookExW` builds its `Err` via `Error::from_thread()` internally, which reads the same thread-local value, so the code is the same Win32 error. |
| B8 | Windows version gate reads `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\CurrentBuildNumber` | **acceptable-difference: mechanism, same observable behavior** | `OperatingSystem.IsWindowsVersionAtLeast(10,0,22000)` is manifest-assisted in .NET (and silently caps at the app manifest, which the csproj declares as Windows 10). Reading the registry `CurrentBuildNumber` is the `RtlGetVersion`-equivalent, truthful on every Windows build and not dependent on a declared manifest — i.e. the *stricter* and more truthful reading. The note in the plan said HKCU; that tree does not carry `CurrentBuildNumber`, so HKLM is used. |
| B9 | Registry autostart always writes `REG_SZ` | **source-wins / verify in review** | C# `key.SetValue("Orbital", $"\"{exePath}\"")` goes through `RegistryValueKind.Unknown` inference, which is `REG_SZ` for strings without `%`. The port always writes `REG_SZ`. Diverges only if the exe path contains `%`, which would be an out-of-scope path to verify separately. |
| B10 | `CF_PALETTE` is not backed up; `CF_TEXT` falls back to `FromUtf8Lossy` | **source-wins / flagged** | The C# `Clipboard.GetDataObject()`/`SetDataObject(backup, true)` round-trips an OLE `IDataObject` of any format. This port copies raw clipboard bytes (`PARITY-CONTRACT.md` decision 2): every memory format (CF_UNICODETEXT, CF_TEXT, CF_HDROP, CF_DIB, CF_LOCALE, CF_RIFF, private formats) is byte-exact, `CF_BITMAP` via `CopyImage` and `CF_ENHMETAFILE` via `CopyEnhMetaFileW`. `CF_PALETTE` is skipped rather than guessed. `CF_TEXT` is decoded with `String::from_utf8_lossy`; .NET's `DataObject.GetText` would convert via the thread's ANSI code page, so non-ASCII ANSI text may differ. `CF_METAFILEPICT`'s contained metafile handle is copied by value, not deep-copied. **Not chosen silently — flagged for a follow-up.** |
| B11 | `TextPatternRange.Replace` is unavailable | **pre-approved (PARITY-CONTRACT.md decision 6)** | C# calls `dynRange.Replace(newText, false)` through `dynamic`. The Rust `uiautomation` 0.25.1 crate wraps `IUIAutomationTextRange` without a `Replace` method (verified in the registry source). `replace_selected_text_via_uia` therefore always returns `false`, and the delete + clipboard-paste path runs — the same observable outcome. |
| B12 | Delay-render clipboard formats are skipped and logged | **source-wins / flagged** | C#'s `Clipboard.GetDataObject()` returns a live `IDataObject` that will render a delayed format on demand. `GetClipboardData` returns null for a delayed-render format, and this port cannot force the render. Those formats are not backed up and are logged at `debug` (format id only, never contents). Flagged for a follow-up; in practice user-copied content is almost always already rendered. |
| B13 | `UIAutomation::new_direct` + explicit `CoInitializeEx` | **acceptable-difference: mechanism, same observable behavior** | `uiautomation::UIAutomation::new()` hard-fails with `RPC_E_CHANGED_MODE` when the thread is already in an STA apartment — which is where this app's main thread lives. `CoInitializeEx(None, COINIT_MULTITHREADED)` is issued first (its failure being tolerated means "already initialized somewhere", which is fine for UIA), then the automation object is built directly. The UIA query sequence (`GetFocusedElement` → `TextPattern` → control-view parent → `GetSelection[0]` → collapsed check → `GetText(-1)`) is identical to the source's. |
| B14 | Clipboard retry class | **acceptable-difference: mechanism, same observable behavior** | C#'s `IsClipboardException` retries only `ExternalException` (COM clipboard contention). The port retries any `io::Error` from its own clipboard wrappers, with the identical `min(10 * 2^i, 500)` ms schedule and 10 retries. Same intent, slightly wider net. |
| B15 | Backdrop uses the raw HWND | **acceptable-difference: API shape** | C# takes a `Window` and reads `new WindowInteropHelper(window).Handle`; the Rust `try_apply(hwnd: isize, ...)` takes the HWND directly. Same call, one indirection removed at the seam. |
| B16 | No `docs/`-style comment drift | — | All comments in my files are English and reference the C# symbol they port. No logging of clipboard contents or API keys anywhere (DPAPI blobs are never traced; clipboard bytes only ever as `format` ids). |

## Not touched (other workstreams' territory)

`src/sound.rs` (workstream A), `src/theme.rs` (`DwmGetColorizationColor` still `todo!()` in
that file — it is not in my allowed edit list), `src/app.rs`, `src/executor.rs`,
`src/llm.rs`, `src/release_flow.rs`, `src/settings.rs`, `src/i18n.rs`,
`src/placement.rs`, `src/action_presets.rs`, `src/action_type.rs`, `src/ui/*`,
`src/tray.rs`.


# Workstream C — services (llm, executor), orchestrator (pp), release flow

Port of Services/LlmApiService.cs, Services/ActionExecutorService.cs,
App.xaml.cs (CheckEditability / IsOverEditableControl / IsClipboardFallbackEditor /
TriggerSelectionMenu / the hook handlers) and the Velopack usage in App.xaml.cs +
Program.cs. Date of record: **2026-10-12**. Every entry below is something a contract
owner must decide on; nothing here was chosen silently. --no-default-features is what
verifies, because the ui feature is still missing files (see C21).

## Source behaviour that is preserved even though it looks like a bug

| # | Item | Triage | Detail |
|---|---|---|---|
| C1 | Cursor.Position is device pixels despite the name cursorDip | **source-wins** (preserved) | ActionExecutorService.ExecuteAsync builds Point cursorDip = new(cursor.X, cursor.Y) from WinForms Cursor.Position, which is physical screen pixels, and passes it to AutomationElement.FromPoint (also physical pixels). The port takes platform::win32::cursor_pos() unchanged and stores it in AnchorContext.cursor_dip. The numbers are identical to the source; only the local's name is misleading. |
| C4 | The error body is truncated on UTF-16 units | **source-wins** | C# 
esponseBody.Length > 500 ? responseBody.Substring(0, 500) + "..." counts UTF-16 code units, so the port counts them too (250 non-BMP characters are exactly 500 units and are kept whole; 251 are cut mid surrogate pair, which rom_utf16_lossy turns into U+FFFD, exactly like a lone surrogate would be everywhere else in .NET). Three dots, not the ellipsis character. |
| C5 | choices is not an array / content is not a string | **source-bug / flagged** | C# choices.GetArrayLength() throws InvalidOperationException for a non-array and content.GetString() throws for a non-string, neither of which is a JsonException, so both escape as a different exception class with a different message. The port maps both onto the closest documented parse message ("Response 'choices' array is empty" and "Response content is null"). The error class (LlmError::Parse) is identical either way; only the wording differs on a malformed upstream response. Owner decision requested. |
| C13 | _lastDismissTime / _lastTriggerAttemptTime start at DateTime.MinValue | **source-wins** | Ported as Option<Instant> with None meaning "never", so the first trigger is neither throttled nor suppressed. |
| C7 | The request's Content-Type carries a charset | **source-wins** | StringContent(json, Encoding.UTF8, "application/json") sends pplication/json; charset=utf-8. reqwest's .json() would send bare pplication/json, so the body and the header are set explicitly to keep the wire bytes 1:1. |
| C6 | Uri.EscapeDataString is re-implemented in executor.rs | **needs a differential check / flagged** | No percent-encoding crate is a dependency and Cargo.toml is not mine to edit, so the RFC 3986 unreserved-set escape (A-Z a-z 0-9 - . _ ~ pass through, every other UTF-8 byte becomes uppercase %XX) is written out. .NET's Uri.EscapeDataString escapes exactly the non-unreserved characters, which is what this reproduces. There is no .NET SDK on this machine, so the exact character set could not be verified against the source. Recommend adding percent-encoding to Cargo.toml and swapping the helper. |

## Target-side mechanism differences (same observable behaviour)

| # | Item | Triage | Detail |
|---|---|---|---|
| C3 | IUIAutomationTextRange.GetBoundingRectangles is not wrapped by uiautomation | **acceptable difference / flagged** | The executor calls the COM method through the crate's own AsRef<IUIAutomationTextRange> and reads the returned 1-D VT_R8 safe array with SafeArrayGetElement (four doubles per rectangle, left, top, width, height — what the .NET interop assembly marshals to double[]). Same gap class as workstream B's B11 (TextPatternRange.Replace). Recommend patching the crate or calling the windows binding directly in both modules. |
| C8 | error_for_status is not used | **acceptable difference** | The source reads the response body *before* inspecting the status, so the port checks status.is_success() after the read. error_for_status would discard the body, and the body is what builds the error detail. |
| C9 | Thread.Sleep(150) (Paste) | **acceptable difference** | 	okio::time::sleep with the same delay, in the same position in the flow. A blocking sleep would stall the runtime the executor runs on. |
| C10 | The 25/80 ms settle delay and the 80 ms throttle are tokio sleeps | **acceptable difference** | Same values and positions. 	rigger_selection_menu / dispatch_hook_event log a debug! and return when there is no tokio runtime in scope, rather than panicking the way 	okio::spawn would: that is a wiring error, not a user-visible condition. |
| C11 | CancellationTokenSource becomes a generation counter plus JoinHandle::abort | **acceptable difference** | A generation is stamped per trigger and re-checked exactly where the source checks 	oken.IsCancellationRequested (before the settle delay, after it, before ShowAtCursor). bort mirrors CancellationTokenSource.Dispose; the source's Task.Run body is not interruptible while CheckEditability is in flight either, so the re-check after the (blocking) UIA call is what actually cancels — the same place the source's cooperative check would. |
| C14 | The long-press path shares the trigger state | **acceptable difference** | TriggerSelectionMenu's frozen signature takes &mut App, but the source runs it on its own Task.Run thread, so the two timestamps and the cancellation gate live behind Arcs and the spawned task reaches them through Arc::clone. |
| C2 | No UIA CacheRequest | **pre-approved (PARITY-CONTRACT.md decision 5)** | Restated for the record: check_editability and is_over_editable_control issue the same property/pattern queries in sequence, and the parity unit stays the resulting (canSelect, canWrite, hasText) triple. |
| C15 | The default tooltip sink is a logging no-op | **was BLOCKED on workstream D — RESOLVED (I5/I6: startup wires the GPUI tooltip sink)** | ui::tooltip::ResultTooltip has no constructor (_private: (), show(&self, ..)), so App::new() installs executor::LoggingTooltipSink. start_app must call App::set_tooltip(...) with the GPUI implementation and then 
ebuild_action_executor — the same order as the source's OpenSettingsWindow. The logging sink records only the result length, never the text. |

## New decisions this workstream needs an owner for

| # | Item | Triage | Detail |
|---|---|---|---|
| C12 | The outside-click hit test moved into the UI layer | **target-side mechanism / flagged** | SystemHookManager_OnAnyMouseDown converts the mouse point from pixels to DIPs with PresentationSource.CompositionTarget.TransformFromDevice and compares it against the radial window's Left/Top/Width/Height. Those bounds exist only in the window layer and UiHooks (frozen) has no bounds accessor, so the hit test is the implementation's job: hide_radial() must decline the hide when the click landed inside the radial window (the point is available from platform::win32::cursor_pos() at that moment). This port routes AnyMouseDown to UiHooks::hide_radial() and only stamps _lastDismissTime when the menu actually went away, which is the source's if (!insideWindow). |
| C16 | A silent Inno install does not relaunch Orbital | **source-behaviour difference / flagged** | installer/Orbital.iss marks the launch entry Flags: nowait postinstall skipifsilent, so a /VERYSILENT install never starts the app, whereas Velopack's 
estart: true did. pply_and_restart spawns the installer with /VERYSILENT /SUPPRESSMSGBOXES /NORESTART and exits(0), so nothing relaunches the app. Either drop skipifsilent from the installer for silent runs or have the app relaunch itself after the spawn. Contract-owner decision. |
| C18 | 	okio::fs is unavailable | **Cargo.toml change requested** | 
elease_flow::download streams with reqwest and writes with a blocking std::fs::File between awaits, because the crate's tokio features are 
t-multi-thread, 	ime, sync. Adding s (and macros, so tests can use #[tokio::test] instead of hand-built runtimes) would clean both up. |
| C19 | ersion_is_newer treats only 1-3 all-numeric parts | **flagged** | "1.0.0-beta.1" (four parts) and "v0.4.8" (non-numeric first part) compare as nothing, so the update is skipped — the source's GithubSource(..., prerelease: false) also skips prereleases. Confirmed intentional, recorded here so nobody "fixes" it into semver. |
| C17 | sound::play_action still held a `todo!()` sink | **was BLOCKED on workstream B - RESOLVED (I4: rodio sink)** | At the time of writing, src/sound.rs::fire_and_forget was a `todo!()` panic, so every executor test ran with sound_enabled = false - the gate itself was still exercised (the executor reads its live setting). The rodio sink has since landed (I4); the note is kept for the record. |
## Verification notes and open items (not fixed here)

* cargo test --no-default-features — **186 passed, 0 failed** (123 lib, 7 gating,
  19 executor, 7 fallback editor, 7 placement, 23 settings). cargo clippy
  --no-default-features --all-targets — zero warnings in my files.
* **C20** No .NET SDK is installed on this machine, so the recorded xunit vectors were
  ported by reading the source rather than by running them: no differential run was
  possible. The vectors are reproduced 1:1 plus extra characterization tests, all green.
* **C21** cargo fmt --all and the default (ui) feature cannot resolve: src/tray.rs
  is referenced by lib.rs but does not exist, and src/ui/radial.rs /
  src/ui/tooltip.rs import ui::bridge and ui::common modules that are missing.
  Those are other workstreams' files, so they were left alone; formatting was verified
  per file with 
ustfmt --edition 2024 on exactly the six files I own.
* **C22** Additive public items (no frozen signature changed): llm::ensure_tls_provider
  and a redacting Debug for OpenAiApiService; executor::LoggingTooltipSink;
  pp::{App::new, App::with_tooltip, App::set_tooltip, App::pending_update_version,
  App::rebuild_action_executor, App::check_for_updates}; 
elease_flow::take_armed_installer.
  App's public fields (store, settings, executor, hooks) are unchanged; the
  private timestamp fields became Option<Instant>/Arc<Mutex<..>> (see C13/C14).

# Workstream D — GPUI UI stack + packaging (tray, radial, tooltip, settings, action edit, installer)

Port of App.xaml.cs (tray / hooks / startup order), RadialMenuWindow, ResultTooltipWindow,
SettingsWindow, ActionEditDialog, installer/Orbital.iss, portable/Cleanup.cmd,
scripts/Build-*.ps1 and .github/workflows/cargo.yml. Date of record: **2026-10-12**.

## API friction points (what the crates actually offered)

| # | Item | What I used | Note |
|---|---|---|---|
| D1 | gpui-kit is a facade over `gpui-pre` | `use gpui_kit::prelude::*;` for `Styled`/`ParentElement`/`InteractiveElement`/`StatefulInteractiveElement`/`IntoElement`/`Render`/`Context`, plus `gpui_kit::application()`, `.with_quit_mode(QuitMode::Explicit)`, `.with_assets(gpui_kit::assets::Assets)`, `gpui_kit::init(cx)`, `gpui_kit::actions!` | `gpui_kit::*` glob re-exports `gpui_pre::*`, so `MenuItem`/`Action`/`Window` are the same types `gpui-tray` uses through its own `gpui` alias. No type mismatch. |
| D2 | `gpui_kit::component` widgets | `component::Button::new(id).primary().label(..).on_click(..)`, `component::checkbox::Checkbox::new(id).label(..).checked(..).on_change(..)`, `component::input::Input::new(&Entity<InputState>)`, `component::input::Textarea::new(&Entity<TextareaState>)` | `Input`/`Textarea` are bound to a state entity, so each field is an `Entity<InputState>` created inside the `cx.open_window` closure (where `&mut Window` is available for `InputState::new(window, cx)`). gpui-component 0.7 has no `TextInput` type and no `ComboBox`/`Select` primitive I could bind cleanly to the source's tag-matched `ComboBox`, so `ComboBox` is expressed as a chip row of the same options (see D6). |
| D3 | `gpui-tray` tray | `gpui_tray::Tray::builder().icon(Icon).tooltip(str).title(str).on_activate(action).menu(build).build(cx)`, then `set_icon(Some(Icon), cx)`, `set_tooltip(Some(str), cx)`, `refresh_menu(cx)`, `set_visible(false, cx)`, `close(cx)` | `Icon::from_gpui(&gpui::Image::from_bytes(ImageFormat::Ico, bytes), cx)` decodes the embedded .ico through GPUI's own image pipeline (`image` crate with the `ico` feature) and normalizes BGRA→RGBA — a direct `image` dependency is not in Cargo.toml and is not needed. Menu items are `gpui::MenuItem::action(label, action)` dispatched through `cx.on_action`. `gpui_tray::Tray` is `!Send` (asserted in the crate), so it lives behind a GPUI global and is only touched from the main thread. |
| D4 | gpui-tray has no balloon API | `cx.show_system_notification(gpui::SystemNotification { tag, title, body, actions })` | The Windows backend owns the `Shell_NotifyIconW` handle, so a second balloon on the same icon is not reachable from outside the crate. The tip is delivered as a Windows toast instead. Wording/title/message are the source's; only the mechanism differs. |
| D5 | No window HWND accessor | `backdrop::try_apply(hwnd, ..)` with the HWND found by `EnumWindows` on own-pid windows with the exact title | `gpui_pre::Window::platform_window` is `pub(crate)` and `HasWindowHandle for Window` needs the `raw_window_handle` crate in scope. The probe is best-effort and cosmetic only (DWM Mica/Acrylic on the settings + edit windows). |
| D6 | `WindowKind::PopUp` on Windows | `common::popup_window_options(bounds, show, focus)` with `kind: WindowKind::PopUp`, `titlebar: None`, `is_movable/is_resizable/is_minimizable: false` | gpui-pre-windows maps PopUp to `WS_EX_TOOLWINDOW | WS_EX_TOPMOST` with `WS_POPUP` and `SWP_SHOWNOACTIVATE` when `focus: false` — the source's `Topmost` + `ShowActivated=False`. The crate has no `show: false`/`SetWindowPlacement` re-position API, and `PlatformWindow` has no move operation, so a popup's position is fixed at creation (see D7). |
| D7 | Popup measurement | `common::text_width` (per-glyph `TextSystem::layout_width` summed) + the XAML's constants | GPUI fixes a window's size at creation (`WindowOptions::window_bounds`) and offers no `SizeToContent`/`ActualWidth` read-back; `WM_SIZE` only arrives after creation. The source shows the pill off-screen, calls `UpdateLayout()`, then re-positions. The port measures through GPUI's text system with the XAML's own constants (button height 36, min width 44, padding 10/6/4, border 1) and places once. `TextSystem::layout_line`/`shape_line` live on `WindowTextSystem` (needs a `&Window`) vs `TextSystem` (only per-glyph advances), so a line is the sum of its glyphs' advance widths — DirectWrite's unshaped run width. **owner decision requested.** |
| D8 | `div().size(px(n))` takes `impl Into<Length>` | `.size(px(n))` (not `gpui::size(w, h)`) | Both forms exist; `size` is single-axis so `gpui::size(...)` compiles for `Size` but not `Length`. |
| D9 | `FocusHandle::focus` takes a window **and** an app | `handle.focus(window, cx)` | One argument more than the shape I first assumed. |
| D10 | `InputState::set_value` takes a window | `state.update(cx, |s, cx| s.set_value(v, window, cx))` | All input writes go through a `&mut Window`; the settings view threads it from the closure argument. |
| D11 | `InputBaseState::default_value` / `set_masked` are consuming/settable builders | `InputState::new(window, cx).default_value(v)`, `state.set_masked(true, window, cx)` | Used for the API-key field's `PasswordChar="●"`. |

## Deviations committed in this pass

| # | Item | Triage | Detail |
|---|---|---|---|
| D12 | `div().w_full()`/`.size()` naming | — | `w_full` does not exist in this GPUI snapshot; `.size_full()` / `.w(px(..))` are the equivalents. |
| D13 | Hotkey capture reads `keystroke.modifiers` | **acceptable difference** | WPF `PreviewKeyDown` gives `Key.LeftCtrl/…` as the key itself; GPUI gives them as `Modifiers { control, alt, .. }` on the keystroke. The source ignores those five keys plus LWin/RWin/System; the port ignores `modifiers.control/platform/alt` on the keystroke, which is the same rule with a different representation. `platform` covers LWin/RWin; `alt` covers AltGr. |
| D14 | `vk_from_key` upper-cases a single ASCII alphanumeric | **source-wins** | GPUI reports the letter of a keypress lowercase (`"a"`), WPF's `KeyInterop.VirtualKeyFromKey(Key.A)` gives 0x41. Upper-casing an ASCII alphanumeric single character reproduces the WPF mapping (0x41..0x5A for letters, 0x30..0x39 for digits) — the hotkey still stores the same VK codes the source stores. |
| D15 | Popup placement is computed before the window opens | **needs owner decision** | The source's `ShowAtCursor` re-positions a live window after `UpdateLayout()`, so its clamp uses the real `ActualWidth`/`ActualHeight`. The port clamps with the measured size. The math (`cursorX - w/2`, `cursorY - h - 8`, `Max(workLeft+4, Min(left, workRight-w-4))`, `top < workTop+4 → cursorY+8`) is identical; only the size it runs on is an estimate. |
| D16 | Restart-with-version uses a toast + relaunch | **source-behaviour difference / flagged** | gpui-tray's menu is GPUI-action based, so `Str_TrayRestart (v{ver})` dispatches `TrayRestart`; the handler applies the pending installer when one exists and otherwise spawns `current_exe()` and quits, exactly like the source's two branches. |
| D17 | `Str_ApiKeyNotConfiguredDetail` guard is on the main thread | **source-wins** | C# `MessageBox.Show` on the dispatcher thread before the `Task.Run`; the port shows the rfd dialog on the GPUI main thread before spawning the executor task. |
| D18 | `show_error`/`show_warning` run rfd on a helper thread | **acceptable difference** | The source's `MessageBox.Show` on the dispatcher pumps a nested modal loop and returns to the caller; rfd's blocking `show()` parks the thread. Running it on a helper thread keeps the GPUI run loop alive while the modal is up, which is the closest available behavior. Startup dialogs (settings recovery, hook error, invalid API config) are shown inline before the GPUI run loop, which matches the source ordering. |
| D19 | Outside-click hide is delegated to the hook side | **pre-approved (C12)** | `ui::radial::radial_bounds_dip()` publishes the radial window's DIP bounds for `App.SystemHookManager_OnAnyMouseDown`'s inside-window test, and `UiHooks::hide_radial()` is what that handler calls. `RadialMenu::is_visible()` is the source's `IsVisible`. |
| D20 | `ComboBox` → chip rows; `ContextMenu` → inline preset panel | **visual-chrome difference** | The provider / model / theme / language / popup-position / output-mode / display-mode selectors and the 17-preset menu render as chip rows and an inline panel instead of WPF `ComboBox`/`ContextMenu`. Option sets, labels (localized, `Str_*` tags matched by tag not content) and the resulting settings are 1:1. |
| D21 | Separator is added before the selection-required check | **source-wins** (ported verbatim) | `PopulateBarButtons` adds the separator border when `!firstVisible` **before** the `IsSelectionRequired && !hasText` continue, so a skipped action still leaves a separator. The port reproduces that ordering exactly. |
| D22 | Settings window applies the DWM accent once per theme | **acceptable difference** | `common::active_theme` caches the theme table with `theme::accent_with_soft()` applied, refreshed when the resolved theme name changes — the source's `ApplyTheme` + `ApplySystemAccentResources` pair. `accent_with_soft` stays `todo!()` in workstream A/B until it lands; until then the five accent keys keep the XAML'd values. |
| D23 | Language is a global, not a resource dictionary | **pre-approved (PARITY-CONTRACT.md decision 1)** | `i18n::set_language` replaces the Strings.*.xaml merge; `i18n::get` replaces `Loc.Get`. |
| D24 | Icons are embedded, not pack:// URIs | **acceptable difference** | `include_bytes!("../assets/*.ico")` + GPUI's ICO decoder replaces the WPF resource stream; same asset bytes. The source's last-resort `SystemIcons.Application` becomes a transparent 1×1 pixel (no stock GPUI icon). |

## Cross-workstream hand-offs (ALL RESOLVED in the Integration pass - I5/I6/I8)

| # | Item | Needed |
|---|---|---|
| D25 | `app::App` cannot be constructed from `ui` | `src/app.rs`'s `last_dismiss` / `last_trigger_attempt` / `pending_update_version` are private, so `ui::new_orchestrator()` returns `None` and the hook event pump is registered but inert. Either make those three fields `pub` or add `App::new(...)`. The exact construction is in the doc comment on `ui::new_orchestrator`. Until then every hook event is reported once at `error!` and everything else (tray, windows, dialogs, executor, update check, quit) runs. **`src/app.rs` is not in my editable file list, so this is reported, not fixed.** RESOLVED (I6): App::new / with_tooltip construct the orchestrator and ui::new_orchestrator injects store/settings/executor/hooks; the inert-pump fallback was deleted.** |
| D26 | `app::dispatch_hook_event`, `app::trigger_selection_menu`, `executor::ActionExecutor::execute`, `llm::OpenAiApiService::new`, `release_flow::*` were still `todo!()` (RESOLVED: workstream C implemented all of them; the 19 executor tests plus the lib app/release tests are green) | Workstreams A/C. The UI stack calls them through the frozen signatures, so nothing in my files needs to change when they land; the end-to-end run only becomes real then. |
| D27 | `sound::play_popup`'s gate | Workstream A (`sound.rs`). `RadialMenu` calls `crate::sound::play_popup(settings.sound_enabled)` exactly like the source's `PlayPopupSound()`. |

## Verification

* `cargo check --all-features` — clean, zero warnings.
* `cargo test --all-features` — **all green** (lib unit tests + the headless workstreams' integration tests in `tests/` + my new characterization tests in `ui::settings::tests`, `ui::action_edit::tests`, `ui::tray_actions::tests`).
* `cargo clippy --all-targets --all-features` — zero warnings in my files.
* `cargo fmt` applied.
* The default `ui` feature is what compiles: `cargo check --all-features` builds GPUI, gpui-kit and gpui-tray.
* Integration wiring for `lib.rs::start_app`: call **`orbital::ui::run()`** (a one-line change in `start_app`, replacing the `todo!`).

## Files changed

```
src/ui/mod.rs            new: run(), startup ordering, UiHooks impl, main-thread bridge wiring
src/ui/bridge.rs         new: MainThreadBridge (Dispatcher.BeginInvoke equivalent)
src/ui/common.rs         new: theme + accent resolution, text measurement, dialogs, styled controls
src/ui/radial.rs         RadialMenu port
src/ui/tooltip.rs        ResultTooltip port
src/ui/settings.rs       SettingsWindow port
src/ui/action_edit.rs    ActionEditDialog port
src/ui/tray_actions.rs   new: the tray menu's GPUI actions
src/tray.rs              moved from the crate root; Tray port over gpui-tray
installer/Orbital.iss    cargo release binary as the source
portable/Cleanup.cmd     verbatim copy from orbital-main
scripts/Build-Installer.ps1, scripts/Build-Portable.ps1
.github/workflows/cargo.yml
PORT_STATUS.md           this section
```

---

## Integration pass — 2026-10-12 (owner: orchestrator)

Closes the cross-workstream gaps the four workstreams flagged. Everything below is
also reflected in the repo state; nothing here changes source-observable behavior.

| # | Item | Triage | Detail |
|---|---|---|---|
| I1 | B1 resolved | block cleared | Cargo.toml adds Win32_System_DataExchange; src/clipboard.rs's seven user32 wrappers (OpenClipboard, CloseClipboard, EmptyClipboard, EnumClipboardFormats, GetClipboardData, SetClipboardData, IsClipboardFormatAvailable) now bind the windows crate directly. The windows::core::link! workaround is deleted. |
| I2 | B2 resolved | block cleared | Cargo.toml adds Win32_UI_HiDpi; src/platform/dpi.rs calls GetDpiForMonitor(MDT_EFFECTIVE_DPI) on MonitorFromPoint's handle. The CreateDCW/GetDeviceCaps path is deleted. |
| I3 | tokio features | block cleared | Cargo.toml adds s and macros (workstream C's download and runtime handles). |
| I4 | sound::fire_and_forget | gap filled | The frozen stub still held 	odo!(). Implemented with rodio 0.23's API (DeviceSinkBuilder::open_default_sink → Player::connect_new(handle.mixer()) → Decoder over the WAV bytes → sleep_until_end()), on a detached thread, swallowing every failure like the source's catch. |
| I5 | lib.rs::start_app wired | integration | Calls ui::run() (workstream D's Program.cs + App.OnStartup port). |
| I6 | ui::new_orchestrator live | integrates D25 | Constructs the real pp::App through C's App::with_tooltip, then injects the startup path's store/settings/executor/hooks. The hook event pump now dispatches into pp::dispatch_hook_event; the warn_orchestrator_missing inert-pump fallback is deleted. |
| I7 | rustdoc warnings | fixed | src/llm.rs module doc escapes choices[0]; src/ui/bridge.rs doc drops the link to the crate-private item. cargo doc --all-features --no-deps is clean. |
| I8 | Stray orbital-rust-port/tray.rs | removed | Duplicate of the original stub at the crate root; the module lives at src/tray.rs. |
| I9 | Theme table count | frozen stub superseded | The scaffold froze ThemeColors.entries at 22; the XAML actually defines 29 (workstream A entry 1). 29 is source-truth; stub count was the plan's estimate. |

### Verification (2026-10-12, this machine)

| Command | Result |
|---|---|
| cargo fmt --all --check | clean |
| cargo clippy --all-targets --all-features | 0 warnings |
| cargo test --all-features | **196 passed, 0 failed** (133 lib + 23 settings + 19 executor + 7 action_gating + 7 fallback_editor + 7 placement) |
| cargo doc --all-features --no-deps | clean |
| cargo build --release --all-features | Orbital.exe, 27.6 MB, single exe + OS DLLs |
| Smoke run (release exe, 10 s) | process alive; %APPDATA%\Orbital\settings.json written byte-compatible with the C# artifact (PascalCase key order, "PopupPlacement": 0 numeric enum, icon U+E8C8) |

### Differential / parity measurement (plan §6, phase 4)

- **Denominator:** 28 recorded xunit test methods across the five Orbit.Tests
  files (12 settings, 5 gating, 4 placement, 3 fallback-editor, 4 executor).
- **Compared:** all 28 are ported 1:1 into 	ests/*.rs (a settings theory or
  gating theory expands to its cases), plus 32 characterization tests and
  proptest generators over placement and gating; LLM wire shape pinned with
  wiremock tests.
- **Not run here (owner decision):** the .NET SDK is absent on this machine
  (runtimes only), so the C# suite could not be executed side-by-side to diff
  recorded vectors. The ported expectations were transcribed from the source test
  files, which are the recorded vectors. Installing the .NET 8 SDK would allow
  running dotnet test on orbital-main/Orbit.Tests for a true both-run diff.

### Environment notes

- The D: drive is nearly full (0.1 GB free at one point), which broke linking
  with LNK1318. All builds/tests were run with CARGO_TARGET_DIR=C:\orbital-target
  (38+ GB free). A fresh machine needs ≥ 15 GB free on the target drive, or the
  same redirect.
- cargo doc without --no-deps documents the whole 900-crate graph and exceeds
  30 minutes here; CI's canonical commands (mt, clippy, 	est, uild) all
  pass as listed above.

---

## Review-pass fixes - 2026-10-12 (owner: orchestrator)

A final parity review over the seams (trigger pipeline, hooks, clipboard, executor,
i18n, settings JSON) against the C# sources found two blocking wiring bugs and
several should-fix divergences. All are fixed below; the review's remaining items
are cosmetic or owner decisions.

| # | Finding | Severity | Fix |
|---|---|---|---|
| R1 | The trigger pipeline was inert in the shipped binary: tokio::runtime::Handle::try_current() always failed on the GPUI main thread, so no radial menu ever opened from a real gesture | **blocking** | ui::dispatch now enters the app's tokio runtime (runtime.enter()) before app::dispatch_hook_event, which is what C# gets from Task.Run on the dispatcher's scheduler. |
| R2 | OnAnyMouseDown hid the radial menu on every mouse-down, including clicks on its own buttons (the outside-click hit test was never implemented) | **blocking** | New UiHooks::point_inside_radial(x, y) (default false, headless unchanged). GpuiUi converts the hook's raw pixels with platform::dpi (96/dpi) and compares against the popup's DIP rect via radial::radial_bounds_dip(), matching App.xaml.cs:473-482. The dismiss now only fires for outside clicks. |
| R3 | _lastDismissTime was never stamped on the outside-click dismiss, so the 150 ms post-dismiss suppression never started there | should-fix | Fixed with R2: the handler stamps then hides, like C# lines 480-481. Regression tests added (an_outside_mouse_down_stamps_the_dismiss_time_it_hides_from, a_mouse_down_inside_the_radial_menu_keeps_it_open). |
| R4 | The 80 ms throttle window slid forever: the port stamped _lastTriggerAttemptTime even on throttled triggers | should-fix | stamp_trigger_attempt now stamps only accepted attempts (C# line 577 vs the early return at 574). Regression test a_throttled_trigger_keeps_the_last_accepted_stamp. |
| R5 | The hook-error dialog showed an HRESULT (-2147024891) inside a sentence instead of the raw Win32 code the source formats into {0} | should-fix | hooks.rs unwraps HRESULT_FROM_WIN32 at the capture site (win32_code_from_hresult), and ui passes the bare code to Str_HookErrorMsg. This also corrects the earlier B7 note. |
| R6 | NearSelection tooltip placement never worked: the selection rect was scaled in the wrong direction (x scale instead of / scale) and its width/height were written into Rect's right/bottom fields | should-fix | ui::tooltip::scale_rect divides every edge by the scale factor (regression test added). Also fixed the same edge-field mistake in radial::action_bar_bounds. |
| R7 | NearActionBar placement never worked: the action-bar rect was read after close_window had cleared it | should-fix | ui::radial::on_action_click captures action_bar_bounds() before closing, matching C#'s read-after-Hide() (WPF keeps the metrics; the port's close clears them, so the same values are taken first). |
| R8 | CheckEditability treated "UIA reported an empty selection array" as a failed call, suppressing the popup entirely for editable controls with no selection | should-fix | has_real_text_selection now distinguishes them: empty array -> Some(false) (pattern answered), call failure -> None (abandon the check). Matches App.xaml.cs:826-827. |
| R9 | Empty action list no longer showed the empty pill / played the popup sound | should-fix | ui::radial::open_at always opens the pill and plays the sound, like ShowAtCursor (the source's early return is inside PopulateBarButtons, whose result ShowAtCursor ignores). |
| R10 | The settings artifact was written with LF; Newtonsoft writes Environment.NewLine (CRLF) | should-fix | SettingsStore::save converts to CRLF. Regression test the_saved_file_uses_crlf_newlines_like_newtonsoft; the on-disk artifact verified (91 LF / 91 CRLF). |

### Open owner decisions (review findings not silently resolved)

- **R11 - "Actions": null in settings.json.** The source assigns null (Newtonsoft
  accepts it) and would throw a NullReferenceException at the first gesture; the
  port's serde rejects it and takes the .corrupt.bak recovery path. This is a
  source-bug-flavored difference: the port is more robust, but the observable
  behavior differs (recovery dialog + defaults vs. crash later). No test covers a
  null body. Owner decision requested: keep the recovery path (recommended) or
  reproduce the crash-on-first-gesture behavior.
- **R12 - RequiresSelection: null is omitted rather than written**: byte-level only,
  parses back to the same value. Accepted-difference.
- **R14 - hook error code**: the raw Win32 code is now captured (R5), so the B7 note
  ("the code is the same Win32 error") is accurate.

Verification after the fixes: cargo fmt --all --check clean, cargo clippy
--all-targets --all-features 0 warnings, cargo test --all-features **202 passed,
0 failed** (138 lib + 24 settings + 19 executor + 21 across action_gating /
fallback_editor / placement), cargo build --release --all-features OK, 10 s smoke
run alive with the CRLF settings artifact verified.

---

## Verification-loop fix wave - 2026-10-12 (owner: orchestrator)

An end-to-end verification loop (5 parallel review agents, codebase as truth) found
P0/P1/P2 gaps after the port was already green. All findings were verified against
the C# source and fixed below. Sources: the verdict's items 1-14.

| # | Finding | Severity | Fix |
|---|---|---|---|
| R15 | Auto-close seconds field rendered but never committed | **P0** | `ui/settings.rs` subscribes the input for `PressEnter`/`Blur`: valid int >= 1 commits + saves, invalid reverts the box (C# `TryApplyPopupAutoCloseSeconds` + `LostFocus`), and the box is dimmed/disabled when the auto-close checkbox is off (`UpdatePopupAutoCloseControls`). |
| R16 | In-window release check dropped its tokio runtime at return, aborting the task | **P0** | The check now runs on the app's long-lived `UiState.shared.runtime` reached through the bridge, and calls `llm::ensure_tls_provider()` first (reqwest is configured rustls-no-provider, so a fresh process needs the ring provider installed before its first HTTPS call). |
| R17 | Language change never rebuilt the tray menu and never re-rendered | **P0** | The language listener now `cx.notify()`s the settings window and posts a tray menu rebuild with the pending-update label (C# `ApplyLanguage` -> `RebuildTrayMenu`). |
| R18 | Theme change never refreshed the tray icon; no OS theme/accent subscription | **P0** | The theme listener posts a tray icon refresh; `ui::mod::spawn_theme_watcher` polls `AppsUseLightTheme` + `DwmGetColorizationColor` every 2 s and on change pushes the tray icon AND re-themes an open settings window (C# `OnSystemUserPreferenceChanged`; polling is the documented stand-in - Rust has no SystemEvents). Stop flag set on quit. |
| R19 | Backdrop never applied on stock Windows (CurrentBuildNumber read as REG_DWORD; Windows stores REG_SZ) and a failed dark-mode set skipped the backdrop step | **P0** | `backdrop.rs` reads the value two-phase (type + size first; REG_SZ UTF-16 parse and REG_DWORD both supported) and every `DwmSetWindowAttribute` result is swallowed like the source's method-wide catch. |
| R20 | Enter in the API-key box did not save | P1 | `PressEnter` on the key input runs the existing save path (C# `ApiKeyBox_KeyDown`). |
| R21 | Hotkey edits applied only on Save&Close | P1 | Every hotkey mutation point pushes `set_hotkey` into the live hook manager right after saving (C# `Hotkey_Changed` -> `SaveHotkeySettings` -> `ApplyHotkeySettings`). |
| R22 | Base URL editable for non-custom providers | **not a divergence** | Verified in the C# XAML: `BaseUrlBox` is a plain editable TextBox (only `HotkeyKeyBox` is `IsReadOnly`). The port matches the source; no change made. |
| R23 | Actions table lacked column headers | P1 | Header row added with `Str_ColIcon` / `Str_ColName` / `Str_ColPrompt` / `Str_ColOutput` (40 / 95 / 265 / 98 as in the GridView). |
| R24 | Tooltip long text clipped and not selectable | P1 | Body is now a read-only gpui-kit `Textarea` (soft wrap, fixed height capped at 320 = C# `MaxHeight="320"`, internal scrollbar; selectable like the WPF read-only TextBox). Window measurement and `placement` math untouched. Gpui's scrollbar fades when idle (WPF's is always on) - visual, documented. |
| R25 | Pill-background click did not dismiss | P1 | Root pill container has a click listener that closes the window (C# `PillBorder_MouseDown`, which hides non-button clicks; button clicks already close, and the bubbled second close is a no-op). |
| R26 | `UpdateActionExecutor(None)` was ignored | P1 | `radial` now clears the stored executor, so removing the API key restores the "API key is not configured" guard (C# passes null when no key). |
| R27 | Unknown placement ints took the corrupt-recovery path | P2 | Any integer loads; 0..=3 map to the members, every other value lands on `NearCursor` - the same observable result as the C# `_ => nearCursor` switch arm. Serialization stays numeric. |
| R28 | Enum NAME strings in the hand-editable settings file were rejected | P2 | `PopupPlacementMode` and `ButtonDisplayMode` accept member names case-insensitively and numeric strings, like Newtonsoft; unknown names error into the source's corrupt-recovery path. (`"DisplayMode": 9` loads as discriminant 0 - the source stores it but has no switch on the executor path.) |
| R29 | Explicit-null JSON scalar/vector fields took the corrupt-recovery path | P2 | `deserialize_with` helpers map JSON null to the field default for all `AppSettings`/`ActionProfile` strings, the actions Vec, and the numbers - Newtonsoft coerces the same way (and the source's `IsNullOrWhiteSpace` checks then substitute the defaults). |
| R30 | `Rect::new` doc mislabeled the WPF ctor; `main.rs` doc claimed the STA/hook ordering | P2 | Docs corrected: `Rect::new` takes (left, top, right, bottom) edges vs WPF's (x, y, width, height); `main.rs` points at `ui::run()` for the ordering. |

### Decisions closed

- **R11 (superseded, closed):** `"Actions": null` now coerces to an empty list
  instead of the corrupt-recovery path. Verified against the source: C#
  `PopulateBarButtons` line 43 is `if (actions == null || actions.Count == 0)
  return;` - the source is null-tolerant on the radial path, so coercion is the
  source-behavior match (recovery was the divergence). The source's
  `SettingsWindow.RefreshActionList` (`new List<ActionProfile>(null)`) would throw
  `ArgumentNullException` when opening Settings; the port renders an empty table.
  That difference is kept deliberately (source-bug flavor, owner decision on file).
- **Digit-only filter not ported (note):** GPUI's input has no composition filter
  hook for C#'s `PreviewTextInput` digit filter; the commit parse reverts anything
  non-numeric or < 1 on Enter/Blur, which is the same observable outcome.
- **Enum-name acceptance (R28):** widened acceptance to match Newtonsoft exactly.

Verification for this wave: `cargo fmt --all --check` clean, `cargo clippy
--all-targets --all-features` and `--no-default-features` both 0 warnings,
`cargo test --all-features` **211 passed / 0 failed** (143 lib + 28 settings +
19 executor + 21 across action_gating / fallback_editor / placement),
`cargo test --no-default-features` 200 passed, `cargo build --release` OK, and a
10 s release smoke run: process alive, settings.json written with 91 LF / 91 CRLF.


---

## Visual-parity fix wave - 2026-10-13 (owner: orchestrator)

The static visual audit (`qa/VISUAL-AUDIT.md`) found one blocking defect plus 18
small fixes; all 19 are applied. Verified after each: fmt clean, clippy 0 warnings,
220 tests green (all-features), release build + 8 s smoke run alive.

| # | Finding | Fix |
|---|---|---|
| V1 (blocking) | Every colour was wrong: `gpui::rgba` takes `0xRRGGBBAA` but the theme tables are `0xAARRGGBB` (verified against crate source), so `AccentVi` rendered red-magenta, `PillBgBrush` translucent red, etc. | `common::theme_color` now converts through `argb_to_rgba` (alpha rotated to the low byte). One fix covers all 25 keys the UI reads. |
| V2 | Separator colour had the same byte-order bug | `gpui::rgba(0x70_80_FF_28)` for `FromArgb(0x28, 0x70, 0x80, 0xFF)` |
| V3 | Radial button labels rendered at GPUI's default 16 px (XAML: 11) | explicit `text_size(11)` on the label |
| V4 | Tooltip brand text `SEMIBOLD` (XAML: `Bold`) | `FontWeight::BOLD` |
| V5 | Tooltip separator missing the XAML `Opacity="0.5"` | `.opacity(0.5)` |
| V6 | Every `text_sm()`/`text_xs()` is 14/12 px; the XAML declares 10/11/12/13 | section headers, field labels and the API-key status are 11 px; hotkey/theme/language/footer/version/update rows 10-12 px as per XAML |
| V7 | API panel used uniform 18 padding (C#: `18,16`) | `panel_with_padding(theme, vertical)`; API panel 16, the other five 14 (the `panel` default) |
| V8 | ActionEditDialog's four field panels were not rendered (bare children, no `BgPanel` surface/border/radius 12) | `panel_tight` (padding 16) wraps name/icon, prompt, output-mode, display-mode + checkboxes |
| V9 | Actions table had no `GridViewColumnHeader` styling | `table_row` takes `header: bool`: padding 10,8, FontSize 11 SemiBold, TextLow, `BgDeep`, 1 px bottom border |
| V10 | Settings title used the localized `Str_Settings` | literal `"Orbital Settings"` (C# `Title=` is a literal) |
| V11 | Base URL stayed editable for non-custom providers (C# `xaml.cs:234` sets `IsReadOnly`) - corrects R22 | `InputState::set_readonly` applied at open and on provider change |
| V12 | Checkbox rows rendered at gpui-kit's 14 px default (C# 12/11) | `checkbox_row` takes `label_size`; settings rows 12, edit rows 11 |
| V13 | Trigger checkboxes were one vertical column (C# 2x2 grid) | two rows: Drag|DoubleClick, LongPress|Keyboard |
| V14 | Auto-close seconds box 140 px (C# 120) | 120 px |
| V15 | Buttons had no C# widths | Save 72, Clear 56, Import/Export/Add 76, up/down 36, Delete/Edit 76, Save&Close 120, plus the Save button's `Margin="8,0,0,0"` |
| V16 | ActionEdit save/cancel order + margins wrong | Save (primary, 82) then Cancel (82), row margin 14 |
| V17 | Name/icon columns were 50/50 (C# 3*/1*) and the icon box was not centered | `flex_grow(3)` / `flex_grow(1)`, `.text_center()` on the icon input |
| V18 | `UI_FONT_FAMILY` dropped the "Segoe UI Variable" half of the C# fallback chain | primary "Segoe UI Variable" + `FontFallbacks::from_fonts(["Segoe UI"])`, matching `App.xaml`'s chain; measured widths re-flow through the same `text_width` path |
| V19 | `Str_General` missing from 6 of 10 locales - `i18n::get`'s key fallback leaked the literal `Str_General` into the panel header | added the key to de/es/fr/it/pt/ru/zh (values: Allgemein, General, General, Generale, Geral, Obshchie, Changgui); all 10 locales now 117 keys |

Owner decisions applied: V11 (readonly - C# does it, so source-wins), V18 (font
chain - source-faithful; interacts with the D7/D15 measurement diffs, which stay
documented), V19 (key added - the raw-key leak was a port rendering artifact, not
the source behaviour). Still-open owner decisions from the audit: the separator
artefact (D21 wording amendment) and the settings title-dot gradient
(`#FF00D4FF -> #FF7080FF`, substituted with `AccentCyColor`).

---

## Runtime verification loop - 2026-10-13 (owner: orchestrator, `qa/RUNTIME-QA-REPORT.md`)

Driven with synthetic input against the release build. **All 18 checks PASS.**

| Verified at runtime | Result |
|---|---|
| Ctrl+A / drag / double-click / long-press / Ctrl+F1 custom hotkey -> radial pill | PASS (log + window rects + screenshots) |
| Pill button clicks -> actions (Copy clipboard, Cut emptied the edit, SelectAll re-show) | PASS |
| Esc + outside-click dismiss | PASS |
| Tray menu (Settings / Restart / Exit) | PASS (native #32768 menu, pid = app) |
| Tray -> Settings -> "Orbital Settings" window 700x689 | PASS |
| Settings UI (API panel, General, Actions table with headers, buttons) | PASS |
| ActionEditDialog (9 output modes, 3 display modes, checkboxes, Save before Cancel) | PASS |
| Color fix (blue accents, dark theme) and font sizes after the V-fixes | PASS |

Follow-ups recorded (not port bugs, no behavior change):
- V31: settings-window mouse-wheel scrolls ~1400px per notch, skipping panels
  (WPF ScrollViewer ~90px). Cosmetic.
- V32: tray icon lands in the Windows 11 overflow chevron (explorer behavior).
- Stretch (LLM Popup tooltip end-to-end) not run: needs an API key; unit-covered
  via LoggingTooltipSink + executor tests.

### Packaging follow-ups closed (2026-10-13)

- **V33**: `.gitignore` extended with `/dist` (installer + portable zip output).
- **V34**: `installer/Orbital.iss` `x64` -> `x64os` (identical behavior, the Inno
  deprecation warning is gone). Verified: `ISCC` exit 0, `Orbital-0.4.8-Setup.exe`
  8.5 MB.
- **Open (owner decision required):** the port has no `release.yml` equivalent — the
  C# repo publishes the release assets the in-app updater (`release_flow.rs`) looks
  for on GitHub. `cargo.yml` (build/test CI) exists per the plan; a release-publishing
  workflow was out of the plan's file list.

## Final gate status (2026-10-13)

| Gate | Result |
|---|---|
| `cargo fmt --all --check` | clean |
| `cargo clippy --all-targets --all-features` | 0 warnings |
| `cargo clippy --all-targets --no-default-features` | 0 warnings |
| `cargo test --all-features` | 220 passed / 0 failed |
| `cargo test --no-default-features` | 200 passed / 0 failed |
| `cargo doc --all-features --no-deps` | clean |
| `cargo build --release --all-features` | Orbital.exe 27.6 MB |
| Runtime QA (18 checks, synthetic input) | all PASS — `qa/RUNTIME-QA-REPORT.md` |
| Differential vs C# xunit (37 cases) | 0 unacceptable diffs — `qa/DIFFERENTIAL-REPORT.md` |
| Visual audit (static constants) | 19/19 fixes applied — `qa/VISUAL-AUDIT.md` |
| Installer compile + silent install | pass (`qa/INSTALLER-RUN.log`) |
| Portable zip build | pass (`qa/portable-run-2.log`) |

---

## Release workflow + packaging loop - 2026-10-14 (owner: orchestrator)

- **V35 (closed): `release.yml` added** (`.github/workflows/release.yml`) — tag
  `v*.*.*` -> version -> Cargo.toml -> `scripts/Build-Installer.ps1` (Inno Setup
  6.4.3 via `Minionguy62/Inno-Setup-Action`) -> `scripts/Build-Portable.ps1`
  -> rename to `OrbitalSetup.exe` (the name `release_flow.rs` prefers) -> GitHub
  Release with the installer + portable zip. This closes the "updater has no
  assets to find" gap.
- Verified end-to-end locally with a simulated tag run (v0.4.9): version extract,
  Cargo.toml bump, both build scripts, the rename, `dist/OrbitalSetup.exe`
  (8.5 MB) + `dist/Orbital-0.4.9-Portable.zip` (10.3 MB). Cargo.toml and the
  lockfile were restored to 0.4.8 afterwards.
- `scripts/Build-Installer.ps1` now also probes the portable Inno location
  (`C:\InnoSetupPortable\ISCC.exe`).

### V31 (settings-wheel scroll) — final diagnosis, needs real-mouse verification

Measured behavior: in the settings window, ANY wheel delta (tested -1, -8, -20,
-40 and a full -120 notch) scrolls ~790 px — effectively the whole content —
so panels in the middle (Appearance / Keyboard / Triggers) are skipped.
Traced the GPUI math: the Windows platform emits `ScrollDelta::Lines(3)` per
notch (`SPI_GETWHEELSCROLLLINES`), and the scrollable element converts it via
`pixel_delta(window.line_height())` = 3 x phi() x 16 px = ~78 px, which is sane.
The jump therefore does not come from the app's layout or our scroll container
(`window_root`'s `overflow_y_scroll`) and is not reproducible through the app's
event path — it was observed only with wheel deltas injected via
`mouse_event(MOUSEEVENTF_WHEEL)`, where any value behaves identically (also a
symptom of an input-injection artifact rather than of the app). Not fixed in the
port: no app-side defect found; **verify with a physical mouse wheel** — if a
real notch also jumps, the cause is inside the GPUI snapshot's wheel handling
and needs a fork patch, which is out of scope for the port itself.