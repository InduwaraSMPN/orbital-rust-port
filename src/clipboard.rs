//! Clipboard port of `ClipboardHelper.cs` — UIA-first selection read, Ctrl+C fallback
//! with backup -> inject -> poll -> restore, retry/backoff `min(10 * 2^i, 500)` on
//! clipboard contention, VK table verbatim, `ORBITAL_EXTRA_INFO` tag on every
//! injected key, 50/150 ms paste sleeps. Win32-direct (see PARITY-CONTRACT.md
//! decision 2) so multi-format backup/restore keeps `IDataObject` fidelity.

use std::io;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use uiautomation::{self, UIAutomation, patterns::UITextPattern, types::TextPatternRangeEndpoint};
use windows::Win32::Foundation::{GetLastError, HANDLE, HGLOBAL, HLOCAL, LocalFree};
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::DataExchange;
use windows::Win32::System::Memory::{
    GLOBAL_ALLOC_FLAGS, GMEM_MOVEABLE, GMEM_ZEROINIT, GlobalAlloc, GlobalLock, GlobalSize,
    GlobalUnlock,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_KEYBOARD, KEYBD_EVENT_FLAGS, KEYBDINPUT, KEYEVENTF_KEYUP, SendInput,
    VIRTUAL_KEY, VK_A, VK_BACK, VK_C, VK_DELETE, VK_ESCAPE, VK_LCONTROL, VK_RETURN, VK_SPACE,
    VK_TAB, VK_V,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CopyImage, GDI_IMAGE_TYPE, IMAGE_BITMAP, IMAGE_FLAGS,
};
use windows::core::BOOL;

/// C# `ORBITAL_EXTRA_INFO` = 0x4F524254 ('ORBT') — also read by `hooks.rs` to skip
/// our own injected keys while still allowing OSK/virtual keyboards.
pub const ORBITAL_EXTRA_INFO: usize = 0x4F524254;

// —— Clipboard format ids —————————————————————————————————————————————————————

/// `CF_TEXT` — ANSI text; read only as a fallback when the source app set no `CF_UNICODETEXT`.
const CF_TEXT: u32 = 1;
/// `CF_BITMAP` — a GDI object rather than a memory block, so it needs a `CopyImage` deep copy.
const CF_BITMAP: u32 = 2;
/// `CF_PALETTE` — a GDI object this port cannot deep-copy, so it is skipped rather than guessed at.
const CF_PALETTE: u32 = 9;
/// `CF_UNICODETEXT` — the format the poll waits for and the one `Clipboard.GetText` reads.
const CF_UNICODETEXT: u32 = 13;
/// `CF_ENHMETAFILE` — a GDI object, copied with `CopyEnhMetaFileW`.
const CF_ENHMETAFILE: u32 = 14;

// —— Timing (C# `RetryAction` / poll loop / `Thread.Sleep` values) ————————————

/// C# `maxRetries = 10`.
const MAX_RETRIES: usize = 10;
/// C# `maxWaitMs` — total poll window for the Ctrl+C confirmation.
const POLL_WINDOW_MS: u64 = 200;
/// C# `pollIntervalMs`.
const POLL_INTERVAL_MS: u64 = 10;
/// C# `Thread.Sleep(50)` shared by every injection path.
const PRE_INJECT_SLEEP_MS: u64 = 50;
/// C# `Thread.Sleep(150)` after the paste keystroke.
const POST_PASTE_SLEEP_MS: u64 = 150;

/// C# `_clipboardLock` — `lock (_clipboardLock)` around every clipboard-touching path.
static CLIPBOARD_LOCK: Mutex<()> = Mutex::new(());

/// C# `GetSelectedText` — UIA first, clipboard-Ctrl+C fallback.
pub fn get_selected_text() -> String {
    // C# `string text = GetSelectedTextViaUIA(); if (!string.IsNullOrEmpty(text)) return text;`
    let text = get_selected_text_via_uia();
    if !text.is_empty() {
        return text;
    }

    // C# `return GetSelectedTextViaClipboard();`
    get_selected_text_via_clipboard()
}

