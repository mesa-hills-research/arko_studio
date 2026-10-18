//! Feed-driven tests of the transcript: a real feed, fed the way a session
//! feeds it (the workflow's events, and what Arko says of the turns), and
//! the view drawn in a test window.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{CallStatus, Delivery, Feed, Kind, Level, Source, WorkerChange};
use arko_agents::AgentEvent;
use arko_workflows::WorkflowEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Context, ElementId, Entity, IntoElement,
    ParentElement as _, Pixels, Point, Render, Styled as _, TestAppContext, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use serde_json::Value;
use uuid::Uuid;

use super::model::RowKey;
use super::{AgentFilter, TranscriptEvent, TranscriptView};
use crate::Startup;
use crate::backend::FakeBackend;
use crate::prefs::{self, TranscriptView as View};

pub(super) fn start(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
}

fn options(width: f32, height: f32) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(width), px(height)),
        })),
        ..Default::default()
    }
}

/// A transcript of `feed` in a window of its own, drawn once.
pub(super) fn open(
    cx: &mut TestAppContext,
    feed: &Feed,
    filter: AgentFilter,
    height: f32,
) -> (AnyWindowHandle, Entity<TranscriptView>) {
    let feed = feed.clone();
    let opened = cx
        .update(|cx| {
            gpui_kit::open_window(options(800., height), cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, filter, window, cx))
            })
        })
        .unwrap();
    settle(cx, opened.0);
    opened
}

/// Lets the feed's updates through and draws until nothing changes: the
/// inbox wakes the view, the documents parse in the background and their
/// blocks become rows.
pub(super) fn settle(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    cx.run_until_parked();
}

pub(super) fn keys(view: &Entity<TranscriptView>, cx: &mut TestAppContext) -> Vec<RowKey> {
    cx.read(|cx| view.read(cx).rows.iter().map(|row| row.key).collect())
}

fn event(agent: &str, event: AgentEvent) -> WorkflowEvent {
    WorkflowEvent {
        timestamp: chrono::Utc::now(),
        workflow_run_id: Uuid::nil(),
        agent_name: agent.into(),
        agent_run_id: Uuid::nil(),
        event,
    }
}

fn say(feed: &Feed, agent: &str, text: &str) {
    let label = (agent != "main").then_some(agent);
    feed.event(
        &event(agent, AgentEvent::AssistantResponseChunk(text.into())),
        label,
    );
}

fn think(feed: &Feed, text: &str) {
    feed.event(
        &event("main", AgentEvent::ReasoningChunk(text.into())),
        None,
    );
}

pub(super) fn call(feed: &Feed, agent: &str, tool: &str, parameters: &[(&str, Value)]) -> Uuid {
    let id = Uuid::new_v4();
    let label = (agent != "main").then_some(agent);
    feed.event(
        &event(
            agent,
            AgentEvent::ToolCallFormatted {
                tool_call_id: id,
                tool_name: tool.into(),
                parameters: parameters
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.clone()))
                    .collect::<HashMap<_, _>>(),
            },
        ),
        label,
    );
    id
}

pub(super) fn result(feed: &Feed, agent: &str, id: Uuid, tool: &str, output: &str) {
    let label = (agent != "main").then_some(agent);
    feed.event(
        &event(
            agent,
            AgentEvent::ToolResult {
                tool_call_id: id,
                tool_name: tool.into(),
                output: output.into(),
            },
        ),
        label,
    );
}

fn block(id: u64, block: usize) -> impl Fn(&RowKey) -> bool {
    move |key| matches!(key, RowKey::Block { id: row, block: b, .. } if *row == id && *b == block)
}

/// The ids of the feed's items, in order.
pub(super) fn item_ids(feed: &Feed) -> Vec<(u64, Kind)> {
    feed.subscribe()
        .0
        .items
        .iter()
        .map(|item| (item.id, item.kind))
        .collect()
}

#[gpui_kit::test]
fn a_turn_shows_as_rows_of_each_kind(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    feed.user("Run the tests", Source::App, Delivery::Turn);
    feed.turn_started();
    think(&feed, "The tests live in the workspace.");
    say(&feed, "main", "I'll run them.\n\n");
    let run = call(&feed, "main", "shell", &[("command", "cargo test".into())]);
    settle(cx, window);
    let ids = item_ids(&feed);
    let (user, thinking, text, shell) = (ids[0].0, ids[1].0, ids[2].0, ids[3].0);
    assert_eq!(
        keys(&view, cx),
        [
            RowKey::User(user),
            RowKey::Thinking(thinking),
            RowKey::Block {
                id: text,
                generation: 0,
                block: 0,
                source: false
            },
            RowKey::Call(shell),
        ]
    );
    cx.read(|cx| {
        let entry = view.read(cx).model.entry(shell).unwrap();
        assert_eq!(entry.item.status, Some(CallStatus::Running));
        assert_eq!(entry.item.tool.as_deref(), Some("shell"));
    });
    result(
        &feed,
        "main",
        run,
        "shell",
        "exit code 0, 2.0 s\nrunning 3 tests\ntest result: ok. 3 passed; 0 failed",
    );
    feed.notice(Level::Warn, "rate limited, retrying in 2 s (attempt 1)");
    say(&feed, "main", "All 3 pass.");
    feed.reply("All 3 pass.");
    feed.turn_ended("12 s · $0.034 (session $0.21)", None);
    settle(cx, window);
    let ids = item_ids(&feed);
    let kinds: Vec<Kind> = ids.iter().map(|(_, kind)| *kind).collect();
    assert_eq!(
        kinds,
        [
            Kind::User,
            Kind::Thinking,
            Kind::Text,
            Kind::Call,
            Kind::Result,
            Kind::Notice,
            Kind::Reply,
            Kind::Turn
        ]
    );
    let (notice, reply, turn) = (ids[5].0, ids[6].0, ids[7].0);
    let rows = keys(&view, cx);
    assert_eq!(
        rows[3..],
        [
            RowKey::Call(shell),
            RowKey::Notice(notice),
            RowKey::Block {
                id: reply,
                generation: 0,
                block: 0,
                source: false
            },
            RowKey::Actions(reply),
            RowKey::Turn(turn),
        ]
    );
    cx.read(|cx| {
        let view = view.read(cx);
        let call = view.model.entry(shell).unwrap();
        assert_eq!(call.item.status, Some(CallStatus::Ok));
        assert!(
            call.result.is_some(),
            "the output block goes under its call"
        );
        // The thinking ended when the text began.
        let thinking = view.model.position(thinking).unwrap();
        assert!(!view.model.is_streaming(thinking));
    });
}

#[gpui_kit::test]
fn updates_wait_for_the_next_frame_and_come_all_at_once(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let before = cx.read(|cx| view.read(cx).render_count());
    say(&feed, "main", "a");
    for _ in 0..500 {
        say(&feed, "main", "b");
    }
    // Drained off the UI thread, and applied only when the view draws: the
    // inbox asks for one frame (the test platform may draw it at once).
    cx.run_until_parked();
    let (entries, renders) = cx.read(|cx| {
        let view = view.read(cx);
        (view.model.entries().len(), view.render_count())
    });
    assert_eq!(entries, renders - before, "applied only by a frame");
    if renders == before {
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.model.entries().len(), 1);
        assert_eq!(view.model.entries()[0].item.text.len(), 501);
        assert_eq!(view.render_count(), before + 1);
    });
}

#[gpui_kit::test]
fn finished_blocks_keep_their_rows_while_text_streams_and_ends(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    feed.turn_started();
    let answer = "Here is the plan.\n\n## Steps\n\n- one\n- two\n\nThat's all.\n";
    let mut views = Vec::new();
    for chunk in answer.as_bytes().chunks(5) {
        say(&feed, "main", std::str::from_utf8(chunk).unwrap());
        settle(cx, window);
        let current: Vec<_> = cx.read(|cx| {
            let view = view.read(cx);
            let id = view.model.entries()[0].item.id;
            view.ui[&id]
                .doc
                .as_ref()
                .unwrap()
                .blocks
                .iter()
                .filter_map(|block| block.view.as_ref().map(|view| view.entity_id()))
                .collect()
        });
        // Every block view made so far is still there, in order.
        assert!(current.starts_with(&views[..views.len().saturating_sub(1)]));
        views = current;
    }
    assert_eq!(
        views.len(),
        4,
        "a paragraph, a heading, a list, a paragraph"
    );
    let rows_before = keys(&view, cx);
    // The feed ends the reply with its text trimmed: nothing is rebuilt.
    feed.reply(answer.trim());
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        let entry = &view.model.entries()[0];
        assert_eq!(entry.item.kind, Kind::Reply);
        let doc = view.ui[&entry.item.id].doc.as_ref().unwrap();
        assert_eq!(doc.generation, 0);
        let now: Vec<_> = doc
            .blocks
            .iter()
            .filter_map(|b| b.view.as_ref().map(|view| view.entity_id()))
            .collect();
        assert_eq!(now, views);
    });
    let mut expected = rows_before;
    let reply = item_ids(&feed)[0].0;
    expected.push(RowKey::Actions(reply));
    assert_eq!(keys(&view, cx), expected);
}

