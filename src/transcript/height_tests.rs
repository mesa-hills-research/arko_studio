//! The rows' estimated heights against the heights the list measures, and
//! what the list counts for rows that change while it doesn't draw them.

use arko::images::ImageRef;
use arko::remote::{Delivery, Feed, Kind, Level, Source, WorkerChange};
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext, px};

use super::height::BlockFacts;
use super::model::RowKey;
use super::tests::{call, item_ids, load_all, open, result, say, settle, start, think};
use super::{AgentFilter, TranscriptView};
use crate::prefs;

/// An image the feed only names. Its tile has the same size either way.
fn image(id: u64) -> ImageRef {
    ImageRef {
        id,
        mime: "image/png".into(),
        label: format!("screenshot-{id}.png"),
        bytes: 1000,
    }
}

/// A transcript with a row of every kind, and prose and code of many
/// lengths.
pub(super) fn every_kind(feed: &Feed) {
    let words = "The list measures a row when it draws it, and estimates the rest from its text. ";
    feed.conversation("New conversation");
    feed.user("Short question", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.reply("Done.");
    feed.turn_ended("3 s · $0.01", None);
    feed.user(&words.repeat(6), Source::App, Delivery::Turn);
    feed.turn_started();
    think(feed, &words.repeat(4));
    feed.worker("r1", "Write the fuzz target", WorkerChange::Started);
    feed.worker(
        "r1",
        "Write the fuzz target",
        WorkerChange::Finished { report: "Done." },
    );
    let id = call(feed, "main", "read_file", &[("path", "src/a.rs".into())]);
    result(feed, "main", id, "read_file", "1 | fn a() {}\n");
    let id = call(feed, "main", "shell", &[("command", "cargo test".into())]);
    let output: String = (1..=60).map(|n| format!("test t{n} ... ok\n")).collect();
    result(feed, "main", id, "shell", &format!("exit code 0\n{output}"));
    let id = call(feed, "main", "shell", &[("command", "ls".into())]);
    result(feed, "main", id, "shell", "exit code 0\na\nb\nc\n");
    let id = call(
        feed,
        "main",
        "edit_file",
        &[
            ("path", "src/a.rs".into()),
            ("old_string", "fn a() {}\n".into()),
            ("new_string", "fn a() {\n    b();\n}\n".into()),
        ],
    );
    result(
        feed,
        "main",
        id,
        "edit_file",
        "Edited src/a.rs at lines 1-3",
    );
    feed.notice(Level::Warn, "rate limited, retrying in 2 s (attempt 1)");
    feed.notice(Level::Info, &words.repeat(3));
    let mut reply = String::new();
    for n in [1, 3, 6, 12] {
        reply.push_str(&words.repeat(n));
        reply.push_str("\n\n");
    }
    reply.push_str("# A heading\n\n## A smaller heading\n\n### Smaller still\n\n");
    reply.push_str("- one\n- two, which is longer than one line when the column is narrow enough to wrap it\n  - nested\n- three\n\n");
    reply.push_str("1. first\n2. second\n\n");
    for lines in [3, 30, 60] {
        let code: String = (1..=lines).map(|n| format!("let x{n} = {n};\n")).collect();
        reply.push_str(&format!("```rust\n{code}```\n\n"));
    }
    // Long lines scroll sideways rather than wrap.
    reply.push_str(&format!("```\n{}\nshort\n```\n\n", "x".repeat(300)));
    reply.push_str("> A quote of a few words.\n\n");
    reply.push_str("| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\n");
    reply.push_str("---\n\nThe end.");
    feed.reply(&reply);
    feed.turn_ended("12 s · $0.034", None);
    feed.user_with(
        "Line one\nLine two\nLine three",
        Source::App,
        Delivery::Turn,
        vec![image(1), image(2), image(3)],
    );
    feed.output("output line 1\noutput line 2");
    crate::questions::ask(
        feed,
        &[("q1", &words.repeat(2)), ("q2", "Tabs or spaces?")],
        false,
    );
    feed.set_question_state(
        "q2",
        arko::questions::QuestionState::Answered {
            answer: "Spaces".into(),
        },
    );
    feed.user("A note", Source::App, Delivery::Note);
}

/// Opens the calls of `feed` that show their output.
fn open_calls(feed: &Feed, view: &Entity<TranscriptView>, cx: &mut gpui_kit::App) {
    let calls: Vec<u64> = item_ids(feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Call)
        .map(|(id, _)| id)
        .collect();
    let thinking = item_ids(feed)
        .into_iter()
        .find(|(_, kind)| *kind == Kind::Thinking)
        .map(|(id, _)| id);
    view.update(cx, |view, cx| {
        for call in &calls[1..] {
            view.set_expanded(*call, true, cx);
        }
        if let Some(thinking) = thinking {
            view.set_expanded(thinking, true, cx);
        }
    });
}

/// Row `ix`'s key, the height the list measured, the estimate, and how far
/// the estimate may be off (a pixel, or a line for wrapped prose).
fn height(view: &TranscriptView, ix: usize, cx: &gpui_kit::App) -> (RowKey, f32, f32, f32) {
    let state = view.list.handle();
    let measured = state
        .bounds_for_item(ix)
        .map_or(f32::NAN, |bounds| f32::from(bounds.size.height));
    let width = f32::from(state.viewport_bounds().size.width);
    let row = view.rows[ix];
    let metrics = view.metrics.clone().unwrap();
    let prose = match row.key {
        RowKey::User(_) | RowKey::Notice(_) | RowKey::Question(_) => true,
        RowKey::Block { id, block, .. } => view
            .ui
            .get(&id)
            .and_then(|ui| ui.doc.as_ref()?.blocks.get(block))
            .is_some_and(|block| !matches!(block.facts, BlockFacts::Code(_))),
        _ => false,
    };
    let off = if prose { metrics.line + 1. } else { 1. };
    (
        row.key,
        measured,
        view.row_height(row, width, &metrics, cx),
        off,
    )
}

/// Each row's key, the height the list measured, the estimate and how far
/// it may be off.
fn heights(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
) -> Vec<(RowKey, f32, f32, f32)> {
    let count = cx.read(|cx| view.read(cx).rows.len());
    (0..count)
        .map(|ix| {
            view.update(cx, |view, cx| {
                view.list.scroll_to(ix, px(0.));
                cx.notify();
            });
            settle(cx, window);
            cx.read(|cx| height(view.read(cx), ix, cx))
        })
        .collect()
}

fn assert_close(heights: &[(RowKey, f32, f32, f32)], width: f32) {
    let table: String = heights
        .iter()
        .map(|(key, measured, estimate, _)| format!("{measured:>8.2} {estimate:>8.2} {key:?}\n"))
        .collect();
    for (key, measured, estimate, off) in heights {
        assert!(
            (estimate - measured).abs() <= *off,
            "{key:?} at {width} px: {estimate} estimated, {measured} measured\n{table}"
        );
    }
}

/// Transcripts laid out with the text system the app has on Linux and its
/// bundled fonts, where characters differ in width as they do on screen.
#[cfg(target_os = "linux")]
pub(super) mod real {
    use std::sync::Arc;
    use std::time::SystemTime;

    use arko::remote::Feed;
    use gpui_kit::{
        AnyWindowHandle, AppContext as _, Bounds, Entity, HeadlessAppContext, WindowBounds,
        WindowOptions, point, px, size,
    };

    use crate::backend::FakeBackend;
    use crate::prefs::{Motion, Prefs};
    use crate::transcript::{AgentFilter, TranscriptView};

    pub fn app() -> HeadlessAppContext {
        let text_system = Arc::new(gpui_wgpu::CosmicTextSystem::new_without_system_fonts(
            "Work Sans",
        ));
        let mut cx =
            HeadlessAppContext::with_asset_source(text_system, Arc::new(crate::assets::Assets));
        let mut prefs = Prefs::default();
        prefs.appearance.motion = Motion::Off;
        let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
        let startup = crate::Startup::for_tests(backend)
            .with_prefs(prefs)
            .with_fonts(crate::fonts::bundled());
        cx.update(|cx| crate::init(startup, cx));
        cx
    }

    pub fn frame(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    pub fn settle(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
        for _ in 0..4 {
            cx.run_until_parked();
            frame(cx, window);
        }
        cx.run_until_parked();
    }

    /// The transcript of `feed` in a window `width` by `height`, with every
    /// entry ready to show, its rows not drawn counted at estimated heights
    /// when `estimates` says so.
    pub fn open(
        cx: &mut HeadlessAppContext,
        feed: &Feed,
        (width, height): (f32, f32),
        estimates: bool,
    ) -> (AnyWindowHandle, Entity<TranscriptView>) {
        let feed = feed.clone();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: point(px(0.), px(0.)),
                size: size(px(width), px(height)),
            })),
            focus: false,
            show: false,
            ..Default::default()
        };
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                        view.estimates = estimates;
                        view
                    })
                })
            })
            .unwrap();
        for _ in 0..40 {
            settle(cx, window);
            if cx.update(|cx| view.read(cx).loaded_from.is_none()) {
                break;
            }
        }
        (window, view)
    }
}

