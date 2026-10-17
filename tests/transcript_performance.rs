//! What a frame of the transcript costs on the UI thread,
//! measured on GPUI's headless platform with the real text system (the plain
//! test platform lays text out without shaping it). A frame's time here is
//! `Window::draw`: applying the feed's updates, layout, prepaint and paint,
//! without the GPU's work and the Markdown parsing, which runs on background
//! threads in the app.
//!
//! The tests are ignored by default. Run them in release:
//!
//! ```sh
//! cargo test --release --test transcript_performance -- --ignored --nocapture --test-threads 1
//! ```
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use arko::remote::{Delivery, Feed, Source};
use arko_agents::AgentEvent;
use arko_studio::backend::FakeBackend;
use arko_studio::prefs::{Motion, Prefs};
use arko_studio::transcript::{AgentFilter, TranscriptView};
use arko_studio::{Startup, assets::Assets, fonts};
use arko_workflows::WorkflowEvent;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Context, Entity, InputEvent as _, IntoElement,
    ParentElement as _, Render, ScrollDelta, ScrollWheelEvent, Styled as _, Window, div, point, px,
    size,
};
use gpui_screenshot::ScreenshotApp;
use serde_json::Value;
use uuid::Uuid;

fn app() -> ScreenshotApp {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut prefs = Prefs::default();
    prefs.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(prefs)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    app
}

fn event(event: AgentEvent) -> WorkflowEvent {
    WorkflowEvent {
        timestamp: chrono::Utc::now(),
        workflow_run_id: Uuid::nil(),
        agent_name: "main".into(),
        agent_run_id: Uuid::nil(),
        event,
    }
}

fn say(feed: &Feed, text: &str) {
    feed.event(
        &event(AgentEvent::AssistantResponseChunk(text.into())),
        None,
    );
}

fn call(feed: &Feed, tool: &str, parameters: &[(&str, Value)], output: &str) {
    let id = Uuid::new_v4();
    let parameters: HashMap<String, Value> = parameters
        .iter()
        .map(|(key, value)| (key.to_string(), value.clone()))
        .collect();
    feed.event(
        &event(AgentEvent::ToolCallFormatted {
            tool_call_id: id,
            tool_name: tool.into(),
            parameters,
        }),
        None,
    );
    feed.event(
        &event(AgentEvent::ToolResult {
            tool_call_id: id,
            tool_name: tool.into(),
            output: output.into(),
        }),
        None,
    );
}

/// A reply of about `bytes` bytes: paragraphs with inline markup, lists,
/// a heading now and then, and a Rust code block every fourth section.
fn reply(seed: usize, bytes: usize) -> String {
    let mut text = String::new();
    let mut section = 0;
    while text.len() < bytes {
        let n = seed + section;
        match section % 4 {
            0 => text.push_str(&format!(
                "The parser keeps **block {n}** open while text streams in, and \
                 `Parser::feed` returns the blocks it completed. A finished block \
                 is never parsed again, so a long answer costs what its last block \
                 costs.\n\n"
            )),
            1 => text.push_str(&format!(
                "## Step {n}\n\n- Read the chunk into `pending`.\n- Find where the \
                 open block ends.\n- Hand out the finished blocks, *in order*.\n\n"
            )),
            2 => {
                text.push_str("```rust\n");
                for line in 0..14 {
                    text.push_str(&format!(
                        "    let value_{line} = self.pending.drain(..{n}).collect::<String>();\n"
                    ));
                }
                text.push_str("```\n\n");
            }
            _ => text.push_str(&format!(
                "Section {n} ends here, with a [link](https://example.com/{n}) and \
                 more words to wrap across the reading column's width.\n\n"
            )),
        }
        section += 1;
    }
    text
}

