//! Places the suggestion list card and the lint flyout card on screen.
//!
//! The geometry is a pure function of its inputs, so drawing and hit-testing compute the same
//! rectangles without measuring a rendered frame.

use crate::rect::Rect;

/// Space between the list card and the flyout card.
pub const PANEL_GAP: f32 = 6.0;
/// Minimum space between either card and the viewport edge.
pub const VIEWPORT_MARGIN: f32 = 8.0;
/// Space between the lint highlight and the list card.
pub const ANCHOR_GAP: f32 = 3.0;

/// The time constant, in seconds, of the sidebar's movement toward its target.
///
/// After this time, the sidebar has moved about 63% of the distance. After three times this value,
/// it has moved about 95%.
pub const SIDEBAR_SMOOTHING: f32 = 0.05;

/// The distance, in points, below which the sidebar jumps to its target.
///
/// Without this, the exponential movement would continue below a visible distance forever.
pub const SIDEBAR_SNAP_DISTANCE: f32 = 0.5;

/// The screen-space rectangles of the two popup cards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PopupLayout {
    /// The card with the suggestion rows and the footer.
    pub list: egui::Rect,
    /// The card with the lint kind, the explanation, and the lint actions.
    pub flyout: egui::Rect,
}

impl PopupLayout {
    /// Moves both cards by `offset`.
    pub fn translate(self, offset: egui::Vec2) -> Self {
        Self {
            list: self.list.translate(offset),
            flyout: self.flyout.translate(offset),
        }
    }

    /// Converts a popup from global screen points to the points of one overlay window.
    ///
    /// `popup_viewport` is the viewport that the popup was placed in, and `window_viewport` is the
    /// window's area. Both are in global screen points. Returns `None` when the popup belongs to
    /// another window, so only one monitor shows the popup.
    pub fn in_window(
        self,
        popup_viewport: egui::Rect,
        window_viewport: egui::Rect,
    ) -> Option<Self> {
        (popup_viewport == window_viewport).then(|| self.translate(-window_viewport.min.to_vec2()))
    }

    /// Checks if a position is on either card.
    ///
    /// The gap between the cards is not part of the popup, so clicks there go to the editor.
    pub fn contains(&self, pos: egui::Pos2) -> bool {
        self.list.contains(pos) || self.flyout.contains(pos)
    }
}

/// Computes the card positions with the algorithm from `popupPosition` in the Chrome extension's
/// `packages/lint-framework/src/lint/SuggestionBox.ts`.
///
/// - The list card goes below the anchor, or above it when there is not sufficient space below.
/// - The flyout card goes to the right of the list, or to the left when there is not sufficient
///   space on the right.
/// - When the viewport is too narrow for the two cards side by side, the flyout goes below the list.
/// - Both cards stay at least [`VIEWPORT_MARGIN`] from the viewport edges.
///
/// One difference from the Chrome extension: when the cards are side by side, the flyout's top
/// moves down by `sidebar_offset`, the distance from the list card's top. The caller animates this
/// value toward the suggestion row that the user hovered last. The flyout stays clamped to the
/// viewport while it moves.
pub fn popup_layout(
    anchor: &Rect,
    viewport: egui::Rect,
    list: egui::Vec2,
    flyout: egui::Vec2,
    sidebar_offset: f32,
) -> PopupLayout {
    let margin = VIEWPORT_MARGIN;
    let anchor_top = anchor.y as f32;
    let anchor_bottom = anchor_top + anchor.height as f32;

    let stacked = list.x + PANEL_GAP + flyout.x + margin * 2.0 > viewport.width();
    let height = if stacked {
        list.y + PANEL_GAP + flyout.y
    } else {
        list.y
    };

    let below = anchor_bottom + ANCHOR_GAP;
    let flipped = below + height + margin > viewport.max.y;
    let top = clamp(
        if flipped {
            anchor_top - height - ANCHOR_GAP
        } else {
            below
        },
        viewport.min.y + margin,
        viewport.max.y - height - margin,
    );

    let left = clamp(
        anchor.x as f32,
        viewport.min.x + margin,
        viewport.max.x - list.x - margin,
    );
    let opens_left = left + list.x + PANEL_GAP + flyout.x + margin > viewport.max.x;
    let left = if opens_left && !stacked {
        clamp(
            left,
            viewport.min.x + margin + flyout.x + PANEL_GAP,
            viewport.max.x - list.x - margin,
        )
    } else {
        left
    };

    let flyout_left = if stacked {
        left
    } else if opens_left {
        left - flyout.x - PANEL_GAP
    } else {
        left + list.x + PANEL_GAP
    };
    let flyout_top = if stacked {
        top + list.y + PANEL_GAP
    } else {
        clamp(
            top + sidebar_offset,
            viewport.min.y + margin,
            viewport.max.y - flyout.y - margin,
        )
    };

    PopupLayout {
        list: egui::Rect::from_min_size(egui::pos2(left, top), list),
        flyout: egui::Rect::from_min_size(egui::pos2(flyout_left, flyout_top), flyout),
    }
}