#[cfg(target_os = "linux")]
#[test]
fn rows_are_estimated_at_the_height_they_measure_in_the_apps_fonts() {
    let mut cx = real::app();
    let feed = Feed::new("main");
    every_kind(&feed);
    for width in [800., 500., 1200.] {
        let (window, view) = real::open(&mut cx, &feed, (width, 600.), true);
        cx.update(|cx| open_calls(&feed, &view, cx));
        real::settle(&mut cx, window);
        let count = cx.update(|cx| view.read(cx).rows.len());
        let heights: Vec<_> = (0..count)
            .map(|ix| {
                cx.update(|cx| {
                    view.update(cx, |view, cx| {
                        view.list.scroll_to(ix, px(0.));
                        cx.notify();
                    })
                });
                real::settle(&mut cx, window);
                cx.update(|cx| height(view.read(cx), ix, cx))
            })
            .collect();
        assert_close(&heights, width);
    }
}

#[gpui_kit::test]
fn rows_are_estimated_at_the_height_they_measure(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    every_kind(&feed);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    cx.update(|cx| open_calls(&feed, &view, cx));
    settle(cx, window);
    let keys = super::tests::keys(&view, cx);
    for kind in [
        "Separator",
        "User",
        "Thinking",
        "Call",
        "Worker",
        "Notice",
        "Output",
        "Block",
        "Actions",
        "Turn",
    ] {
        assert!(
            keys.iter().any(|key| format!("{key:?}").starts_with(kind)),
            "no {kind} row in {keys:?}"
        );
    }
    assert_close(&heights(cx, window, &view), 800.);
    // The long reply as its source, and then with Markdown off.
    let reply = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Reply)
        .nth(1)
        .unwrap()
        .0;
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    settle(cx, window);
    assert_close(&heights(cx, window, &view), 800.);
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    settle(cx, window);
    assert_close(&heights(cx, window, &view), 800.);
    // Calls in a group (Quiet) and counted (Messages).
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.render_markdown = true;
            prefs.view = prefs::TranscriptView::Quiet;
        })
    });
    settle(cx, window);
    let keys = super::tests::keys(&view, cx);
    assert!(keys.iter().any(|key| matches!(key, RowKey::Group(_))));
    assert_close(&heights(cx, window, &view), 800.);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = prefs::TranscriptView::Messages));
    settle(cx, window);
    let keys = super::tests::keys(&view, cx);
    assert!(keys.iter().any(|key| matches!(key, RowKey::Count(_))));
    assert_close(&heights(cx, window, &view), 800.);
}