/// C# `ReplaceSelectedText` — UIA replace, clipboard fallback.
pub fn replace_selected_text(new_text: &str) {
    if replace_selected_text_via_uia(new_text) {
        return;
    }
    replace_selected_text_via_clipboard(new_text);
}

/// C# `DeleteSelectedText` — Delete key press/release.
pub fn delete_selected_text() {
    // The source takes the same lock here even though no clipboard call is made.
    let _guard = clipboard_lock();
    thread::sleep(Duration::from_millis(PRE_INJECT_SLEEP_MS));
    simulate_single_key(VK_DELETE);
}

/// C# `SimulateKey` — returns false for unknown names.
pub fn simulate_key(key_name: &str) -> bool {
    let vk = match key_name.trim().to_lowercase().as_str() {
        "delete" => VK_DELETE,
        "backspace" => VK_BACK,
        "enter" => VK_RETURN,
        "return" => VK_RETURN,
        "space" => VK_SPACE,
        "escape" => VK_ESCAPE,
        "esc" => VK_ESCAPE,
        "tab" => VK_TAB,
        _ => VIRTUAL_KEY(0),
    };
    // C# `if (vk == 0) return false;` — before the lock and before the sleep.
    if vk.0 == 0 {
        return false;
    }

    let _guard = clipboard_lock();
    thread::sleep(Duration::from_millis(PRE_INJECT_SLEEP_MS));
    simulate_single_key(vk);
    true
}

/// C# `SimulatePaste` — Ctrl+V.
pub fn simulate_paste() {
    let _guard = clipboard_lock();
    thread::sleep(Duration::from_millis(PRE_INJECT_SLEEP_MS));
    simulate_key_stroke(VK_LCONTROL, VK_V);
}

/// C# `SimulateSelectAll` — Ctrl+A.
pub fn simulate_select_all() {
    let _guard = clipboard_lock();
    thread::sleep(Duration::from_millis(PRE_INJECT_SLEEP_MS));
    simulate_key_stroke(VK_LCONTROL, VK_A);
}

/// C# `CopyToClipboard` — retry-wrapped `SetText`.
pub fn copy_to_clipboard(text: &str) {
    let _guard = clipboard_lock();
    retry_action(|| set_clipboard_unicode_text(text));
}

// —— Keyboard injection ———————————————————————————————————————————————————————

/// C# `SimulateKeyStroke` — Ctrl down, key down, key up, Ctrl up, all tagged.
fn simulate_key_stroke(modifier: VIRTUAL_KEY, key: VIRTUAL_KEY) {
    send_keyboard_inputs(&[
        keyboard_input(modifier, false),
        keyboard_input(key, false),
        keyboard_input(key, true),
        keyboard_input(modifier, true),
    ]);
}

/// C# `DeleteSelectedText` / `SimulateKey` — two tagged events, down then up.
fn simulate_single_key(vk: VIRTUAL_KEY) {
    send_keyboard_inputs(&[keyboard_input(vk, false), keyboard_input(vk, true)]);
}

/// Build one tagged `INPUT_KEYBOARD` event with `dwExtraInfo = ORBITAL_EXTRA_INFO`.
fn keyboard_input(vk: VIRTUAL_KEY, key_up: bool) -> INPUT {
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: vk,
                wScan: 0,
                dwFlags: if key_up {
                    KEYEVENTF_KEYUP
                } else {
                    KEYBD_EVENT_FLAGS(0)
                },
                time: 0,
                dwExtraInfo: ORBITAL_EXTRA_INFO,
            },
        },
    }
}

/// C# `SendInput((uint)inputs.Length, inputs, INPUT.Size)`.
fn send_keyboard_inputs(inputs: &[INPUT]) {
    if inputs.is_empty() {
        return;
    }
    // SAFETY: `inputs` is a live slice of `size_of::<INPUT>()`-sized records held for
    // the whole call, and `SendInput` only reads them.
    let sent = unsafe { SendInput(inputs, std::mem::size_of::<INPUT>() as i32) };
    if sent as usize != inputs.len() {
        tracing::debug!(
            expected = inputs.len(),
            sent = sent,
            "SendInput delivered fewer events"
        );
    }
}

