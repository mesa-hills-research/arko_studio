//! The transcript's scrollbar while the reader scrolls and drags it, over
//! rows of very different heights that the list hasn't all drawn yet.

use arko::remote::{Delivery, Feed, Kind, Level, Source};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Entity, InputEvent as _, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, ScrollDelta, TestAppContext, point, px,
};

use super::tests::{call, item_ids, load_all, open, result, settle, start};
use super::{AgentFilter, TranscriptView};

pub(super) const WIDTH: f32 = 800.;
pub(super) const HEIGHT: f32 = 600.;
/// The kit's shortest thumb.
const MIN_THUMB: f32 = 48.;
/// How far one turn of the wheel scrolls.
const WHEEL: f32 = 120.;

/// The thumb's length and how far down the track it starts, as the kit
/// draws them from the list's state.
#[derive(Clone, Copy, Debug)]
pub(super) struct Thumb {
    pub(super) length: f32,
    pub(super) top: f32,
    pub(super) fraction: f32,
    pub(super) max: f32,
}

fn thumb(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> Thumb {
    cx.read(|cx| {
        let state = view.read(cx).list.handle();
        let track = f32::from(state.viewport_bounds().size.height);
        let max = f32::from(state.max_offset_for_scrollbar().y);
        let offset = -f32::from(state.scroll_px_offset_for_scrollbar().y);
        let content = track + max;
        let length = (track / content * track).clamp(MIN_THUMB, track);
        let fraction = if max > 0. {
            (offset / max).clamp(0., 1.)
        } else {
            0.
        };
        Thumb {
            length,
            top: fraction * (track - length),
            fraction,
            max,
        }
    })
}

/// A user's question, a few reads, a long shell output every third turn,
/// and a reply that is one line, prose, a list, or a code block of 30 or
/// 120 lines (shown capped at 40).
fn mixed_turns(feed: &Feed, count: usize) {
    let prose = "The list measures a row when it draws it, and estimates the rest. ".repeat(8);
    for turn in 0..count {
        feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        for file in 0..turn % 3 {
            let path = format!("src/f{turn}_{file}.rs");
            let id = call(feed, "main", "read_file", &[("path", path.into())]);
            result(feed, "main", id, "read_file", "1 | fn a() {}\n");
        }
        if turn % 3 == 1 {
            let id = call(feed, "main", "shell", &[("command", "cargo test".into())]);
            let output: String = (1..=60).map(|n| format!("test t{n} ... ok\n")).collect();
            result(feed, "main", id, "shell", &format!("exit code 0\n{output}"));
        }
        let reply = match turn % 5 {
            0 => "Done.".to_string(),
            1 => [prose.as_str(); 5].join("\n\n"),
            2 => {
                let code: String = (1..=30).map(|n| format!("let x{n} = {n};\n")).collect();
                format!("Here:\n\n```rust\n{code}```")
            }
            3 => {
                let code: String = (1..=120).map(|n| format!("let y{n} = {n};\n")).collect();
                format!("All of it:\n\n```rust\n{code}```")
            }
            _ => (1..=8).map(|n| format!("- Point {n}\n")).collect(),
        };
        feed.reply(&reply);
        feed.turn_ended("3 s · $0.01", None);
    }
}

/// A transcript of 60 mixed turns with every fourth call open, drawn at its
/// end.
fn transcript(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TranscriptView>) {
    start(cx);
    let feed = Feed::new("main");
    mixed_turns(&feed, 60);
    let (window, view) = open(cx, &feed, AgentFilter::Main, HEIGHT);
    load_all(cx, window, &view);
    let calls: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Call)
        .map(|(id, _)| id)
        .collect();
    view.update(cx, |view, cx| {
        for id in calls.iter().step_by(4) {
            view.set_expanded(*id, true, cx);
        }
        view.scroll_to_end(cx);
    });
    settle(cx, window);
    (window, view)
}

fn wheel(cx: &mut TestAppContext, window: AnyWindowHandle, dy: f32) {
    cx.update_window(window, |_, window, cx| {
        window.scroll("transcript", ScrollDelta::Pixels(point(px(0.), px(dy))), cx)
    })
    .unwrap();
}

fn pointer(cx: &mut TestAppContext, window: AnyWindowHandle, at: Point<f32>, pressed: bool) {
    cx.update_window(window, |_, window, cx| {
        let event = MouseMoveEvent {
            position: point(px(at.x), px(at.y)),
            pressed_button: pressed.then_some(MouseButton::Left),
            modifiers: Default::default(),
        };
        window.dispatch_event(event.to_platform_input(), cx);
        window.render_frame(cx);
    })
    .unwrap();
}

fn press(cx: &mut TestAppContext, window: AnyWindowHandle, at: Point<f32>, down: bool) {
    cx.update_window(window, |_, window, cx| {
        let position = point(px(at.x), px(at.y));
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
        window.render_frame(cx);
    })
    .unwrap();
}

