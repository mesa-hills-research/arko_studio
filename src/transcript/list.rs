//! The transcript's list of rows, virtualized and laid out by the moves of
//! each frame (GPUI's queued moves), with a scrollbar and the marker of what
//! is new below. The transcript owns the reader's choice to follow the end or
//! read, and the hold of a press. The list owns where the lines are and
//! reports what each move did, from which [`Following`] sets the choice.
//!
//! The lines never move down by themselves. When the end gets shorter,
//! blank space opens below it and stays until what comes fills it. A scroll
//! up that moves the lines reads, a scroll down that leaves the end in sight
//! follows, and End, Esc, the marker and sending follow again.

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::rc::Rc;
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
/// The space, in rem, above the first row and below the last.
pub(super) const PADDING: f32 = 0.5;
const JUMP_BUTTON_TRANSITION: Duration = Duration::from_millis(200);
/// How far a press may move before it is a drag, which keeps its place.
const DRAG: Pixels = px(4.);

/// What the transcript keeps of following, the reader's choice and the hold.
#[derive(Debug)]
pub(super) struct Following {
    /// The reader's choice to follow the end (or read where they are).
    follow: bool,
    /// The left button is down in the transcript.
    hold: Option<Hold>,
    /// Whether the last frame followed.
    followed: bool,
    /// The reader scrolled, or asked for the end or the start, since the
    /// transcript last asked.
    reader_moved: bool,
}

/// A press in the transcript, until its release.
#[derive(Debug, Clone, Copy)]
struct Hold {
    /// The frame at the press followed.
    followed: bool,
    at: Point<Pixels>,
    dragged: bool,
}

impl Following {
    fn new() -> Self {
        Self {
            follow: true,
            hold: None,
            followed: true,
            reader_moved: false,
        }
    }

    /// What a frame does with the end, from what its moves did. A scroll or
    /// a thumb's drag that moved the lines down a device pixel or more reads,
    /// and so do Home and a row revealed that moved them down. A scroll down
    /// that leaves the end in sight follows, also when nothing moved, and so
    /// do the thumb at the bottom of its track and the end asked for. The
    /// moves act in order, so the last that sets the choice wins. A frame
    /// follows when the choice is follow, nothing holds the view and the end
    /// shows. Where no move moved the lines down or put them somewhere, an
    /// end that showed in the last frame still shows, though output grew it
    /// in this one.
    fn decide(&mut self, frame: &ListFrame<'_>) -> Follow {
        let was = self.follow;
        for effect in frame.moves {
            if matches!(
                effect.what,
                ListMove::Scroll { .. }
                    | ListMove::Thumb { .. }
                    | ListMove::ToEnd
                    | ListMove::ToStart
            ) {
                self.reader_moved = true;
            }
            match effect.what {
                ListMove::Scroll { delta, .. } => {
                    if effect.moved > 0 {
                        self.follow = false;
                    } else if delta > px(0.) && effect.end_shows {
                        self.follow = true;
                    }
                }
                // (Only a drag to the bottom of the track follows, since with
                // blank below the end the thumb rests there while the reader
                // reads.)
                ListMove::Thumb { to: None } => {
                    if effect.moved > 0 {
                        self.follow = false;
                    } else if effect.end_shows {
                        self.follow = true;
                    }
                }
                ListMove::Thumb { .. } => {
                    if effect.moved > 0 {
                        self.follow = false;
                    }
                }
                ListMove::ToEnd => self.follow = true,
                ListMove::ToStart | ListMove::Reveal(_) => {
                    if effect.moved > 0 {
                        self.follow = false;
                    }
                }
                ListMove::To(_) | ListMove::KeepTop { .. } | ListMove::CloseBlank => {}
                _ => {}
            }
        }
        let placed = frame.moves.iter().any(|effect| {
            effect.moved > 0
                || matches!(
                    effect.what,
                    ListMove::KeepTop { .. }
                        | ListMove::To(_)
                        | ListMove::ToStart
                        | ListMove::Reveal(_)
                )
        });
        let shows = if placed {
            frame.end_shows
        } else {
            frame.end_showed || frame.end_shows
        };
        // (Following that starts again by a move, a scroll down to the end or
        // End, gives nothing back that earlier shorter views took.)
        let again = self.follow && !was;
        self.followed = self.follow && self.hold.is_none() && shows;
        match () {
            _ if !self.followed => Follow::No,
            _ if frame.width_changed => Follow::ToEnd,
            _ if again => Follow::Again,
            _ => Follow::Yes,
        }
    }
}

/// The list's state (its rows, their measured heights and where the reader
/// is), and what the reader chose.
pub(super) struct RowList {
    state: ListState,
    following: Rc<RefCell<Following>>,
    /// The marker last drawn, whose words stay while it fades.
    shown_marker: Cell<Option<Marker>>,
}

