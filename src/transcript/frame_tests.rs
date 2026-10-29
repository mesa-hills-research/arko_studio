//! What a frame costs at the end of a long conversation, by what draws it:
//! the spinner alone, the second's tick of the running times, and text
//! streaming in. The window's root draws in each frame and holds the
//! transcript cached, as the session view does.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use arko::remote::{Delivery, Feed, Source, WorkerChange};
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _,
    Point, Render, Styled as _, TestAppContext, Window, WindowBounds, WindowOptions, div, px, size,
};

use super::tests::{call, result, say, settle, start, think};
use super::{AgentFilter, TranscriptView};
use crate::prefs::{self, TranscriptView as View};

/// The window's root: drawn in every frame, with the transcript cached in it.
struct Host {
    transcript: Entity<TranscriptView>,
    renders: Rc<Cell<usize>>,
}

impl Render for Host {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.renders.set(self.renders.get() + 1);
        div()
            .size_full()
            .child(TranscriptView::cached(&self.transcript))
    }
}

fn open_hosted(
    cx: &mut TestAppContext,
    feed: &Feed,
) -> (AnyWindowHandle, Entity<TranscriptView>, Rc<Cell<usize>>) {
    let feed = feed.clone();
    let renders = Rc::new(Cell::new(0));
    let host_renders = renders.clone();
    let (window, host) = cx
        .update(|cx| {
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: Point::default(),
                        size: size(px(900.), px(900.)),
                    })),
                    ..Default::default()
                },
                cx,
                |window, cx| {
                    let transcript =
                        cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx));
                    cx.new(|_| Host {
                        transcript,
                        renders: host_renders,
                    })
                },
            )
        })
        .unwrap();
    let transcript = cx.read(|cx| host.read(cx).transcript.clone());
    settle(cx, window);
    (window, transcript, renders)
}

const REPLY: &str = "## What changed\n\nThe change touches `src/parser.rs` and **two** tests. Here is what reading the code showed, in a paragraph long enough to wrap over a few lines of the column at its usual width, as replies do.\n\n- The lexer keeps its tokens, with `inline code` and a [link](https://example.com).\n- The parser takes them one at a time and builds the tree, a point long enough to wrap too.\n- The tests cover both.\n\n```rust\nfn parse(input: &str) -> Result<Ast, Error> {\n    let mut tokens = lex(input)?;\n    let mut ast = Ast::default();\n    while let Some(token) = tokens.next() {\n        match token {\n            Token::Word(word) => ast.words.push(word),\n            Token::Number(n) => ast.numbers.push(n),\n            Token::Space => {}\n        }\n    }\n    Ok(ast)\n}\n```\n\n| Case | Before | After |\n|---|---|---|\n| words | 12 ms | 3 ms |\n| numbers | 8 ms | 2 ms |\n\nAnd a closing paragraph that says what to do next, and why it matters for the release.";

fn code_output(lines: usize) -> String {
    (1..=lines)
        .map(|n| format!("{n:>4} | let value_{n} = compute(input, {n}) * factor + offset;\n"))
        .collect()
}