/// How the thumb moved from frame to frame, scrolling one way.
#[derive(Debug, Default)]
struct Moves {
    /// The largest change of the thumb's length in a frame.
    resize: f32,
    /// Frames where the length changed by more than half a pixel.
    resizes: usize,
    /// The largest step of the thumb in a frame, and the median step.
    step: f32,
    median: f32,
    /// The largest step against the scrolling, and how many there were.
    backward: f32,
    backwards: usize,
}

fn moves(thumbs: &[Thumb], down: bool) -> Moves {
    let mut moves = Moves::default();
    let mut steps = Vec::new();
    for pair in thumbs.windows(2) {
        let resize = (pair[1].length - pair[0].length).abs();
        moves.resize = moves.resize.max(resize);
        moves.resizes += usize::from(resize > 0.5);
        let step = pair[1].top - pair[0].top;
        let forward = if down { step } else { -step };
        if forward < -0.5 {
            moves.backward = moves.backward.max(-forward);
            moves.backwards += 1;
        }
        moves.step = moves.step.max(step.abs());
        steps.push(step.abs());
    }
    steps.sort_by(f32::total_cmp);
    moves.median = steps.get(steps.len() / 2).copied().unwrap_or_default();
    moves
}

/// Turns the wheel until the thumb reaches the end of its track, and
/// returns the thumb after each turn.
fn scroll_through(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    down: bool,
) -> Vec<Thumb> {
    let mut thumbs = vec![thumb(cx, view)];
    for _ in 0..2000 {
        wheel(cx, window, if down { -WHEEL } else { WHEEL });
        let now = thumb(cx, view);
        thumbs.push(now);
        let end = if down { 1. } else { 0. };
        if now.fraction == end {
            break;
        }
    }
    thumbs
}

/// Numbers for a seeded transcript (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    /// A number in `range`.
    fn int(&mut self, range: std::ops::RangeInclusive<usize>) -> usize {
        range.start() + (self.next() % (range.end() - range.start() + 1) as u64) as usize
    }

    /// Whether a thing with `percent` chances in a hundred happens.
    fn chance(&mut self, percent: usize) -> bool {
        self.int(0..=99) < percent
    }

    /// A word of [`WORDS`].
    fn word(&mut self) -> &'static str {
        let count = WORDS.split(' ').count();
        WORDS
            .split(' ')
            .nth(self.int(0..=count - 1))
            .unwrap_or("word")
    }
}

/// The words of the transcript's prose, inline code and emphasis among them.
const WORDS: &str = "the list measures a row when it draws and estimates rest from its text parser keeps \
    block open while streams in `Parser::feed` returns blocks completed finished is never \
    parsed again so long answer costs what last configuration file `src/transcript/row.rs` \
    function None width changes scrollbar thumb moves evenly tests pass cargo build I will \
    check whether this right because we need to update **lock** file with every dependency \
    that of for on at be by an or if implementation worker session transcript message \
    window frame";

fn sentence(rng: &mut Rng) -> String {
    let count = rng.int(5..=22);
    let mut words: Vec<String> = (0..count).map(|_| rng.word().to_string()).collect();
    if let Some(first) = words.first_mut() {
        let mut chars = first.chars();
        if let Some(c) = chars.next() {
            *first = c.to_uppercase().chain(chars).collect();
        }
    }
    let mut sentence = words.join(" ");
    sentence.push(if rng.chance(10) { ':' } else { '.' });
    sentence
}