/// Scrolls so that the row at `ix` is at the top, as the reader would.
fn scroll_to_row(cx: &mut TestAppContext, view: &Entity<TranscriptView>, ix: usize) {
    view.update(cx, |view, cx| {
        view.list.scroll_to(ix, px(0.));
        cx.notify();
    });
}

/// Where the row `key` is drawn, if it is.
fn row_top(cx: &mut TestAppContext, window: AnyWindowHandle, key: RowKey) -> Option<f32> {
    cx.update_window(window, |_, window, _| {
        window
            .try_find(super::row::element_id(key))
            .filter(|row| row.visible())
            .map(|row| f32::from(row.bounds().origin.y))
    })
    .unwrap()
}

#[gpui_kit::test]
fn the_view_follows_the_bottom_until_the_reader_scrolls_up(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    for ix in 0..40 {
        feed.user(&format!("message {ix}"), Source::App, Delivery::Turn);
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 300.);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    let user = |feed: &Feed, ix: usize| RowKey::User(item_ids(feed)[ix].0);
    assert!(row_top(cx, window, user(&feed, 39)).is_some());
    assert!(row_top(cx, window, user(&feed, 0)).is_none());
    // At the bottom, new rows are followed.
    feed.user("one more", Source::App, Delivery::Turn);
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    assert!(row_top(cx, window, user(&feed, 40)).is_some());
    // The reader scrolls up: new rows don't move what they read.
    scroll_to_row(cx, &view, 5);
    settle(cx, window);
    assert!(!cx.read(|cx| view.read(cx).is_following(cx)));
    let reading = user(&feed, 6);
    let before = row_top(cx, window, reading).expect("row 6 on screen");
    for ix in 0..3 {
        feed.user(&format!("while away {ix}"), Source::App, Delivery::Turn);
        settle(cx, window);
    }
    assert_eq!(row_top(cx, window, reading), Some(before));
    assert!(row_top(cx, window, user(&feed, 43)).is_none());
    assert_eq!(cx.read(|cx| view.read(cx).new_while_away), 3);
    // Back to the latest: followed again, and the count clears.
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    assert_eq!(cx.read(|cx| view.read(cx).new_while_away), 0);
    assert!(row_top(cx, window, user(&feed, 43)).is_some());
}

/// `count` turns of a user message, `calls` file reads, a two-paragraph
/// reply and a footer.
fn turns(feed: &Feed, count: usize, calls: usize) {
    for turn in 0..count {
        feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        for file in 0..calls {
            let id = call(
                feed,
                "main",
                "read_file",
                &[("path", format!("src/f{turn}_{file}.rs").into())],
            );
            result(feed, "main", id, "read_file", "1 | fn a() {}\n");
        }
        feed.reply(&format!(
            "Answer {turn}, first paragraph.\n\nAnswer {turn}, **second** paragraph."
        ));
        feed.turn_ended("3 s · $0.01", None);
    }
}

/// The row of the user's message of turn `turn`.
fn user_row(feed: &Feed, turn: usize) -> RowKey {
    let users: Vec<u64> = item_ids(feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::User)
        .map(|(id, _)| id)
        .collect();
    RowKey::User(users[turn])
}

/// Draws until every entry is ready to show.
pub(super) fn load_all(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
) {
    for _ in 0..40 {
        settle(cx, window);
        if cx.read(|cx| view.read(cx).loaded_from.is_none()) {
            return;
        }
    }
}

/// Scrolls so that the row `key` is at the top, and returns its top.
fn scroll_to_key(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    key: RowKey,
) -> f32 {
    let ix = keys(view, cx).iter().position(|row| *row == key).unwrap();
    scroll_to_row(cx, view, ix);
    settle(cx, window);
    row_top(cx, window, key).expect("scrolled to it")
}

/// Changes the preferences while the reader reads turn 30, and checks
/// that turn 30 stays where it was.
fn change_while_reading(cx: &mut TestAppContext, change: impl Fn(&mut prefs::Prefs)) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 60, 4);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    let reading = user_row(&feed, 30);
    let top = scroll_to_key(cx, window, &view, reading);
    cx.update(|cx| prefs::update(cx, &change));
    settle(cx, window);
    assert_eq!(row_top(cx, window, reading), Some(top));
    assert!(!cx.read(|cx| view.read(cx).is_following(cx)));
}

#[gpui_kit::test]
fn changing_the_view_while_scrolled_up_keeps_the_reader_at_their_turn(cx: &mut TestAppContext) {
    change_while_reading(cx, |prefs| prefs.view = View::Quiet);
}

#[gpui_kit::test]
fn the_messages_view_keeps_the_reader_at_their_turn(cx: &mut TestAppContext) {
    change_while_reading(cx, |prefs| prefs.view = View::Messages);
}

#[gpui_kit::test]
fn turning_markdown_off_while_scrolled_up_keeps_the_reader_in_place(cx: &mut TestAppContext) {
    change_while_reading(cx, |prefs| prefs.appearance.render_markdown = false);
}

#[gpui_kit::test]
fn the_source_of_a_long_reply_shows_where_the_reader_reads(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Explain", Source::App, Delivery::Turn);
    let text: String = (0..30)
        .map(|n| format!("Paragraph {n} of the answer, with **bold** words.\n\n"))
        .collect();
    feed.reply(&text);
    // More below, so that the screen isn't the end of the transcript.
    turns(&feed, 10, 0);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 300.);
    let reply = item_ids(&feed)[1].0;
    let block = |n: usize, source: bool| RowKey::Block {
        id: reply,
        generation: 0,
        block: n,
        source,
    };
    let top = scroll_to_key(cx, window, &view, block(25, false));
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    settle(cx, window);
    assert_eq!(row_top(cx, window, block(25, true)), Some(top));
}

#[gpui_kit::test]
fn a_live_quiet_run_keeps_the_call_the_reader_opened(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    let feed = Feed::new("main");
    feed.user("Run the tests", Source::App, Delivery::Turn);
    feed.turn_started();
    let output: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    for n in 0..3 {
        let id = call(
            &feed,
            "main",
            "shell",
            &[("command", format!("cargo test -p c{n}").into())],
        );
        result(
            &feed,
            "main",
            id,
            "shell",
            &format!("exit code 0, 1.0 s\n{output}"),
        );
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    let calls: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Call)
        .map(|(id, _)| id)
        .collect();
    let third = calls[2];
    view.update(cx, |view, cx| view.toggle(third, false, cx));
    settle(cx, window);
    let top = scroll_to_key(cx, window, &view, RowKey::Call(third));
    // A fourth call starts: the run folds into a group, which stays open
    // on the call the reader reads.
    call(&feed, "main", "shell", &[("command", "cargo build".into())]);
    settle(cx, window);
    let rows = keys(&view, cx);
    assert!(rows.contains(&RowKey::Group(calls[0])), "{rows:?}");
    assert_eq!(row_top(cx, window, RowKey::Call(third)), Some(top));
}

