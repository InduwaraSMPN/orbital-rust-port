//! DPI helpers port of the `PresentationSource.CompositionTarget.TransformFromDevice`
//! usage in App.xaml.cs (mouse px → DIP conversion for the outside-click hit test)
//! and the tooltip anchor normalization.

use windows::Win32::Foundation::POINT;
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONEAREST, MonitorFromPoint};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};

/// DPI of a 96 dpi desktop — the identity scale, also the fallback when the monitor
/// or the DPI query cannot be resolved.
const BASELINE_DPI: f64 = 96.0;

/// Scale factor (dpi/96.0) of the monitor containing the point.
///
/// The source reads `PresentationSource.CompositionTarget.TransformFromDevice` per
/// call, so nothing is cached here either.
pub fn scale_for_point(x: i32, y: i32) -> f64 {
    let dpi = monitor_dpi_for_point(x, y).unwrap_or(BASELINE_DPI);
    if dpi <= 0.0 {
        return 1.0;
    }
    dpi / BASELINE_DPI
}

/// Device pixels → DIPs at the given point.
///
/// The source's `TransformFromDevice.M11`/`.M22` is `96/dpi`, so `px * (96/dpi)` is
/// exactly `px / (dpi/96)` — the division by the scale factor this module returns.
pub fn to_dip(x: f64, y: f64, scale: f64) -> (f64, f64) {
    if scale == 0.0 {
        (x, y)
    } else {
        (x / scale, y / scale)
    }
}

/// `MonitorFromPoint(..., MONITOR_DEFAULTTONEAREST)` → that monitor's effective dpi,
/// via `GetDpiForMonitor(MDT_DEFAULT-equivalent)`.
fn monitor_dpi_for_point(x: i32, y: i32) -> Option<f64> {
    // SAFETY: `MonitorFromPoint` takes the point by value and returns a handle owned by
    // the window manager.
    let monitor = unsafe { MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST) };
    if monitor.0.is_null() {
        return None;
    }

    let mut dpi_x = 0u32;
    let mut dpi_y = 0u32;
    // SAFETY: both out-parameters are live locals, and the handle comes from
    // `MonitorFromPoint` above.
    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &raw mut dpi_x, &raw mut dpi_y) }.ok()?;

    Some(f64::from(dpi_x))
}
