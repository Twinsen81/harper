mod draw_highlight;
mod popup_layout;
mod render_lint_card;

use std::time::Instant;

use egui_commonmark::CommonMarkCache;
use harper_core::{Document, linting::LintKind};

use self::{
    draw_highlight::draw_highlight,
    popup_layout::{PopupLayout, approach, viewport_for_anchor},
    render_lint_card::{
        FIRST_ROW_OFFSET, LintCardAction, popup_layout_for_lint, render_lint_card, row_count,
        sidebar_target,
    },
};
use super::{AddToDictionary, DisableRule, IgnoreLint, OpenSettings};
use crate::{
    lint_kind_color::lint_kind_color,
    rect::{ActionableLint, Rect},
};

/// A lint marker, popup, or empty overlay region found during cursor hit-testing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HitTarget {
    Lint(usize),
    Popup,
    None,
}

/// Stores highlighter-specific drawing state and renders it into an egui frame.
///
/// `Window` owns the native window and GPU plumbing; `RenderState` owns the content we want to draw
/// into that plumbing. This keeps future highlight rectangles, styling, and animation state out of
/// the platform/rendering infrastructure.
pub struct RenderState {
    /// Last successful accessibility read, retained when a later read fails.
    last_lints: Option<Vec<ActionableLint>>,

    /// Index of the lint whose suggestion popup is currently visible.
    highlighted_lint: Option<usize>,

    /// Index of the suggestion row that the user hovered last in the visible popup.
    shown_suggestion: Option<usize>,

    /// The flyout card's drawn offset from the list card's top, in points.
    current_sidebar_position: f32,

    /// The flyout card's drawn height in the previous frame, or `None` before the first frame
    /// of the visible popup.
    flyout_height: Option<f32>,

    /// When `render` last ran, used to move the sidebar at a speed that follows real time.
    last_render: Option<Instant>,

    /// Cache for markdown-rendered lint messages so popup redraws do not repeatedly rebuild markdown
    /// resources from scratch.
    markdown_cache: CommonMarkCache,

    /// Called when the user dismisses a lint so app-level ignored-lint state can be updated.
    ignore_lint: IgnoreLint,

    /// Called when the user adds a spelling lint's source text to the local dictionary.
    add_to_dictionary: AddToDictionary,

    /// Called when the user disables the rule that produced the selected lint.
    disable_rule: DisableRule,

    open_settings: OpenSettings,
}

impl RenderState {
    /// Creates render state through `set_lints` so initial lint data gets the same stale-popup guard
    /// used by later accessibility refreshes.
    pub fn new(
        rects: Vec<ActionableLint>,
        ignore_lint: IgnoreLint,
        add_to_dictionary: AddToDictionary,
        disable_rule: DisableRule,
        open_settings: OpenSettings,
    ) -> Self {
        let mut state = Self {
            last_lints: None,
            highlighted_lint: None,
            shown_suggestion: None,
            current_sidebar_position: FIRST_ROW_OFFSET,
            flyout_height: None,
            last_render: None,
            markdown_cache: CommonMarkCache::default(),
            ignore_lint,
            add_to_dictionary,
            disable_rule,
            open_settings,
        };
        state.set_lints(rects);
        state
    }

    /// Saves successful accessibility reads while retaining the previous lints after a failed read.
    ///
    /// Closes the popup when its lint disappeared, and forgets the shown suggestion when the
    /// refreshed lint has fewer rows.
    pub fn set_lints(&mut self, lints: Vec<ActionableLint>) {
        self.last_lints = Some(lints);

        if self
            .highlighted_lint
            .is_some_and(|index| index >= self.lints().len())
        {
            self.close_popup();
        }

        let row_count = self
            .highlighted_lint
            .and_then(|index| self.lints().get(index))
            .map(|positioned_lint| row_count(&positioned_lint.lint));
        self.shown_suggestion = self
            .shown_suggestion
            .filter(|index| row_count.is_some_and(|row_count| *index < row_count));
    }

    fn lints(&self) -> &[ActionableLint] {
        self.last_lints.as_deref().unwrap_or_default()
    }

    /// Updates which lint owns the suggestion popup without exposing render-state internals.
    ///
    /// Window/input code decides what the user interacted with, while `RenderState` owns the popup
    /// selection. Filtering invalid indexes here keeps stale cursor state from selecting a lint that
    /// disappeared after the latest accessibility read.
    ///
    /// Like the Chrome extension, each new selection starts again from the first suggestion row.
    pub fn set_highlighted_lint(&mut self, highlighted_lint: Option<usize>) {
        self.highlighted_lint = highlighted_lint.filter(|index| *index < self.lints().len());
        self.shown_suggestion = None;
        self.current_sidebar_position = FIRST_ROW_OFFSET;
        self.flyout_height = None;
    }