/// Lock the clipboard mutex, tolerating a poisoned lock — a panicking clipboard path
/// must not take the rest of the app down with it.
fn clipboard_lock() -> std::sync::MutexGuard<'static, ()> {
    match CLIPBOARD_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

// —— Retry / backoff ——————————————————————————————————————————————————————————

/// C# `RetryAction(Action, int maxRetries = 10)`.
fn retry_action<F>(mut action: F)
where
    F: FnMut() -> io::Result<()>,
{
    for i in 0..=MAX_RETRIES {
        match action() {
            Ok(()) => return,
            Err(error) => {
                if i == MAX_RETRIES {
                    tracing::debug!(%error, "clipboard retry exhausted");
                } else {
                    thread::sleep(Duration::from_millis(backoff_delay_ms(i)));
                }
            }
        }
    }
}

/// C# `RetryFunc<T>(Func<T>, T fallback, int maxRetries = 10)`.
fn retry_func<T, F>(mut func: F, fallback: T) -> T
where
    F: FnMut() -> io::Result<T>,
{
    for i in 0..=MAX_RETRIES {
        match func() {
            Ok(value) => return value,
            Err(error) => {
                if i == MAX_RETRIES {
                    tracing::debug!(%error, "clipboard retry exhausted");
                } else {
                    thread::sleep(Duration::from_millis(backoff_delay_ms(i)));
                }
            }
        }
    }
    fallback
}

/// C# `Math.Min(10 * (1 << i), 500)`.
fn backoff_delay_ms(attempt: usize) -> u64 {
    let step = 1u64.checked_shl(attempt.min(20) as u32).unwrap_or(u64::MAX);
    std::cmp::min(10u64.saturating_mul(step), 500)
}

// —— Selection read / write via UIA ———————————————————————————————————————————

/// C# `GetSelectedTextViaUIA` — `AutomationElement.FocusedElement`, `TextPattern` on the
/// element or its control-view parent, collapsed-selection guard, `GetText(-1)`.
fn get_selected_text_via_uia() -> String {
    let Some((_element, pattern)) = focused_text_pattern() else {
        return String::new();
    };

    // C# `TextPatternRange[] selection = textPattern.GetSelection();
    //     if (selection == null || selection.Length == 0) return string.Empty;`
    let Ok(selection) = pattern.get_selection() else {
        return String::new();
    };
    let Some(range) = selection.into_iter().next() else {
        return String::new();
    };

    // C# `if (range.CompareEndpoints(Start, range, End) == 0) return string.Empty;`
    if is_collapsed(&range) {
        return String::new();
    }

    // C# `range.GetText(-1)` — the whole selection.
    range.get_text(-1).unwrap_or_default()
}

/// C# `ReplaceSelectedTextViaUIA`.
///
/// The C# reaches `TextPatternRange.Replace` through `dynamic`, which the .NET interop
/// surface does not expose. The Rust `uiautomation` crate does not wrap that COM method
/// either, so the whole path reports failure and the clipboard fallback runs. The
/// guards are still evaluated in source order, so the fallback happens under exactly
/// the conditions the source's `try/catch` would produce (PARITY-CONTRACT.md decision 6).
fn replace_selected_text_via_uia(_new_text: &str) -> bool {
    let Some((_element, pattern)) = focused_text_pattern() else {
        return false;
    };
    let Ok(selection) = pattern.get_selection() else {
        return false;
    };
    let Some(range) = selection.into_iter().next() else {
        return false;
    };
    if is_collapsed(&range) {
        return false;
    }

    // C# `dynamic dynRange = selectedRange; dynRange.Replace(newText, false);`
    tracing::debug!(
        "UIA TextPatternRange.Replace is not exposed by the uiautomation crate; replacing via the clipboard"
    );
    false
}