#[gpui_kit::test]
fn opening_a_call_at_the_bottom_keeps_it_in_place(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 6, 1);
    feed.user("Run it", Source::App, Delivery::Turn);
    feed.turn_started();
    let output: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    let id = call(&feed, "main", "shell", &[("command", "cargo test".into())]);
    result(
        &feed,
        "main",
        id,
        "shell",
        &format!("exit code 1, 1.0 s\n{output}"),
    );
    feed.notice(Level::Info, "Done.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    let shell = item_ids(&feed)
        .into_iter()
        .rev()
        .find(|(_, kind)| *kind == Kind::Call)
        .unwrap()
        .0;
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    let before = row_top(cx, window, RowKey::Call(shell)).expect("on screen");
    cx.update_window(window, |_, window, cx| {
        window.click(("call-row", shell), cx)
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(row_top(cx, window, RowKey::Call(shell)), Some(before));
    // Back at the bottom, the view follows again.
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
}

#[gpui_kit::test]
fn showing_all_of_a_code_block_at_the_bottom_keeps_its_top(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 6, 1);
    let code: String = (1..=120).map(|n| format!("let x{n} = {n};\n")).collect();
    feed.reply(&format!("Here:\n\n```rust\n{code}```"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let reply = item_ids(&feed).last().unwrap().0;
    let key = RowKey::Block {
        id: reply,
        generation: 0,
        block: 1,
        source: false,
    };
    let before = row_top(cx, window, key).expect("on screen");
    cx.update_window(window, |_, window, cx| {
        window
            .within(super::row::element_id(key))
            .click("show-all", cx)
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(row_top(cx, window, key), Some(before));
    let _ = view;
}

#[gpui_kit::test]
fn a_long_code_block_in_a_list_item_is_capped_too(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let code: String = (1..=200).map(|n| format!("   let x{n} = {n};\n")).collect();
    feed.reply(&format!(
        "Steps:\n\n1. Run this:\n\n   ```rust\n{code}   ```\n2. Done.\n"
    ));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 2000.);
    let reply = item_ids(&feed)[0].0;
    let list = RowKey::Block {
        id: reply,
        generation: 0,
        block: 1,
        source: false,
    };
    let (height, show_all) = cx
        .update_window(window, |_, window, _| {
            let height = window
                .try_find(super::row::element_id(list))
                .map(|row| f32::from(row.bounds().size.height));
            let show_all = window
                .within(super::row::element_id(list))
                .try_find("show-all")
                .is_some();
            (height, show_all)
        })
        .unwrap();
    assert!(show_all);
    // Forty lines, not two hundred.
    assert!(height.is_some_and(|height| height < 1200.), "{height:?}");
    cx.read(|cx| {
        let doc = view.read(cx).ui[&reply].doc.as_ref().unwrap();
        assert_eq!(doc.blocks[1].code_lines, 200);
    });
}

#[gpui_kit::test]
fn tables_scroll_sideways_in_their_row(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| {
        let style = super::row::text_style(cx);
        assert_eq!(style.table().overflow.x, Some(gpui_kit::Overflow::Scroll));
    });
}

/// The ids of the entries of `kind`, oldest first.
fn ids_of(feed: &Feed, kind: Kind) -> Vec<u64> {
    item_ids(feed)
        .into_iter()
        .filter(|(_, of)| *of == kind)
        .map(|(id, _)| id)
        .collect()
}

#[gpui_kit::test]
fn a_replys_actions_stay_while_the_pointer_moves_onto_them_and_work(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Explain", Source::App, Delivery::Turn);
    feed.reply("First paragraph.\n\nSecond **paragraph**.");
    feed.user("Thanks", Source::App, Delivery::Turn);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let thanks = ids_of(&feed, Kind::User)[1];
    let hovered = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).hovered);
    let text = keys(&view, cx).into_iter().find(block(reply, 1)).unwrap();
    cx.update_window(window, |_, window, cx| {
        window.hover(super::row::element_id(text), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(reply));
    // From the text down onto the actions, they stay, and they work.
    cx.update_window(window, |_, window, cx| {
        window.hover(("copy-reply", reply), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(reply));
    cx.update_window(window, |_, window, cx| {
        window.click(("copy-reply", reply), cx)
    })
    .unwrap();
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some("First paragraph.\nSecond paragraph.")
    );
    cx.update_window(window, |_, window, cx| {
        window.click(("copy-markdown", reply), cx)
    })
    .unwrap();
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some("First paragraph.\n\nSecond **paragraph**.")
    );
    cx.update_window(window, |_, window, cx| {
        window.click(("view-source", reply), cx)
    })
    .unwrap();
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).ui[&reply].source));
    // Onto the next message: its Copy shows instead, and copies it.
    cx.update_window(window, |_, window, cx| {
        window.hover(super::row::element_id(RowKey::User(thanks)), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(thanks));
    cx.update_window(window, |_, window, cx| {
        window.click(("copy-user", thanks), cx)
    })
    .unwrap();
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("Thanks".to_string())
    );
    // Off the entry, the actions stay a moment, then go.
    view.update(cx, |view, cx| {
        view.row_hovered(RowKey::User(thanks), false, cx)
    });
    cx.run_until_parked();
    assert_eq!(hovered(cx), Some(thanks));
    cx.executor()
        .advance_clock(super::HOVER_GRACE + std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert_eq!(hovered(cx), None);
}

#[gpui_kit::test]
fn tab_reaches_the_transcript_and_its_thumbnails_but_not_hidden_actions(cx: &mut TestAppContext) {
    use arko::images::ImageRef;
    start(cx);
    let feed = Feed::new("main");
    let mut user = arko::remote::Item::new(Kind::User, "What does this show?");
    user.id = 1;
    user.images = vec![ImageRef {
        id: 1,
        mime: "image/png".into(),
        label: "a.png".into(),
        bytes: 10,
    }];
    let mut reply = arko::remote::Item::new(Kind::Reply, "A picture.");
    reply.id = 2;
    feed.restore(vec![user, reply]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    view.update(cx, |view, cx| {
        view.set_image_lookup(
            |_| Some((Arc::<[u8]>::from(PNG), "image/png".to_string())),
            cx,
        )
    });
    settle(cx, window);
    let thumbnail: ElementId = ("image", 1u64).into();
    let focused = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            (
                window.find("transcript").focused(),
                window.find(thumbnail.clone()).focused(),
            )
        })
        .unwrap()
    };
    let tab = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.focus_next(cx))
            .unwrap();
        settle(cx, window);
    };
    let focus = cx.read(|cx| view.read(cx).focus_handle.clone());
    cx.update_window(window, |_, window, cx| focus.focus(window, cx))
        .unwrap();
    settle(cx, window);
    assert_eq!(focused(cx), (Some(true), Some(false)));
    tab(cx);
    assert_eq!(focused(cx), (Some(true), Some(true)));
    let tile = cx
        .update_window(window, |_, window, _| {
            let tile = window.find(thumbnail.clone());
            (tile.role(), tile.label().map(str::to_string))
        })
        .unwrap();
    assert_eq!(tile.0, Some(gpui_kit::Role::Button));
    assert_eq!(tile.1.as_deref(), Some("Open image a.png · 10 B"));
    // The message's Copy and the reply's actions are hidden: Tab goes
    // round to the transcript.
    tab(cx);
    assert_eq!(focused(cx), (Some(true), Some(false)));
}

#[gpui_kit::test]
fn toggles_say_whether_they_are_open(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let id = call(&feed, "main", "shell", &[("command", "cargo test".into())]);
    result(&feed, "main", id, "shell", "exit code 0, 1.0 s\nok");
    let (window, _) = open(cx, &feed, AgentFilter::Main, 720.);
    let shell = ids_of(&feed, Kind::Call)[0];
    let toggle = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            let toggle = window.find(("call-toggle", shell));
            (
                toggle.role(),
                toggle.expanded(),
                toggle.label().map(str::to_string),
            )
        })
        .unwrap()
    };
    let (role, expanded, label) = toggle(cx);
    assert_eq!(role, Some(gpui_kit::Role::Button));
    assert_eq!(expanded, Some(false));
    assert!(label.is_some_and(|label| label.contains("cargo test")));
    // Tab goes from the transcript to the toggle, and Enter opens it.
    cx.update_window(window, |_, window, cx| {
        window.focus_next(cx);
        window.render_frame(cx);
        window.focus_next(cx);
        window.render_frame(cx);
        assert_eq!(window.find(("call-toggle", shell)).focused(), Some(true));
        window.press("enter", cx);
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(toggle(cx).1, Some(true));
    cx.update_window(window, |_, window, cx| {
        window.click(("call-toggle", shell), cx)
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(toggle(cx).1, Some(false));
}

#[gpui_kit::test]
fn the_keyboard_scrolls_the_transcript(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 20, 1);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    let following = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).is_following(cx));
    let press = |cx: &mut TestAppContext, key: &str| {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        settle(cx, window);
    };
    let focus = cx.read(|cx| view.read(cx).focus_handle.clone());
    cx.update_window(window, |_, window, cx| focus.focus(window, cx))
        .unwrap();
    let last = user_row(&feed, 19);
    let bottom = row_top(cx, window, last).expect("the last turn shows");
    press(cx, "pageup");
    assert!(!following(cx));
    let moved = row_top(cx, window, last).map_or(f32::INFINITY, |top| top - bottom);
    assert!(moved > 300., "{moved}");
    press(cx, "pagedown");
    assert_eq!(row_top(cx, window, last), Some(bottom));
    press(cx, "home");
    assert!(row_top(cx, window, user_row(&feed, 0)).is_some());
    assert!(row_top(cx, window, last).is_none());
    press(cx, "end");
    assert!(following(cx));
    assert_eq!(row_top(cx, window, last), Some(bottom));
}