impl RowList {
    pub fn new() -> Self {
        let state = ListState::new(0, ListAlignment::Top, OVERDRAW).with_queued_moves();
        state.queue_move(ListMove::ToEnd);
        Self {
            state,
            following: Rc::new(RefCell::new(Following::new())),
            shown_marker: Cell::new(None),
        }
    }

    /// The list's state, for an element in a row that scrolls it.
    pub(super) fn handle(&self) -> ListState {
        self.state.clone()
    }

    /// Whether the reader chose to follow the end.
    pub fn is_following(&self) -> bool {
        self.following.borrow().follow
    }

    /// Whether the last frame followed the end.
    pub fn followed(&self) -> bool {
        self.following.borrow().followed
    }

    /// Whether the reader scrolled, or asked for the end or the start, since
    /// the last time this was asked.
    pub fn take_reader_moved(&self) -> bool {
        std::mem::take(&mut self.following.borrow_mut().reader_moved)
    }

    /// Whether moves wait for the next frame.
    pub fn moves_waiting(&self) -> bool {
        self.state.moves_waiting()
    }

    /// Whether the end showed in the last frame, with no device pixel row of
    /// it below the bottom edge.
    pub fn end_shows(&self) -> bool {
        self.state.report().end_shows
    }

    /// How far the end was below the bottom edge in the last frame, and
    /// below zero by the blank's height where blank showed below it.
    pub fn below(&self) -> Pixels {
        self.state.report().below
    }