/// C# `range.CompareEndpoints(TextPatternRangeEndpoint.Start, range, TextPatternRangeEndpoint.End) == 0`.
fn is_collapsed(range: &uiautomation::patterns::UITextRange) -> bool {
    range
        .compare_endpoints(
            TextPatternRangeEndpoint::Start,
            range,
            TextPatternRangeEndpoint::End,
        )
        .map(|delta| delta == 0)
        .unwrap_or(false)
}

/// C# `AutomationElement.FocusedElement` + `TryGetCurrentPattern(TextPattern.Pattern)`,
/// falling back to `TreeWalker.ControlViewWalker.GetParent(element)`.
fn focused_text_pattern() -> Option<(uiautomation::core::UIElement, UITextPattern)> {
    let automation = create_automation()?;

    // C# `AutomationElement.FocusedElement` — no focused element is a failure, not null.
    let element = automation.get_focused_element().ok()?;

    // C# `element.TryGetCurrentPattern(TextPattern.Pattern, out object? patternObj)
    //     && patternObj is TextPattern tp`
    if let Ok(pattern) = element.get_pattern::<UITextPattern>() {
        return Some((element, pattern));
    }

    // C# `AutomationElement? parent = TreeWalker.ControlViewWalker.GetParent(element);`
    let walker = automation.get_control_view_walker().ok()?;
    let parent = walker.get_parent(&element).ok()?;

    // C# `parent.TryGetCurrentPattern(TextPattern.Pattern, out patternObj) && patternObj is TextPattern tp2`
    let pattern = parent.get_pattern::<UITextPattern>().ok()?;
    Some((parent, pattern))
}

/// Build a UIAutomation client, initializing COM on the calling thread when needed.
///
/// The crate's `UIAutomation::new()` hard-fails with `RPC_E_CHANGED_MODE` when the
/// thread is already in an STA apartment — which is exactly where the Rust port's main
/// thread lives — so COM is initialized tolerantly and the object is built directly.
/// `new_direct` yields `CO_E_NOTINITIALIZED` when neither works, and the caller then
/// falls back to the clipboard path just like the source's `catch` blocks do.
fn create_automation() -> Option<UIAutomation> {
    // SAFETY: `CoInitializeEx` is per-thread bookkeeping and takes no pointers.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
    UIAutomation::new_direct().ok()
}

// —— Selection read via clipboard —————————————————————————————————————————————

/// C# `GetSelectedTextViaClipboard`.
fn get_selected_text_via_clipboard() -> String {
    let _guard = clipboard_lock();

    // 1) C# `backup = Clipboard.GetDataObject(); Clipboard.Clear();`
    let mut backup = retry_func(backup_and_clear, Vec::new());

    // C# `SimulateKeyStroke(VK_LCONTROL, VK_C)`.
    simulate_key_stroke(VK_LCONTROL, VK_C);

    // C# poll loop — sleep 10 ms, test, break on the first hit, up to 200 ms total.
    let mut elapsed: u64 = 0;
    while elapsed < POLL_WINDOW_MS {
        thread::sleep(Duration::from_millis(POLL_INTERVAL_MS));
        elapsed += POLL_INTERVAL_MS;
        if retry_func(clipboard_contains_text, false) {
            break;
        }
    }

    // 2) C# read + restore in one dispatcher call, returning the text.
    let text = retry_func(|| read_text_and_restore(&backup), String::new());
    free_backup(&mut backup);
    text
}

/// C# `ReplaceSelectedTextViaClipboard`.
fn replace_selected_text_via_clipboard(new_text: &str) {
    let _guard = clipboard_lock();

    // C# `backup = Clipboard.GetDataObject(); Clipboard.SetText(newText);`
    let mut backup = retry_func(|| backup_and_set_text(new_text), Vec::new());

    thread::sleep(Duration::from_millis(PRE_INJECT_SLEEP_MS));

    // C# `SimulateKeyStroke(VK_LCONTROL, VK_V)`.
    simulate_key_stroke(VK_LCONTROL, VK_V);

    // C# `Thread.Sleep(150)` — let the target app process the paste.
    thread::sleep(Duration::from_millis(POST_PASTE_SLEEP_MS));

    // C# `if (backup != null) Clipboard.SetDataObject(backup, true);`
    retry_action(|| {
        open_clipboard()?;
        let restored = restore_backup(&backup);
        close_clipboard();
        restored
    });
    free_backup(&mut backup);
}