/// Selects the viewport (one monitor's area, in global screen points) that shows `anchor`.
///
/// This is the viewport that contains the anchor's center. When no viewport contains it, this is
/// the viewport nearest to the center, so a lint at a screen edge still gets a popup. Returns `None`
/// only when there are no viewports.
pub fn viewport_for_anchor(anchor: &Rect, viewports: &[egui::Rect]) -> Option<egui::Rect> {
    let center = egui::pos2(
        (anchor.x + anchor.width / 2.0) as f32,
        (anchor.y + anchor.height / 2.0) as f32,
    );

    viewports.iter().copied().min_by(|a, b| {
        a.distance_sq_to_pos(center)
            .total_cmp(&b.distance_sq_to_pos(center))
    })
}

/// Moves `current` toward `target` by an exponential lerp over `dt` seconds.
///
/// The lerp factor is `1 - exp(-dt / SIDEBAR_SMOOTHING)`. Because two steps of `dt / 2` give the same
/// result as one step of `dt`, the speed does not depend on the frame rate or on how many windows
/// call this in each frame. The result never goes past `target`, and it snaps to `target` within
/// [`SIDEBAR_SNAP_DISTANCE`].
pub fn approach(current: f32, target: f32, dt: f32) -> f32 {
    let factor = 1.0 - (-dt.max(0.0) / SIDEBAR_SMOOTHING).exp();
    let next = current + (target - current) * factor;

    if (target - next).abs() < SIDEBAR_SNAP_DISTANCE {
        target
    } else {
        next
    }
}