/// The scroll range the scrollbar has now, what the list counts for its
/// rows past the screen.
fn range(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> f32 {
    cx.read(|cx| f32::from(view.read(cx).list.handle().max_offset_for_scrollbar().y))
}

#[gpui_kit::test]
fn a_reply_streaming_below_the_reader_counts_as_it_grows(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    for turn in 0..20 {
        feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        feed.reply(&format!("Answer {turn}."));
        feed.turn_ended("3 s · $0.01", None);
    }
    feed.user("Explain it all", Source::App, Delivery::Turn);
    feed.turn_started();
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    // The reader reads the start while one long paragraph streams in below.
    view.update(cx, |view, cx| {
        view.list.scroll_to(0, px(0.));
        cx.notify();
    });
    settle(cx, window);
    let before = range(cx, &view);
    let sentence = "The list measures a row when it draws it, and estimates the rest. ";
    for _ in 0..60 {
        say(&feed, "main", sentence);
        settle(cx, window);
    }
    // The scrollbar counts the paragraph as it is now, not as it began.
    let streamed = range(cx, &view);
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    settle(cx, window);
    let drawn = range(cx, &view);
    // (Counted at its first height, the paragraph would add a line.)
    assert!(drawn - before > 1000., "{before} then {drawn}");
    assert!(
        (streamed - drawn).abs() <= (drawn - before) * 0.1,
        "{before} before, {streamed} counted while it streamed, {drawn} once drawn"
    );
}

#[gpui_kit::test]
fn a_zoom_counts_the_rows_not_drawn_at_their_new_size(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    every_kind(&feed);
    every_kind(&feed);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    view.update(cx, |view, cx| {
        view.list.scroll_to(0, px(0.));
        cx.notify();
    });
    settle(cx, window);
    let before = range(cx, &view);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.zoom = 1.25));
    settle(cx, window);
    let zoomed = range(cx, &view);
    // Every row drawn at the new size.
    let count = cx.read(|cx| view.read(cx).rows.len());
    for ix in (0..count).step_by(3) {
        view.update(cx, |view, cx| {
            view.list.scroll_to(ix, px(0.));
            cx.notify();
        });
        settle(cx, window);
    }
    let drawn = range(cx, &view);
    assert!(zoomed > before * 1.15, "{before} then {zoomed}");
    assert!(
        (zoomed - drawn).abs() <= drawn * 0.01,
        "{zoomed} at the zoom, {drawn} once drawn"
    );
}