/// C# `backup = Clipboard.GetDataObject(); Clipboard.Clear();`.
fn backup_and_clear() -> io::Result<Vec<ClipboardEntry>> {
    open_clipboard()?;
    let backup = enumerate_formats();
    empty_clipboard()?;
    close_clipboard();
    Ok(backup)
}

/// C# `backup = Clipboard.GetDataObject(); Clipboard.SetText(newText);`.
fn backup_and_set_text(new_text: &str) -> io::Result<Vec<ClipboardEntry>> {
    open_clipboard()?;
    let backup = enumerate_formats();
    let written = write_unicode_text(new_text);
    close_clipboard();
    written?;
    Ok(backup)
}

/// C# `Clipboard.ContainsText()` — UnicodeText or Text.
fn clipboard_contains_text() -> io::Result<bool> {
    open_clipboard()?;
    let contains = is_format_available(CF_UNICODETEXT) || is_format_available(CF_TEXT);
    close_clipboard();
    Ok(contains)
}

/// C# `if (Clipboard.ContainsText()) text = Clipboard.GetText();
///    if (backup != null) Clipboard.SetDataObject(backup, true); return text;`
///
/// Must be called with the clipboard already open; `restore_backup` below has the
/// same precondition.
fn read_text_and_restore(backup: &[ClipboardEntry]) -> io::Result<String> {
    open_clipboard()?;
    let text = read_clipboard_text();
    let restored = restore_backup(backup);
    close_clipboard();
    restored?;
    Ok(text)
}
/// C# `Clipboard.SetDataObject(backup, true)` — every backed-up format is written back.
///
/// Needs the clipboard to already be open, and writes a fresh copy of each entry so a
/// retry after a contention failure can run again.
fn restore_backup(backup: &[ClipboardEntry]) -> io::Result<()> {
    if backup.is_empty() {
        return Ok(());
    }
    let _ = empty_clipboard();
    for entry in backup.iter() {
        match duplicate_entry(entry) {
            Some(object) => {
                if set_clipboard_data(entry.format, object).is_err() {
                    tracing::debug!(format = entry.format, "restore SetClipboardData failed");
                    free_object(entry.format, object);
                }
            }
            None => {
                tracing::debug!(
                    format = entry.format,
                    "restore could not duplicate a backed-up clipboard entry"
                );
            }
        }
    }
    Ok(())
}

/// Release every private copy made by `backup_and_clear`.
fn free_backup(backup: &mut Vec<ClipboardEntry>) {
    for entry in backup.drain(..) {
        free_object(entry.format, entry.object);
    }
}

/// C# `Clipboard.GetText()` — UnicodeText, falling back to Text the way the
/// auto-converting `DataObject` does.
fn read_clipboard_text() -> String {
    if let Some(bytes) = get_clipboard_data(CF_UNICODETEXT).and_then(locked_bytes) {
        return utf16_z_to_string(&bytes);
    }
    if let Some(bytes) = get_clipboard_data(CF_TEXT).and_then(locked_bytes) {
        return String::from_utf8_lossy(&bytes).into_owned();
    }
    String::new()
}

// —— Raw clipboard object backup ——————————————————————————————————————————————

/// One backed-up clipboard format: the format id plus a private copy of its data.
///
/// The copy is required because `EmptyClipboard` destroys whatever handles the
/// clipboard currently holds, so the backup must own its data before the paste runs.
struct ClipboardEntry {
    format: u32,
    object: HGLOBAL,
}

