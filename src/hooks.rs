//! Low-level hooks port of `SystemHookManager.cs` — `SetWindowsHookExW` WH_MOUSE_LL(14)
//! / WH_KEYBOARD_LL(13) with a message-pump thread, `'static` procs, 300 ms long-press
//! timer, 8 px drag threshold, double-click via `GetDoubleClickTime` + SM_CX/DOUBLECLK
//! (36/37), shell-class guards, self-injection skip (ORBITAL_EXTRA_INFO), Shift/Ctrl
//! tracking, modifier mask Ctrl 0x02 / Alt 0x01 / Shift 0x04, hotkey match, and a
//! channel send per event — the hook proc never blocks.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::thread;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{HINSTANCE, HMODULE, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleHandleExW,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, GetDoubleClickTime};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GA_ROOT, GetAncestor, GetClientRect, GetMessageW, GetSystemMetrics,
    GetWindowRect, HHOOK, KBDLLHOOKSTRUCT, MSG, MSLLHOOKSTRUCT, PostThreadMessageW,
    SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_KEYDOWN, WM_KEYUP,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_QUIT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_SYSKEYUP,
};
use windows::Win32::UI::WindowsAndMessaging::{SM_CXDOUBLECLK, SM_CYDOUBLECLK};

use crate::clipboard::ORBITAL_EXTRA_INFO;
use crate::platform::win32;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MousePoint {
    pub x: i32,
    pub y: i32,
}

impl MousePoint {
    /// C# `Math.Abs(pt.X - other.X)` / `Math.Abs(pt.Y - other.Y)`.
    fn delta(self, other: MousePoint) -> (i32, i32) {
        (
            self.x.abs_diff(other.x) as i32,
            self.y.abs_diff(other.y) as i32,
        )
    }
}

impl From<POINT> for MousePoint {
    fn from(point: POINT) -> Self {
        Self {
            x: point.x,
            y: point.y,
        }
    }
}

/// Port of the SystemHookManager event set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    AnyMouseDown(MousePoint),
    MouseUp(MousePoint),
    LongPress(MousePoint),
    DoubleClickRelease(MousePoint),
    KeyboardSelection(MousePoint),
    EscapePressed,
    ClipboardShortcut,
    CustomHotkey(MousePoint),
}

/// C# `HotkeyModifiers` / `HotkeyVirtualKey` — live-updated by the settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HotkeyState {
    pub modifiers: u32,
    pub virtual_key: u32,
}

pub struct HookManager {
    events: Receiver<HookEvent>,
    hotkey: Arc<Mutex<HotkeyState>>,
    last_down: Arc<Mutex<(MousePoint, String)>>,
    /// Pump threads, behind their own lock so `stop` and `Drop` share the teardown.
    threads: Mutex<Vec<thread::JoinHandle<()>>>,
}

/// Mouse hook failure is fatal to the app (C# shows Str_HookErrorMsg and exits);
/// keyboard hook failure is tolerated.
#[derive(Debug)]
pub struct HookStartError {
    pub mouse_error_code: Option<i32>,
}

impl std::fmt::Display for HookStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.mouse_error_code {
            Some(code) => write!(f, "mouse hook installation failed with Win32 error {code}"),
            None => write!(f, "hook manager failed to start"),
        }
    }
}

impl std::error::Error for HookStartError {}

// —— Shared hook state ————————————————————————————————————————————————————————

/// C# `DragThreshold` = 8.
const DRAG_THRESHOLD: i32 = 8;
/// C# `LongPressMs` = 300.
const LONG_PRESS_MS: u64 = 300;

/// Bounded event queue. Overflow drops the event instead of blocking the hook proc,
/// which a low-level hook must never do.
const EVENT_QUEUE_DEPTH: usize = 4096;

/// How long `stop` waits for the pump thread to exit before giving up on the join.
const JOIN_TIMEOUT: Duration = Duration::from_millis(2_000);
/// How long `start` waits for the pump thread to install its hooks.
const START_TIMEOUT: Duration = Duration::from_secs(5);

/// C# virtual-key codes used by the keyboard hook.
const VK_SHIFT: u32 = 0x10;
const VK_CONTROL: u32 = 0x11;
const VK_ALT: u32 = 0x12;
const VK_LSHIFT: u32 = 0xA0;
const VK_RSHIFT: u32 = 0xA1;
const VK_LCONTROL: u32 = 0xA2;
const VK_RCONTROL: u32 = 0xA3;
const VK_ESCAPE: u32 = 0x1B;
const VK_A: u32 = 0x41;
const VK_C: u32 = 0x43;
const VK_V: u32 = 0x56;
const VK_X: u32 = 0x58;

/// C# `s_navKeys` — Shift + any of these starts a keyboard selection.
const NAV_KEYS: [u32; 8] = [
    0x25, 0x26, 0x27, 0x28, // VK_LEFT, VK_UP, VK_RIGHT, VK_DOWN
    0x21, 0x22, // VK_PRIOR (page up), VK_NEXT (page down)
    0x23, 0x24, // VK_END, VK_HOME
];