#[gpui_kit::test]
fn rows_draw_without_making_their_ids_or_the_users_text_again(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    // Long enough to be kept on the heap, not in the string itself.
    let message = "A message of the user's. ".repeat(10);
    feed.user(&message, Source::App, Delivery::Turn);
    feed.reply("A reply.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let user = ids_of(&feed, Kind::User)[0];
    let block = keys(&view, cx)
        .into_iter()
        .find(|key| matches!(key, RowKey::Block { .. }))
        .unwrap();
    assert!(matches!(
        super::row::element_id(block),
        ElementId::NamedInteger(..)
    ));
    let text =
        |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).ui[&user].text.clone().unwrap());
    let first = text(cx);
    for _ in 0..3 {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    assert_eq!(text(cx).as_ptr(), first.as_ptr());
    assert_eq!(text(cx), message.trim());
}

/// How many times the transcript draws in a second of the test clock.
fn draws_in_a_second(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> usize {
    for _ in 0..200 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
    }
    let before = cx.read(|cx| view.read(cx).render_count());
    for _ in 0..1000 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1));
        cx.run_until_parked();
    }
    cx.read(|cx| view.read(cx).render_count()) - before
}

#[gpui_kit::test]
fn one_indicator_turns_at_a_time(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| {
        prefs::update(cx, |prefs| prefs.appearance.motion = prefs::Motion::On);
        cx.set_reduce_motion(false);
    });
    let feed = Feed::new("main");
    turns(&feed, 4, 1);
    feed.user("Build", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Write the fuzz target", WorkerChange::Started);
    call(&feed, "main", "shell", &[("command", "cargo build".into())]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    // A worker and a call run: the call's indicator turns, a dozen times a
    // second, and the elapsed times tick once.
    let draws = draws_in_a_second(cx, &view);
    assert!((10..=16).contains(&draws), "{draws} draws");
    // Then thinking streams: its shimmer is the one that moves.
    think(&feed, "hmm");
    settle(cx, window);
    let draws = draws_in_a_second(cx, &view);
    assert!((25..=34).contains(&draws), "{draws} draws");
    let thinking = item_ids(&feed).last().unwrap().0;
    assert_eq!(cx.read(|cx| view.read(cx).animated), Some(thinking));
}

#[gpui_kit::test]
fn the_new_count_counts_what_the_view_shows(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
    let feed = Feed::new("main");
    turns(&feed, 30, 1);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 300.);
    load_all(cx, window, &view);
    scroll_to_key(cx, window, &view, user_row(&feed, 10));
    feed.user("Next", Source::App, Delivery::Turn);
    feed.turn_started();
    for n in 0..5 {
        let id = call(
            &feed,
            "main",
            "read_file",
            &[("path", format!("x{n}").into())],
        );
        result(&feed, "main", id, "read_file", "1 | x\n");
    }
    think(&feed, "hmm");
    feed.notice(Level::Info, "Handed over.");
    settle(cx, window);
    // The message and the one line counting the calls.
    assert_eq!(cx.read(|cx| view.read(cx).new_while_away), 2);
}

#[gpui_kit::test]
fn a_long_transcript_shows_its_end_first_and_the_rest_a_chunk_a_frame(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    for ix in 0..450 {
        feed.user(&format!("message {ix}"), Source::App, Delivery::Turn);
    }
    let opened = feed.clone();
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(options(800., 400.), cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&opened, AgentFilter::Main, window, cx))
            })
        })
        .unwrap();
    // The history comes off the UI thread, and shows from the next frame
    // on: its latest 200 entries and a hundred older ones, then a hundred
    // more with each frame.
    settle(cx, window);
    let mut frames: Vec<usize> = cx.read(|cx| view.read(cx).frames.clone());
    frames.dedup();
    assert_eq!(frames, [0, 300, 400, 450]);
    // The reader stayed at the latest message all along.
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    assert!(row_top(cx, window, RowKey::User(item_ids(&feed)[449].0)).is_some());
}

#[gpui_kit::test]
fn block_views_far_off_screen_are_let_go(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    // 400 reply blocks.
    turns(&feed, 200, 0);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    let views = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .ui
                .values()
                .filter_map(|ui| ui.doc.as_ref())
                .flat_map(|doc| &doc.blocks)
                .filter(|block| block.view.is_some())
                .count()
        })
    };
    // Up through the whole conversation, a screen at a time.
    let count = cx.read(|cx| view.read(cx).row_count());
    let mut most = 0;
    for ix in (0..count).rev().step_by(4) {
        scroll_to_row(cx, &view, ix);
        settle(cx, window);
        most = most.max(views(cx));
    }
    scroll_to_row(cx, &view, 0);
    settle(cx, window);
    assert!(row_top(cx, window, user_row(&feed, 0)).is_some());
    let bound = super::BLOCK_VIEWS + super::RELEASE_EVERY + 20;
    assert!(most <= bound, "{most} views, more than {bound}");
    // Back at the end, its rows have views again.
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    settle(cx, window);
    let last = keys(&view, cx)
        .into_iter()
        .rev()
        .find(|key| matches!(key, RowKey::Block { .. }));
    assert!(row_top(cx, window, last.unwrap()).is_some());
}

#[gpui_kit::test]
fn a_transcript_not_drawn_takes_its_updates_a_moment_after_they_come(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 5, 1);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let rows = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).row_count());
    let renders = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).render_count());
    // Another session shows: the transcript is kept, not drawn.
    cx.update_window(window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    let (before, drawn) = (rows(cx), renders(cx));
    // A hundred turns come, in bursts, half a second apart.
    for _ in 0..10 {
        turns(&feed, 10, 1);
        cx.run_until_parked();
        cx.executor().advance_clock(super::inbox::CATCH_UP);
        cx.run_until_parked();
    }
    assert_eq!(renders(cx), drawn);
    // They're made into rows as they come: nothing waits for the frame
    // that shows the transcript again.
    assert!(rows(cx) >= before + 100 * 5, "{} rows", rows(cx));
    let waiting = cx.read(|cx| view.read(cx).inbox.take());
    assert!(waiting.snapshot.is_none() && waiting.updates.is_empty());
}

#[gpui_kit::test]
fn a_truncate_and_another_conversation_drop_rows(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("first", Source::App, Delivery::Turn);
    feed.reply("one");
    feed.user("second", Source::App, Delivery::Turn);
    feed.reply("two");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    assert_eq!(keys(&view, cx).len(), 6);
    let second = item_ids(&feed)[2].0;
    feed.truncate(second);
    settle(cx, window);
    let first = item_ids(&feed)[0].0;
    assert_eq!(keys(&view, cx)[0], RowKey::User(first));
    assert_eq!(keys(&view, cx).len(), 3);
    feed.conversation("Conversation 2 · Fonts");
    feed.user("third", Source::App, Delivery::Turn);
    settle(cx, window);
    let ids = item_ids(&feed);
    let marker = ids
        .iter()
        .find(|(_, kind)| *kind == Kind::Conversation)
        .unwrap()
        .0;
    let third = ids.last().unwrap().0;
    assert_eq!(
        keys(&view, cx),
        [RowKey::Separator(marker), RowKey::User(third)]
    );
}

/// The app's icon, a PNG.
const PNG: &[u8] = include_bytes!("../../packaging/linux/com.mesahillsresearch.ArkoStudio.png");

