//! Popup placement port of `PopupPlacement.cs` + `PopupPlacementCalculator.cs`.
//! Own f64 point/rect types; numeric vectors ported 1:1:
//! +12 cursor offset, 20px corner inset, ±4 clamp, 10px above/below.

use serde::{Deserialize, Serialize};

/// f64 screen-space point (DIPs), replacing WPF `System.Windows.Point`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f64,
    pub y: f64,
}

impl Point {
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// f64 rect given by edges, replacing WPF `System.Windows.Rect`
/// (`width = right - left`, `height = bottom - top`).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}
impl Rect {
    /// WPF-style `new Rect(left, top, right, bottom)`.
    ///
    /// **Edge order is OURS, not WPF's**: this constructor takes the four edge
    /// coordinates, matching the `Rect` record the port uses everywhere. WPF's
    /// `System.Windows.Rect` ctor instead takes `(x, y, width, height)`, i.e. the
    /// top-left corner plus the size — the two are *not* interchangeable, and the
    /// port's `Rect::width`/`Rect::height` derive the size from the edges.
    pub fn new(left: f64, top: f64, right: f64, bottom: f64) -> Self {
        Self {
            left,
            top,
            right,
            bottom,
        }
    }

    /// C# `Rect.Width` — `right - left`, derived from the stored edges.
    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    /// C# `Rect.Height` — `bottom - top`, derived from the stored edges.
    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }
}

/// f64 size, replacing WPF `System.Windows.Size`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub width: f64,
    pub height: f64,
}

impl Size {
    pub fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupPlacementMode {
    BottomRight,
    NearCursor,
    NearSelection,
    NearActionBar,
}

/// Newtonsoft default-enum semantics: discriminant 0 (`BottomRight`).
impl Default for PopupPlacementMode {
    fn default() -> Self {
        Self::BottomRight
    }
}

/// Newtonsoft writes/reads this enum as its **numeric** value (no StringEnumConverter
/// in the source), so the wire format is 0..=3. Missing or null → BottomRight.
///
/// Acceptance mirrors Newtonsoft exactly: any integer loads (values outside the member
/// list land on `NearCursor` — the same observable result as
/// `PopupPlacementCalculator.CalculatePosition`'s `_ => nearCursor` default arm), and a
/// JSON *string* is matched case-insensitively against the member names (Newtonsoft also
/// accepts numeric strings like "3"). An unknown name is a deserialization error, which
/// the source's `catch` turns into the corrupt-file recovery path.
impl<'de> Deserialize<'de> for PopupPlacementMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;

        let Some(value) = Option::<serde_json::Value>::deserialize(deserializer)? else {
            return Ok(Self::BottomRight);
        };
        match value {
            serde_json::Value::Number(number) => Ok(match number.as_i64() {
                Some(0) => Self::BottomRight,
                Some(1) => Self::NearCursor,
                Some(2) => Self::NearSelection,
                Some(3) => Self::NearActionBar,
                _ => Self::NearCursor,
            }),
            serde_json::Value::String(raw) => {
                let raw = raw.trim();
                if let Ok(number) = raw.parse::<i64>() {
                    return Ok(match number {
                        0 => Self::BottomRight,
                        1 => Self::NearCursor,
                        2 => Self::NearSelection,
                        3 => Self::NearActionBar,
                        _ => Self::NearCursor,
                    });
                }
                match raw.to_ascii_lowercase().as_str() {
                    "bottomright" => Ok(Self::BottomRight),
                    "nearcursor" => Ok(Self::NearCursor),
                    "nearselection" => Ok(Self::NearSelection),
                    "nearactionbar" => Ok(Self::NearActionBar),
                    _ => Err(D::Error::custom(format!(
                        "Unknown PopupPlacementMode name {raw:?}"
                    ))),
                }
            }
            other => Err(D::Error::custom(format!(
                "Invalid PopupPlacementMode value {other}"
            ))),
        }
    }
}

impl Serialize for PopupPlacementMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(match self {
            Self::BottomRight => 0,
            Self::NearCursor => 1,
            Self::NearSelection => 2,
            Self::NearActionBar => 3,
        })
    }
}

/// Port of `PopupAnchorContext` (record struct).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct AnchorContext {
    pub cursor_dip: Point,
    pub selection_dip: Option<Rect>,
    pub action_bar_dip: Option<Rect>,
}