/// C# hotkey modifier masks.
const HOTKEY_MOD_ALT: u32 = 0x01;
const HOTKEY_MOD_CONTROL: u32 = 0x02;
const HOTKEY_MOD_SHIFT: u32 = 0x04;

/// The state of the one long-press timer the source keeps per mouse-down.
///
/// The source's `System.Threading.Timer` is replaced by a detached thread that sleeps
/// `LONG_PRESS_MS` and then updates the shared flags. A thread cannot be cancelled
/// cheaply, so the timer is *disarmed* instead: `armed` is cleared on every button-up
/// (the source's `Dispose`) and a monotonically increasing `generation` is stamped when
/// a timer starts. A waking timer applies only when both still match its own values,
/// which is what makes a cancelled or superseded timer a no-op — exactly like the
/// source's disposed timer never invoking its callback.
///
/// `generation` never wraps in practice (a click every 10 ms for a century) and never
/// resets, because a reset would let an old timer alias a new one.
#[derive(Debug, Default, Clone, Copy)]
struct LongPressTimer {
    generation: u64,
    armed: bool,
}

impl LongPressTimer {
    /// C# `_longPressTimer?.Dispose(); _longPressTimer = null;`
    ///
    /// The generation is deliberately left alone: clearing `armed` is what stops the
    /// in-flight thread, while the untouched generation still identifies it uniquely.
    fn disarm(&mut self) {
        self.armed = false;
    }

    /// C# `_longPressTimer = new Timer(LongPressTimerCallback, null, LongPressMs, Timeout.Infinite);`
    fn arm(&mut self) -> Self {
        self.generation = self.generation.wrapping_add(1);
        self.armed = true;
        *self
    }

    /// True only for the timer this process most recently armed and never disarmed.
    fn is_current(&self, other: &LongPressTimer) -> bool {
        self.armed && other.armed && self.generation == other.generation
    }
}

/// All mutable state the two hook procs and the long-press timers share.
///
/// The source keeps this in `static` fields. It lives behind one mutex here so the
/// procs, the timer threads and `stop` can all reach it; every critical section is a
/// handful of scalar writes, so a hook proc never waits behind a slow operation.
#[derive(Debug, Default)]
struct HookState {
    /// C# `_buttonDownPos`.
    button_down_pos: MousePoint,
    /// C# `_mouseDownInClient`.
    mouse_down_in_client: bool,
    /// C# `_isLongPressed`.
    long_pressed: bool,
    /// C# `_isDoubleClick`.
    double_click: bool,
    /// C# `_lastClickTime` / `_lastClickPos` — `None` is C# `DateTime.MinValue`, the
    /// "no previous click" sentinel that always fails the double-click time test.
    last_click: Option<(Instant, MousePoint)>,
    /// C# `_shiftDown` / `_ctrlDown`.
    shift_down: bool,
    ctrl_down: bool,
    /// C# `_pendingKeyboardSelection`.
    pending_keyboard_selection: bool,
    /// C# `HotkeyModifiers` / `HotkeyVirtualKey`.
    hotkey: HotkeyState,
    /// Long-press timer bookkeeping for the current press.
    long_press: LongPressTimer,
}

/// The runtime installed on the dedicated pump thread: the channel the procs publish
/// into, the hook handles they are called through, and the shared state they mutate.
struct HookRuntime {
    state: Mutex<HookState>,
    events: SyncSender<HookEvent>,
    stop_requested: AtomicBool,
    pump_thread_id: AtomicU32,
    mouse_hook: AtomicUsize,
    keyboard_hook: AtomicUsize,
    /// Last Win32 error code from `SetWindowsHookExW`, captured on the pump thread where
    /// `GetLastError` is still meaningful (the caller's thread-local would be stale).
    mouse_install_error: std::sync::atomic::AtomicI32,
}

/// Process-wide handle on the installed runtime, so the `'static` hook procs and the
/// long-press timer threads can reach it without capturing anything.
static RUNTIME: Mutex<Option<Arc<HookRuntime>>> = Mutex::new(None);

/// C# `LastButtonDownPos` + `LastButtonDownHwndClass` snapshot shared with the manager.
type ButtonDownSnapshot = Arc<Mutex<(MousePoint, String)>>;

/// C# `LastButtonDownPos` snapshot, published by the pump thread before events fire.
static LAST_BUTTON_DOWN: Mutex<Option<ButtonDownSnapshot>> = Mutex::new(None);