#[gpui_kit::test]
fn images_show_as_thumbnails_that_open_the_viewer(cx: &mut TestAppContext) {
    use arko::images::ImageRef;
    use gpui_kit::component::WindowExt as _;

    start(cx);
    let feed = Feed::new("main");
    let image = |id: u64, label: &str| ImageRef {
        id,
        mime: "image/png".into(),
        label: label.into(),
        bytes: PNG.len(),
    };
    let mut user = arko::remote::Item::new(Kind::User, "What does this icon show?");
    user.id = 1;
    user.images = vec![image(1, "icon.png")];
    let mut call = arko::remote::Item::new(Kind::Call, "read shots/");
    call.id = 2;
    call.tool = Some("read_file".into());
    call.status = Some(CallStatus::Ok);
    call.images = vec![image(2, "shots/a.png"), image(3, "shots/b.png")];
    feed.restore(vec![user, call]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    view.update(cx, |view, cx| {
        // The store kept the first two only.
        view.set_image_lookup(
            |id| (id < 3).then(|| (Arc::<[u8]>::from(PNG), "image/png".to_string())),
            cx,
        )
    });
    settle(cx, window);
    let tile = |id: u64| -> ElementId { ("image", id).into() };
    cx.update_window(window, |_, window, cx| {
        for id in [1, 2, 3] {
            assert!(window.try_find(tile(id)).is_some(), "{id}");
        }
        assert!(!window.has_active_image_viewer(cx));
        window.click(tile(2), cx);
        assert!(window.has_active_image_viewer(cx));
    })
    .unwrap();
}

#[gpui_kit::test]
fn images_are_read_from_the_store_as_they_show(cx: &mut TestAppContext) {
    use arko::images::ImageRef;
    use gpui_kit::component::WindowExt as _;
    use std::cell::RefCell;
    use std::collections::HashSet;
    use std::rc::Rc;

    start(cx);
    let feed = Feed::new("main");
    let calls = (1..=40u64)
        .map(|id| {
            let mut call = arko::remote::Item::new(Kind::Call, format!("read shots/{id}.png"));
            call.id = id;
            call.tool = Some("read_file".into());
            call.status = Some(CallStatus::Ok);
            call.images = vec![ImageRef {
                id,
                mime: "image/png".into(),
                label: format!("shots/{id}.png"),
                bytes: PNG.len(),
            }];
            call
        })
        .collect();
    feed.restore(calls);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let store: Rc<RefCell<HashSet<u64>>> = Rc::new(RefCell::new((1..=40).collect()));
    let kept = store.clone();
    view.update(cx, |view, cx| {
        view.set_image_lookup(
            move |id| {
                kept.borrow()
                    .contains(&id)
                    .then(|| (Arc::<[u8]>::from(PNG), "image/png".to_string()))
            },
            cx,
        )
    });
    settle(cx, window);
    let copies = |cx: &mut TestAppContext| -> Vec<u64> {
        cx.read(|cx| {
            let view = view.read(cx);
            let copies = view.image_copies.borrow();
            copies.images.iter().map(|(id, _)| *id).collect()
        })
    };
    // The tiles drawn, at the end of the conversation.
    let drawn = copies(cx);
    assert!(!drawn.is_empty() && drawn.len() < 10, "{drawn:?}");
    assert!(drawn.contains(&40));
    // The viewer, which can browse all 40, reads only the one it shows.
    cx.update_window(window, |_, window, cx| {
        window.click(("image", 40u64), cx);
        assert!(window.has_active_image_viewer(cx));
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(copies(cx).len(), drawn.len());
    // The store lets an image go: its tile says so, and its copy goes.
    store.borrow_mut().remove(&40);
    view.update(cx, |_, cx| cx.notify());
    settle(cx, window);
    assert!(!copies(cx).contains(&40));
}

#[gpui_kit::test]
fn thumbnails_are_decoded_for_the_zoom(cx: &mut TestAppContext) {
    use arko::images::ImageRef;

    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.zoom = 1.5));
    let feed = Feed::new("main");
    let mut user = arko::remote::Item::new(Kind::User, "This icon");
    user.id = 1;
    user.images = vec![ImageRef {
        id: 1,
        mime: "image/png".into(),
        label: "icon.png".into(),
        bytes: PNG.len(),
    }];
    feed.restore(vec![user]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    view.update(cx, |view, cx| {
        view.set_image_lookup(
            |_| Some((Arc::<[u8]>::from(PNG), "image/png".to_string())),
            cx,
        )
    });
    settle(cx, window);
    let (rem, scale) = cx
        .update_window(window, |_, window, _| {
            (f32::from(window.rem_size()), window.scale_factor())
        })
        .unwrap();
    assert!(rem > 16., "{rem}");
    // The 256 px icon, fit in a tile 7.5 rem high on the screen, and never
    // made larger than it is.
    let side = (super::row::THUMBNAIL.1 * rem * scale).min(256.) as usize;
    let used = cx.read(|cx| view.read(cx).image_cache.read(cx).used_bytes());
    assert_eq!(used, side * side * 4);
}

#[test]
fn image_copies_keep_the_latest_within_their_budget() {
    use super::{IMAGE_COPIES, ImageCopies, ImageLookup};
    use gpui_kit::ImageFormat;
    use std::rc::Rc;

    // Ten images of 4 MB: the copies keep the last ones drawn, within
    // 32 MB.
    let lookup: ImageLookup =
        Rc::new(|id| Some((Arc::from(vec![id as u8; 4 << 20]), ImageFormat::Png)));
    let mut copies = ImageCopies::default();
    for id in 1..=10 {
        assert!(copies.get(id, &lookup).is_some());
    }
    assert!(copies.bytes <= IMAGE_COPIES, "{}", copies.bytes);
    let kept: Vec<u64> = copies.images.iter().map(|(id, _)| *id).collect();
    assert_eq!(kept, [3, 4, 5, 6, 7, 8, 9, 10]);
    // Drawn again, an image is the latest, and the oldest goes next.
    copies.get(3, &lookup);
    copies.get(11, &lookup);
    let kept: Vec<u64> = copies.images.iter().map(|(id, _)| *id).collect();
    assert_eq!(kept, [5, 6, 7, 8, 9, 10, 3, 11]);
}

#[gpui_kit::test]
fn a_failed_responses_text_is_taken_back_and_its_retry_streams_anew(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    say(&feed, "main", "Before.\n\n");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let started = || AgentEvent::ModelRequestStarted {
        model: "gpt-6-luna".into(),
        subagent: None,
        sizes: None,
        bytes_per_token: 3.0,
    };
    feed.event(&event("main", started()), None);
    say(&feed, "main", "Broken");
    think(&feed, "Hmm");
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).model.entries().len()), 2);
    feed.event(
        &event(
            "main",
            AgentEvent::StreamedOutputDiscarded {
                text_bytes: 6,
                reasoning_bytes: 3,
            },
        ),
        None,
    );
    feed.event(&event("main", started()), None);
    say(&feed, "main", "Fixed.");
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        let entries = view.model.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].item.text, "Before.\n\nFixed.");
        let doc = view.ui[&entries[0].item.id].doc.as_ref().unwrap();
        assert_eq!(doc.text, "Before.\n\nFixed.");
        assert_eq!(doc.blocks.len(), 2);
    });
}