/// `turns` turns of a long conversation: the user's message, thinking,
/// `calls` reads with their output, text, a long Markdown reply, the footer.
/// A worker ran in some of them, with calls of its own.
pub(crate) fn long_conversation(feed: &Feed, turns: usize, calls: usize) {
    let output = code_output(60);
    let thinking = "Let me look at how the parser is put together before changing it. ".repeat(12);
    for turn in 0..turns {
        feed.user(
            &format!("Question {turn}: please look at the parser and say what to change."),
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        think(feed, &thinking);
        say(feed, "main", "I'll read the files first.\n\n");
        for file in 0..calls {
            let id = call(
                feed,
                "main",
                "read_file",
                &[("path", format!("src/f{turn}_{file}.rs").into())],
            );
            result(feed, "main", id, "read_file", &output);
        }
        if turn % 10 == 5 {
            let worker = format!("r{turn}");
            feed.worker(&worker, "Fix the parser", WorkerChange::Started);
            for n in 0..6 {
                let id = call(
                    feed,
                    &worker,
                    "shell",
                    &[("command", format!("cargo test {n}").into())],
                );
                result(feed, &worker, id, "shell", &output);
            }
            feed.worker(&worker, "", WorkerChange::Finished { report: "Done." });
        }
        say(feed, "main", REPLY);
        feed.reply(REPLY);
        feed.turn_ended("3 s · $0.01", None);
    }
}

#[derive(Default)]
struct Frames {
    /// How long each frame took, by whether the transcript drew in it.
    alone: Vec<Duration>,
    drawn: Vec<Duration>,
    asked: usize,
}

impl Frames {
    fn report(&mut self, name: &str) {
        let stats = |times: &mut Vec<Duration>| {
            times.sort();
            if times.is_empty() {
                return "none".to_string();
            }
            let ms = |d: Duration| d.as_secs_f64() * 1000.;
            format!(
                "{} frames, median {:.3} ms, p90 {:.3} ms, max {:.3} ms",
                times.len(),
                ms(times[times.len() / 2]),
                ms(times[times.len() * 9 / 10]),
                ms(*times.last().unwrap())
            )
        };
        eprintln!(
            "{name}: {} asked or drawn. Without the transcript: {}. With it: {}.",
            self.asked,
            stats(&mut self.alone),
            stats(&mut self.drawn)
        );
    }
}

/// `count` frames of a 60 Hz display, with `before` run ahead of each.
fn frames(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    count: usize,
    mut before: impl FnMut(usize),
) -> Frames {
    let mut frames = Frames::default();
    for n in 0..count {
        before(n);
        let renders = cx.read(|cx| view.read(cx).render_count());
        let start = Instant::now();
        cx.executor().advance_clock(Duration::from_micros(16_667));
        let asked = cx
            .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
        let took = start.elapsed();
        frames.asked += usize::from(asked > 0);
        if cx.read(|cx| view.read(cx).render_count()) > renders {
            frames.drawn.push(took);
        } else {
            frames.alone.push(took);
        }
    }
    frames
}

fn motion_on(cx: &mut TestAppContext, view: View) {
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.motion = prefs::Motion::On;
            prefs.view = view;
        });
        cx.set_reduce_motion(false);
    });
}

/// Draws until every entry is ready to show and the list has settled: the
/// scrollbar that showed as the rows came has faded again.
fn ready(cx: &mut TestAppContext, window: AnyWindowHandle, view: &Entity<TranscriptView>) {
    for _ in 0..200 {
        settle(cx, window);
        if cx.read(|cx| view.read(cx).loaded_from.is_none()) {
            break;
        }
    }
    for _ in 0..240 {
        cx.executor().advance_clock(Duration::from_millis(17));
        cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx))
            .unwrap();
        cx.run_until_parked();
    }
}

/// Each kind of frame at the end of the conversation in `feed`, reported.
fn scenarios(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    host: &Rc<Cell<usize>>,
    feed: &Feed,
) {
    // The last turn runs a command.
    feed.user("Build it.", Source::App, Delivery::Turn);
    feed.turn_started();
    let build = call(feed, "main", "shell", &[("command", "cargo build".into())]);
    ready(cx, window, view);
    let (entries, rows) = cx.read(|cx| {
        let view = view.read(cx);
        (view.model.entries().len(), view.rows.len())
    });
    eprintln!("{entries} entries, {rows} rows");
    let host_before = host.get();
    frames(cx, window, view, 180, |_| {}).report("a command runs");
    eprintln!("  the root drew {} times", host.get() - host_before);
    result(feed, "main", build, "shell", "exit code 0, 3.0 s");
    // The agent waits for a worker, whose text streams in its own
    // transcript.
    feed.worker("r99", "Fix the parser", WorkerChange::Started);
    let wait = call(feed, "main", "wait_for_agents", &[]);
    ready(cx, window, view);
    frames(cx, window, view, 120, |_| say(feed, "r99", "worker words "))
        .report("waiting for a worker that writes");
    frames(cx, window, view, 120, |n| {
        if n % 4 == 0 {
            say(feed, "r99", "worker words ")
        }
    })
    .report("waiting for a worker that writes 15 times a second");
    frames(cx, window, view, 120, |n| {
        if n % 4 == 0 {
            let id = call(feed, "r99", "read_file", &[("path", "src/lib.rs".into())]);
            result(feed, "r99", id, "read_file", "1 | fn a() {}\n");
        }
    })
    .report("waiting for a worker that calls tools 15 times a second");
    feed.worker("r99", "", WorkerChange::Finished { report: "Done." });
    result(feed, "main", wait, "wait_for_agents", "r99 finished");
    // Text streams in after the command.
    ready(cx, window, view);
    let chunk = "Some more of the answer, which streams in a few words at a time. ";
    frames(cx, window, view, 120, |_| say(feed, "main", chunk)).report("text streams");
    // Working…: the agent waits on the model.
    feed.reply("Done.");
    feed.turn_ended("3 s · $0.01", None);
    feed.user("Go on.", Source::App, Delivery::Turn);
    feed.turn_started();
    ready(cx, window, view);
    frames(cx, window, view, 180, |_| {}).report("Working…");
    // Thinking streams.
    think(feed, "hmm");
    ready(cx, window, view);
    frames(cx, window, view, 120, |_| think(feed, " and more")).report("thinking streams");
    feed.turn_ended("3 s · $0.01", None);
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
}