impl HookRuntime {
    /// Read the shared state, recovering from a poisoned lock.
    fn lock_state(&self) -> std::sync::MutexGuard<'_, HookState> {
        match self.state.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// Read the installed runtime, if a hook manager is currently running.
fn runtime() -> Option<Arc<HookRuntime>> {
    match RUNTIME.lock() {
        Ok(guard) => guard.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

impl HookManager {
    /// C# `StartMouseHook` + `StartKeyboardHook`.
    ///
    /// A dedicated OS thread installs both hooks — deriving the module handle from the
    /// proc address instead of the exe name, which keeps the install correct for a
    /// low-level hook — and then runs the GetMessageW pump, exactly like the source's
    /// hook thread. A failed mouse hook is reported to the caller as `Err`, because the
    /// source treats that as fatal; a failed keyboard hook is only logged.
    pub fn start() -> Result<Self, HookStartError> {
        let (sender, events) = sync_channel::<HookEvent>(EVENT_QUEUE_DEPTH);
        let runtime = Arc::new(HookRuntime {
            state: Mutex::new(HookState::default()),
            events: sender,
            stop_requested: AtomicBool::new(false),
            pump_thread_id: AtomicU32::new(0),
            mouse_hook: AtomicUsize::new(0),
            keyboard_hook: AtomicUsize::new(0),
            mouse_install_error: std::sync::atomic::AtomicI32::new(0),
        });

        // Hotkey state lives in the shared state, so a manager started later keeps
        // whatever `set_hotkey` last published.
        let hotkey = Arc::new(Mutex::new(runtime.lock_state().hotkey));
        let last_down = Arc::new(Mutex::new((MousePoint::default(), String::new())));

        {
            let mut slot = lock_runtime_slot();
            *slot = Some(Arc::clone(&runtime));
        }
        *lock_last_button_down_slot() = Some(Arc::clone(&last_down));

        let pump_runtime = Arc::clone(&runtime);
        let pump = match thread::Builder::new()
            .name("orbital-hook-pump".to_string())
            .spawn(move || pump_thread_main(pump_runtime))
        {
            Ok(handle) => handle,
            Err(error) => {
                tracing::warn!(%error, "could not spawn the hook pump thread");
                *lock_runtime_slot() = None;
                *lock_last_button_down_slot() = None;
                return Err(HookStartError {
                    mouse_error_code: None,
                });
            }
        };

        // Wait for the pump thread to report whether the mouse hook could be installed.
        let started = Instant::now();
        while started.elapsed() < START_TIMEOUT {
            let installed = runtime.mouse_hook.load(Ordering::SeqCst);
            if installed != 0 {
                break;
            }
            if pump.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }

        if runtime.mouse_hook.load(Ordering::SeqCst) == 0 {
            let error_code = runtime.mouse_install_error.load(Ordering::SeqCst);
            runtime.stop_requested.store(true, Ordering::SeqCst);
            post_wm_quit(runtime.pump_thread_id.load(Ordering::SeqCst));
            let _ = pump.join();
            *lock_runtime_slot() = None;
            *lock_last_button_down_slot() = None;
            return Err(HookStartError {
                mouse_error_code: Some(error_code),
            });
        }

        Ok(Self {
            events,
            hotkey,
            last_down,
            threads: Mutex::new(vec![pump]),
        })
    }

    pub fn events(&self) -> &Receiver<HookEvent> {
        &self.events
    }

    /// C# `ApplyHotkeySettings` — the custom hotkey is process-wide in the source, so
    /// it is published on the shared state the procs read as well as on this manager.
    pub fn set_hotkey(&self, modifiers: u32, virtual_key: u32) {
        if let Ok(mut hotkey) = self.hotkey.lock() {
            hotkey.modifiers = modifiers;
            hotkey.virtual_key = virtual_key;
        }
        if let Some(runtime) = runtime() {
            let mut state = runtime.lock_state();
            state.hotkey.modifiers = modifiers;
            state.hotkey.virtual_key = virtual_key;
        }
    }

    /// C# `LastButtonDownPos` + `LastButtonDownHwndClass`.
    ///
    /// Both are captured on the pump thread before the matching events are published,
    /// so reading them in an event handler sees the click that produced the event.
    pub fn last_button_down(&self) -> (MousePoint, String) {
        match self.last_down.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// C# `StopHooks` — disposing the manager stops the pump threads.
    pub fn stop(self) {
        // The state is torn down in `Drop`, so both `stop` and a plain drop behave the
        // same. `HookTriggerTeardown` below documents the teardown path.
    }

    /// C# `StopHooks` — unhook, wake the pump thread, join it, clear the runtime slot.
    fn shutdown(&self) {
        let Some(runtime) = runtime() else {
            return;
        };

        if runtime.stop_requested.swap(true, Ordering::SeqCst) {
            return;
        }

        // Hooks go first and always: a proc that fires after this point would otherwise
        // publish into a teardown-time state.
        unhook(&runtime.mouse_hook);
        unhook(&runtime.keyboard_hook);
        post_wm_quit(runtime.pump_thread_id.load(Ordering::SeqCst));

        // Bound the join so a stuck pump thread cannot hang the caller.
        let handles = std::mem::take(&mut self.threads_lock());
        for handle in handles {
            join_bounded(handle);
        }

        *lock_runtime_slot() = None;
        *lock_last_button_down_slot() = None;
    }

    /// Take the pump handles out of the manager so `stop` can consume them, while
    /// `Drop` can also call `shutdown` on the same manager.
    fn threads_lock(&self) -> Vec<thread::JoinHandle<()>> {
        match self.threads.lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(poisoned) => std::mem::take(&mut *poisoned.into_inner()),
        }
    }
}

impl Drop for HookManager {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock_runtime_slot() -> std::sync::MutexGuard<'static, Option<Arc<HookRuntime>>> {
    match RUNTIME.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn lock_last_button_down_slot() -> std::sync::MutexGuard<'static, Option<ButtonDownSnapshot>> {
    match LAST_BUTTON_DOWN.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// Publish the current null-[button-down](self) snapshot where the app can read it.
fn publish_button_down(point: MousePoint, class: &str) {
    let slot = lock_last_button_down_slot();
    if let Some(shared) = slot.as_ref() {
        let mut guard = match shared.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        *guard = (point, class.to_string());
    }
}

/// Publish one event, dropping it rather than ever blocking the hook proc.
fn emit(runtime: &HookRuntime, event: HookEvent) {
    match runtime.events.try_send(event) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            // The queue is 4096 deep; only a very slow consumer can hit this. Dropping
            // is the only option that keeps a low-level hook proc safe.
        }
        Err(TrySendError::Disconnected(_)) => {}
    }
}

// —— Pump thread ——————————————————————————————————————————————————————————————

/// Install both hooks, then pump messages until quit.
fn pump_thread_main(runtime: Arc<HookRuntime>) {
    runtime
        .pump_thread_id
        .store(current_thread_id(), Ordering::SeqCst);
    install_hooks(&runtime);

    while !runtime.stop_requested.load(Ordering::SeqCst) {
        let mut message = MSG::default();
        // SAFETY: `message` is a live out slot and a null HWND selects this thread's
        // queue; GetMessageW blocks until a message or WM_QUIT arrives.
        let result = unsafe { GetMessageW(&raw mut message, None, 0, 0) };
        if result.0 <= 0 {
            break;
        }
        // Input arrives through the LL hook procs, not through this queue, so there is
        // nothing to dispatch — matching the source's pump.
    }

    unhook(&runtime.mouse_hook);
    unhook(&runtime.keyboard_hook);
}

/// `HRESULT_FROM_WIN32(code)` unwrap: the Win32 error code inside a 0x8007xxxx
/// HRESULT. Anything else is returned unchanged rather than guessed at.
fn win32_code_from_hresult(hresult: i32) -> i32 {
    const FACILITY_WIN32: u32 = 0x8007_0000;
    if (hresult as u32) & 0xFFFF_0000 == FACILITY_WIN32 {
        hresult & 0xFFFF
    } else {
        hresult
    }
}

/// C# `StartMouseHook` + `StartKeyboardHook` bodies: install mouse first (its failure is
/// fatal), then keyboard (tolerated).
fn install_hooks(runtime: &HookRuntime) {
    let module = module_for_hook_proc();

    // SAFETY: the proc is a `'static` fn item of this image, and the module handle comes
    // from the proc's own address; a zero thread id makes it a global low-level hook.
    let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook_proc), Some(module), 0) };
    match mouse {
        Ok(hook) => {
            runtime
                .mouse_hook
                .store(hook_address(hook), Ordering::SeqCst);
            tracing::info!("the low-level mouse hook is installed");
        }
        Err(error) => {
            // Capture the code on this thread: `GetLastError` is thread-local, so the
            // caller cannot read it after `start` returns. `windows_core::Error` wraps
            // the Win32 code as `HRESULT_FROM_WIN32` (0x8007xxxx); the source shows the
            // raw Win32 code (C# `Marshal.GetLastWin32Error`), so unwrap it here.
            runtime
                .mouse_install_error
                .store(win32_code_from_hresult(error.code().0), Ordering::SeqCst);
            tracing::warn!(%error, "failed to install the low-level mouse hook");
        }
    }

    if runtime.mouse_hook.load(Ordering::SeqCst) == 0 {
        return;
    }

    // SAFETY: as above, for the keyboard proc.
    let keyboard =
        unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook_proc), Some(module), 0) };
    match keyboard {
        Ok(hook) => {
            runtime
                .keyboard_hook
                .store(hook_address(hook), Ordering::SeqCst);
            tracing::info!("the low-level keyboard hook is installed");
        }
        Err(error) => tracing::warn!(%error, "failed to install the low-level keyboard hook"),
    }
}