/// C# `Clipboard.GetDataObject()` snapshot — every format currently on the clipboard.
///
/// The clipboard must already be open. Formats whose `GetClipboardData` returns null
/// (delayed-render, or a GDI object this port cannot deep-copy) are skipped rather
/// than guessed at.
fn enumerate_formats() -> Vec<ClipboardEntry> {
    let mut entries = Vec::new();
    let mut format: u32 = 0;
    loop {
        let next = enum_clipboard_formats(format);
        if next == 0 {
            break;
        }
        format = next;
        match copy_clipboard_object(format) {
            Some(object) => entries.push(ClipboardEntry { format, object }),
            None => {
                tracing::debug!(
                    format,
                    "clipboard format has no readable data (delayed render or unsupported handle type); not backed up"
                );
            }
        }
    }
    entries
}

/// Take a private copy of one clipboard object.
fn copy_clipboard_object(format: u32) -> Option<HGLOBAL> {
    let source = get_clipboard_data(format)?;
    match format {
        CF_BITMAP => copy_bitmap(source),
        CF_ENHMETAFILE => copy_enh_metafile(source),
        CF_PALETTE => None,
        _ => copy_global_memory(source),
    }
}

/// Re-take a private copy of an already backed-up entry, for the restore write-back.
fn duplicate_entry(entry: &ClipboardEntry) -> Option<HGLOBAL> {
    match entry.format {
        CF_BITMAP => copy_bitmap(entry.object),
        CF_ENHMETAFILE => copy_enh_metafile(entry.object),
        CF_PALETTE => None,
        _ => copy_global_memory(entry.object),
    }
}

/// `GlobalAlloc(GMEM_MOVEABLE)` plus a byte copy of a moveable clipboard memory block.
///
/// This covers the synthesized formats the source treats as raw bytes
/// (`CF_UNICODETEXT`, `CF_HDROP`, `CF_DIB`, `CF_LOCALE`, and private formats).
fn copy_global_memory(source: HGLOBAL) -> Option<HGLOBAL> {
    let size = global_size(source);
    if size == 0 {
        return None;
    }
    let destination = global_alloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size)?;

    // SAFETY: both handles are live process handles that this port owns or holds
    // through the open clipboard.
    let source_pointer = unsafe { GlobalLock(source) };
    if source_pointer.is_null() {
        free_hglobal(destination);
        return None;
    }
    // SAFETY: `destination` is a live handle allocated for `size` bytes.
    let destination_pointer = unsafe { GlobalLock(destination) };
    if destination_pointer.is_null() {
        // SAFETY: `source` is locked above and remains alive.
        let _ = unsafe { GlobalUnlock(source) };
        free_hglobal(destination);
        return None;
    }

    // SAFETY: `source_pointer` exposes exactly `size` readable bytes and `destination`
    // was allocated with the same size, so the two regions cannot overlap.
    unsafe {
        std::ptr::copy_nonoverlapping(
            source_pointer.cast::<u8>(),
            destination_pointer.cast::<u8>(),
            size,
        );
    }

    // The block must be handed to SetClipboardData unlocked.
    // SAFETY: both handles were locked above and are still alive.
    unsafe {
        let _ = GlobalUnlock(destination);
        let _ = GlobalUnlock(source);
    }
    Some(destination)
}

/// `CF_BITMAP` deep copy — a GDI object, not a memory block, so bytes alone would not
/// restore it.
fn copy_bitmap(source: HGLOBAL) -> Option<HGLOBAL> {
    // SAFETY: `source` is a handle held by the open clipboard; CopyImage only reads it
    // and returns an independent object of the same type and dimensions.
    let copied = unsafe {
        CopyImage(
            HANDLE(source.0),
            GDI_IMAGE_TYPE(IMAGE_BITMAP.0),
            0,
            0,
            IMAGE_FLAGS(0),
        )
    };
    match copied {
        Ok(handle) => Some(HGLOBAL(handle.0)),
        Err(_) => None,
    }
}

/// `CF_ENHMETAFILE` deep copy — a GDI object; `CopyEnhMetaFileW` with a null file name
/// builds the in-memory copy.
fn copy_enh_metafile(source: HGLOBAL) -> Option<HGLOBAL> {
    windows::core::link!("gdi32.dll" "system" fn CopyEnhMetaFileW(henh: HGLOBAL, lpfilename: *const u16) -> HGLOBAL);
    // SAFETY: `source` is a handle held by the open clipboard and the null file name
    // selects the in-memory copy of it.
    let copied = unsafe { CopyEnhMetaFileW(source, std::ptr::null()) };
    if copied.0.is_null() {
        None
    } else {
        Some(copied)
    }
}