    /// Centralizes the close behavior so the popup close button only has to clear the selected lint.
    pub fn close_popup(&mut self) {
        self.highlighted_lint = None;
        self.shown_suggestion = None;
        self.current_sidebar_position = FIRST_ROW_OFFSET;
        self.flyout_height = None;
    }

    /// Checks if a given position touches a hit target.
    ///
    /// `pos` and `viewports` are in global screen points. `viewports` are the areas of the overlay
    /// windows, one for each monitor.
    pub fn hit_target_at_pos(&self, pos: egui::Pos2, viewports: &[egui::Rect]) -> HitTarget {
        if self
            .popup_layout(viewports)
            .is_some_and(|layout| layout.contains(pos))
        {
            return HitTarget::Popup;
        }

        self.lints()
            .iter()
            .position(|positioned_lint| {
                rect_bounds(&positioned_lint.rect.with_friendly_padding()).contains(pos)
            })
            .map_or(HitTarget::None, HitTarget::Lint)
    }

    /// Computes popup hit-test bounds from our layout contract instead of waiting for egui to report
    /// rendered bounds, which keeps hit-testing available before the next render pass completes.
    ///
    /// The popup is placed inside the viewport of the monitor that shows the lint. The layout is in
    /// global screen points.
    pub fn popup_layout(&self, viewports: &[egui::Rect]) -> Option<PopupLayout> {
        self.popup_placement(viewports).map(|(_, layout)| layout)
    }

    /// Returns the viewport that shows the popup, and the popup layout in global screen points.
    fn popup_placement(&self, viewports: &[egui::Rect]) -> Option<(egui::Rect, PopupLayout)> {
        let positioned_lint = self
            .highlighted_lint
            .and_then(|index| self.lints().get(index))?;
        let viewport = viewport_for_anchor(&positioned_lint.rect, viewports)?;

        Some((
            viewport,
            popup_layout_for_lint(
                &positioned_lint.rect,
                &positioned_lint.lint,
                viewport,
                self.current_sidebar_position,
                self.flyout_height,
            ),
        ))
    }

    /// Draws highlights and the active popup from the same state used by hit-testing so visible
    /// regions and clickable regions do not drift apart.
    ///
    /// Each overlay window calls this with its own `viewport`, and `viewports` has the areas of all
    /// overlay windows, all in global screen points. A window draws only the highlights on its
    /// monitor, and only the window of the monitor that shows the selected lint draws the popup.
    ///
    /// Each call also moves the sidebar toward its target by the time since the previous call.
    pub fn render(&mut self, ui: &mut egui::Ui, viewport: egui::Rect, viewports: &[egui::Rect]) {
        let now = Instant::now();
        let time_since_last_render = self
            .last_render
            .map_or(0.0, |last_render| (now - last_render).as_secs_f32());
        self.last_render = Some(now);

        if let Some(positioned_lint) = self
            .highlighted_lint
            .and_then(|index| self.lints().get(index))
        {
            let target = sidebar_target(&positioned_lint.lint, self.shown_suggestion);
            self.current_sidebar_position = approach(
                self.current_sidebar_position,
                target,
                time_since_last_render,
            );
        }

        for positioned_lint in self.lints() {
            if let Some(bounds) = window_bounds(&positioned_lint.rect, viewport) {
                draw_highlight(ui, bounds, &positioned_lint.lint);
            }
        }

        if let Some(index) = self.highlighted_lint
            && let Some(positioned_lint) = self.lints().get(index)
            && let Some((popup_viewport, layout)) = self.popup_placement(viewports)
            && let Some(layout) = layout.in_window(popup_viewport, viewport)
        {
            let lint = positioned_lint.lint.clone();
            let source_text = positioned_lint.source_text.clone();

            let (action, flyout_height) = render_lint_card(
                ui,
                layout,
                &lint,
                &source_text,
                self.shown_suggestion,
                &mut self.markdown_cache,
            );
            self.flyout_height = Some(flyout_height);

            match action {
                Some(LintCardAction::Close) => self.close_popup(),
                Some(LintCardAction::OpenSettings) => {
                    (self.open_settings)();
                    self.close_popup();
                }
                Some(LintCardAction::HoverSuggestion(suggestion)) => {
                    self.shown_suggestion = Some(suggestion);
                }
                Some(LintCardAction::ApplySuggestion(suggestion)) => {
                    if let Some(actionable_lint) = self
                        .last_lints
                        .as_mut()
                        .and_then(|lints| lints.get_mut(index))
                    {
                        actionable_lint.apply_suggestion(suggestion);
                    }

                    self.close_popup();
                }
                Some(LintCardAction::IgnoreLint) => {
                    if let Some((lint, source_text)) =
                        self.lints().get(index).map(|actionable_lint| {
                            (
                                actionable_lint.lint.clone(),
                                actionable_lint.source_text.clone(),
                            )
                        })
                    {
                        let document = Document::new_markdown_default_curated(&source_text);
                        (self.ignore_lint)(&lint, &document);
                    }

                    self.close_popup();
                }
                Some(LintCardAction::AddToDictionary) => {
                    if let Some((lint, source_text)) =
                        self.lints().get(index).map(|actionable_lint| {
                            (
                                actionable_lint.lint.clone(),
                                actionable_lint.source_text.clone(),
                            )
                        })
                    {
                        let document = Document::new_markdown_default_curated(&source_text);
                        let word = lint.get_str(document.get_source());
                        (self.add_to_dictionary)(&word);
                    }

                    self.close_popup();
                }
                Some(LintCardAction::DisableRule) => {
                    if let Some(rule_name) = self
                        .lints()
                        .get(index)
                        .map(|actionable_lint| actionable_lint.rule_name.clone())
                    {
                        (self.disable_rule)(&rule_name);
                    }

                    self.close_popup();
                }
                None => {}
            }
        }
    }
}