/// C# `GetModuleHandle(curModule.ModuleName)` made robust.
///
/// `GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS` on the proc's own address returns the image
/// the proc actually lives in — the same value the exe's main module would report for an
/// in-exe proc, but correct even if the proc is reached through some other mapping.
fn module_for_hook_proc() -> HINSTANCE {
    let proc_address: *const () = (mouse_hook_proc as *const std::ffi::c_void).cast();
    let mut module = HMODULE::default();
    // SAFETY: `proc_address` is the address of a `'static` fn item in this image, so it
    // stays valid for the process lifetime; `UNCHANGED_REFCOUNT` leaves the refcount
    // alone, and `module` is a live out slot.
    let result = unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(proc_address.cast::<u16>()),
            &raw mut module,
        )
    };
    if result.is_err() {
        tracing::warn!("could not resolve the module handle of the hook procs");
    }
    HINSTANCE(module.0)
}

/// `UnhookWindowsHookEx` a stored handle, clearing the slot first so it runs once.
fn unhook(slot: &AtomicUsize) {
    let address = slot.swap(0, Ordering::SeqCst);
    if address == 0 {
        return;
    }
    tracing::debug!(hook = address, "removing a low-level hook");
    // SAFETY: `address` came from a successful `SetWindowsHookExW` in this process, and
    // the swap above already cleared the slot, so the handle is unhooked at most once.
    unsafe {
        let _ = UnhookWindowsHookEx(HHOOK(address as *mut std::ffi::c_void));
    }
}