/// Release a private copy that never reached `SetClipboardData` — ownership is still
/// with this process, so it must be freed by the handle's own owner.
fn free_object(format: u32, object: HGLOBAL) {
    if object.0.is_null() {
        return;
    }
    match format {
        CF_BITMAP => {
            // SAFETY: `object` is a process-private bitmap copy that was never handed
            // to the clipboard.
            unsafe {
                let _ = DeleteObject(HGDIOBJ(object.0));
            }
        }
        CF_ENHMETAFILE => {
            windows::core::link!("gdi32.dll" "system" fn DeleteEnhMetaFile(hmf: HGLOBAL) -> BOOL);
            // SAFETY: `object` is a process-private metafile copy that was never handed
            // to the clipboard.
            unsafe {
                let _ = DeleteEnhMetaFile(object);
            }
        }
        _ => free_hglobal(object),
    }
}

/// Free a local-heap block with the same allocator `GlobalAlloc` used.
fn free_hglobal(object: HGLOBAL) {
    if object.0.is_null() {
        return;
    }
    // SAFETY: `object` is a process-private clone that was never handed to
    // SetClipboardData, so this process still owns it.
    unsafe {
        LocalFree(Some(HLOCAL(object.0)));
    }
}

/// Copy a locked clipboard memory block into an owned buffer.
///
/// The copy keeps the lock window as short as possible and avoids handing out a
/// slice borrowed from Windows-owned memory.
fn locked_bytes(handle: HGLOBAL) -> Option<Vec<u8>> {
    let size = global_size(handle);
    if size == 0 {
        return None;
    }
    // SAFETY: `handle` is held by the open clipboard and is alive for the reads below.
    let pointer = unsafe { GlobalLock(handle) };
    if pointer.is_null() {
        return None;
    }
    let bytes = {
        // SAFETY: `pointer` exposes exactly `size` readable bytes while locked.
        unsafe { std::slice::from_raw_parts(pointer.cast::<u8>(), size) }
    };
    let result = bytes.to_vec();
    // SAFETY: `handle` was locked above.
    unsafe {
        let _ = GlobalUnlock(handle);
    }
    Some(result)
}

fn global_size(object: HGLOBAL) -> usize {
    // SAFETY: `object` is a live process handle.
    unsafe { GlobalSize(object) }
}

fn global_alloc(flags: GLOBAL_ALLOC_FLAGS, bytes: usize) -> Option<HGLOBAL> {
    // SAFETY: no pointer arguments; `GlobalAlloc` returns a fresh moveable handle.
    unsafe { GlobalAlloc(flags, bytes).ok() }
}

// —— Raw clipboard access —————————————————————————————————————————————————————
//
// Thin wrappers over the `windows` crate's `Win32::System::DataExchange` bindings;
// the source calls the same user32 entry points through `Clipboard.GetDataObject`.

/// `OpenClipboard(NULL)` — null owner window, like the source's clipboard calls.
fn open_clipboard() -> io::Result<()> {
    // SAFETY: `OpenClipboard` takes only the owner HWND, passed as None.
    unsafe { DataExchange::OpenClipboard(None) }
        .map_err(|error| std::io::Error::from_raw_os_error(error.code().0))
}

/// `CloseClipboard()` — failures are ignored by the source as well.
fn close_clipboard() {
    // SAFETY: `CloseClipboard` takes no arguments.
    unsafe {
        let _ = DataExchange::CloseClipboard();
    }
}

/// `EmptyClipboard()`.
fn empty_clipboard() -> io::Result<()> {
    // SAFETY: no arguments; the calling thread must already own the clipboard.
    unsafe { DataExchange::EmptyClipboard() }
        .map_err(|error| std::io::Error::from_raw_os_error(error.code().0))
}