pub fn calculate_position(
    mode: PopupPlacementMode,
    anchors: AnchorContext,
    work_area: Rect,
    popup_size: Size,
) -> Point {
    let near_cursor = Point::new(anchors.cursor_dip.x + 12.0, anchors.cursor_dip.y + 12.0);

    // C#'s `_ => nearCursor` default arm catches every value the enum does not
    // name. An out-of-range integer is folded onto `NearCursor` by `Deserialize`
    // below, so the ported arms stay exhaustive with the same math.
    let placement = match mode {
        PopupPlacementMode::BottomRight => Point::new(
            work_area.right - popup_size.width - 20.0,
            work_area.bottom - popup_size.height - 20.0,
        ),
        PopupPlacementMode::NearActionBar => near_action_bar_or_fallback(&anchors, near_cursor),
        PopupPlacementMode::NearSelection => {
            near_selection_or_fallback(&anchors, near_cursor, popup_size)
        }
        PopupPlacementMode::NearCursor => near_cursor,
    };

    clamp_to_working_area(placement, work_area, popup_size)
}

/// C# `GetNearSelectionOrFallback` — a missing, zero-width or zero-height
/// selection falls back to the cursor point.
fn near_selection_or_fallback(anchors: &AnchorContext, fallback: Point, popup_size: Size) -> Point {
    let Some(selection) = anchors.selection_dip else {
        return fallback;
    };
    if selection.width() <= 0.0 || selection.height() <= 0.0 {
        return fallback;
    }

    let y_above = selection.top - popup_size.height - 10.0;
    if y_above >= 0.0 {
        return Point::new(selection.left, y_above);
    }

    Point::new(selection.left, selection.bottom + 10.0)
}

/// C# `GetNearActionBarOrFallback` — a missing, zero-width or zero-height action
/// bar falls back to the cursor point.
fn near_action_bar_or_fallback(anchors: &AnchorContext, fallback: Point) -> Point {
    let Some(action_bar) = anchors.action_bar_dip else {
        return fallback;
    };
    if action_bar.width() <= 0.0 || action_bar.height() <= 0.0 {
        return fallback;
    }

    Point::new(action_bar.left, action_bar.bottom + 10.0)
}

pub fn clamp_to_working_area(preferred: Point, work_area: Rect, popup_size: Size) -> Point {
    // .NET Math.Min/Math.Max return NaN when either operand is NaN; Rust's
    // f64::min/f64::max silently drop the NaN operand. The helpers below keep the
    // 1:1 double semantics of the source.
    let x = math_max(
        work_area.left + 4.0,
        math_min(preferred.x, work_area.right - popup_size.width - 4.0),
    );
    let y = math_max(
        work_area.top + 4.0,
        math_min(preferred.y, work_area.bottom - popup_size.height - 4.0),
    );
    Point::new(x, y)
}

/// C# `Math.Min(double, double)` — NaN in, NaN out.
fn math_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a < b { a } else { b }
}