#[gpui_kit::test]
fn a_cache_refresh_shows_as_one_quiet_row(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Hi", Source::App, Delivery::Turn);
    feed.reply("Hello.");
    feed.user(
        arko::remote::REFRESH_MESSAGE,
        Source::App,
        Delivery::Refresh,
    );
    feed.turn_started();
    say(&feed, "main", "Acknowledged");
    feed.reply("Acknowledged");
    feed.turn_ended("1 s · $0.001", None);
    let (_, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let refresh = item_ids(&feed)[2].0;
    let rows = keys(&view, cx);
    assert_eq!(rows.last(), Some(&RowKey::Refresh(refresh)));
    assert_eq!(
        rows.iter()
            .filter(|key| matches!(key, RowKey::Block { .. }))
            .count(),
        1,
        "only the first reply's text"
    );
}

#[gpui_kit::test]
fn the_views_show_more_or_less_of_the_same_feed(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Look around", Source::App, Delivery::Turn);
    feed.turn_started();
    for file in ["a.rs", "b.rs", "c.rs", "d.rs"] {
        let id = call(&feed, "main", "read_file", &[("path", file.into())]);
        result(&feed, "main", id, "read_file", "1 | fn main() {}\n");
    }
    feed.reply("Four files.");
    feed.turn_ended("3 s", None);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let ids = item_ids(&feed);
    let first_call = ids[1].0;
    let full = keys(&view, cx);
    assert_eq!(
        full.iter()
            .filter(|key| matches!(key, RowKey::Call(_)))
            .count(),
        4
    );
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    settle(cx, window);
    let quiet = keys(&view, cx);
    assert!(quiet.contains(&RowKey::Group(first_call)));
    assert!(!quiet.iter().any(|key| matches!(key, RowKey::Call(_))));
    // Opening the group shows its calls.
    view.update(cx, |view, cx| view.toggle(first_call, false, cx));
    settle(cx, window);
    assert_eq!(
        keys(&view, cx)
            .iter()
            .filter(|key| matches!(key, RowKey::Call(_)))
            .count(),
        4
    );
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
    settle(cx, window);
    let messages = keys(&view, cx);
    assert!(messages.contains(&RowKey::Count(first_call)));
    assert!(!messages.iter().any(|key| matches!(key, RowKey::Turn(_))));
    // Its count opens the full view at the calls.
    view.update(cx, |view, cx| view.open_run(first_call, cx));
    settle(cx, window);
    assert_eq!(cx.read(|cx| prefs::get(cx).view), View::Full);
    assert_eq!(keys(&view, cx), full);
}

#[gpui_kit::test]
fn thinking_shimmers_while_it_streams_then_says_how_long(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    think(&feed, "Weighing the options");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let thinking = item_ids(&feed)[0].0;
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(view.model.is_streaming(0));
    });
    // Folded by default: one row. Opened: its text's blocks follow.
    assert_eq!(keys(&view, cx), [RowKey::Thinking(thinking)]);
    view.update(cx, |view, cx| view.toggle(thinking, false, cx));
    settle(cx, window);
    assert!(keys(&view, cx).iter().any(block(thinking, 0)));
    say(&feed, "main", "Done thinking.");
    settle(cx, window);
    cx.read(|cx| assert!(!view.read(cx).model.is_streaming(0)));
    // "Expand thinking" opens it by default in the Full view only.
    let (window, other) = open(cx, &feed, AgentFilter::Main, 720.);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.expand_thinking = true));
    settle(cx, window);
    assert!(keys(&other, cx).iter().any(block(thinking, 0)));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    settle(cx, window);
    assert!(!keys(&other, cx).iter().any(block(thinking, 0)));
}

#[gpui_kit::test]
fn a_failed_call_says_why_and_its_details_open(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    let run = call(&feed, "main", "shell", &[("command", "cargo test".into())]);
    let output: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    result(
        &feed,
        "main",
        run,
        "shell",
        &format!("exit code 101, 2.0 s\n{output}"),
    );
    let edit = call(
        &feed,
        "main",
        "edit_file",
        &[
            ("path", "src/lib.rs".into()),
            ("old_string", "fn a() {}\n".into()),
            ("new_string", "fn a() -> u8 { 1 }\n".into()),
        ],
    );
    result(
        &feed,
        "main",
        edit,
        "edit_file",
        "Edited src/lib.rs at lines 3-3",
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let ids = item_ids(&feed);
    let (shell, edit_id) = (ids[0].0, ids[2].0);
    cx.read(|cx| {
        let view = view.read(cx);
        let failed = view.model.entry(shell).unwrap();
        assert_eq!(failed.item.status, Some(CallStatus::Failed));
        assert_eq!(failed.item.outcome.as_deref(), Some("exit 101"));
        let output = failed.result.as_ref().unwrap();
        assert_eq!(output.whole.as_ref().unwrap().lines().count(), 60);
        let edited = view.model.entry(edit_id).unwrap();
        assert!(edited.item.parameters.is_some());
        assert!(edited.item.output.is_some());
    });
    // Details open on a click, an edit's with its diff.
    cx.update_window(window, |_, window, cx| {
        window.click(("call-row", shell), cx);
    })
    .unwrap();
    view.update(cx, |view, cx| view.toggle(edit_id, false, cx));
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.ui[&shell].expanded, Some(true));
        let diff = view.ui[&edit_id].diff.as_ref().expect("a diff");
        let diff = diff.read(cx).diff();
        assert_eq!((diff.added_lines(), diff.removed_lines()), (1, 1));
    });
}

#[gpui_kit::test]
fn worker_cards_open_the_worker_and_its_own_transcript(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    say(&feed, "r1", "Reading the parser.");
    let read = call(&feed, "r1", "read_file", &[("path", "src/parse.rs".into())]);
    result(&feed, "r1", read, "read_file", "1 | fn parse() {}\n");
    feed.to_worker("r1", "Use plan B", Source::App);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let ids = item_ids(&feed);
    let card = ids
        .iter()
        .find(|(_, kind)| *kind == Kind::WorkerLine)
        .unwrap()
        .0;
    let rows = keys(&view, cx);
    assert!(rows.contains(&RowKey::Worker(card)));
    // The worker's text is its own, and its call shows in Full.
    assert_eq!(
        rows.iter()
            .filter(|key| matches!(key, RowKey::Block { .. }))
            .count(),
        0
    );
    assert_eq!(
        rows.iter()
            .filter(|key| matches!(key, RowKey::Call(_)))
            .count(),
        1
    );
    let opened = Arc::new(std::sync::Mutex::new(Vec::new()));
    let _subscription = cx.update({
        let opened = opened.clone();
        |cx| {
            cx.subscribe(&view, move |_, event: &TranscriptEvent, _| {
                opened.lock().unwrap().push(event.clone());
            })
        }
    });
    cx.update_window(window, |_, window, cx| {
        window.click(("worker-row", card), cx);
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *opened.lock().unwrap(),
        [TranscriptEvent::OpenWorker("r1".into())]
    );
    // The worker's own transcript: its text, its call, what the user wrote
    // to it, without the card.
    let (_, worker) = open(cx, &feed, AgentFilter::Worker("r1".into()), 720.);
    let rows = keys(&worker, cx);
    assert!(matches!(rows[0], RowKey::Block { .. }));
    assert!(matches!(rows[1], RowKey::Call(_)));
    assert!(matches!(rows[2], RowKey::User(_)));
    assert_eq!(rows.len(), 3);
}

#[gpui_kit::test]
fn long_code_blocks_show_forty_lines_until_asked(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let code: String = (1..=60).map(|n| format!("let x{n} = {n};\n")).collect();
    feed.reply(&format!("Here:\n\n```rust\n{code}```\n\nDone."));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = item_ids(&feed)[0].0;
    cx.read(|cx| {
        let view = view.read(cx);
        let doc = view.ui[&reply].doc.as_ref().unwrap();
        assert_eq!(doc.blocks.len(), 3);
        let code = doc.blocks[1].range.clone().unwrap();
        assert_eq!(super::row::code_lines(&doc.text[code]), 60);
        assert!(!view.ui[&reply].uncapped.contains(&1));
    });
    let code = RowKey::Block {
        id: reply,
        generation: 0,
        block: 1,
        source: false,
    };
    cx.update_window(window, |_, window, cx| {
        window
            .within(super::row::element_id(code))
            .click("show-all", cx);
    })
    .unwrap();
    cx.run_until_parked();
    cx.read(|cx| assert!(view.read(cx).ui[&reply].uncapped.contains(&1)));
}

#[gpui_kit::test]
fn markdown_off_and_view_source_show_the_text_as_written(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.reply("**Bold** move.\n\n- a\n- b");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = item_ids(&feed)[0].0;
    let sources = |cx: &mut TestAppContext| {
        keys(&view, cx)
            .iter()
            .filter_map(|key| match key {
                RowKey::Block { source, .. } => Some(*source),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(sources(cx), [false, false]);
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true]);
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true]);
    // Copy gives the text as seen, Copy as Markdown as written.
    view.update(cx, |view, cx| view.copy_entry(reply, true, cx));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("**Bold** move.\n\n- a\n- b".to_string())
    );
    view.update(cx, |view, cx| view.copy_entry(reply, false, cx));
    let plain = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    assert!(plain.starts_with("Bold move."), "{plain:?}");
}

