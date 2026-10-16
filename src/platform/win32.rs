//! Platform Win32 helpers — port of the P/Invoke surface spread across App.xaml.cs,
//! SystemHookManager.cs, ClipboardHelper.cs and Services/ActionExecutorService.cs.

use windows::Win32::Foundation::{CloseHandle, HWND, POINT};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GetAncestor, GetClassNameW, GetWindowThreadProcessId, WindowFromPoint,
};

/// `GetCursorPos` — `POINT { 0, 0 }` when the call fails, like the source's out param.
pub fn cursor_pos() -> POINT {
    let mut point = POINT::default();
    // SAFETY: `point` is a live stack local passed as the single out parameter.
    let _ = unsafe { windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut point) };
    point
}

/// `WindowFromPoint` + `GetClassName` (128-char buffer, like the source).
///
/// Mirrors C# `App.GetWindowClass`: empty string for a null handle or a failed call.
pub fn window_class(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    read_class_name(hwnd, CLASS_BUFFER_LEN)
}

/// `GetAncestor(GA_ROOT = 2)` class name (falls back to the window itself).
///
/// Mirrors C# `App.GetRootWindowClass`.
pub fn root_window_class(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    // SAFETY: `hwnd` was validated non-null above; `GetAncestor` only reads it.
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if root.0.is_null() {
        window_class(hwnd)
    } else {
        window_class(root)
    }
}

/// Process name from HWND via `GetWindowThreadProcessId`.
///
/// Mirrors C# `App.GetProcessNameFromHwnd` — `Process.GetProcessById(pid).ProcessName`,
/// i.e. the image file name without extension. Empty string when the pid is 0 or the
/// process cannot be queried (exited, access denied).
pub fn process_name(hwnd: HWND) -> String {
    if hwnd.0.is_null() {
        return String::new();
    }
    let mut process_id: u32 = 0;
    // SAFETY: `process_id` is a live stack local; a null `lpdwProcessId` is legal but
    // this port needs the pid.
    let _ = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut process_id)) };
    if process_id == 0 {
        return String::new();
    }

    let Ok(process) =
        (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) })
    else {
        return String::new();
    };

    let mut buffer = [0u16; MAX_PATH_LEN];
    let mut size: u32 = buffer.len() as u32;
    // SAFETY: `buffer` holds `size` writable u16 units, which `QueryFullProcessImageNameW`
    // treats as a byte capacity in characters; the handle is alive for the call.
    let query = unsafe {
        QueryFullProcessImageNameW(
            process,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut size,
        )
    };
    let _ = unsafe { CloseHandle(process) };

    if query.is_err() || size == 0 {
        return String::new();
    }

    let path = String::from_utf16_lossy(&buffer[..size as usize]);
    match path.rsplit(['\\', '/']).next() {
        Some(file_name) => match file_name.rsplit_once('.') {
            Some((stem, _)) => stem.to_string(),
            None => file_name.to_string(),
        },
        None => String::new(),
    }
}

/// C# `App.WindowFromPoint(new POINT { X = screenX, Y = screenY })`.
pub fn window_from_point(x: i32, y: i32) -> HWND {
    // SAFETY: `WindowFromPoint` takes the point by value and returns a borrowed HWND
    // owned by the window manager.
    unsafe { WindowFromPoint(POINT { x, y }) }
}

/// Shell-class guard used by both the hook manager and the editability fast-path.
pub fn is_system_shell_window_class(class: &str) -> bool {
    matches!(
        class,
        "Shell_TrayWnd" | "Shell_SecondaryTrayWnd" | "Progman" | "WorkerW"
    )
}

/// C# `GetClassName` with a `StringBuilder` of `capacity` characters.
fn read_class_name(hwnd: HWND, capacity: usize) -> String {
    let mut buffer: Vec<u16> = vec![0u16; capacity];
    // SAFETY: `buffer` is a live `capacity`-element slice; `GetClassNameW` writes at
    // most `capacity` UTF-16 units (including the terminator) and reports the length
    // excluding it.
    let len = unsafe { GetClassNameW(hwnd, &mut buffer) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buffer[..len as usize])
}

/// Same buffer size the C# call sites pass to `GetClassName` (SystemHookManager, App).
const CLASS_BUFFER_LEN: usize = 128;
/// Generous but bounded path capacity for `QueryFullProcessImageNameW`.
const MAX_PATH_LEN: usize = 260 + 256;