fn paragraph(rng: &mut Rng, sentences: std::ops::RangeInclusive<usize>) -> String {
    let count = rng.int(sentences);
    (0..count)
        .map(|_| sentence(rng))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A long conversation as agents have them, the same for a seed:
/// questions of a line, a paragraph or a few lines, reads and commands
/// (some opened), notices now and then, and replies of prose, headings,
/// lists, quotes, tables and code of many lengths.
fn realistic_turns(feed: &Feed, turns: usize, seed: u64) {
    let mut rng = Rng(seed);
    for turn in 0..turns {
        let question = match rng.int(0..=9) {
            0..=5 => sentence(&mut rng),
            6..=8 => paragraph(&mut rng, 2..=5),
            _ => (0..rng.int(2..=4))
                .map(|_| sentence(&mut rng))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        feed.user(&question, Source::App, Delivery::Turn);
        feed.turn_started();
        for file in 0..rng.int(0..=4) {
            let (tool, output) = if rng.chance(40) {
                let lines = rng.int(3..=90);
                let output: String = (1..=lines)
                    .map(|n| format!("test transcript::case_{turn}_{n} ... ok\n"))
                    .collect();
                ("shell", format!("exit code 0\n{output}"))
            } else {
                ("read_file", "1 | fn a() {}\n".to_string())
            };
            let parameter = if tool == "shell" {
                ("command", "cargo test --lib".into())
            } else {
                ("path", format!("src/f{turn}_{file}.rs").into())
            };
            let id = super::tests::call(feed, "main", tool, &[parameter]);
            super::tests::result(feed, "main", id, tool, &output);
        }
        if rng.chance(5) {
            feed.notice(Level::Warn, &sentence(&mut rng));
        }
        let mut reply = Vec::new();
        for _ in 0..rng.int(1..=6) {
            reply.push(match rng.int(0..=19) {
                0..=10 => paragraph(&mut rng, 1..=6),
                11 => format!("## {}", sentence(&mut rng)),
                12..=14 => (0..rng.int(2..=8))
                    .map(|_| format!("- {}", paragraph(&mut rng, 1..=2)))
                    .collect::<Vec<_>>()
                    .join("\n"),
                15..=17 => {
                    let lines = rng.int(3..=120);
                    let code: String = (1..=lines)
                        .map(|n| format!("    let value_{n} = self.pending.drain(..{n});\n"))
                        .collect();
                    format!("```rust\n{code}```")
                }
                18 => format!("> {}", paragraph(&mut rng, 1..=3)),
                _ => {
                    let rows: String = (0..rng.int(2..=6))
                        .map(|n| format!("| row {n} | {} |\n", sentence(&mut rng)))
                        .collect();
                    format!("| name | what |\n|---|---|\n{rows}")
                }
            });
        }
        feed.reply(&reply.join("\n\n"));
        feed.turn_ended("12 s · $0.034", None);
    }
}

/// How evenly the thumb moved over a leg, as GPUI's scroll harness measures
/// it. Of the thumb's travel per wheel step, it takes the coefficient of
/// variation (the standard deviation over the mean) and the largest step
/// over the median step. The last step stops at the end of the track, short
/// of a full step, and doesn't count.
#[derive(Debug, Clone, Copy)]
struct Evenness {
    cv: f32,
    spike: f32,
    steps: usize,
}

fn evenness(thumbs: &[Thumb], down: bool) -> Evenness {
    let direction = if down { 1. } else { -1. };
    let mut travel: Vec<f32> = thumbs
        .windows(2)
        .map(|pair| (pair[1].top - pair[0].top) * direction)
        .collect();
    travel.pop();
    let mean = travel.iter().sum::<f32>() / travel.len().max(1) as f32;
    let sd = (travel.iter().map(|t| (t - mean).powi(2)).sum::<f32>() / travel.len().max(1) as f32)
        .sqrt();
    let mut sorted = travel.clone();
    sorted.sort_by(f32::total_cmp);
    let median = sorted
        .get(((sorted.len().max(1) - 1) as f32 * 0.5).round() as usize)
        .copied()
        .unwrap_or_default();
    let max = sorted.last().copied().unwrap_or_default();
    Evenness {
        cv: if mean > 0. { sd / mean } else { 0. },
        spike: if median > 0. { max / median } else { 0. },
        steps: travel.len(),
    }
}

/// The thumb as the kit draws it from `state`.
pub(super) fn thumb_of(state: &gpui_kit::ListState) -> Thumb {
    let track = f32::from(state.viewport_bounds().size.height);
    let max = f32::from(state.max_offset_for_scrollbar().y);
    let offset = -f32::from(state.scroll_px_offset_for_scrollbar().y);
    let content = track + max;
    let length = (track / content * track).clamp(MIN_THUMB, track);
    let fraction = if max > 0. {
        (offset / max).clamp(0., 1.)
    } else {
        0.
    };
    Thumb {
        length,
        top: fraction * (track - length),
        fraction,
        max,
    }
}

/// Opens a realistic transcript of 40 turns, laid out with the app's fonts,
/// at its end, and turns the wheel up to its start. Returns how evenly the
/// thumb moved.
#[cfg(target_os = "linux")]
fn scroll_up_a_long_transcript(estimates: bool) -> Evenness {
    use super::height_tests::real;
    let feed = Feed::new("main");
    realistic_turns(&feed, 40, 7);
    let mut cx = real::app();
    let (window, view) = real::open(&mut cx, &feed, (WIDTH, HEIGHT), estimates);
    let calls: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Call)
        .map(|(id, _)| id)
        .collect();
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            for id in calls.iter().step_by(7) {
                view.set_expanded(*id, true, cx);
            }
            view.scroll_to_end(cx);
        })
    });
    real::settle(&mut cx, window);
    let read = |cx: &mut gpui_kit::HeadlessAppContext| {
        cx.update(|cx| thumb_of(&view.read(cx).list.handle()))
    };
    let mut thumbs = vec![read(&mut cx)];
    for _ in 0..4000 {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                gpui_kit::ScrollWheelEvent {
                    position: point(px(WIDTH / 2.), px(HEIGHT / 2.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(WHEEL))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
        real::frame(&mut cx, window);
        let now = read(&mut cx);
        thumbs.push(now);
        if now.fraction == 0. {
            break;
        }
    }
    evenness(&thumbs, false)
}

#[cfg(target_os = "linux")]
#[test]
fn the_thumb_moves_evenly_up_a_long_transcript_it_has_not_drawn() {
    let before = scroll_up_a_long_transcript(false);
    let after = scroll_up_a_long_transcript(true);
    eprintln!("without estimates: {before:?}\nwith estimates: {after:?}");
    assert!(after.steps > 100, "{after:?}");
    assert!(after.cv < 0.02, "{after:?}");
    assert!(after.spike < 1.1, "{after:?}");
}

#[gpui_kit::test]
fn the_thumb_keeps_its_size_and_moves_without_jumps_through_rows_of_mixed_heights(
    cx: &mut TestAppContext,
) {
    let (window, view) = transcript(cx);
    let up = scroll_through(cx, window, &view, false);
    let down = scroll_through(cx, window, &view, true);
    let (up_moves, down_moves) = (moves(&up, false), moves(&down, true));
    eprintln!("up: {up_moves:?}\ndown: {down_moves:?}");
    for (thumbs, moves) in [(&up, up_moves), (&down, down_moves)] {
        let end = thumbs.last().unwrap();
        assert!(end.fraction == 0. || end.fraction == 1., "{end:?}");
        assert_eq!(moves.resizes, 0, "{moves:?}");
        assert_eq!(moves.backwards, 0, "{moves:?}");
        assert!(moves.step <= moves.median * 2. + 1., "{moves:?}");
    }
}

#[gpui_kit::test]
fn the_thumb_stays_under_the_pointer_while_it_is_dragged(cx: &mut TestAppContext) {
    let (window, view) = transcript(cx);
    // A turn of the wheel shows the scrollbar.
    wheel(cx, window, WHEEL);
    let x = WIDTH - 6.;
    let start = thumb(cx, &view);
    let grab = point(x, start.top + start.length / 2.);
    pointer(cx, window, grab, false);
    press(cx, window, grab, true);
    let dragging = cx.read(|cx| view.read(cx).list.handle().is_scrollbar_dragging());
    assert!(dragging, "the press didn't start a drag");
    let held = thumb(cx, &view);
    // Up the track a few pixels a frame, nearly to its top, then back.
    let mut drift: f32 = 0.;
    let steps = (held.top / 6.) as usize;
    for step in (1..steps).chain((1..steps).rev()) {
        let y = grab.y - step as f32 * 6.;
        pointer(cx, window, point(x, y), true);
        let now = thumb(cx, &view);
        assert_eq!(now.max, held.max, "the track's scale changed mid-drag");
        // Where the thumb is, against where the pointer put it.
        drift = drift.max((now.top - held.top - (y - grab.y)).abs());
    }
    press(cx, window, grab, false);
    eprintln!("drift under the pointer: {drift} px");
    assert!(drift <= 0.5, "the thumb moved {drift} px under the pointer");
}

#[gpui_kit::test]
fn dragging_the_thumb_past_the_end_of_its_track_shows_the_last_row(cx: &mut TestAppContext) {
    let (window, view) = transcript(cx);
    wheel(cx, window, WHEEL);
    let x = WIDTH - 6.;
    // To the top first, so the rows between are left unmeasured.
    let at = thumb(cx, &view);
    let grab = point(x, at.top + at.length / 2.);
    pointer(cx, window, grab, false);
    press(cx, window, grab, true);
    pointer(cx, window, point(x, at.length / 2. - 20.), true);
    press(cx, window, point(x, at.length / 2. - 20.), false);
    settle(cx, window);
    assert_eq!(thumb(cx, &view).fraction, 0.);
    // Then down past the bottom of the track, a step a frame.
    wheel(cx, window, -1.);
    let at = thumb(cx, &view);
    let grab = point(x, at.top + at.length / 2.);
    pointer(cx, window, grab, false);
    press(cx, window, grab, true);
    let mut y = grab.y;
    while y < HEIGHT + 40. {
        y += 37.;
        pointer(cx, window, point(x, y), true);
    }
    press(cx, window, point(x, y), false);
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    let end = thumb(cx, &view);
    assert_eq!(end.fraction, 1., "{end:?}");
    let last = *super::tests::keys(&view, cx).last().unwrap();
    let bottom = cx
        .update_window(window, |_, window, _| {
            window
                .try_find(super::row::element_id(last))
                .filter(|row| row.visible())
                .map(|row| f32::from(row.bounds().bottom()))
        })
        .unwrap();
    assert!(bottom.is_some(), "the last row isn't on screen");
}