fn view_from_env() -> View {
    match std::env::var("VIEW").as_deref() {
        Ok("quiet") => View::Quiet,
        Ok("messages") => View::Messages,
        _ => View::Full,
    }
}

/// Frame costs at the end of conversations of a few lengths, for each kind
/// of frame: ignored by default.
///
/// ```sh
/// cargo test --lib frame_costs -- --ignored --nocapture --test-threads 1
/// ```
#[gpui_kit::test]
#[ignore]
fn frame_costs(cx: &mut TestAppContext) {
    start(cx);
    let turns: Vec<usize> = std::env::var("TURNS")
        .ok()
        .map(|turns| turns.split(',').filter_map(|n| n.parse().ok()).collect())
        .unwrap_or_else(|| vec![10, 100, 300]);
    motion_on(cx, view_from_env());
    for turns in turns {
        let feed = Feed::new("main");
        let (window, view, host) = open_hosted(cx, &feed);
        long_conversation(&feed, turns, 8);
        eprint!("--- {turns} turns: ");
        scenarios(cx, window, &view, &host, &feed);
    }
}

/// Frame costs at the end of a copy of a saved session: its main agent's
/// conversation (`main-agent.json`, the agent's record in the session's
/// `run/agents`) and its workers' calls (`events.jsonl`), in the folder
/// `SESSION`. Ignored by default.
///
/// ```sh
/// SESSION=/path/to/copy cargo test --lib frame_costs_of_a_saved_session -- --ignored --nocapture --test-threads 1
/// ```
#[gpui_kit::test]
#[ignore]
fn frame_costs_of_a_saved_session(cx: &mut TestAppContext) {
    use std::collections::HashMap;
    use std::io::BufRead as _;

    let Ok(folder) = std::env::var("SESSION") else {
        eprintln!("SESSION names no folder");
        return;
    };
    let folder = std::path::PathBuf::from(folder);
    start(cx);
    motion_on(cx, view_from_env());
    let feed = Feed::new("main");
    let (window, view, host) = open_hosted(cx, &feed);
    let record: serde_json::Value =
        serde_json::from_slice(&std::fs::read(folder.join("main-agent.json")).unwrap()).unwrap();
    let messages: Vec<arko_ai::Message> =
        serde_json::from_value(record["conversation"]["messages"].clone()).unwrap();
    let images = arko::images::Images::default();
    for chunk in messages.chunks(500) {
        feed.restore(arko::remote::history(chunk, true, &images));
    }
    // The workers' calls, which the main transcript keeps.
    let mut labels: HashMap<String, String> = HashMap::new();
    let events = std::fs::File::open(folder.join("events.jsonl")).unwrap();
    let mut calls = 0;
    for line in std::io::BufReader::new(events).lines() {
        let line = line.unwrap();
        let value: serde_json::Value = serde_json::from_str(&line).unwrap();
        if let (Some(run), Some(handle)) = (value["run_id"].as_str(), value["handle"].as_str()) {
            labels.insert(run.to_string(), handle.to_string());
            match value["kind"]["kind"].as_str() {
                Some("started") => feed.worker(handle, "A task", WorkerChange::Started),
                Some("finished") => {
                    feed.worker(handle, "", WorkerChange::Finished { report: "Done." })
                }
                _ => {}
            }
            continue;
        }
        if value.get("event").is_none() || value["agent_name"] == "main" {
            continue;
        }
        let Ok(event) = serde_json::from_value::<arko_workflows::WorkflowEvent>(value) else {
            continue;
        };
        let Some(label) = labels.get(&event.agent_run_id.to_string()) else {
            continue;
        };
        calls += usize::from(matches!(
            event.event,
            arko_agents::AgentEvent::ToolCallFormatted { .. }
        ));
        feed.event(&event, Some(label));
    }
    eprint!(
        "--- {} messages and {calls} calls of workers: ",
        messages.len()
    );
    scenarios(cx, window, &view, &host, &feed);
}