/// Send WM_QUIT to the pump thread so its `GetMessageW` returns.
fn post_wm_quit(pump_thread_id: u32) {
    if pump_thread_id == 0 {
        return;
    }
    // The pump thread creates its message queue on its first GetMessageW call, so the
    // post is retried briefly rather than racing it.
    for _ in 0..100 {
        // SAFETY: a zeroed message is a valid WM_QUIT post and the thread id was read
        // from `GetCurrentThreadId` inside that very thread.
        if unsafe { PostThreadMessageW(pump_thread_id, WM_QUIT, WPARAM(0), LPARAM(0)) }.is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(10));
    }
    tracing::warn!("could not post WM_QUIT to the hook pump thread");
}

fn hook_address(hook: HHOOK) -> usize {
    hook.0 as usize
}

fn current_thread_id() -> u32 {
    // SAFETY: `GetCurrentThreadId` takes no arguments and only reads thread-local state.
    unsafe { GetCurrentThreadId() }
}

/// Join a pump thread with a timeout, using a watcher thread that owns the blocking join.
fn join_bounded(handle: thread::JoinHandle<()>) {
    let (done, receipt) = std::sync::mpsc::sync_channel::<()>(1);
    if thread::Builder::new()
        .name("orbital-hook-joiner".to_string())
        .spawn(move || {
            let _ = handle.join();
            let _ = done.try_send(());
        })
        .is_err()
    {
        return;
    }
    let _ = receipt.recv_timeout(JOIN_TIMEOUT);
}

// —— Mouse hook proc ——————————————————————————————————————————————————————————

/// C# `MouseHookCallback`.
unsafe extern "system" fn mouse_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && let Some(runtime) = runtime()
    {
        // Only the messages the handler acts on are interesting; the low-level hook
        // also sees every WM_MOUSEMOVE, which would bury the gesture in noise.
        let message = wparam.0 as u32;
        if message == WM_LBUTTONDOWN || message == WM_LBUTTONUP || message == WM_RBUTTONDOWN {
            tracing::trace!(message, "the mouse hook proc was called");
        }
        // SAFETY: for a WH_MOUSE_LL hook, `lparam` addresses an MSLLHOOKSTRUCT that
        // stays valid for the duration of this call.
        let info = unsafe { *(lparam.0 as *const MSLLHOOKSTRUCT) };
        handle_mouse_event(&runtime, message, MousePoint::from(info.pt));
    }

    // C# `return CallNextHookEx(_mouseHookID, nCode, wParam, lParam);`
    next_hook(code, wparam, lparam, HookSlot::MouseHook)
}

/// C# `MouseHookCallback` dispatch.
fn handle_mouse_event(runtime: &Arc<HookRuntime>, message: u32, point: MousePoint) {
    match message {
        WM_LBUTTONDOWN => on_left_button_down(runtime, point),
        WM_LBUTTONUP => on_left_button_up(runtime, point),
        WM_RBUTTONDOWN => emit(runtime, HookEvent::AnyMouseDown(point)),
        _ => {}
    }
}