/// A feed of `turns` finished turns of six items each: the user's message,
/// a file read, a command and its output block, the reply, the footer.
fn long_feed(turns: usize) -> Feed {
    let feed = Feed::new("main");
    for turn in 0..turns {
        feed.user(
            &format!("Question {turn}: what does the parser do with a chunk?"),
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        call(
            &feed,
            "read_file",
            &[("path", format!("src/parse_{turn}.rs").into())],
            "1 | pub struct Parser {}\n",
        );
        call(
            &feed,
            "shell",
            &[("command", "cargo test -p parse".into())],
            "exit code 0, 2.0 s\nrunning 12 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\n\
             test d ... ok\ntest result: ok. 12 passed; 0 failed",
        );
        let text = reply(turn, if turn % 2 == 0 { 600 } else { 1500 });
        say(&feed, &text);
        feed.reply(&text);
        feed.turn_ended("12 s · $0.034", None);
    }
    feed
}

/// Opens a window with the transcript of `feed`, and returns how long
/// making the view took on the UI thread. (The headless platform runs
/// background tasks on the same thread when a window opens, so the window's
/// opening as a whole would count the languages' warm-up and the documents'
/// parsing, which run on background threads in the app.)
fn open(
    app: &mut ScreenshotApp,
    feed: &Feed,
) -> (AnyWindowHandle, Entity<TranscriptView>, Duration) {
    let mut view = None;
    let mut took = Duration::ZERO;
    let window = app
        .open_window_with(size(px(1000.), px(900.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                let start = Instant::now();
                let made = cx.new(|cx| TranscriptView::new(feed, AgentFilter::Main, window, cx));
                took = start.elapsed();
                view = Some(made.clone());
                made
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    (window, view.unwrap(), took)
}

/// Draws a frame of `window` and returns how long it took.
fn frame(app: &mut ScreenshotApp, window: AnyWindowHandle) -> Duration {
    app.update_window(window, |_, window, cx| {
        let start = Instant::now();
        window.draw(cx).clear(cx);
        start.elapsed()
    })
    .unwrap()
}

fn report(name: &str, times: &[Duration]) {
    let mut ms: Vec<f64> = times.iter().map(|t| t.as_secs_f64() * 1000.).collect();
    ms.sort_by(f64::total_cmp);
    let at = |q: f64| ms[((ms.len() - 1) as f64 * q).round() as usize];
    let over = ms.iter().filter(|ms| **ms > 16.).count();
    println!(
        "{name}: {} frames, p50 {:.2} ms, p95 {:.2} ms, max {:.2} ms, {over} over 16 ms",
        ms.len(),
        at(0.5),
        at(0.95),
        at(1.)
    );
}

#[test]
#[ignore = "a measurement: run it in release"]
fn scrolling_a_3000_item_transcript() {
    let mut app = app();
    // A first transcript, empty, for what the app does once (it starts
    // warming the code languages up).
    let (empty, _, took) = open(&mut app, &Feed::new("main"));
    println!(
        "an empty transcript made in {:.1} ms",
        took.as_secs_f64() * 1000.
    );
    app.update_window(empty, |_, window, _| window.remove_window())
        .unwrap();
    let feed = long_feed(500);
    // What subscribing costs: the history copied under the feed's lock (the
    // view does it on a background thread).
    let start = Instant::now();
    let items = feed.subscribe_batched().0.items.len();
    let copied = start.elapsed();
    let (window, view, opened) = open(&mut app, &feed);
    // The history comes with the next frame.
    app.run_until_parked();
    let shown = frame(&mut app, window);
    let ready = app.update(|cx| view.read(cx).row_count());
    // The older entries are made ready a chunk a frame.
    let mut loading = Vec::new();
    loop {
        let before = app.update(|cx| view.read(cx).row_count());
        app.run_until_parked();
        loading.push(frame(&mut app, window));
        if app.update(|cx| view.read(cx).row_count()) == before {
            break;
        }
    }
    let rows = app.update(|cx| view.read(cx).row_count());
    println!(
        "{items} items: the history copied in {:.1} ms off the UI thread; the view made in {:.1} ms, the history shown in a {:.1} ms frame with {ready} rows ready, then {rows} rows in {} frames",
        copied.as_secs_f64() * 1000.,
        opened.as_secs_f64() * 1000.,
        shown.as_secs_f64() * 1000.,
        loading.len(),
    );
    report("  frames while the older entries are made ready", &loading);
    println!(
        "  all of it on the UI thread: {:.1} ms",
        (opened + loading.iter().sum::<Duration>()).as_secs_f64() * 1000.
    );
    let center = point(px(500.), px(450.));
    let scroll = |app: &mut ScreenshotApp, delta: f32| {
        app.update_window(window, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: center,
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
    };
    // Up through the whole transcript, a wheel step of 240 px a frame.
    let mut up = Vec::new();
    for ix in 0..4000 {
        scroll(&mut app, 240.);
        app.run_until_parked();
        let time = frame(&mut app, window);
        if time > Duration::from_millis(16) {
            let rows = app.update(|cx| view.read(cx).row_count());
            println!(
                "  frame {ix}: {:.1} ms, {rows} rows",
                time.as_secs_f64() * 1000.
            );
        }
        up.push(time);
    }
    report("scrolling up through 3000 items", &up);
    let mut down = Vec::new();
    for _ in 0..600 {
        scroll(&mut app, -240.);
        app.run_until_parked();
        down.push(frame(&mut app, window));
    }
    report("scrolling down again", &down);
    let mut still = Vec::new();
    for _ in 0..100 {
        app.update_window(window, |_, window, _| window.refresh())
            .unwrap();
        still.push(frame(&mut app, window));
    }
    report("redrawing in place after scrolling through", &still);
}

#[test]
#[ignore = "a measurement: run it in release"]
fn streaming_a_20k_token_reply() {
    let mut app = app();
    let feed = Feed::new("main");
    feed.user("Explain the parser.", Source::App, Delivery::Turn);
    feed.turn_started();
    let (window, _, _) = open(&mut app, &feed);
    for _ in 0..3 {
        app.run_until_parked();
        frame(&mut app, window);
    }
    // About 20k tokens: 80 KB, in chunks of 40 bytes as models stream them.
    let text = reply(0, 80_000);
    let mut frames = Vec::new();
    let mut at = 0;
    while at < text.len() {
        let mut end = (at + 40).min(text.len());
        while !text.is_char_boundary(end) {
            end += 1;
        }
        say(&feed, &text[at..end]);
        at = end;
        // The chunk is applied in one frame, and its parse lands (on the
        // background executor) before the next.
        app.run_until_parked();
        frames.push(frame(&mut app, window));
        app.run_until_parked();
        frames.push(frame(&mut app, window));
    }
    report("streaming a 20k-token reply, 40-byte chunks", &frames);
    let tail = &frames[frames.len() - 400..];
    report("  its last 200 chunks", tail);
}

/// Streams the last 8 KB of a 200 KB Rust code block, at the top level or
/// inside a list item, and reports the frames.
fn stream_a_long_code_block(nested: bool) {
    let mut app = app();
    let feed = Feed::new("main");
    feed.user("Write the parser.", Source::App, Delivery::Turn);
    feed.turn_started();
    let indent = if nested { "   " } else { "" };
    let mut code = String::new();
    let mut line = 0;
    while code.len() < 200 * 1024 {
        code.push_str(&format!(
            "{indent}    let value_{line} = self.pending.drain(..{line}).collect::<String>();\n"
        ));
        line += 1;
    }
    let head = if nested {
        "Steps:\n\n1. Write this:\n\n   ```rust\n"
    } else {
        "Here it is:\n\n```rust\n"
    };
    let text = format!("{head}{code}");
    let split = text.len() - 8 * 1024;
    say(&feed, &text[..split]);
    let (window, _, _) = open(&mut app, &feed);
    for _ in 0..4 {
        app.run_until_parked();
        frame(&mut app, window);
    }
    let mut frames = Vec::new();
    let mut at = split;
    while at < text.len() {
        let end = (at + 40).min(text.len());
        say(&feed, &text[at..end]);
        at = end;
        app.run_until_parked();
        frames.push(frame(&mut app, window));
    }
    report(
        if nested {
            "streaming a 200 KB code block in a list item, 40-byte chunks"
        } else {
            "streaming a 200 KB code block, 40-byte chunks"
        },
        &frames,
    );
}

#[test]
#[ignore = "a measurement: run it in release"]
fn streaming_a_200_kb_code_block() {
    stream_a_long_code_block(false);
    stream_a_long_code_block(true);
}

/// A window with a sibling of the transcript, as the composer is.
struct Column {
    sibling: Entity<Sibling>,
    transcript: Entity<TranscriptView>,
}

struct Sibling(usize);

impl Render for Sibling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(format!("{} keystrokes", self.0))
    }
}

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .h(px(800.))
                    .child(TranscriptView::cached(&self.transcript)),
            )
            .child(self.sibling.clone())
    }
}