#[gpui_kit::test]
fn a_selection_across_rows_copies_with_the_keyboard(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Hello there", Source::App, Delivery::Turn);
    feed.reply("General Kenobi.\n\nYou are a bold one.");
    let (window, _view) = open(cx, &feed, AgentFilter::Main, 720.);
    let ids = item_ids(&feed);
    let (user, reply) = (ids[0].0, ids[1].0);
    let bounds = |cx: &mut TestAppContext, key: RowKey| {
        cx.update_window(window, |_, window, _| {
            window.find(super::row::element_id(key)).bounds()
        })
        .unwrap()
    };
    // The user's words, in the bubble at the right of the column.
    let first = cx
        .update_window(window, |_, window, _| {
            window.find(("user-message", user)).bounds()
        })
        .unwrap();
    let last = bounds(
        cx,
        RowKey::Block {
            id: reply,
            generation: 0,
            block: 1,
            source: false,
        },
    );
    cx.update_window(window, |_, window, cx| {
        window.drag(
            first.origin + gpui_kit::point(px(20.), first.size.height / 2.),
            last.origin + gpui_kit::point(last.size.width - px(20.), last.size.height - px(4.)),
            cx,
        );
        window.press("secondary-c", cx);
    })
    .unwrap();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert!(copied.contains("General Kenobi."), "{copied:?}");
    assert!(copied.contains("bold"), "{copied:?}");
}

/// A window with a sibling of the transcript, as the composer is.
struct Column {
    sibling: Entity<Sibling>,
    transcript: Entity<TranscriptView>,
}

struct Sibling {
    keystrokes: usize,
}

impl Render for Sibling {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child(format!("{} keystrokes", self.keystrokes))
    }
}

impl Render for Column {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .child(
                div()
                    .h(px(500.))
                    .child(TranscriptView::cached(&self.transcript)),
            )
            .child(self.sibling.clone())
    }
}

#[gpui_kit::test]
fn a_keystroke_beside_the_transcript_does_not_draw_it(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Hello", Source::App, Delivery::Turn);
    feed.reply("Hi! **How** can I help?");
    let (window, column) = cx
        .update(|cx| {
            gpui_kit::open_window(options(800., 720.), cx, |window, cx| {
                let transcript =
                    cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx));
                let sibling = cx.new(|_| Sibling { keystrokes: 0 });
                cx.new(|_| Column {
                    sibling,
                    transcript,
                })
            })
        })
        .unwrap();
    let draw = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    for _ in 0..3 {
        draw(cx);
    }
    let (sibling, transcript) = cx.read(|cx| {
        let column = column.read(cx);
        (column.sibling.clone(), column.transcript.clone())
    });
    let before = cx.read(|cx| transcript.read(cx).render_count());
    for _ in 0..20 {
        sibling.update(cx, |sibling, cx| {
            sibling.keystrokes += 1;
            cx.notify();
        });
        draw(cx);
    }
    assert_eq!(cx.read(|cx| transcript.read(cx).render_count()), before);
    // What changes in the transcript draws it.
    feed.user("More", Source::App, Delivery::Turn);
    draw(cx);
    assert!(cx.read(|cx| transcript.read(cx).render_count()) > before);
}

/// Subscribing copies the session's history under the feed's lock: the
/// view takes it on a background thread and shows it with the next frame.
/// A row opened before then (as a window opening a call does) opens once
/// its entry comes.
#[gpui_kit::test]
fn the_history_comes_off_the_ui_thread_and_rows_opened_before_stay_open(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Read the notes.", Source::App, Delivery::Turn);
    let id = call(
        &feed,
        "main",
        "read_file",
        &[("path", Value::from("notes.md"))],
    );
    result(&feed, "main", id, "read_file", "Some notes.");
    let call_item = item_ids(&feed)
        .into_iter()
        .find(|(_, kind)| *kind == Kind::Call)
        .unwrap()
        .0;
    let opened = feed.clone();
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(options(800., 600.), cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&opened, AgentFilter::Main, window, cx);
                    // Made with nothing yet: the history comes later.
                    assert!(view.model.entries().is_empty());
                    view.set_expanded(call_item, true, cx);
                    view
                })
            })
        })
        .unwrap();
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.model.entries().len(), 2);
        assert_eq!(
            view.ui.get(&call_item).and_then(|ui| ui.expanded),
            Some(true)
        );
    });
}

/// A user's message is a bubble at the right of the reading column, as wide
/// as its words up to half the column. A narrow column lets it take the width
/// of a comfortable line, and a very narrow one all of it. Its actions sit in
/// a row under it, at its right edge, and showing them moves nothing.
#[gpui_kit::test]
fn a_users_message_is_a_bubble_at_the_right_of_the_column(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Now fuzz it.", Source::App, Delivery::Turn);
    feed.user(
        &"Make the parser streaming, and add a test that feeds it a byte at a time. ".repeat(3),
        Source::App,
        Delivery::Turn,
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let [short, long] = ids_of(&feed, Kind::User)[..] else {
        panic!("two messages")
    };
    let rem = cx
        .update_window(window, |_, window, _| f32::from(window.rem_size()))
        .unwrap();
    // The window's width, and the long message's widest: half the column,
    // a comfortable line, the whole column.
    type Widest = fn(f32, f32) -> f32;
    let widths: [(f32, Widest); 3] = [
        (800., |column, _| column / 2.),
        (480., |_, rem| 17.5 * rem),
        (300., |column, _| column),
    ];
    for (width, widest) in widths {
        cx.simulate_window_resize(window, size(px(width), px(600.)));
        settle(cx, window);
        cx.update_window(window, |_, window, _| {
            // The reading column: at most 46 rem, centred, with 1 rem inside
            // each side.
            let column = width.min(46. * rem) - 2. * rem;
            let right = px((width + column) / 2.);
            let short_bubble = window.find(("user-message", short)).bounds();
            let long_bubble = window.find(("user-message", long)).bounds();
            assert_eq!(short_bubble.right(), right, "{width}");
            assert_eq!(long_bubble.right(), right, "{width}");
            assert!(
                (long_bubble.size.width - px(widest(column, rem))).abs() < px(1.),
                "{width}: {long_bubble:?}"
            );
            assert!(short_bubble.size.width < long_bubble.size.width, "{width}");
            // Its actions sit under it, at its right edge.
            for (message, bubble) in [(short, short_bubble), (long, long_bubble)] {
                let copy = window.find(("copy-user", message)).bounds();
                assert_eq!(copy.right(), bubble.right(), "{width}");
                assert!(copy.top() >= bubble.bottom(), "{width}");
                assert!(copy.top() < bubble.bottom() + px(rem / 2.), "{width}");
            }
        })
        .unwrap();
    }
    // The pointer on the long message shows its actions where they were.
    let place = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            (
                window.find(("user-message", long)).bounds(),
                window.find(("copy-user", long)).bounds(),
            )
        })
        .unwrap()
    };
    let before = place(cx);
    cx.update_window(window, |_, window, cx| {
        window.hover(("user-message", long), cx)
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).hovered), Some(long));
    assert_eq!(place(cx), before);
}

/// A conversation as a session opened again reads it: the user's message,
/// then its turn's reply (or, without one, a notice that it stopped) and
/// footer. A message that isn't `changeable` can't be sent again.
fn ended_turn(replied: bool, changeable: bool) -> Feed {
    let mut message = arko::remote::Item::new(Kind::User, "Name a fruit.");
    message.id = 1;
    message.turn = Some(1);
    message.message_index = changeable.then_some(0);
    message.source = Some(Source::App);
    message.delivery = Some(Delivery::Turn);
    let mut ending = if replied {
        arko::remote::Item::new(Kind::Reply, "A pear.")
    } else {
        arko::remote::Item::new(Kind::Notice, "Stopped.").level(Level::Warn)
    };
    ending.id = 2;
    ending.turn = Some(1);
    let mut footer = arko::remote::Item::new(Kind::Turn, "3 s · $0.004 (session $0.01)");
    footer.id = 3;
    footer.turn = Some(1);
    let feed = Feed::new("main");
    feed.restore(vec![message, ending, footer]);
    feed
}