/// C# `if (wParam == (IntPtr)WM_LBUTTONDOWN)` branch.
fn on_left_button_down(runtime: &Arc<HookRuntime>, point: MousePoint) {
    // C# `OnAnyMouseDown?.Invoke(null, hookStruct.pt);`
    emit(runtime, HookEvent::AnyMouseDown(point));

    // C# `IntPtr hwnd = WindowFromPoint(hookStruct.pt);`
    let hwnd = win32::window_from_point(point.x, point.y);
    // C# `var hwndClsBuf = new StringBuilder(128); GetClassName(hwnd, hwndClsBuf, hwndClsBuf.Capacity);
    //     _buttonDownHwndClass = hwndClsBuf.ToString();`
    let hwnd_class = win32::window_class(hwnd);

    // C# `IntPtr rootHwnd = GetAncestor(hwnd, GA_ROOT); if (rootHwnd == IntPtr.Zero) rootHwnd = hwnd;`
    // SAFETY: `hwnd` comes from WindowFromPoint; GetAncestor only reads it.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    let root = if root.0.is_null() { hwnd } else { root };
    // C# `_mouseDownInClient = IsPointInClientArea(rootHwnd, hookStruct.pt) && !IsSystemShellWindow(rootHwnd);`
    let in_client = is_point_in_client_area(root, point)
        && !win32::is_system_shell_window_class(&win32::root_window_class(root));

    publish_button_down(point, &hwnd_class);

    let mut armed_timer = None;
    let mut double_click = false;

    {
        let mut state = runtime.lock_state();

        // C# `_buttonDownPos = hookStruct.pt; _isLongPressed = false; _isDoubleClick = false;`
        state.button_down_pos = point;
        state.long_pressed = false;
        state.double_click = false;
        state.mouse_down_in_client = in_client;
        // C# `_longPressTimer?.Dispose();` — disarm before deciding on a new timer.
        state.long_press.disarm();

        let mut start_long_press_timer = true;
        if in_client {
            let now = Instant::now();
            // C# `msSinceLast = (now - _lastClickTime).TotalMilliseconds` — with `_lastClickTime`
            // at DateTime.MinValue this is effectively infinite, and the double-click test fails.
            let (ms_since_last, (ddx, ddy)) = match state.last_click {
                Some((last_time, last_pos)) => (
                    now.saturating_duration_since(last_time).as_secs_f64() * 1000.0,
                    point.delta(last_pos),
                ),
                None => (f64::INFINITY, (0, 0)),
            };

            // C# `msSinceLast <= GetDoubleClickTime() && ddx <= GetSystemMetrics(SM_CXDOUBLECLK) && ddy <= GetSystemMetrics(SM_CYDOUBLECLK)`
            let within_time = ms_since_last <= f64::from(double_click_time_ms());
            let (cx, cy) = double_click_limits();
            if within_time && ddx <= cx && ddy <= cy {
                state.double_click = true;
                // C# `_lastClickTime = DateTime.MinValue;` — a double click cannot start another.
                state.last_click = None;
                start_long_press_timer = false;
                double_click = true;
            } else {
                // C# `_lastClickTime = now; _lastClickPos = hookStruct.pt;`
                state.last_click = Some((now, point));
            }
        } else {
            // C# `_lastClickTime = DateTime.MinValue;`
            state.last_click = None;
        }

        if start_long_press_timer {
            armed_timer = Some(state.long_press.arm());
        }
    }

    if double_click {
        // C# `OnDoubleClick?.Invoke(null, hookStruct.pt)` — the source's down-time event,
        // kept for back-compat and unused by `App.xaml.cs`; the port's event set carries
        // only `DoubleClickRelease`, which is what the app subscribes to.
        tracing::trace!("double-click detected for button down");
    }

    if let Some(timer) = armed_timer {
        let timer_runtime = Arc::clone(runtime);
        thread::spawn(move || long_press_timer_thread(timer_runtime, point, timer));
    }
}

/// C# `if (wParam == (IntPtr)WM_LBUTTONUP)` branch.
fn on_left_button_up(runtime: &Arc<HookRuntime>, point: MousePoint) {
    let mut event = None;

    {
        let mut state = runtime.lock_state();
        // C# `_longPressTimer?.Dispose(); _longPressTimer = null;`
        state.long_press.disarm();

        if state.mouse_down_in_client {
            let (dx, dy) = point.delta(state.button_down_pos);

            // C# `ThreadPool.QueueUserWorkItem(_ => OnDoubleClickRelease?.Invoke(null, pt));`
            if state.double_click {
                // Word selection is complete at mouse-up time.
                event = Some(HookEvent::DoubleClickRelease(point));
            } else if state.long_pressed && dx <= DRAG_THRESHOLD && dy <= DRAG_THRESHOLD {
                event = Some(HookEvent::LongPress(point));
            } else if !state.long_pressed && (dx > DRAG_THRESHOLD || dy > DRAG_THRESHOLD) {
                event = Some(HookEvent::MouseUp(point));
            }
        }

        // C# `_isLongPressed = false; _isDoubleClick = false;`
        state.long_pressed = false;
        state.double_click = false;
    }

    if let Some(event) = event {
        emit(runtime, event);
    }
}

/// C# `LongPressTimerCallback` on a timer thread.
///
/// Sleeping instead of cancelling means a button-up cannot join the timer; the armed
/// flag plus the generation check below is what makes a superseded timer a no-op.
fn long_press_timer_thread(runtime: Arc<HookRuntime>, down_pos: MousePoint, timer: LongPressTimer) {
    thread::sleep(Duration::from_millis(LONG_PRESS_MS));

    let mut state = runtime.lock_state();
    // C# `_longPressTimer?.Dispose(); _longPressTimer = null;` — the callback runs once.
    if !timer.is_current(&state.long_press) {
        // Superseded by a later press or cancelled by a button-up.
        return;
    }
    state.long_press.disarm();

    // C# `GetCursorPos(out MousePoint currentPos); int dx = Math.Abs(currentPos.X - _buttonDownPos.X); ...`
    let (dx, dy) = point_from_cursor().delta(down_pos);
    if dx <= DRAG_THRESHOLD && dy <= DRAG_THRESHOLD {
        // C# `_isLongPressed = true;`
        state.long_pressed = true;
    }
}

fn point_from_cursor() -> MousePoint {
    MousePoint::from(win32::cursor_pos())
}

// —— Keyboard hook proc ———————————————————————————————————————————————————————

