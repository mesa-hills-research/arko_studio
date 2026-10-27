//! The transcript's place and its scrollbar's thumb while a reply streams
//! in: below a reader who has scrolled away from the end, and at the end,
//! where the transcript follows it. The reply is a code block, a table, a
//! list or a quote, which grow by a line for every few deltas, in the
//! three views and over a transcript of thousands of rows.
//!
//! A reader's place is one row, the question the reply answers, that
//! nothing streams into, so the only thing that may move it on the screen
//! is the reader. The tests draw with the app's fonts and estimate the
//! heights of rows not drawn, as the app does.

use arko::remote::{Delivery, Feed, Source};
use gpui_kit::{
    AnyWindowHandle, Entity, HeadlessAppContext, InputEvent as _, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ScrollDelta, point, px, size,
};

use super::TranscriptView;
use super::height::BlockFacts;
use super::height_tests::real;
use super::model::RowKey;
use super::scrollbar_tests::{HEIGHT, Thumb, WIDTH, thumb_of};
use super::tests::{call, result, say};
use crate::prefs::{self, TranscriptView as View};

/// What the transcript showed after a step.
#[derive(Clone, Debug)]
struct Frame {
    /// The row at the top of the screen, and how far into it the screen
    /// starts.
    item: usize,
    offset: f32,
    top: Option<RowKey>,
    following: bool,
    scrolled_up: bool,
    thumb: Thumb,
    rows: usize,
    /// Where the reader's row is on the screen, if it is not above it.
    anchor_y: Option<f32>,
    /// How far the bottom of the last row is below the bottom of the screen.
    end_gap: Option<f32>,
    /// The turn of the wheel the step was, up when positive.
    wheel: Option<f32>,
}

struct Scene {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    view: Entity<TranscriptView>,
    feed: Feed,
    anchor: usize,
    frames: Vec<Frame>,
    /// Where the reader's row should be after each frame, from the turns
    /// of the wheel alone.
    expected: Vec<Option<f32>>,
}

impl Scene {
    /// A transcript of `turns` questions and short answers and a question
    /// the reply to which is about to stream, drawn at its end.
    fn new(turns: usize, view: View) -> Self {
        Self::sized(turns, view, WIDTH)
    }

    /// [`Self::new`] in a window `width` wide.
    fn sized(turns: usize, view: View, width: f32) -> Self {
        let mut cx = real::app();
        cx.update(|cx| prefs::update(cx, |prefs| prefs.view = view));
        let feed = Feed::new("main");
        let prose = "The list measures a row when it draws it, and estimates the rest. ".repeat(8);
        for turn in 0..turns {
            feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
            feed.turn_started();
            let path = format!("src/f{turn}.rs");
            let call = call(&feed, "main", "read_file", &[("path", path.into())]);
            result(&feed, "main", call, "read_file", "1 | fn a() {}\n");
            feed.reply(&prose);
            feed.turn_ended("3 s · $0.01", None);
        }
        feed.user("Show me", Source::App, Delivery::Turn);
        feed.turn_started();
        let (window, view) = real::open(&mut cx, &feed, (width, HEIGHT), true);
        cx.update(|cx| view.update(cx, |view, cx| view.scroll_to_end(cx)));
        real::settle(&mut cx, window);
        let anchor = cx.update(|cx| {
            view.read(cx)
                .rows
                .iter()
                .rposition(|row| matches!(row.key, RowKey::User(_)))
                .expect("the question has a row")
        });
        let mut scene = Self {
            cx,
            window,
            view,
            feed,
            anchor,
            frames: Vec::new(),
            expected: Vec::new(),
        };
        scene.record(None);
        scene
    }

