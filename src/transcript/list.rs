//! The transcript's list of rows: virtualized, following the latest row while
//! the reader is at the bottom, with a scrollbar and a jump-to-latest
//! button. It does what GPUI Kit's MessageScroller does, with the list's
//! scroll position in reach, so the transcript can keep the reader's place
//! when it replaces rows and when the reader opens one at the bottom.

use std::ops::Range;
use std::time::Duration;

use gpui_kit::base::motion::{Transition, transition};
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollableMask};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName,
    button::{Button, ButtonVariants as _},
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

/// Rows laid out past the screen's edges, for smooth scrolling.
const OVERDRAW: Pixels = px(400.);
const JUMP_BUTTON_TRANSITION: Duration = Duration::from_millis(200);

/// The list's state: which rows, their measured heights, and where the
/// reader is.
pub(super) struct RowList {
    state: ListState,
}

impl RowList {
    pub fn new() -> Self {
        let state = ListState::new(0, ListAlignment::Top, OVERDRAW);
        state.set_follow_mode(FollowMode::Tail);
        Self { state }
    }

    /// The list's state, for an element in a row that scrolls it.
    pub(super) fn handle(&self) -> ListState {
        self.state.clone()
    }

    /// Whether the reader has scrolled away from the latest row.
    pub fn is_scrolled_up(&self) -> bool {
        self.state.max_offset_for_scrollbar().y > px(0.)
            && !self.state.is_following_tail()
            && !self.state.is_scrolled_to_end().unwrap_or(false)
    }

    pub fn is_following(&self) -> bool {
        self.state.is_following_tail()
    }

    /// Starts over with `count` rows, following the latest.
    pub fn reset(&self, count: usize) {
        self.state.reset(count);
        self.state.set_follow_mode(FollowMode::Tail);
    }

    pub fn splice(&self, old: Range<usize>, count: usize) {
        self.state.splice(old, count);
    }

    pub fn remeasure(&self) {
        self.state.remeasure();
    }

    pub fn remeasure_row(&self, ix: usize) {
        if ix < self.state.item_count() {
            self.state.remeasure_items(ix..ix + 1);
        }
    }

    /// The row at the top of the screen, and how far into it the screen
    /// starts.
    pub fn top(&self) -> ListOffset {
        self.state.logical_scroll_top()
    }

    /// Puts the row `ix` at the top of the screen, `offset` into it.
    pub fn scroll_to(&self, ix: usize, offset: Pixels) {
        self.state.scroll_to(ListOffset {
            item_ix: ix,
            offset_in_item: offset,
        });
    }

    /// Scrolls to the latest row and follows the rows from there.
    pub fn scroll_to_end(&self) {
        self.state.set_follow_mode(FollowMode::Tail);
        self.state.scroll_to_end();
    }

    /// Scrolls by `distance`, down when positive.
    pub fn scroll_by(&self, distance: Pixels) {
        self.state.scroll_by(distance);
    }

    /// The screen's height, as last laid out.
    pub fn viewport_height(&self) -> Pixels {
        self.state.viewport_bounds().size.height
    }

    /// The rows on screen, wholly or in part, top to bottom, as last laid
    /// out.
    pub fn on_screen(&self) -> Vec<usize> {
        let viewport = self.state.viewport_bounds();
        let mut rows = Vec::new();
        let mut ix = self.state.logical_scroll_top().item_ix;
        while let Some(bounds) = self.state.bounds_for_item(ix) {
            if bounds.top() >= viewport.bottom() {
                break;
            }
            if bounds.bottom() > viewport.top() {
                rows.push(ix);
            }
            ix += 1;
        }
        rows
    }

    /// Whether row `ix` is wholly on screen, as last laid out.
    pub fn shows_whole(&self, ix: usize) -> bool {
        let viewport = self.state.viewport_bounds();
        self.state.bounds_for_item(ix).is_some_and(|bounds| {
            bounds.top() >= viewport.top() && bounds.bottom() <= viewport.bottom()
        })
    }

    /// Scrolls row `ix` into view, by as little as it takes: to the top of
    /// the screen when it is above, its bottom to the screen's bottom when
    /// it is below (its top, if it's taller than the screen).
    pub fn reveal(&self, ix: usize) {
        let viewport = self.state.viewport_bounds();
        match self.state.bounds_for_item(ix) {
            Some(bounds) if bounds.top() < viewport.top() => {
                self.state.scroll_by(bounds.top() - viewport.top())
            }
            Some(bounds) if bounds.bottom() > viewport.bottom() => {
                let down = (bounds.bottom() - viewport.bottom()).min(bounds.top() - viewport.top());
                self.state.scroll_by(down);
            }
            Some(_) => {}
            None => self.scroll_to(ix, px(0.)),
        }
    }

    /// Stops following the latest row where the screen is now, until the
    /// reader comes back to the bottom: a row the reader opens grows
    /// downward instead of pushing itself up off the screen.
    pub fn hold_place(&self, ix: usize) {
        if self.state.is_following_tail() && self.state.bounds_for_item(ix).is_some() {
            self.state.pause_following_tail();
        }
    }
}

/// Draws `list`, each row by `row`, with the jump-to-latest button while
/// the reader is scrolled up; it says how many entries came meanwhile
/// (`new`).
pub(super) fn render(
    id: &'static str,
    list: &RowList,
    new: usize,
    row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state = list.state.clone();
    let scrolled_up = list.is_scrolled_up();
    let visibility = transition(
        "transcript-jump-button",
        if scrolled_up { 1. } else { 0. },
        Transition::new(JUMP_BUTTON_TRANSITION),
        window,
        cx,
    );
    let theme = cx.theme();
    let (border, background, foreground) = (theme.border, theme.background, theme.foreground);
    let radius = theme.radius_full();
    let rows = gpui_kit::list(state.clone(), row)
        .size_full()
        .min_h_0()
        .py_2();
    div()
        .id(id)
        .relative()
        .size_full()
        .min_h_0()
        .overflow_hidden()
        .child(
            div()
                .id("transcript-viewport")
                // Rows that come are announced, as a log is.
                .role(Role::Log)
                .size_full()
                .min_h_0()
                .min_w_0()
                .child(rows)
                .vertical_scrollbar(&state),
        )
        // Vertical wheel scrolling stays in the list while it can move.
        .child(ScrollableMask::new(Axis::Vertical, &state).id("transcript-mask"))
        .when(visibility > 0., |this| {
            let jump = state.clone();
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(rems(0.5 + visibility * 0.5))
                    .flex()
                    .justify_center()
                    .opacity(visibility)
                    .child(
                        Button::new("jump-to-latest")
                            .secondary()
                            .icon(IconName::ArrowDown)
                            .tooltip("Jump to latest")
                            .when(new > 0, |button| button.label(format!("{new} new")))
                            .rounded(radius)
                            .border_1()
                            .border_color(border)
                            .bg(background)
                            .text_color(foreground)
                            .disabled(!scrolled_up)
                            .on_click(move |_, window, _| {
                                jump.set_follow_mode(FollowMode::Tail);
                                jump.scroll_to_end();
                                window.refresh();
                            }),
                    ),
            )
        })
        .into_any_element()
}