/// C# `Math.Max(double, double)` — NaN in, NaN out.
fn math_max(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        return f64::NAN;
    }
    if a > b { a } else { b }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Vectors ported 1:1 from orbital-main/Orbit.Tests/PopupPlacementCalculatorTests.cs.

    #[test]
    fn calculate_position_bottom_right_places_at_bottom_right_with_margin() {
        let work_area = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let popup = Size::new(300.0, 200.0);
        let anchors = AnchorContext {
            cursor_dip: Point::new(500.0, 500.0),
            selection_dip: None,
            action_bar_dip: None,
        };

        let pos = calculate_position(PopupPlacementMode::BottomRight, anchors, work_area, popup);

        assert_eq!(pos.x, 1600.0);
        assert_eq!(pos.y, 860.0);
    }

    #[test]
    fn calculate_position_near_selection_when_selection_missing_falls_back_to_cursor() {
        let work_area = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let popup = Size::new(300.0, 200.0);
        let anchors = AnchorContext {
            cursor_dip: Point::new(100.0, 200.0),
            selection_dip: None,
            action_bar_dip: None,
        };

        let pos = calculate_position(PopupPlacementMode::NearSelection, anchors, work_area, popup);

        assert_eq!(pos.x, 112.0);
        assert_eq!(pos.y, 212.0);
    }

    #[test]
    fn calculate_position_near_action_bar_when_action_bar_missing_falls_back_to_cursor() {
        let work_area = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let popup = Size::new(300.0, 200.0);
        let anchors = AnchorContext {
            cursor_dip: Point::new(50.0, 60.0),
            selection_dip: None,
            action_bar_dip: None,
        };

        let pos = calculate_position(PopupPlacementMode::NearActionBar, anchors, work_area, popup);

        assert_eq!(pos.x, 62.0);
        assert_eq!(pos.y, 72.0);
    }

    #[test]
    fn clamp_to_working_area_clamps_outside_coordinates() {
        let work_area = Rect::new(0.0, 0.0, 1000.0, 700.0);
        let popup = Size::new(200.0, 100.0);

        let pos = clamp_to_working_area(Point::new(950.0, 650.0), work_area, popup);

        assert_eq!(pos.x, 796.0);
        assert_eq!(pos.y, 596.0);
    }

    // Extra characterization of the arms the xunit file does not cover, so the
    // fallback / above / below branches stay pinned.

    #[test]
    fn near_selection_prefers_above_and_falls_back_below() {
        let popup = Size::new(300.0, 200.0);
        let anchors = AnchorContext {
            cursor_dip: Point::new(0.0, 0.0),
            selection_dip: Some(Rect::new(400.0, 300.0, 600.0, 340.0)),
            action_bar_dip: None,
        };

        assert_eq!(
            near_selection_or_fallback(&anchors, Point::new(1.0, 1.0), popup),
            Point::new(400.0, 90.0)
        );

        let low = AnchorContext {
            cursor_dip: Point::new(0.0, 0.0),
            selection_dip: Some(Rect::new(400.0, 100.0, 600.0, 140.0)),
            action_bar_dip: None,
        };
        assert_eq!(
            near_selection_or_fallback(&low, Point::new(1.0, 1.0), popup),
            Point::new(400.0, 150.0)
        );
    }

    #[test]
    fn degenerate_selection_and_action_bar_fall_back_to_cursor() {
        let fallback = Point::new(7.0, 9.0);
        let popup = Size::new(300.0, 200.0);

        for selection in [
            Some(Rect::new(10.0, 10.0, 10.0, 40.0)),
            Some(Rect::new(10.0, 10.0, 40.0, 10.0)),
            Some(Rect::new(50.0, 40.0, 30.0, 20.0)),
            None,
        ] {
            let anchors = AnchorContext {
                cursor_dip: Point::new(0.0, 0.0),
                selection_dip: selection,
                action_bar_dip: None,
            };
            assert_eq!(
                near_selection_or_fallback(&anchors, fallback, popup),
                fallback
            );
        }

        for action_bar in [
            Some(Rect::new(10.0, 10.0, 10.0, 40.0)),
            Some(Rect::new(10.0, 10.0, 40.0, 10.0)),
            None,
        ] {
            let anchors = AnchorContext {
                cursor_dip: Point::new(0.0, 0.0),
                selection_dip: None,
                action_bar_dip: action_bar,
            };
            assert_eq!(near_action_bar_or_fallback(&anchors, fallback), fallback);
        }
    }

    #[test]
    fn near_action_bar_uses_left_and_bottom_offset() {
        let anchors = AnchorContext {
            cursor_dip: Point::new(0.0, 0.0),
            selection_dip: None,
            action_bar_dip: Some(Rect::new(320.0, 480.0, 900.0, 520.0)),
        };

        assert_eq!(
            near_action_bar_or_fallback(&anchors, Point::new(1.0, 1.0)),
            Point::new(320.0, 530.0)
        );
    }

    #[test]
    fn popup_placement_mode_serializes_as_numbers() {
        assert_eq!(
            serde_json::to_string(&PopupPlacementMode::BottomRight).expect("serialize"),
            "0"
        );
        assert_eq!(
            serde_json::to_string(&PopupPlacementMode::NearActionBar).expect("serialize"),
            "3"
        );

        let parsed: PopupPlacementMode = serde_json::from_str("2").expect("deserialize");
        assert_eq!(parsed, PopupPlacementMode::NearSelection);

        let null: PopupPlacementMode = serde_json::from_str("null").expect("deserialize");
        assert_eq!(null, PopupPlacementMode::BottomRight);
    }

    // C# Newtonsoft reads an enum back as its numeric value; the members keep
    // 0..=3 on the wire.

    #[test]
    fn popup_placement_mode_reads_every_named_discriminant() {
        for (raw, expected) in [
            ("0", PopupPlacementMode::BottomRight),
            ("1", PopupPlacementMode::NearCursor),
            ("2", PopupPlacementMode::NearSelection),
            ("3", PopupPlacementMode::NearActionBar),
        ] {
            let parsed: PopupPlacementMode = serde_json::from_str(raw).expect("deserialize");
            assert_eq!(parsed, expected, "discriminant {raw}");
        }
    }

    // Newtonsoft accepts and stores any integer for the enum, and
    // `PopupPlacementCalculator.CalculatePosition`'s `_ => nearCursor` arm makes an
    // unnamed value behave exactly like NearCursor, so the port must load it too
    // instead of taking the settings file's corrupt-recovery path.

    #[test]
    fn popup_placement_mode_accepts_out_of_range_integers_as_near_cursor() {
        for raw in ["4", "7", "255", "-1", "4096"] {
            let parsed: PopupPlacementMode = serde_json::from_str(raw).expect("deserialize");
            assert_eq!(parsed, PopupPlacementMode::NearCursor, "value {raw}");
        }

        // The observable result is the source's NearCursor placement.
        let work_area = Rect::new(0.0, 0.0, 1920.0, 1080.0);
        let popup = Size::new(300.0, 200.0);
        let anchors = AnchorContext {
            cursor_dip: Point::new(400.0, 500.0),
            selection_dip: None,
            action_bar_dip: None,
        };

        assert_eq!(
            calculate_position(
                serde_json::from_str("7").expect("deserialize"),
                anchors,
                work_area,
                popup
            ),
            calculate_position(PopupPlacementMode::NearCursor, anchors, work_area, popup)
        );
    }

    #[test]
    fn clamp_propagates_nan_the_way_dotnet_does() {
        // Rust's `f64::min`/`f64::max` drop the NaN operand, .NET's do not.
        assert!(math_min(1.0, f64::NAN).is_nan());
        assert!(math_min(f64::NAN, 1.0).is_nan());
        assert!(math_max(1.0, f64::NAN).is_nan());
        assert!(math_max(f64::NAN, 1.0).is_nan());
        assert_eq!(math_min(1.0, 2.0), 1.0);
        assert_eq!(math_min(2.0, 1.0), 1.0);
        assert_eq!(math_max(1.0, 2.0), 2.0);
        assert_eq!(math_max(2.0, 1.0), 2.0);

        let work_area = Rect::new(0.0, 0.0, 1000.0, 700.0);
        let popup = Size::new(200.0, 100.0);

        let pos = clamp_to_working_area(Point::new(f64::NAN, 10.0), work_area, popup);
        assert!(pos.x.is_nan());
        assert_eq!(pos.y, 10.0);
    }

    // Newtonsoft also accepts an enum's member NAME (case-insensitively) and numeric
    // strings; the settings file is hand-editable, so those forms must load.

    #[test]
    fn popup_placement_mode_accepts_member_names_case_insensitively() {
        for (raw, expected) in [
            ("\"BottomRight\"", PopupPlacementMode::BottomRight),
            ("\"nearcursor\"", PopupPlacementMode::NearCursor),
            ("\"NearSelection\"", PopupPlacementMode::NearSelection),
            ("\"nearactionbar\"", PopupPlacementMode::NearActionBar),
            ("\"  NearActionBar  \"", PopupPlacementMode::NearActionBar),
        ] {
            let parsed: PopupPlacementMode = serde_json::from_str(raw).expect("deserialize");
            assert_eq!(parsed, expected, "name {raw}");
        }
    }

    #[test]
    fn popup_placement_mode_accepts_numeric_strings() {
        let parsed: PopupPlacementMode = serde_json::from_str("\"3\"").expect("deserialize");
        assert_eq!(parsed, PopupPlacementMode::NearActionBar);
        let parsed: PopupPlacementMode = serde_json::from_str("\"7\"").expect("deserialize");
        assert_eq!(parsed, PopupPlacementMode::NearCursor);
    }

    #[test]
    fn popup_placement_mode_rejects_unknown_names_like_newtonsoft() {
        let error = serde_json::from_str::<PopupPlacementMode>("\"NotAMode\"")
            .expect_err("an unknown name is an error");
        assert!(
            error.to_string().contains("NotAMode"),
            "the error should name the bad value: {error}"
        );
    }
}