    fn record(&mut self, wheel: Option<f32>) {
        let anchor = self.anchor;
        let frame = self.cx.update(|cx| {
            let view = self.view.read(cx);
            let state = view.list.handle();
            let top = state.logical_scroll_top();
            let viewport = state.viewport_bounds();
            let last = view.rows.len() - 1;
            Frame {
                item: top.item_ix,
                offset: f32::from(top.offset_in_item),
                top: view.rows.get(top.item_ix).map(|row| row.key),
                following: view.list.is_following(),
                scrolled_up: view.list.is_scrolled_up(),
                thumb: thumb_of(&state),
                rows: view.rows.len(),
                anchor_y: state
                    .bounds_for_item(anchor)
                    .map(|bounds| f32::from(bounds.top() - viewport.top())),
                end_gap: state
                    .bounds_for_item(last)
                    .map(|bounds| f32::from(bounds.bottom() - viewport.bottom())),
                wheel,
            }
        });
        let expected = match (self.frames.last(), self.expected.last().copied().flatten()) {
            (Some(before), Some(expected)) if !before.following => {
                Some(expected + wheel.unwrap_or(0.))
            }
            _ => frame.anchor_y,
        };
        self.expected.push(expected);
        self.frames.push(frame);
    }

    /// Takes the place the reader's row is at now as the one to hold, as
    /// after the window's size changed.
    fn rebase(&mut self) {
        if let (Some(expected), Some(frame)) = (self.expected.last_mut(), self.frames.last()) {
            *expected = frame.anchor_y;
        }
    }

    /// Draws a frame, after letting through what is waiting: the parses of
    /// the text that came.
    fn frame(&mut self) {
        self.cx.run_until_parked();
        real::frame(&mut self.cx, self.window);
        self.record(None);
    }

    /// Text of the reply comes, and a frame draws it.
    fn delta(&mut self, text: &str) {
        say(&self.feed, "main", text);
        self.frame();
    }

    /// Lets through what waits and stops before the frame, as the platform
    /// does between two frames, when the text came in the last one has been
    /// parsed and its rows counted anew.
    fn between_frames(&mut self) {
        self.cx.run_until_parked();
        self.record(None);
    }

