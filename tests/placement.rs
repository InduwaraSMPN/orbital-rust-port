//! Placement vectors ported 1:1 from
//! `orbital-main/Orbit.Tests/PopupPlacementCalculatorTests.cs`, plus a `proptest`
//! generator over arbitrary f64 coordinates.

use proptest::prelude::*;

use orbital::placement::{
    AnchorContext, Point, PopupPlacementMode, Rect, Size, calculate_position, clamp_to_working_area,
};

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

proptest! {
    /// For arbitrary finite f64 geometry the result must stay inside the clamp
    /// bounds and never become NaN — the source's `Math.Max/Math.Min` behavior.
    #[test]
    fn calculate_position_stays_within_clamp_bounds(
        mode in prop::sample::select(vec![
            PopupPlacementMode::BottomRight,
            PopupPlacementMode::NearCursor,
            PopupPlacementMode::NearSelection,
            PopupPlacementMode::NearActionBar,
        ]),
        cursor in point_strategy(),
        selection in option_rect_strategy(),
        action_bar in option_rect_strategy(),
        work_area in rect_strategy(),
        popup in size_strategy(),
    ) {
        prop_assume!(cursor.x.is_finite() && cursor.y.is_finite());
        prop_assume!(work_area.left.is_finite() && work_area.top.is_finite()
            && work_area.right.is_finite() && work_area.bottom.is_finite());
        prop_assume!(popup.width.is_finite() && popup.height.is_finite());
        for rect in [selection, action_bar].into_iter().flatten() {
            prop_assume!(rect.left.is_finite() && rect.top.is_finite()
                && rect.right.is_finite() && rect.bottom.is_finite());
        }

        let anchors = AnchorContext {
            cursor_dip: cursor,
            selection_dip: selection,
            action_bar_dip: action_bar,
        };

        let pos = calculate_position(mode, anchors, work_area, popup);

        prop_assert!(pos.x.is_finite() && pos.y.is_finite());

        // x = Math.Max(workArea.Left + 4, Math.Min(preferred.X, workArea.Right - w - 4))
        let min_x = work_area.left + 4.0;
        let max_x = work_area.right - popup.width - 4.0;
        prop_assert!(pos.x >= min_x);
        prop_assert!(pos.x <= min_x.max(max_x));

        let min_y = work_area.top + 4.0;
        let max_y = work_area.bottom - popup.height - 4.0;
        prop_assert!(pos.y >= min_y);
        prop_assert!(pos.y <= min_y.max(max_y));
    }

    /// `ClampToWorkingArea` is idempotent for any finite input.
    #[test]
    fn clamp_to_working_area_is_idempotent(
        preferred in point_strategy(),
        work_area in rect_strategy(),
        popup in size_strategy(),
    ) {
        let once = clamp_to_working_area(preferred, work_area, popup);
        let twice = clamp_to_working_area(once, work_area, popup);

        prop_assert!((once.x - twice.x).abs() <= f64::EPSILON);
        prop_assert!((once.y - twice.y).abs() <= f64::EPSILON);
        prop_assert!(once.x.is_finite());
        prop_assert!(once.y.is_finite());
    }

    /// A point that already sits inside the clamp band is left untouched; one
    /// outside it is pulled to the nearest edge.
    #[test]
    fn clamp_is_the_identity_inside_the_band(
        area_and_popup in work_area_with_popup_strategy(),
        mode in prop::sample::select(vec![
            PopupPlacementMode::BottomRight,
            PopupPlacementMode::NearCursor,
            PopupPlacementMode::NearSelection,
            PopupPlacementMode::NearActionBar,
        ]),
        cursor in point_strategy(),
        selection in option_rect_strategy(),
        action_bar in option_rect_strategy(),
    ) {
        let (work_area, popup) = area_and_popup;
        let min_x = work_area.left + 4.0;
        let max_x = work_area.right - popup.width - 4.0;
        let min_y = work_area.top + 4.0;
        let max_y = work_area.bottom - popup.height - 4.0;
        // Guaranteed by the strategy (the popup always fits with room to spare).
        prop_assert!(max_x > min_x && max_y > min_y);

        let anchors = AnchorContext {
            cursor_dip: cursor,
            selection_dip: selection,
            action_bar_dip: action_bar,
        };

        let pos = calculate_position(mode, anchors, work_area, popup);
        let clamped = clamp_to_working_area(pos, work_area, popup);

        if pos.x >= min_x && pos.x <= max_x {
            prop_assert_eq!(clamped.x, pos.x);
        } else {
            prop_assert!((min_x..=max_x).contains(&clamped.x));
        }

        if pos.y >= min_y && pos.y <= max_y {
            prop_assert_eq!(clamped.y, pos.y);
        } else {
            prop_assert!((min_y..=max_y).contains(&clamped.y));
        }
    }
}

fn point_strategy() -> impl Strategy<Value = Point> {
    (-10_000.0f64..10_000.0, -10_000.0f64..10_000.0).prop_map(|(x, y)| Point::new(x, y))
}

fn size_strategy() -> impl Strategy<Value = Size> {
    (1.0f64..5_000.0, 1.0f64..5_000.0).prop_map(|(width, height)| Size::new(width, height))
}

/// Rect angles first, then normalized so `right >= left` / `bottom >= top`.
fn rect_strategy() -> impl Strategy<Value = Rect> {
    (
        -2_000.0f64..4_000.0,
        -2_000.0f64..4_000.0,
        100.0f64..4_000.0,
        100.0f64..4_000.0,
    )
        .prop_map(|(left, top, width, height)| Rect::new(left, top, left + width, top + height))
}

/// A work area plus a popup that always fits inside it with room to spare, so the
/// clamp band is never empty (degenerate geometries are covered by the unit tests
/// instead, which keeps proptest free of reject storms).
fn work_area_with_popup_strategy() -> impl Strategy<Value = (Rect, Size)> {
    rect_strategy().prop_flat_map(|area| {
        let (width, height) = (area.width(), area.height());
        (0.05f64..0.5, 0.05f64..0.5)
            .prop_map(move |(fx, fy)| (area, Size::new(width * fx, height * fy)))
    })
}

fn option_rect_strategy() -> impl Strategy<Value = Option<Rect>> {
    prop_oneof![rect_strategy().prop_map(Some), Just(None::<Rect>),]
}