/// C# `KeyboardHookCallback`.
unsafe extern "system" fn keyboard_hook_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0
        && let Some(runtime) = runtime()
    {
        // SAFETY: for a WH_KEYBOARD_LL hook, `lparam` addresses a KBDLLHOOKSTRUCT
        // that stays valid for the duration of this call.
        let info = unsafe { *(lparam.0 as *const KBDLLHOOKSTRUCT) };
        tracing::trace!(
            code,
            message = wparam.0 as u32,
            vk_code = info.vkCode,
            "the keyboard hook proc was called"
        );

        // C# `if (ks.dwExtraInfo != ClipboardHelper.ORBITAL_EXTRA_INFO)` — Orbital's own
        // injected keys are skipped, virtual keyboards (OSK) still get through.
        if info.dwExtraInfo != ORBITAL_EXTRA_INFO {
            handle_keyboard_event(&runtime, wparam.0 as u32, info.vkCode);
        }
    }

    // C# `return CallNextHookEx(_keyboardHookID, nCode, wParam, lParam);`
    next_hook(code, wparam, lparam, HookSlot::KeyboardHook)
}

/// C# `KeyboardHookCallback` body: Shift/Ctrl tracking, selection intent, clipboard
/// shortcut, custom hotkey, key-up triggers.
fn handle_keyboard_event(runtime: &Arc<HookRuntime>, message: u32, vk: u32) {
    let is_key_down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
    let is_key_up = message == WM_KEYUP || message == WM_SYSKEYUP;
    // Two events can come out of one key-down (a clipboard shortcut that is also the
    // custom hotkey), and one out of a key-up in the source's ordering.
    let mut events: [Option<HookEvent>; 2] = [None, None];
    let mut event_count = 0usize;

    if is_key_down {
        let mut state = runtime.lock_state();

        // C# `if (vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT) _shiftDown = true;`
        if is_shift(vk) {
            state.shift_down = true;
        }
        // C# `if (vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL) _ctrlDown = true;`
        if is_control(vk) {
            state.ctrl_down = true;
        }

        tracing::trace!(
            vk,
            is_key_down,
            shift_down = state.shift_down,
            ctrl_down = state.ctrl_down,
            "keyboard hook saw a key down"
        );

        // C# `if (_shiftDown && s_navKeys.Contains(vk)) _pendingKeyboardSelection = true;`
        if state.shift_down && NAV_KEYS.contains(&vk) {
            state.pending_keyboard_selection = true;
        }
        // C# `if (_ctrlDown && vk == VK_A) _pendingKeyboardSelection = true;`
        if state.ctrl_down && vk == VK_A {
            state.pending_keyboard_selection = true;
        }

        // C# clipboard shortcut — dismiss the popup.
        if state.ctrl_down && (vk == VK_C || vk == VK_X || vk == VK_V) {
            events[event_count] = Some(HookEvent::ClipboardShortcut);
            event_count += 1;
        }

        // C# custom hotkey match.
        if hotkey_matches(&state, vk) {
            // C# `GetCursorPos(out MousePoint pt); OnCustomHotkey?.Invoke(null, pt);`
            let point = point_from_cursor();
            events[event_count] = Some(HookEvent::CustomHotkey(point));
            event_count += 1;
        }
    } else if is_key_up {
        let mut state = runtime.lock_state();

        // C# `if (vk == VK_ESCAPE) OnEscapePressed?.Invoke(null, EventArgs.Empty);`
        if vk == VK_ESCAPE {
            events[event_count] = Some(HookEvent::EscapePressed);
            event_count += 1;
        }

        // C# Shift-release trigger: generic and L/R variants.
        if is_shift(vk) && state.pending_keyboard_selection {
            state.pending_keyboard_selection = false;
            let point = point_from_cursor();
            events[event_count] = Some(HookEvent::KeyboardSelection(point));
            event_count += 1;
        }

        // C# Ctrl+A release trigger: `_ctrlDown` need not be true now, as long as the
        // intent was captured during key-down.
        if vk == VK_A && state.pending_keyboard_selection {
            state.pending_keyboard_selection = false;
            let point = point_from_cursor();
            tracing::trace!(
                x = point.x,
                y = point.y,
                "Ctrl+A produced a keyboard selection"
            );
            events[event_count] = Some(HookEvent::KeyboardSelection(point));
            event_count += 1;
        }

        // C# `if (vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT) _shiftDown = false;`
        if is_shift(vk) {
            state.shift_down = false;
        }
        // C# `if (vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL) _ctrlDown = false;`
        if is_control(vk) {
            state.ctrl_down = false;
        }
    }

    for event in events.into_iter().take(event_count).flatten() {
        emit(runtime, event);
    }
}

/// C# `vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT`.
fn is_shift(vk: u32) -> bool {
    vk == VK_SHIFT || vk == VK_LSHIFT || vk == VK_RSHIFT
}

/// C# `vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL`.
fn is_control(vk: u32) -> bool {
    vk == VK_CONTROL || vk == VK_LCONTROL || vk == VK_RCONTROL
}