    /// Starts over with `count` rows, following the end.
    pub fn reset(&self, count: usize) {
        self.state.reset(count);
        self.following.borrow_mut().follow = true;
        self.state.queue_move(ListMove::ToEnd);
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

    /// Measures the rows in `range` in the next frame, wherever they are.
    pub fn measure(&self, range: Range<usize>) {
        self.state.measure_items(range);
    }

    /// The height of row `ix`, where it was measured since it last changed.
    pub fn height(&self, ix: usize) -> Option<Pixels> {
        self.state.item_height(ix)
    }

    /// The row at the top of the screen, and how far into it the screen
    /// starts.
    pub fn top(&self) -> ListOffset {
        self.state.logical_scroll_top()
    }

    /// Puts the row `ix` at the top of the screen, `offset` into it, as the
    /// rows made again keep the reader's place. The choice stays.
    pub fn scroll_to(&self, ix: usize, offset: Pixels) {
        self.state.queue_move(ListMove::To(ListOffset {
            item_ix: ix,
            offset_in_item: offset,
        }));
    }

    /// The end on the bottom edge, following from there, as End, the marker
    /// and sending ask.
    pub fn scroll_to_end(&self) {
        self.state.queue_move(ListMove::ToEnd);
    }

    /// The start on the top edge, as Home asks. It reads where the lines
    /// moved.
    pub fn scroll_to_start(&self) {
        self.state.queue_move(ListMove::ToStart);
    }

    /// Follows from where the reader is where the end shows, and at the end
    /// where it doesn't, as Esc asks. It never closes the blank below the
    /// end, and what a taller view gives back stays (rule 7).
    pub fn follow_here(&self) {
        if self.end_shows() {
            self.following.borrow_mut().follow = true;
        } else {
            self.scroll_to_end();
        }
    }

    /// The reader's scroll by `distance`, down when positive, as Page Up and
    /// Page Down ask.
    pub fn scroll_by(&self, distance: Pixels) {
        self.state.queue_move(ListMove::Scroll {
            delta: distance,
            gesture: ScrollGesture::step(),
        });
    }

    /// Where blank shows below the end and rows are hidden above, the end
    /// comes down to the bottom edge, as after another view (rule 9).
    pub fn close_blank(&self) {
        self.state.queue_move(ListMove::CloseBlank);
    }

    /// Keeps row `ix`'s top `by` below where the last frame drew it, and
    /// every row above with it. A row the reader opened or closed keeps its
    /// line still this way while the space above it changes by `-by`.
    pub fn keep_top(&self, ix: usize, by: Pixels) {
        self.state.queue_move(ListMove::KeepTop { ix, by });
    }

    /// The left button went down at `at` in the transcript, and the view
    /// holds until its release.
    pub fn press(&self, at: Point<Pixels>) {
        let mut following = self.following.borrow_mut();
        let followed = following.followed;
        following.hold = Some(Hold {
            followed,
            at,
            dragged: false,
        });
    }

    /// The pointer moved to `at` with the button down.
    pub fn drag(&self, at: Point<Pixels>) {
        let mut following = self.following.borrow_mut();
        if let Some(hold) = following.hold.as_mut()
            && ((at.x - hold.at.x).abs() > DRAG || (at.y - hold.at.y).abs() > DRAG)
        {
            hold.dragged = true;
        }
    }

    /// The hold ends, as the button came up or the window lost the focus. A
    /// plain click by a follower, during which output took the end off the
    /// screen, goes to the end. A drag stays, so its selection stays in view.
    /// Returns whether the hold ended.
    pub fn release(&self) -> bool {
        let hold = self.following.borrow_mut().hold.take();
        let Some(hold) = hold else {
            return false;
        };
        let catch_up = hold.followed && !hold.dragged && self.is_following() && !self.end_shows();
        if catch_up {
            self.scroll_to_end();
        }
        true
    }

    /// Whether the end is off the screen (tests).
    #[cfg(test)]
    pub fn is_scrolled_up(&self) -> bool {
        !self.end_shows()
    }

    /// Puts the row `ix` at the top of the screen, `offset` into it, as a
    /// reader who scrolled there and reads (tests).
    #[cfg(test)]
    pub fn read_at(&self, ix: usize, offset: Pixels) {
        self.following.borrow_mut().follow = false;
        self.scroll_to(ix, offset);
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

    /// The row on screen at `position` in the window, as last laid out, in
    /// a window whose rem is `rem` wide. The list's bounds for a row leave
    /// out the space above the first one.
    pub fn row_at(&self, position: Point<Pixels>, rem: Pixels) -> Option<usize> {
        let position = position - point(px(0.), rem * PADDING);
        self.on_screen().into_iter().find(|&ix| {
            self.state
                .bounds_for_item(ix)
                .is_some_and(|bounds| bounds.contains(&position))
        })
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
    /// it is below (its top, if it's taller than the screen). A row that
    /// goes down onto the screen reads.
    pub fn reveal(&self, ix: usize) {
        self.state.queue_move(ListMove::Reveal(ix));
    }
}

/// What the marker of what is new below says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Marker {
    /// "↓ end", where the end is off the screen and nothing new came below.
    End,
    /// "↓ N new lines".
    New(usize),
}

impl Marker {
    pub fn words(self) -> String {
        match self {
            Marker::End => "end".into(),
            Marker::New(1) => "1 new line".into(),
            Marker::New(n) => format!("{n} new lines"),
        }
    }
}

/// Draws `list`, each row by `row`, with the marker while the end is off
/// the screen (`marker`), a click on which is End. The scrollbar counts the
/// rows not drawn yet at the heights `estimate` gives them at a width, if
/// given.
pub(super) fn render(
    id: &'static str,
    list: &RowList,
    marker: Option<Marker>,
    row: impl FnMut(usize, &mut Window, &mut App) -> AnyElement + 'static,
    estimate: Option<impl FnMut(usize, Pixels, &mut Window, &mut App) -> Pixels + 'static>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let state = list.state.clone();
    // (The view the list is in, which draws again when the marker is clicked.)
    let view = window.current_view();
    let visibility = transition(
        "transcript-jump-button",
        if marker.is_some() { 1. } else { 0. },
        Transition::new(JUMP_BUTTON_TRANSITION),
        window,
        cx,
    );
    let theme = cx.theme();
    let (border, background, foreground) = (theme.border, theme.background, theme.foreground);
    let radius = theme.radius_full();
    let following = list.following.clone();
    let rows = gpui_kit::list(state.clone(), row)
        .follow_end(move |frame, _, _| following.borrow_mut().decide(frame));
    let rows = match estimate {
        Some(estimate) => rows.estimate_item_heights(estimate),
        None => rows,
    };
    let rows = rows.size_full().min_h_0().py(rems(PADDING));
    // (The words stay while the marker fades, and a click on it then does
    // nothing.)
    if marker.is_some() {
        list.shown_marker.set(marker);
    }
    let words = marker.or(list.shown_marker.get()).map(Marker::words);
    let fading = marker.is_none();
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
        .when(visibility > 0. && words.is_some(), |this| {
            let words = words.unwrap_or_default();
            let jump = state.clone();
            this.child(
                div()
                    .absolute()
                    .left_0()
                    .right_0()
                    .bottom(rems(1.))
                    .flex()
                    .justify_center()
                    .opacity(visibility)
                    .child(
                        div()
                            .id("transcript-marker")
                            .test_support()
                            // A press on the marker holds nothing.
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .child(
                                Button::new("jump-to-latest")
                                    .secondary()
                                    .icon(IconName::ArrowDown)
                                    .label(words)
                                    .rounded(radius)
                                    .border_1()
                                    .border_color(border)
                                    .bg(background)
                                    .text_color(foreground)
                                    .disabled(fading)
                                    .on_click(move |_, _, cx| {
                                        jump.queue_move(ListMove::ToEnd);
                                        cx.notify(view);
                                    }),
                            ),
                    ),
            )
        })
        .into_any_element()
}