    /// A turn of the wheel over the transcript, up when `dy` is positive,
    /// and the frame after it unless `draw` says not.
    fn wheel(&mut self, dy: f32, draw: bool) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    gpui_kit::ScrollWheelEvent {
                        position: point(px(WIDTH / 2.), px(HEIGHT / 2.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        if draw {
            self.cx.run_until_parked();
            real::frame(&mut self.cx, self.window);
        }
        self.record(Some(dy));
    }

    fn resize(&mut self, width: f32, height: f32) {
        self.cx
            .update_window(self.window, |_, window, _| {
                window.resize(size(px(width), px(height)));
            })
            .unwrap();
        real::settle(&mut self.cx, self.window);
        self.record(None);
    }

    fn pointer(&mut self, x: f32, y: f32, pressed: bool) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let event = MouseMoveEvent {
                    position: point(px(x), px(y)),
                    pressed_button: pressed.then_some(MouseButton::Left),
                    modifiers: Default::default(),
                };
                window.dispatch_event(event.to_platform_input(), cx);
                window.draw(cx).clear(cx);
            })
            .unwrap();
        self.record(None);
    }

    fn press(&mut self, x: f32, y: f32, down: bool) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                let position = point(px(x), px(y));
                let event = if down {
                    MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input()
                } else {
                    MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Default::default(),
                        click_count: 1,
                    }
                    .to_platform_input()
                };
                window.dispatch_event(event, cx);
                window.draw(cx).clear(cx);
            })
            .unwrap();
        self.record(None);
    }

    /// How far the last row's bottom is from the screen's when the
    /// transcript follows the end, which the list's padding makes.
    fn pinned_gap(&self) -> f32 {
        self.frames[0]
            .end_gap
            .expect("the last row shows at the start")
    }

    /// Whether `frame` follows the end with the last row at the bottom.
    fn is_pinned(&self, frame: &Frame) -> bool {
        frame.following
            && !frame.scrolled_up
            && frame
                .end_gap
                .is_some_and(|gap| (gap - self.pinned_gap()).abs() < 1.)
    }

    /// The largest distance, from the frame `from` on, between where the
    /// reader's row is on the screen and where the reader's wheel put it,
    /// over the frames that don't follow the end.
    fn view_jump(&self, from: usize) -> f32 {
        (from..self.frames.len())
            .filter(|&n| !self.frames[n].following)
            .filter_map(|n| Some((self.frames[n].anchor_y? - self.expected[n]?).abs()))
            .fold(0., f32::max)
    }

    /// The largest step of the thumb from the frame `from` on, beyond what
    /// the turn of the wheel in the step explains (a thumb moves by the
    /// share of its track that the turn is of the scroll range), and the
    /// frame it was in.
    fn thumb_jump(&self, from: usize) -> (f32, usize) {
        let mut worst = (0f32, from);
        for n in from.max(1)..self.frames.len() {
            let (before, now) = (&self.frames[n - 1], &self.frames[n]);
            // Reaching the end puts the thumb at the end of its track, and
            // a row that comes below the reader makes the content longer.
            if now.following != before.following || now.rows != before.rows {
                continue;
            }
            let track = HEIGHT - now.thumb.length;
            let allowed = 1.
                + now
                    .wheel
                    .map_or(0., |dy| dy.abs() / now.thumb.max.max(1.) * track * 2.);
            let excess = (now.thumb.top - before.thumb.top).abs() - allowed;
            if excess > worst.0 {
                worst = (excess, n);
            }
        }
        worst
    }

    /// The frames around `n`, for a failure to show.
    fn show(&self, n: usize) -> String {
        (n.saturating_sub(3)..=n.min(self.frames.len() - 1))
            .map(|k| {
                let f = &self.frames[k];
                format!(
                    "  {k}: top {}+{:.1} following {} thumb {:.1} wheel {:?} reader's row {:?} (should be {:?})",
                    f.item, f.offset, f.following, f.thumb.top, f.wheel, f.anchor_y, self.expected[k]
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// A block of the reply that grows a line for every few deltas.
#[derive(Clone, Copy, Debug)]
enum Block {
    Code,
    Table,
    List,
    Quote,
}

/// The reply's text in the deltas a model sends it in: a paragraph first,
/// then the block, a line to two deltas, and a closing paragraph.
fn reply(block: Block, lines: usize) -> Vec<String> {
    let mut deltas = vec![
        "Here is the change.\n\n".to_string(),
        "Some more words first, ".to_string(),
    ];
    match block {
        Block::Code => deltas.push("and then the code.\n\n```rust\n".into()),
        Block::Table => deltas.extend([
            "and then the table.\n\n".to_string(),
            "| name | what |\n".to_string(),
            "|---|---|\n".to_string(),
        ]),
        Block::List => deltas.push("and then the list.\n\n".into()),
        Block::Quote => deltas.push("and then the quote.\n\n".into()),
    }
    for n in 1..=lines {
        let line = match block {
            Block::Code => format!("    let value_{n} = self.pending.drain(..{n});\n"),
            Block::Table => format!("| row {n} | what this row says about it, a few words |\n"),
            Block::List => {
                format!(
                    "- Point {n}, which says enough to run to a second line in a narrow column\n"
                )
            }
            Block::Quote => {
                format!("> Line {n} of a quote that goes on to a second line in a narrow column\n")
            }
        };
        let cut = line.len() / 3;
        deltas.push(line[..cut].to_string());
        deltas.push(line[cut..].to_string());
    }
    match block {
        Block::Code => deltas.push("```\n\nThat is all.".into()),
        _ => deltas.push("\nThat is all.".into()),
    }
    deltas
}

/// A seeded sequence of numbers (xorshift).
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// The reply streams in, the reader scrolls up from the end, and then
/// steps of every kind follow in a seeded order: deltas, frames, the
/// moments between frames, and turns of the wheel in both directions, some
/// of them arriving before the frame. Returns the scene and the frame the
/// reader scrolled away at.
fn read_while_streaming(scene: &mut Scene, block: Block, seed: u64) -> usize {
    let deltas = reply(block, 60);
    let mut dice = Dice(seed * 104_729 + 17);
    let start = 10 + (dice.next() % 30) as usize;
    let mut next = 0;
    while next < start {
        scene.delta(&deltas[next]);
        next += 1;
    }
    scene.wheel(150. + (dice.next() % 900) as f32, true);
    let from = scene.frames.len();
    let mut steps = 0;
    while next < deltas.len() && steps < 500 {
        steps += 1;
        match dice.next() % 16 {
            0..=4 => {
                scene.delta(&deltas[next]);
                next += 1;
            }
            5..=6 => {
                // The delta reaches the transcript in the next frame, which
                // follows the next step.
                say(&scene.feed, "main", &deltas[next]);
                next += 1;
            }
            7 => scene.frame(),
            8 => scene.between_frames(),
            9 => scene.wheel(-20., true),
            10 => scene.wheel(-60., true),
            11 => scene.wheel(-120., true),
            12 => scene.wheel(-40., false),
            13 => scene.wheel(25., false),
            14 => scene.wheel(25., true),
            _ => scene.wheel(80., true),
        }
    }
    from
}

fn assert_holds(label: &str, scene: &Scene, from: usize) {
    let view = scene.view_jump(from);
    let (thumb, at) = scene.thumb_jump(from);
    eprintln!(
        "{label}: view jump {view:.2} px, thumb step beyond the wheel's {thumb:.2} px, {} frames",
        scene.frames.len() - from
    );
    assert!(view < 1., "{label}: the view jumped by {view} px");
    assert!(
        thumb < 1.,
        "{label}: the thumb jumped by {thumb} px\n{}",
        scene.show(at)
    );
}

#[test]
fn a_reader_above_a_streaming_block_keeps_the_view_and_the_thumb() {
    for block in [Block::Code, Block::Table, Block::List, Block::Quote] {
        for seed in [3, 11] {
            let mut scene = Scene::new(30, View::Full);
            let from = read_while_streaming(&mut scene, block, seed);
            assert_holds(&format!("{block:?} {seed}"), &scene, from);
        }
    }
}

#[test]
fn the_quiet_and_messages_views_keep_the_view_and_the_thumb_too() {
    for view in [View::Quiet, View::Messages] {
        for block in [Block::Code, Block::Table] {
            for seed in [3, 11] {
                let mut scene = Scene::new(30, view);
                let from = read_while_streaming(&mut scene, block, seed);
                assert_holds(&format!("{view:?} {block:?} {seed}"), &scene, from);
            }
        }
    }
}

#[test]
fn a_transcript_of_thousands_of_rows_keeps_the_view_and_the_thumb() {
    for block in [Block::Code, Block::Table] {
        let mut scene = Scene::new(1500, View::Full);
        let rows = scene.cx.update(|cx| scene.view.read(cx).rows.len());
        assert!(rows > 3000, "{rows} rows");
        let from = read_while_streaming(&mut scene, block, 3);
        assert_holds(&format!("{rows} rows {block:?}"), &scene, from);
    }
}

#[test]
fn at_the_end_the_transcript_follows_a_streaming_block() {
    for block in [Block::Code, Block::Table, Block::List, Block::Quote] {
        let mut scene = Scene::new(30, View::Full);
        for (n, delta) in reply(block, 60).iter().enumerate() {
            scene.delta(delta);
            if n % 3 == 0 {
                scene.between_frames();
            }
            let frame = scene.frames.last().unwrap();
            assert!(
                scene.is_pinned(frame),
                "{block:?}: not at the end at delta {n}\n{}",
                scene.show(scene.frames.len() - 1)
            );
        }
    }
}

#[test]
fn scrolling_back_to_the_end_follows_the_stream_again() {
    for block in [Block::Code, Block::Table] {
        let mut scene = Scene::new(30, View::Full);
        let deltas = reply(block, 60);
        let mut next = 0;
        for _ in 0..30 {
            scene.delta(&deltas[next]);
            next += 1;
        }
        scene.wheel(700., true);
        assert!(!scene.frames.last().unwrap().following);
        assert!(scene.frames.last().unwrap().scrolled_up);
        // The reader scrolls down a turn a delta, to the end and on.
        let from = scene.frames.len();
        let mut turns = 0;
        while !scene.frames.last().unwrap().following && turns < 100 {
            scene.delta(&deltas[next]);
            next += 1;
            scene.wheel(-60., true);
            turns += 1;
        }
        let last = scene.frames.last().unwrap();
        assert!(
            last.following,
            "{block:?}: still not at the end after {turns} turns"
        );
        assert!(!last.scrolled_up, "{block:?}");
        // On the way down the row moved up the screen and never back down.
        let anchors: Vec<f32> = scene.frames[from..]
            .iter()
            .filter(|frame| !frame.following)
            .filter_map(|frame| frame.anchor_y)
            .collect();
        for pair in anchors.windows(2) {
            assert!(
                pair[1] <= pair[0] + 0.5,
                "{block:?}: the view went back up {pair:?}"
            );
        }
        assert!(scene.view_jump(from) < 1., "{block:?}");
        // And from there it follows.
        for delta in &deltas[next..next + 20] {
            scene.delta(delta);
            let frame = scene.frames.last().unwrap();
            assert!(
                scene.is_pinned(frame),
                "{block:?}: not at the end\n{}",
                scene.show(scene.frames.len() - 1)
            );
        }
    }
}

#[test]
fn a_window_resized_while_a_block_streams_keeps_the_readers_row() {
    for block in [Block::Code, Block::Table] {
        let mut scene = Scene::new(30, View::Full);
        let deltas = reply(block, 60);
        let mut next = 0;
        for _ in 0..30 {
            scene.delta(&deltas[next]);
            next += 1;
        }
        scene.wheel(800., true);
        for (width, height) in [(600., 500.), (900., 700.), (700., 600.)] {
            let before = scene.frames.last().unwrap().top;
            scene.resize(width, height);
            scene.rebase();
            let now = scene.frames.last().unwrap();
            assert!(now.scrolled_up && !now.following, "{block:?} at {width}");
            assert_eq!(
                now.top, before,
                "{block:?}: another row at the top at {width}"
            );
            // The stream goes on, and the reader's place with it.
            let from = scene.frames.len();
            for _ in 0..12 {
                scene.delta(&deltas[next]);
                next += 1;
                scene.between_frames();
            }
            assert_holds(&format!("{block:?} at {width}"), &scene, from);
        }
    }
}

#[test]
fn dragging_the_thumb_down_while_a_block_streams_moves_the_view_down() {
    for block in [Block::Code, Block::Table] {
        let mut scene = Scene::new(30, View::Full);
        let deltas = reply(block, 60);
        let mut next = 0;
        for _ in 0..30 {
            scene.delta(&deltas[next]);
            next += 1;
        }
        scene.wheel(1500., true);
        let at = scene.frames.last().unwrap().clone();
        let x = WIDTH - 6.;
        let grab = point(x, at.thumb.top + at.thumb.length / 2.);
        scene.pointer(grab.x, grab.y, false);
        scene.press(grab.x, grab.y, true);
        let from = scene.frames.len();
        let held = scene.frames.last().unwrap().thumb;
        let mut y = grab.y;
        let mut drift = 0f32;
        // Down to the end of the thumb's track.
        let travel = HEIGHT - held.length - held.top;
        while y < grab.y + travel - 1. && next < deltas.len() {
            scene.delta(&deltas[next]);
            next += 1;
            scene.between_frames();
            y = (y + 3.).min(grab.y + travel);
            scene.pointer(x, y, true);
            let thumb = scene.frames.last().unwrap().thumb;
            assert_eq!(
                thumb.max, held.max,
                "{block:?}: the track's scale changed mid-drag"
            );
            drift = drift.max((thumb.top - held.top - (y - grab.y)).abs());
        }
        scene.press(x, y, false);
        let places: Vec<(usize, f32)> = scene.frames[from..]
            .iter()
            .map(|frame| (frame.item, frame.offset))
            .collect();
        for pair in places.windows(2) {
            let back =
                pair[1].0 < pair[0].0 || (pair[1].0 == pair[0].0 && pair[1].1 < pair[0].1 - 0.5);
            assert!(!back, "{block:?}: the view went back up {pair:?}");
        }
        assert!(
            drift <= 1.,
            "{block:?}: the thumb slid {drift} px from the pointer"
        );
        assert!(
            places.len() > 20,
            "{block:?}: the drag was too short to tell"
        );
    }
}

/// A table of the kind a model writes, wide enough for its cells to wrap
/// in a narrow window, and the text before it.
fn table_reply() -> String {
    let mut text = String::from(
        "Here are the options.\n\n| Option | What it does | When to use it |\n|---|---|---|\n",
    );
    for (option, what, when) in [
        (
            "`--release`",
            "Builds with optimizations, which takes longer and runs faster",
            "Benchmarks and anything you ship",
        ),
        ("`--locked`", "Fails if the lock file would change", "CI"),
        (
            "`--offline`",
            "Uses only crates already downloaded, and fails on anything it would have to fetch from the network",
            "On a plane",
        ),
        (
            "`-p NAME`",
            "Builds one package of the workspace",
            "When the workspace is large and you only touched one crate of it",
        ),
        (
            "`-j N`",
            "Runs at most N jobs at once",
            "When the machine is shared with other builds that need memory",
        ),
    ] {
        text.push_str(&format!("| {option} | {what} | {when} |\n"));
    }
    text
}

impl Scene {
    /// Where the reply's table starts on the screen and how tall it is, if
    /// it shows.
    fn table(&mut self) -> Option<(f32, f32)> {
        self.cx.update(|cx| {
            let view = self.view.read(cx);
            let state = view.list.handle();
            let viewport = state.viewport_bounds();
            view.rows.iter().enumerate().find_map(|(ix, row)| {
                let RowKey::Block { id, block, .. } = row.key else {
                    return None;
                };
                let doc = view.ui.get(&id)?.doc.as_ref()?;
                matches!(doc.blocks.get(block)?.facts, BlockFacts::Table(_))
                    .then(|| state.bounds_for_item(ix))
                    .flatten()
                    .map(|bounds| {
                        (
                            f32::from(bounds.top() - viewport.top()),
                            f32::from(bounds.size.height),
                        )
                    })
            })
        })
    }
}

/// At the end, a table that streams in a few characters at a time grows a
/// whole row at a time and never moves down the screen, also where its
/// cells wrap and its columns widen as rows come.
#[test]
fn at_the_end_a_streaming_table_never_moves_down() {
    for width in [WIDTH, 560.] {
        let mut scene = Scene::sized(30, View::Full, width);
        let text: Vec<char> = table_reply().chars().collect();
        let mut tops = Vec::new();
        for chunk in text.chunks(4) {
            scene.delta(&chunk.iter().collect::<String>());
            let frame = scene.frames.last().unwrap();
            assert!(scene.is_pinned(frame), "at {width}: not at the end");
            tops.extend(scene.table().map(|(top, _)| top));
        }
        assert!(tops.len() > 100, "at {width}: the table showed late");
        for pair in tops.windows(2) {
            assert!(
                pair[1] <= pair[0],
                "at {width}: the table moved down from {} to {}",
                pair[0],
                pair[1]
            );
        }
    }
}

/// A table row the turn ended in the middle of shows then.
#[test]
fn a_table_row_cut_off_by_the_end_of_the_turn_shows() {
    let mut scene = Scene::new(30, View::Full);
    for delta in [
        "Options:\n\n",
        "| Option | What it does |\n|---|---|\n",
        "| `-p` | One package |\n",
        "| `-j N` | Runs at",
    ] {
        scene.delta(delta);
    }
    let (_, cut) = scene.table().expect("the table shows");
    scene.feed.turn_ended("3 s · $0.01", None);
    scene.frame();
    let (_, ended) = scene.table().expect("the table shows");
    assert!(ended > cut, "the last row didn't show: {cut} then {ended}");
}