#[test]
#[ignore = "a measurement: run it in release"]
fn a_keystroke_beside_a_long_transcript() {
    let mut app = app();
    let feed = long_feed(100);
    let mut column = None;
    let window = app
        .open_window_with(size(px(1000.), px(900.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                let transcript =
                    cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx));
                let sibling = cx.new(|_| Sibling(0));
                let made = cx.new(|_| Column {
                    sibling,
                    transcript,
                });
                column = Some(made.clone());
                made
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    let column = column.unwrap();
    for _ in 0..4 {
        app.run_until_parked();
        frame(&mut app, window);
    }
    let (sibling, transcript) = app.update(|cx| {
        let column = column.read(cx);
        (column.sibling.clone(), column.transcript.clone())
    });
    let renders = app.update(|cx| transcript.read(cx).render_count());
    let mut frames = Vec::new();
    for _ in 0..200 {
        app.update(|cx| {
            sibling.update(cx, |sibling, cx| {
                sibling.0 += 1;
                cx.notify();
            })
        });
        frames.push(frame(&mut app, window));
    }
    report("a keystroke's frame beside 600 transcript items", &frames);
    let drawn = app.update(|cx| transcript.read(cx).render_count()) - renders;
    println!("the transcript drew {drawn} times in 200 keystrokes");
    assert_eq!(drawn, 0);
}