/// While the agent waits for a worker, the worker's text streams in the
/// feed. The main transcript keeps none of it, so it doesn't draw for it:
/// it draws for the second's tick of the running times only, while the
/// spinner asks for every frame.
#[gpui_kit::test]
fn a_workers_text_doesnt_draw_the_main_transcript(cx: &mut TestAppContext) {
    start(cx);
    motion_on(cx, View::Quiet);
    let feed = Feed::new("main");
    long_conversation(&feed, 3, 2);
    feed.user("Fix it with a worker.", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    call(&feed, "main", "wait_for_agents", &[]);
    let (window, view, _) = open_hosted(cx, &feed);
    ready(cx, window, &view);
    let drawn = frames(cx, window, &view, 120, |_| say(&feed, "r1", "a few words "));
    assert_eq!(drawn.asked, 120, "the spinner turns in every frame");
    assert!(
        drawn.drawn.len() <= 3,
        "the transcript drew in {} of 120 frames",
        drawn.drawn.len()
    );
    // What the worker says still reaches its own transcript.
    let worker = cx
        .update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| {
                    TranscriptView::new(&feed, AgentFilter::Worker("r1".into()), window, cx)
                })
            })
        })
        .unwrap();
    settle(cx, worker.0);
    let text = cx.read(|cx| {
        let view = worker.1.read(cx);
        view.model
            .entries()
            .last()
            .map(|entry| entry.item.text.clone())
    });
    assert_eq!(text.as_deref(), Some("a few words ".repeat(120).as_str()));
}

/// Every running indicator the view shows turns: the workers' cards and the
/// call the agent waits in. They turn as they paint, so the window asks for
/// every frame of the display and draws the transcript only for the
/// second's tick of the running times, however many turn.
#[gpui_kit::test]
fn every_running_indicator_turns_without_drawing_the_transcript(cx: &mut TestAppContext) {
    use super::model::RowKey;
    use gpui_kit::test::TestWindowExt as _;

    start(cx);
    motion_on(cx, View::Quiet);
    let feed = Feed::new("main");
    feed.user("Fix it with workers.", Source::App, Delivery::Turn);
    feed.turn_started();
    for worker in ["r1", "r2", "r3", "r4"] {
        feed.worker(worker, "Fix the parser", WorkerChange::Started);
    }
    call(&feed, "main", "wait_for_agents", &[]);
    let (window, view, host) = open_hosted(cx, &feed);
    ready(cx, window, &view);
    let running: Vec<u64> = cx.read(|cx| {
        view.read(cx)
            .rows
            .iter()
            .filter_map(|row| match row.key {
                RowKey::Worker(id) | RowKey::Call(id) => Some(id),
                _ => None,
            })
            .collect()
    });
    assert_eq!(running.len(), 5, "four cards and the wait");
    let turning = cx
        .update_window(window, |_, window, _| {
            running
                .iter()
                .filter(|id| window.try_find(("running", **id)).is_some())
                .count()
        })
        .unwrap();
    let hosted = host.get();
    let mut drawn = frames(cx, window, &view, 120, |_| {});
    drawn.report("five running indicators");
    assert_eq!(turning, 5, "every running indicator turns");
    assert_eq!(drawn.asked, 120, "the window asks for every frame");
    assert!(host.get() - hosted >= 120, "the window draws every frame");
    assert!(
        drawn.drawn.len() <= 3,
        "the transcript drew in {} of 120 frames",
        drawn.drawn.len()
    );
}