/// C# custom hotkey predicate — skipped when no hotkey is configured.
fn hotkey_matches(state: &HookState, vk: u32) -> bool {
    if state.hotkey.virtual_key == 0 || vk != state.hotkey.virtual_key {
        return false;
    }

    // C# `HotkeyModifiers & 0x02 / 0x01 / 0x04`.
    let want_control = state.hotkey.modifiers & HOTKEY_MOD_CONTROL != 0;
    let want_alt = state.hotkey.modifiers & HOTKEY_MOD_ALT != 0;
    let want_shift = state.hotkey.modifiers & HOTKEY_MOD_SHIFT != 0;

    // C# `bool altDown = (GetAsyncKeyState(VK_ALT) & 0x8000) != 0;` — Alt is read live
    // because the source's `_altDown` flag is never tracked, unlike Shift and Ctrl.
    let alt_down = async_key_down(VK_ALT);

    // C# `_ctrlDown == wantCtrl && altDown == wantAlt && _shiftDown == wantShift`
    state.ctrl_down == want_control && alt_down == want_alt && state.shift_down == want_shift
}

/// C# `(GetAsyncKeyState(vKey) & 0x8000) != 0`.
fn async_key_down(vk: u32) -> bool {
    // SAFETY: `GetAsyncKeyState` takes a plain virtual-key code by value.
    let state = unsafe { GetAsyncKeyState(vk as i32) };
    state as u16 & 0x8000 != 0
}

/// C# `GetDoubleClickTime()`.
fn double_click_time_ms() -> u32 {
    // SAFETY: `GetDoubleClickTime` takes no arguments.
    unsafe { GetDoubleClickTime() }
}

/// C# `GetSystemMetrics(SM_CXDOUBLECLK)` / `GetSystemMetrics(SM_CYDOUBLECLK)`.
fn double_click_limits() -> (i32, i32) {
    (
        // SAFETY: `GetSystemMetrics` takes a plain index by value.
        unsafe { GetSystemMetrics(SM_CXDOUBLECLK) },
        // SAFETY: `GetSystemMetrics` takes a plain index by value.
        unsafe { GetSystemMetrics(SM_CYDOUBLECLK) },
    )
}

// —— Shared client-area / shell helpers ———————————————————————————————————————

/// C# `IsPointInClientArea` — client rect with the origin moved to screen space,
/// falling back to the window rect.
fn is_point_in_client_area(hwnd: windows::Win32::Foundation::HWND, point: MousePoint) -> bool {
    if hwnd.0.is_null() {
        return false;
    }

    let mut client_rect = RECT::default();
    // SAFETY: `client_rect` is a live out slot and `hwnd` was checked non-null.
    let got_client = unsafe { GetClientRect(hwnd, &raw mut client_rect) }.is_ok();
    if got_client && client_rect.right > 0 && client_rect.bottom > 0 {
        let mut origin = POINT::default();
        // SAFETY: `origin` is a live out slot; ClientToScreen maps it into screen space.
        if unsafe { ClientToScreen(hwnd, &raw mut origin) }.as_bool() {
            return point.x >= origin.x
                && point.x < origin.x + client_rect.right
                && point.y >= origin.y
                && point.y < origin.y + client_rect.bottom;
        }
    }

    let mut window_rect = RECT::default();
    // SAFETY: `window_rect` is a live out slot.
    if unsafe { GetWindowRect(hwnd, &raw mut window_rect) }.is_ok() {
        return point.x >= window_rect.left
            && point.x < window_rect.right
            && point.y >= window_rect.top
            && point.y < window_rect.bottom;
    }
    false
}

// —— `CallNextHookEx` —————————————————————————————————————————————————————————

/// Which hook a `CallNextHookEx` belongs to, so the right stored handle is passed.
enum HookSlot {
    MouseHook,
    KeyboardHook,
}

/// C# `return CallNextHookEx(...);` — every proc ends on this call.
fn next_hook(code: i32, wparam: WPARAM, lparam: LPARAM, slot: HookSlot) -> LRESULT {
    let hook = runtime().and_then(|runtime| {
        let stored = match slot {
            HookSlot::MouseHook => runtime.mouse_hook.load(Ordering::SeqCst),
            HookSlot::KeyboardHook => runtime.keyboard_hook.load(Ordering::SeqCst),
        };
        if stored == 0 {
            None
        } else {
            Some(HHOOK(stored as *mut std::ffi::c_void))
        }
    });

    // SAFETY: the handle is either a live HHOOK from this process's SetWindowsHookExW or
    // None (NULL), which CallNextHookEx also accepts; `code`/`wparam`/`lparam` come
    // straight from the hook call.
    unsafe { CallNextHookEx(hook, code, wparam, lparam) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win32_code_from_hresult_unwraps_hresult_from_win32() {
        // ERROR_ACCESS_DENIED (5) -> HRESULT_FROM_WIN32 -> 5.
        assert_eq!(win32_code_from_hresult(0x8007_0005u32 as i32), 5);
        // A non-Win32 HRESULT is passed through rather than guessed at.
        assert_eq!(
            win32_code_from_hresult(0x8000_4005u32 as i32),
            0x8000_4005u32 as i32
        );
    }
}