/// Converts accessibility rectangles into egui rectangles so rendering and hit-testing use the same
/// screen-space geometry.
fn rect_bounds(rect: &Rect) -> egui::Rect {
    egui::Rect::from_min_size(
        egui::pos2(rect.x as f32, rect.y as f32),
        egui::vec2(rect.width as f32, rect.height as f32),
    )
}

/// Converts a rectangle from global screen points to the points of the window with `viewport`.
///
/// Returns `None` when the rectangle is not in the window, so each monitor's window draws only
/// what is on that monitor.
fn window_bounds(rect: &Rect, viewport: egui::Rect) -> Option<egui::Rect> {
    let bounds = rect_bounds(rect);

    bounds
        .intersects(viewport)
        .then(|| bounds.translate(-viewport.min.to_vec2()))
}

fn lint_kind_color32(lint_kind: LintKind) -> egui::Color32 {
    let color = lint_kind_color(lint_kind);

    egui::Color32::from_rgb(color.r, color.g, color.b)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A MacBook display at the origin and an external display on its right, in global screen
    /// points.
    const BUILT_IN: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1728.0, 1117.0));
    const EXTERNAL: egui::Rect =
        egui::Rect::from_min_max(egui::pos2(1728.0, 0.0), egui::pos2(4288.0, 1440.0));

    #[test]
    fn window_bounds_keeps_primary_monitor_points() {
        let rect = Rect::new(400.0, 1050.0, 60.0, 16.0);

        assert_eq!(window_bounds(&rect, BUILT_IN), Some(rect_bounds(&rect)));
    }

    #[test]
    fn window_bounds_skips_rect_on_other_monitor() {
        assert_eq!(
            window_bounds(&Rect::new(400.0, 1050.0, 60.0, 16.0), EXTERNAL),
            None
        );
        assert_eq!(
            window_bounds(&Rect::new(2000.0, 100.0, 60.0, 16.0), BUILT_IN),
            None
        );
    }

    #[test]
    fn window_bounds_translates_by_monitor_origin() {
        let bounds = window_bounds(&Rect::new(2000.0, 100.0, 60.0, 16.0), EXTERNAL);

        assert_eq!(
            bounds,
            Some(egui::Rect::from_min_size(
                egui::pos2(272.0, 100.0),
                egui::vec2(60.0, 16.0)
            ))
        );
    }

    #[test]
    fn window_bounds_supports_negative_origins() {
        let left =
            egui::Rect::from_min_size(egui::pos2(-2560.0, -323.0), egui::vec2(2560.0, 1440.0));
        let bounds = window_bounds(&Rect::new(-300.0, -100.0, 60.0, 16.0), left);

        assert_eq!(
            bounds,
            Some(egui::Rect::from_min_size(
                egui::pos2(2260.0, 223.0),
                egui::vec2(60.0, 16.0)
            ))
        );
    }

    #[test]
    fn window_bounds_draws_rect_across_monitors_on_both() {
        let rect = Rect::new(1700.0, 100.0, 60.0, 16.0);

        assert!(window_bounds(&rect, BUILT_IN).is_some());
        assert_eq!(
            window_bounds(&rect, EXTERNAL).map(|bounds| bounds.min),
            Some(egui::pos2(-28.0, 100.0))
        );
    }
}