/// `EnumClipboardFormats(format)` — 0 means "enumeration complete" or a failure.
fn enum_clipboard_formats(format: u32) -> u32 {
    // SAFETY: `format` is a plain format id, 0 to start the enumeration.
    unsafe { DataExchange::EnumClipboardFormats(format) }
}

/// `GetClipboardData(format)` — `None` for delayed-render formats and errors, which the
/// source's backup could not read either.
fn get_clipboard_data(format: u32) -> Option<HGLOBAL> {
    // SAFETY: `format` is a plain format id; the handle belongs to the clipboard, so
    // it is only locked (never freed) while the clipboard stays open.
    match unsafe { DataExchange::GetClipboardData(format) } {
        Ok(handle) => {
            if handle.0.is_null() {
                None
            } else {
                Some(HGLOBAL(handle.0))
            }
        }
        Err(_) => None,
    }
}

/// `SetClipboardData(format, hmem)` — the system takes ownership on success.
fn set_clipboard_data(format: u32, hmem: HGLOBAL) -> io::Result<()> {
    // SAFETY: `hmem` is a moveable block this process still owns; the clipboard takes
    // it over on success, and on failure ownership stays with the caller, which frees it.
    let result = unsafe { DataExchange::SetClipboardData(format, Some(HANDLE(hmem.0))) };
    match result {
        Ok(handle) if !handle.0.is_null() => Ok(()),
        _ => Err(last_os_error()),
    }
}

/// `IsClipboardFormatAvailable(format)`.
fn is_format_available(format: u32) -> bool {
    // SAFETY: `format` is a plain format id.
    unsafe { DataExchange::IsClipboardFormatAvailable(format) }.is_ok()
}

// —— Text write-back ——————————————————————————————————————————————————————————

/// C# `Clipboard.SetText(text)` — retry-wrapped `SetText` with the same contention backoff.
fn set_clipboard_unicode_text(text: &str) -> io::Result<()> {
    open_clipboard()?;
    let written = write_unicode_text(text);
    close_clipboard();
    written
}

/// Write `CF_UNICODETEXT` into the already-open clipboard.
fn write_unicode_text(text: &str) -> io::Result<()> {
    // `SetText` on a WPF data object clears the previous contents implicitly.
    let _ = empty_clipboard();

    let encoded: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let object = match global_alloc(
        GMEM_MOVEABLE | GMEM_ZEROINIT,
        std::mem::size_of::<u16>() * encoded.len(),
    ) {
        Some(object) => object,
        None => return Err(last_os_error()),
    };

    // SAFETY: `object` is a live handle this process owns.
    let pointer = unsafe { GlobalLock(object) };
    if pointer.is_null() {
        free_hglobal(object);
        return Err(last_os_error());
    }
    // SAFETY: the block was allocated for exactly `encoded.len()` UTF-16 units and
    // `pointer` is its locked address, so the copy fits.
    unsafe {
        std::ptr::copy_nonoverlapping(encoded.as_ptr(), pointer.cast::<u16>(), encoded.len());
    }
    // SAFETY: `object` was locked above.
    unsafe {
        let _ = GlobalUnlock(object);
    }

    match set_clipboard_data(CF_UNICODETEXT, object) {
        Ok(()) => Ok(()),
        Err(error) => {
            free_hglobal(object);
            Err(error)
        }
    }
}

// —— Small helpers ————————————————————————————————————————————————————————————

/// Current thread's last Win32 error as an `io::Error`.
fn last_os_error() -> io::Error {
    // SAFETY: `GetLastError` only reads the calling thread's last-error value.
    let code = unsafe { GetLastError() };
    io::Error::from_raw_os_error(code.0 as i32)
}

/// Decode a NUL-terminated UTF-16 block, truncated at the terminator.
fn utf16_z_to_string(bytes: &[u8]) -> String {
    let units: &[u16] =
        unsafe { std::slice::from_raw_parts(bytes.as_ptr().cast::<u16>(), bytes.len() / 2) };
    let units = &units[..units
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(units.len())];
    String::from_utf16_lossy(units)
}