/// The right edge of the reading column's text in a window `width` wide:
/// the column is at most 46 rem, centred, with 1 rem inside each side.
fn column_right(width: f32, rem: Pixels) -> Pixels {
    let rem = f32::from(rem);
    let column = width.min(46. * rem) - 2. * rem;
    px((width + column) / 2.)
}

/// Regenerate sits alone at the right end of the latest reply's actions,
/// away from Copy, Copy as Markdown and View Source, in a wide window and
/// in a narrow one. The message it sends again offers it no longer.
#[gpui_kit::test]
fn regenerate_sits_alone_at_the_right_end_of_the_reply(cx: &mut TestAppContext) {
    start(cx);
    let feed = ended_turn(true, true);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let message = ids_of(&feed, Kind::User)[0];
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    assert_eq!(cx.read(|cx| view.read(cx).regenerate_entry()), Some(reply));
    for width in [800., 360.] {
        cx.simulate_window_resize(window, size(px(width), px(600.)));
        settle(cx, window);
        cx.update_window(window, |_, window, _| {
            let rem = window.rem_size();
            let row = window
                .find(super::row::element_id(RowKey::Actions(reply)))
                .bounds();
            let source = window.find(("view-source", reply)).bounds();
            let regenerate = window.find(("regenerate", reply)).bounds();
            assert_eq!(regenerate.right(), column_right(width, rem), "{width}");
            assert!(regenerate.right() < row.right());
            assert!(regenerate.left() - source.right() >= rem * 2., "{width}");
            assert_eq!(regenerate.top(), source.top(), "{width}");
            assert!(window.try_find(("regenerate", message)).is_none());
        })
        .unwrap();
    }
}

/// A turn that ended without a reply offers Regenerate at the right end
/// of its last line, where the reply would be: its footer, or in the
/// Messages view, which hides footers, the notice that says it stopped.
/// The line keeps its height, and the pointer over it shows Regenerate.
#[gpui_kit::test]
fn a_turn_without_a_reply_offers_regenerate_on_its_last_line(cx: &mut TestAppContext) {
    start(cx);
    // The same turn, but its message can't be sent again: no Regenerate.
    let plain = ended_turn(false, false);
    let (window, view) = open(cx, &plain, AgentFilter::Main, 600.);
    assert_eq!(cx.read(|cx| view.read(cx).regenerate_entry()), None);
    let footer = ids_of(&plain, Kind::Turn)[0];
    let plain_height = cx
        .update_window(window, |_, window, _| {
            assert!(window.try_find(("regenerate", footer)).is_none());
            window
                .find(super::row::element_id(RowKey::Turn(footer)))
                .bounds()
                .size
                .height
        })
        .unwrap();

    let feed = ended_turn(false, true);
    let message = ids_of(&feed, Kind::User)[0];
    let notice = ids_of(&feed, Kind::Notice)[0];
    let footer = ids_of(&feed, Kind::Turn)[0];
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    assert_eq!(cx.read(|cx| view.read(cx).regenerate_entry()), Some(footer));
    let line = RowKey::Turn(footer);
    cx.update_window(window, |_, window, _| {
        let rem = window.rem_size();
        let row = window.find(super::row::element_id(line)).bounds();
        let regenerate = window.find(("regenerate", footer)).bounds();
        assert_eq!(regenerate.right(), column_right(800., rem));
        // Laid over the line, centred on it: the line is no taller.
        assert_eq!(row.size.height, plain_height);
        assert!((regenerate.bottom() - row.bottom()).abs() <= px(3.));
        assert!(window.try_find(("regenerate", message)).is_none());
        assert!(window.try_find(("regenerate", notice)).is_none());
    })
    .unwrap();
    cx.update_window(window, |_, window, cx| {
        window.hover(super::row::element_id(line), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(cx.read(|cx| view.read(cx).hovered), Some(footer));

    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
    settle(cx, window);
    assert!(!keys(&view, cx).contains(&line), "no footers");
    assert_eq!(cx.read(|cx| view.read(cx).regenerate_entry()), Some(notice));
    cx.update_window(window, |_, window, _| {
        let rem = window.rem_size();
        let row = window
            .find(super::row::element_id(RowKey::Notice(notice)))
            .bounds();
        let regenerate = window.find(("regenerate", notice)).bounds();
        assert_eq!(regenerate.right(), column_right(800., rem));
        assert!(regenerate.top() >= row.top());
        assert!(regenerate.bottom() <= row.bottom() + px(1.));
    })
    .unwrap();
}

/// A user's message's bubble is the theme's background a step toward its
/// text, channel by channel, so it keeps the theme's tones.
#[gpui_kit::test]
fn the_bubble_is_the_background_a_step_toward_the_text(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    start(cx);
    cx.update(|cx| {
        let theme = cx.theme();
        let step = if theme.is_dark() { 0.09 } else { 0.13 };
        let bubble = super::row::bubble_color(theme).to_rgb();
        let (background, text) = (theme.background.to_rgb(), theme.foreground.to_rgb());
        for (bubble, background, text) in [
            (bubble.r, background.r, text.r),
            (bubble.g, background.g, text.g),
            (bubble.b, background.b, text.b),
        ] {
            let expected = background + (text - background) * step;
            assert!((bubble - expected).abs() < 0.002, "{bubble} {expected}");
        }
        assert_ne!(super::row::bubble_color(theme), theme.background);
    });
}

/// A note typed while the agent's reply streams waits below the reply,
/// marked as not read yet, and the reply goes on in one piece. Once the
/// agent reads it, it moves to where the agent read it, the keyboard's
/// place on it goes with the old one, and what the agent says next comes
/// below.
#[gpui_kit::test]
fn a_note_waits_below_the_streaming_reply_until_the_agent_reads_it(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Rename the parser.", Source::App, Delivery::Turn);
    feed.turn_started();
    say(&feed, "main", "It turned out to be a kit f");
    let note = feed.note("Keep the old name.", Source::App, 1, Vec::new());
    say(&feed, "main", "ix, so that went to the kit.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let [message, _] = ids_of(&feed, Kind::User)[..] else {
        panic!("a message and a note")
    };
    let reply = ids_of(&feed, Kind::Text)[0];
    let text_of = |id: u64, cx: &mut TestAppContext| {
        cx.read(|cx| {
            view.read(cx)
                .model
                .entry(id)
                .map(|entry| entry.item.text.clone())
        })
    };
    assert_eq!(
        text_of(reply, cx).as_deref(),
        Some("It turned out to be a kit fix, so that went to the kit.")
    );
    // The reply, then the note below it.
    let rows = keys(&view, cx);
    assert_eq!(rows.len(), 3, "{rows:?}");
    assert_eq!(rows[0], RowKey::User(message));
    assert!(block(reply, 0)(&rows[1]), "{rows:?}");
    assert_eq!(rows[2], RowKey::User(note));
    let note_row = super::row::element_id(RowKey::User(note));
    cx.update_window(window, |_, window, cx| {
        assert_eq!(
            window.find(note_row.clone()).label(),
            Some("You, not read yet, Keep the old name.")
        );
        // The keyboard on it: the last row.
        view.update(cx, |view, cx| view.focus_rows(window, cx));
    })
    .unwrap();
    settle(cx, window);
    let on = |cx: &mut TestAppContext| cx.read(|cx| (view.read(cx).hovered, view.read(cx).cursor));
    assert_eq!(on(cx), (Some(note), Some(RowKey::User(note))));

    feed.event(
        &event(
            "main",
            AgentEvent::ContextInjected {
                ids: vec!["note:1".into()],
                bytes: 100,
            },
        ),
        None,
    );
    say(&feed, "main", "Kept the old name.");
    settle(cx, window);
    let read = *ids_of(&feed, Kind::User).last().unwrap();
    assert!(read > note, "a new id");
    let next = *ids_of(&feed, Kind::Text).last().unwrap();
    let rows = keys(&view, cx);
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(block(reply, 0)(&rows[1]), "{rows:?}");
    assert_eq!(rows[2], RowKey::User(read));
    assert!(block(next, 0)(&rows[3]), "{rows:?}");
    assert_eq!(on(cx), (None, None));
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find(note_row.clone()).is_none());
        assert_eq!(
            window
                .find(super::row::element_id(RowKey::User(read)))
                .label(),
            Some("You, Keep the old name.")
        );
    })
    .unwrap();
}