/// Clamps like the Chrome extension: when `max < min`, the result is `min`.
fn clamp(value: f32, min: f32, max: f32) -> f32 {
    value.min(max.max(min)).max(min)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: egui::Vec2 = egui::vec2(212.0, 100.0);
    const FLYOUT: egui::Vec2 = egui::vec2(192.0, 150.0);

    fn viewport(width: f32, height: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, height))
    }

    fn anchor() -> Rect {
        Rect::new(50.0, 40.0, 30.0, 12.0)
    }

    #[test]
    fn opens_right_below_anchor() {
        let layout = popup_layout(&anchor(), viewport(1000.0, 1000.0), LIST, FLYOUT, 0.0);

        assert_eq!(layout.list.min, egui::pos2(50.0, 40.0 + 12.0 + ANCHOR_GAP));
        assert_eq!(layout.list.size(), LIST);
        assert_eq!(
            layout.flyout.min,
            egui::pos2(50.0 + LIST.x + PANEL_GAP, layout.list.top())
        );
        assert_eq!(layout.flyout.size(), FLYOUT);
    }

    #[test]
    fn flyout_follows_sidebar_offset() {
        for sidebar_offset in [0.0, 30.0, 95.0] {
            let layout = popup_layout(
                &anchor(),
                viewport(1000.0, 1000.0),
                LIST,
                FLYOUT,
                sidebar_offset,
            );

            assert_eq!(layout.flyout.top(), layout.list.top() + sidebar_offset);
        }
    }

    #[test]
    fn opens_left_near_right_edge() {
        let anchor = Rect::new(700.0, 40.0, 30.0, 12.0);
        let layout = popup_layout(&anchor, viewport(1000.0, 1000.0), LIST, FLYOUT, 0.0);

        assert_eq!(layout.list.left(), 700.0);
        assert_eq!(layout.flyout.right(), 700.0 - PANEL_GAP);
    }

    #[test]
    fn opening_left_shifts_list_to_fit_flyout() {
        let layout = popup_layout(&anchor(), viewport(450.0, 1000.0), LIST, FLYOUT, 0.0);

        assert_eq!(layout.flyout.left(), VIEWPORT_MARGIN);
        assert_eq!(layout.flyout.right(), layout.list.left() - PANEL_GAP);
    }

    #[test]
    fn flips_above_anchor_near_bottom() {
        let anchor = Rect::new(50.0, 950.0, 30.0, 12.0);
        let layout = popup_layout(&anchor, viewport(1000.0, 1000.0), LIST, FLYOUT, 0.0);

        assert_eq!(layout.list.bottom(), 950.0 - ANCHOR_GAP);
    }

    #[test]
    fn flyout_clamps_to_viewport_bottom() {
        let layout = popup_layout(&anchor(), viewport(1000.0, 250.0), LIST, FLYOUT, 90.0);

        assert_eq!(layout.flyout.bottom(), 250.0 - VIEWPORT_MARGIN);
    }

    #[test]
    fn stacks_on_narrow_viewport() {
        let layout = popup_layout(&anchor(), viewport(400.0, 1000.0), LIST, FLYOUT, 60.0);

        assert_eq!(layout.flyout.left(), layout.list.left());
        assert_eq!(layout.flyout.top(), layout.list.bottom() + PANEL_GAP);
    }

    #[test]
    fn clamps_list_to_viewport_margins() {
        let anchor = Rect::new(-20.0, -40.0, 30.0, 12.0);
        let layout = popup_layout(&anchor, viewport(1000.0, 1000.0), LIST, FLYOUT, 0.0);

        assert_eq!(
            layout.list.min,
            egui::pos2(VIEWPORT_MARGIN, VIEWPORT_MARGIN)
        );
    }

    #[test]
    fn contains_cards_but_not_gap() {
        let layout = popup_layout(&anchor(), viewport(1000.0, 1000.0), LIST, FLYOUT, 0.0);
        let gap = egui::pos2(
            layout.list.right() + PANEL_GAP / 2.0,
            layout.list.center().y,
        );

        assert!(layout.contains(layout.list.center()));
        assert!(layout.contains(layout.flyout.center()));
        assert!(!layout.contains(gap));
    }

    /// A MacBook display at the origin and a larger external display on its right, with the top
    /// edges aligned, in global screen points.
    fn two_monitors() -> [egui::Rect; 2] {
        [
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1728.0, 1117.0)),
            egui::Rect::from_min_size(egui::pos2(1728.0, 0.0), egui::vec2(2560.0, 1440.0)),
        ]
    }

    #[test]
    fn in_window_translates_to_window_points() {
        let [_, external] = two_monitors();
        let anchor = Rect::new(2000.0, 100.0, 60.0, 16.0);
        let layout = popup_layout(&anchor, external, LIST, FLYOUT, 0.0);
        let local = layout.in_window(external, external).unwrap();

        assert_eq!(
            local.list.min,
            egui::pos2(2000.0 - 1728.0, 116.0 + ANCHOR_GAP)
        );
        assert_eq!(local.list.size(), layout.list.size());
        assert_eq!(local.flyout.left(), local.list.right() + PANEL_GAP);
    }

    #[test]
    fn in_window_keeps_primary_monitor_points() {
        let [built_in, _] = two_monitors();
        let layout = popup_layout(&anchor(), built_in, LIST, FLYOUT, 0.0);

        assert_eq!(layout.in_window(built_in, built_in), Some(layout));
    }

    #[test]
    fn in_window_skips_other_monitors() {
        let [built_in, external] = two_monitors();
        let layout = popup_layout(&anchor(), built_in, LIST, FLYOUT, 0.0);

        assert_eq!(layout.in_window(built_in, external), None);
    }

    #[test]
    fn viewport_for_anchor_picks_monitor_with_anchor() {
        let [built_in, external] = two_monitors();

        assert_eq!(
            viewport_for_anchor(&Rect::new(400.0, 1050.0, 60.0, 16.0), &two_monitors()),
            Some(built_in)
        );
        assert_eq!(
            viewport_for_anchor(&Rect::new(2000.0, 1300.0, 60.0, 16.0), &two_monitors()),
            Some(external)
        );
    }

    #[test]
    fn viewport_for_anchor_supports_negative_origins() {
        let built_in = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1728.0, 1117.0));
        let left =
            egui::Rect::from_min_size(egui::pos2(-2560.0, -400.0), egui::vec2(2560.0, 1440.0));
        let above = egui::Rect::from_min_size(egui::pos2(0.0, -1440.0), egui::vec2(2560.0, 1440.0));
        let viewports = [built_in, left, above];

        assert_eq!(
            viewport_for_anchor(&Rect::new(-300.0, -100.0, 60.0, 16.0), &viewports),
            Some(left)
        );
        assert_eq!(
            viewport_for_anchor(&Rect::new(300.0, -100.0, 60.0, 16.0), &viewports),
            Some(above)
        );
        assert_eq!(
            viewport_for_anchor(&Rect::new(300.0, 100.0, 60.0, 16.0), &viewports),
            Some(built_in)
        );
    }

    #[test]
    fn viewport_for_anchor_uses_center_for_anchor_on_two_monitors() {
        let [built_in, external] = two_monitors();

        assert_eq!(
            viewport_for_anchor(&Rect::new(1700.0, 100.0, 40.0, 16.0), &two_monitors()),
            Some(built_in)
        );
        assert_eq!(
            viewport_for_anchor(&Rect::new(1720.0, 100.0, 40.0, 16.0), &two_monitors()),
            Some(external)
        );
    }

    #[test]
    fn viewport_for_anchor_falls_back_to_nearest_monitor() {
        let [built_in, external] = two_monitors();

        // Below the MacBook display, where the external display does not reach.
        assert_eq!(
            viewport_for_anchor(&Rect::new(400.0, 1200.0, 60.0, 16.0), &two_monitors()),
            Some(built_in)
        );
        assert_eq!(
            viewport_for_anchor(&Rect::new(5000.0, 100.0, 60.0, 16.0), &two_monitors()),
            Some(external)
        );
    }

    #[test]
    fn viewport_for_anchor_without_monitors() {
        assert_eq!(
            viewport_for_anchor(&Rect::new(400.0, 100.0, 60.0, 16.0), &[]),
            None
        );
    }

    #[test]
    fn popup_near_monitor_bottom_stays_inside_monitor() {
        let [built_in, _] = two_monitors();
        let anchor = Rect::new(400.0, 1050.0, 60.0, 16.0);
        let layout = popup_layout(&anchor, built_in, LIST, FLYOUT, 0.0);

        assert_eq!(layout.list.bottom(), 1050.0 - ANCHOR_GAP);
        assert!(built_in.contains_rect(layout.list));
        assert!(built_in.contains_rect(layout.flyout));
    }

    #[test]
    fn popup_on_external_monitor_stays_inside_it() {
        let [_, external] = two_monitors();
        let anchor = Rect::new(4200.0, 1400.0, 60.0, 16.0);
        let layout = popup_layout(&anchor, external, LIST, FLYOUT, 0.0);

        assert!(external.contains_rect(layout.list));
        assert!(external.contains_rect(layout.flyout));
    }
}
