//! Feed-driven tests of the transcript: a real feed, fed the way a session
//! feeds it (the workflow's events, and what Arko says of the turns), and
//! the view drawn in a test window.

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
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
use super::{AgentFilter, TranscriptEvent, TranscriptView, image_caption};
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

pub(super) fn say(feed: &Feed, agent: &str, text: &str) {
    let label = (agent != "main").then_some(agent);
    feed.event(
        &event(agent, AgentEvent::AssistantResponseChunk(text.into())),
        label,
    );
}

pub(super) fn think(feed: &Feed, text: &str) {
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
    change_something_while_reading(cx, |cx| prefs::update(cx, &change));
}

/// Runs `change` while the reader reads turn 30, and checks that turn 30
/// stays where it was.
fn change_something_while_reading(cx: &mut TestAppContext, change: impl Fn(&mut gpui_kit::App)) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 60, 4);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    let reading = user_row(&feed, 30);
    let top = scroll_to_key(cx, window, &view, reading);
    cx.update(change);
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
fn the_sessions_source_view_while_scrolled_up_keeps_the_reader_in_place(cx: &mut TestAppContext) {
    change_something_while_reading(cx, |cx| super::set_show_source(true, cx));
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

#[gpui_kit::test]
fn code_blocks_sit_on_the_editor_surface_with_a_hairline(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;
    start(cx);
    for mode in [prefs::ThemeMode::Dark, prefs::ThemeMode::Light] {
        cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.theme_mode = mode));
        cx.update(|cx| {
            let style = super::row::text_style(cx);
            assert_eq!(style.code_background(), cx.theme().editor_background());
            assert_eq!(style.code_border(), Some(cx.theme().border));
        });
    }
}

/// The ids of the entries of `kind`, oldest first.
fn ids_of(feed: &Feed, kind: Kind) -> Vec<u64> {
    item_ids(feed)
        .into_iter()
        .filter(|(_, of)| *of == kind)
        .map(|(id, _)| id)
        .collect()
}

/// Claude Code's message about a request that failed mid-response shows
/// once, as a warning notice of its own, and neither the reply nor the
/// error after it repeats it.
#[gpui_kit::test]
fn a_failed_requests_message_shows_once_as_a_warning(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let note = "API Error: Connection lost mid-response. The response above may be incomplete.";
    feed.turn_started();
    say(&feed, "main", "The answer is");
    feed.event(
        &event("main", AgentEvent::ServiceWarning { text: note.into() }),
        None,
    );
    feed.event(
        &event(
            "main",
            AgentEvent::ErrorEncountered {
                error_message: note.into(),
                requires_approval_to_retry: true,
                tool_call_id: None,
                error_kind: None,
            },
        ),
        None,
    );
    settle(cx, window);
    let shown: Vec<(Kind, Option<Level>)> = cx.read(|cx| {
        view.read(cx)
            .model
            .entries()
            .iter()
            .filter(|entry| entry.item.text.contains("Connection lost"))
            .map(|entry| (entry.item.kind, entry.item.level))
            .collect()
    });
    assert_eq!(shown, [(Kind::Notice, Some(Level::Warn))]);
    let notice = ids_of(&feed, Kind::Notice)[0];
    assert!(keys(&view, cx).contains(&RowKey::Notice(notice)));
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
    // Copy gives the reply as the agent wrote it, with its Markdown.
    let copied = cx.read_from_clipboard().and_then(|item| item.text());
    assert_eq!(
        copied.as_deref(),
        Some("First paragraph.\n\nSecond **paragraph**.")
    );
    // The source view is in the menu of the "more" button.
    cx.update_window(window, |_, window, cx| {
        window.click(("reply-menu", reply), cx)
    })
    .unwrap();
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        window.within("popup-menu").click(0usize, cx)
    })
    .unwrap();
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).shows_source(reply)));
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

/// Rows that scroll far away from under the pointer never say that the
/// pointer left them. The actions still show for the entry the pointer is
/// on, stay while it rests on each of them until its tooltip shows, work,
/// and go once the pointer leaves.
#[gpui_kit::test]
fn actions_follow_the_pointer_after_rows_scroll_away_from_under_it(cx: &mut TestAppContext) {
    start(cx);
    // A long conversation whose messages can be edited and sent again.
    let mut items = Vec::new();
    for turn in 0..30 {
        let id = turn * 3;
        let mut message = arko::remote::Item::new(Kind::User, format!("Question {turn}"));
        message.message_index = Some(turn as usize * 2);
        message.source = Some(Source::App);
        message.delivery = Some(Delivery::Turn);
        let reply = arko::remote::Item::new(Kind::Reply, format!("Answer {turn}."));
        let footer = arko::remote::Item::new(Kind::Turn, "3 s");
        for (offset, mut item) in [message, reply, footer].into_iter().enumerate() {
            item.id = id + offset as u64 + 1;
            item.turn = Some(turn + 1);
            items.push(item);
        }
    }
    let feed = Feed::new("main");
    feed.restore(items);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    let replies = ids_of(&feed, Kind::Reply);
    let (reply, message) = (replies[29], ids_of(&feed, Kind::User)[29]);
    let (events, _subscription) = events_of(&view, cx);
    let hovered = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).hovered);
    let hover = |cx: &mut TestAppContext, target: ElementId| {
        cx.update_window(window, |_, window, cx| window.hover(target, cx))
            .unwrap();
        cx.run_until_parked();
    };
    // The pointer rests on a reply's text while the reader jumps to the end.
    for turn in [3, 8, 13] {
        scroll_to_key(cx, window, &view, user_row(&feed, turn));
        let text = keys(&view, cx)
            .into_iter()
            .find(block(replies[turn], 0))
            .unwrap();
        hover(cx, super::row::element_id(text));
        assert_eq!(hovered(cx), Some(replies[turn]));
        view.update(cx, |view, cx| view.scroll_to_end(cx));
        settle(cx, window);
    }
    // Rests the pointer on `target`, an action of `entry`, until its
    // tooltip shows.
    let rest_on = |cx: &mut TestAppContext, target: (&'static str, u64), entry: u64| {
        hover(cx, target.into());
        for _ in 0..6 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            cx.update_window(window, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
        let tooltip = gpui_kit::VisualTestContext::from_window(window, cx).debug_bounds("tooltip");
        assert!(tooltip.is_some(), "{target:?} shows its tooltip");
        assert_eq!(hovered(cx), Some(entry), "{target:?}");
        let actions = if entry == reply {
            ("reply-actions", reply)
        } else {
            ("user-actions", message)
        };
        let shown = cx
            .update_window(window, |_, window, _| window.find(actions).visible())
            .unwrap();
        assert!(shown, "the actions show with {target:?}'s tooltip");
    };
    let click = |cx: &mut TestAppContext, target: (&'static str, u64)| {
        cx.update_window(window, |_, window, cx| window.click(target, cx))
            .unwrap();
        cx.run_until_parked();
    };

    // From the latest reply's text onto Copy, the menu and Regenerate.
    let text = keys(&view, cx).into_iter().find(block(reply, 0)).unwrap();
    hover(cx, super::row::element_id(text));
    assert_eq!(hovered(cx), Some(reply));
    rest_on(cx, ("copy-reply", reply), reply);
    click(cx, ("copy-reply", reply));
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("Answer 29.".to_string())
    );
    rest_on(cx, ("reply-menu", reply), reply);
    rest_on(cx, ("regenerate", reply), reply);
    click(cx, ("regenerate", reply));
    assert!(matches!(
        events.borrow().last(),
        Some(TranscriptEvent::Regenerate)
    ));
    // From the message onto Edit, Branch and Copy.
    hover(cx, super::row::element_id(RowKey::User(message)));
    assert_eq!(hovered(cx), Some(message));
    rest_on(cx, ("edit-message", message), message);
    rest_on(cx, ("branch-message", message), message);
    rest_on(cx, ("copy-user", message), message);
    click(cx, ("branch-message", message));
    assert!(matches!(
        events.borrow().last(),
        Some(TranscriptEvent::Branch(id)) if *id == message
    ));
    // Onto the turn's footer, which has no actions: they go.
    hover(cx, super::row::element_id(RowKey::Turn(reply + 1)));
    cx.executor()
        .advance_clock(super::HOVER_GRACE + std::time::Duration::from_millis(10));
    cx.run_until_parked();
    assert_eq!(hovered(cx), None);
}

/// Copy puts the reply on the clipboard as the agent wrote it, Markdown,
/// code fences and all, also while the reply shows formatted or as source.
#[gpui_kit::test]
fn copy_gives_the_replys_markdown_source(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    let text = "Run this:\n\n```sh\ncargo test\n```\n\n- **one**\n- two";
    feed.reply(&format!("{text}\n"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let copy = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.click(("copy-reply", reply), cx)
        })
        .unwrap();
        cx.read_from_clipboard().and_then(|item| item.text())
    };
    assert_eq!(copy(cx).as_deref(), Some(text));
    cx.update(|cx| cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("other".into())));
    view.update(cx, |view, cx| view.toggle_source(reply, cx));
    // Copy takes a click again once its check mark is gone.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    settle(cx, window);
    assert_eq!(copy(cx).as_deref(), Some(text));
    // The names a screen reader says, and nothing more on the buttons.
    cx.update_window(window, |_, window, _| {
        let mut label = |id: &'static str| {
            window
                .within((id, reply))
                .find("button")
                .label()
                .map(str::to_string)
        };
        assert_eq!(label("copy-reply").as_deref(), Some("Copy message"));
        assert_eq!(label("reply-menu").as_deref(), Some("More actions"));
    })
    .unwrap();
}

/// The "more" button's menu has View source, which switches that reply
/// and no other to its Markdown source, and View formatted back.
#[gpui_kit::test]
fn the_more_menu_shows_the_source_of_one_reply(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The first **reply**.");
    feed.user("Two", Source::App, Delivery::Turn);
    feed.reply("The second **reply**.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let replies = ids_of(&feed, Kind::Reply);
    let (first, second) = (replies[0], replies[1]);
    let sources = |cx: &mut TestAppContext, reply: u64| -> Vec<bool> {
        keys(&view, cx)
            .iter()
            .filter_map(|key| match key {
                RowKey::Block { id, source, .. } if *id == reply => Some(*source),
                _ => None,
            })
            .collect()
    };
    let pick = |cx: &mut TestAppContext, reply: u64, label: &str| {
        cx.update_window(window, |_, window, cx| {
            window.click(("reply-menu", reply), cx)
        })
        .unwrap();
        settle(cx, window);
        cx.update_window(window, |_, window, cx| {
            assert_eq!(
                window.within("popup-menu").find(0usize).label(),
                Some(label)
            );
            window.within("popup-menu").click(0usize, cx)
        })
        .unwrap();
        settle(cx, window);
        // Picking closes the menu.
        cx.update_window(window, |_, window, _| {
            assert!(window.try_find("popup-menu").is_none())
        })
        .unwrap();
    };
    assert_eq!(sources(cx, first), [false]);
    pick(cx, first, "View source");
    assert_eq!(sources(cx, first), [true]);
    assert_eq!(sources(cx, second), [false]);
    pick(cx, second, "View source");
    pick(cx, first, "View formatted");
    assert_eq!(sources(cx, first), [false]);
    assert_eq!(sources(cx, second), [true]);
}

/// An earlier reply switched to its source while a later reply streams
/// leaves the streaming reply's blocks as they are, and they go on growing
/// to the end of the reply.
#[gpui_kit::test]
fn an_earlier_replys_source_leaves_a_streaming_reply_alone(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The earlier **reply**.");
    feed.user("Two", Source::App, Delivery::Turn);
    feed.turn_started();
    let answer = "Here is the plan.\n\n## Steps\n\n- one\n- two\n\nThat's all.\n";
    let (first, rest) = answer.split_at(24);
    say(&feed, "main", first);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let earlier = ids_of(&feed, Kind::Reply)[0];
    let streaming = ids_of(&feed, Kind::Text)[0];
    let block_views = |cx: &mut TestAppContext| -> Vec<_> {
        cx.read(|cx| {
            let view = view.read(cx);
            let doc = view.ui[&streaming].doc.as_ref().unwrap();
            doc.blocks
                .iter()
                .filter_map(|block| block.view.as_ref().map(|view| view.entity_id()))
                .collect()
        })
    };
    let streaming_rows = |cx: &mut TestAppContext| -> Vec<RowKey> {
        keys(&view, cx)
            .into_iter()
            .filter(|key| matches!(key, RowKey::Block { id, .. } if *id == streaming))
            .collect()
    };
    let before = (block_views(cx), streaming_rows(cx));
    assert_eq!(before.0.len(), 2, "a paragraph and a heading");
    cx.update_window(window, |_, window, cx| {
        window.click(("reply-menu", earlier), cx)
    })
    .unwrap();
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        window.within("popup-menu").click(0usize, cx)
    })
    .unwrap();
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).shows_source(earlier)));
    assert_eq!((block_views(cx), streaming_rows(cx)), before);
    // The rest streams in, a few bytes at a time, as the blocks go on.
    for chunk in rest.as_bytes().chunks(4) {
        say(&feed, "main", std::str::from_utf8(chunk).unwrap());
        settle(cx, window);
        let now = block_views(cx);
        assert!(now.starts_with(&before.0), "no block view was made again");
    }
    assert_eq!(
        block_views(cx).len(),
        4,
        "a paragraph, a heading, a list, a paragraph"
    );
    feed.reply(answer.trim());
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(view.shows_source(earlier));
        assert!(!view.shows_source(streaming));
    });
    assert!(
        keys(&view, cx)
            .iter()
            .any(|key| matches!(key, RowKey::Actions(id) if *id == streaming))
    );
}

/// The actions of a reply stay while its menu is open, though the pointer
/// went off the reply for good, and go when the menu closes and the keyboard
/// is elsewhere.
#[gpui_kit::test]
fn the_replys_actions_stay_while_its_menu_is_open(cx: &mut TestAppContext) {
    start(cx);
    let feed = ended_turn(true, true);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let message = ids_of(&feed, Kind::User)[0];
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let held = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.find(("reply-actions", reply)).focused() == Some(true)
        })
        .unwrap()
    };
    let menu_open = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.try_find("popup-menu").is_some()
        })
        .unwrap()
    };
    assert!(!held(cx));
    cx.update_window(window, |_, window, cx| {
        window.click(("reply-menu", reply), cx)
    })
    .unwrap();
    settle(cx, window);
    assert!(menu_open(cx) && held(cx));
    // The pointer goes to another message and stays there.
    cx.update_window(window, |_, window, cx| {
        window.hover(super::row::element_id(RowKey::User(message)), cx)
    })
    .unwrap();
    cx.executor()
        .advance_clock(super::HOVER_GRACE + std::time::Duration::from_millis(10));
    settle(cx, window);
    cx.read(|cx| assert_eq!(view.read(cx).hovered, Some(message)));
    assert!(menu_open(cx) && held(cx));
    // Escape closes the menu, and with the pointer elsewhere the actions
    // go.
    cx.update_window(window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    settle(cx, window);
    assert!(!menu_open(cx));
    assert!(!held(cx));
}

/// With Markdown off every reply already shows as written, so the menu
/// has nothing to offer and its button is not there.
#[gpui_kit::test]
fn the_more_button_is_there_only_while_markdown_shows_formatted(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.reply("A **bold** claim.");
    let (window, _view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let has = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            (
                window.try_find(("copy-reply", reply)).is_some(),
                window.try_find(("reply-menu", reply)).is_some(),
            )
        })
        .unwrap()
    };
    assert_eq!(has(cx), (true, true));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    settle(cx, window);
    assert_eq!(has(cx), (true, false));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = true));
    settle(cx, window);
    assert_eq!(has(cx), (true, true));
}

/// The row of a reply's actions has the same height hidden, shown and with
/// the menu open, and its buttons stay where they are. Switching the reply
/// to its source moves the row with the text above it, and changes nothing
/// else about it.
#[gpui_kit::test]
fn the_replys_actions_keep_their_height_and_places(cx: &mut TestAppContext) {
    start(cx);
    let feed = ended_turn(true, true);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let measure = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            let row = window
                .find(super::row::element_id(RowKey::Actions(reply)))
                .bounds();
            let copy = window.find(("copy-reply", reply)).bounds();
            let menu = window.find(("reply-menu", reply)).bounds();
            let regenerate = window.find(("regenerate", reply)).bounds();
            [row, copy, menu, regenerate]
        })
        .unwrap()
    };
    cx.read(|cx| assert_eq!(view.read(cx).hovered, None));
    let hidden = measure(cx);
    cx.update_window(window, |_, window, cx| {
        window.hover(("copy-reply", reply), cx)
    })
    .unwrap();
    settle(cx, window);
    cx.read(|cx| assert_eq!(view.read(cx).hovered, Some(reply)));
    assert_eq!(measure(cx), hidden);
    cx.update_window(window, |_, window, cx| {
        window.click(("reply-menu", reply), cx)
    })
    .unwrap();
    settle(cx, window);
    assert_eq!(measure(cx), hidden);
    cx.update_window(window, |_, window, cx| {
        window.within("popup-menu").click(0usize, cx)
    })
    .unwrap();
    settle(cx, window);
    let after = measure(cx);
    for (now, before) in after.iter().zip(&hidden) {
        assert_eq!(now.size, before.size);
        assert_eq!(now.origin.x, before.origin.x);
    }
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

/// How many times the transcript draws in a second of the test clock, with a
/// frame of a 60 Hz display every 1/60 second. The transcript is the root of
/// the window here, so it draws with each draw of the window.
fn draws_in_a_second(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
) -> usize {
    let frames = |cx: &mut TestAppContext, count: usize| {
        for _ in 0..count {
            cx.executor()
                .advance_clock(std::time::Duration::from_micros(16_667));
            cx.update_window(window, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
    };
    frames(cx, 12);
    let before = cx.read(|cx| view.read(cx).render_count());
    frames(cx, 60);
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
    // A worker and a call run: the call's indicator turns, in each frame of
    // the display.
    let draws = draws_in_a_second(cx, window, &view);
    assert!((58..=62).contains(&draws), "{draws} draws");
    // Then thinking streams: its shimmer is the one that moves, a dozen times
    // a second.
    think(&feed, "hmm");
    settle(cx, window);
    // The text fades in first, by the test clock, which takes a few hundred
    // milliseconds of frames of its own.
    draws_in_a_second(cx, window, &view);
    let draws = draws_in_a_second(cx, window, &view);
    assert!((10..=16).contains(&draws), "{draws} draws");
    let thinking = item_ids(&feed).last().unwrap().0;
    assert_eq!(cx.read(|cx| view.read(cx).animated), Some(thinking));
}

/// How many of `count` frames of a 60 Hz display, one every 1/60 second of
/// the test clock, the window asks for: one each while an indicator turns.
fn frames_asked(cx: &mut TestAppContext, window: AnyWindowHandle, count: usize) -> usize {
    (0..count)
        .filter(|_| {
            cx.executor()
                .advance_clock(std::time::Duration::from_micros(16_667));
            let asked = cx
                .update_window(window, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap();
            cx.run_until_parked();
            asked > 0
        })
        .count()
}

/// The indicator that turns is the latest of those the view shows. A
/// worker's call that runs after the agent's `wait_for_agents` is in the
/// transcript, but only the Full view shows it, so in the Quiet view the
/// wait's indicator goes on turning.
#[gpui_kit::test]
fn the_indicator_that_turns_is_one_the_view_shows(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.motion = prefs::Motion::On;
            prefs.view = View::Quiet;
        });
        cx.set_reduce_motion(false);
    });
    let feed = Feed::new("main");
    feed.user("Fix the parser with a worker.", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    call(&feed, "main", "wait_for_agents", &[]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let wait = *ids_of(&feed, Kind::Call).last().unwrap();
    let card = ids_of(&feed, Kind::WorkerLine)[0];
    let animated = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).animated);
    // Whatever fades in when the rows come has ended by then.
    frames_asked(cx, window, 30);
    assert_eq!(animated(cx), Some(wait));
    assert_eq!(frames_asked(cx, window, 60), 60);

    // The worker runs a call, which the Quiet view leaves out.
    let build = call(&feed, "r1", "shell", &[("command", "cargo build".into())]);
    settle(cx, window);
    let worker_call = *ids_of(&feed, Kind::Call).last().unwrap();
    assert!(!keys(&view, cx).contains(&RowKey::Call(worker_call)));
    assert_eq!(frames_asked(cx, window, 60), 60);
    assert_eq!(animated(cx), Some(wait));

    // The Full view shows it, and it's the latest that runs.
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Full));
    settle(cx, window);
    assert_eq!(animated(cx), Some(worker_call));
    assert_eq!(frames_asked(cx, window, 60), 60);

    // The Messages view counts the calls, so the worker's card turns.
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
    settle(cx, window);
    assert_eq!(animated(cx), Some(card));
    assert_eq!(frames_asked(cx, window, 60), 60);

    // Its call ends, and in the Quiet view the wait still turns.
    result(&feed, "r1", build, "shell", "Finished");
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    settle(cx, window);
    assert_eq!(animated(cx), Some(wait));
    assert_eq!(frames_asked(cx, window, 60), 60);
}

/// The "Working…" row of `view`, if it shows.
fn working_row(view: &Entity<TranscriptView>, cx: &mut TestAppContext) -> Option<RowKey> {
    keys(view, cx)
        .into_iter()
        .find(|key| matches!(key, RowKey::Working(_)))
}

/// Gives `view` a clock that tests move by hand, from now.
fn hand_clock(view: &Entity<TranscriptView>, cx: &mut TestAppContext) -> Rc<Cell<i64>> {
    let now = Rc::new(Cell::new(chrono::Utc::now().timestamp_millis()));
    let clock = now.clone();
    view.update(cx, |view, cx| view.set_clock(move || clock.get(), cx));
    now
}

/// "Working…" shows while the agent waits on the model with nothing else
/// showing it's busy: before the turn's first output, after a call's
/// result, and once its text stops coming. It goes as soon as output
/// comes. It's the one indicator that turns.
#[gpui_kit::test]
fn working_shows_while_the_agent_waits_on_the_model(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| {
        prefs::update(cx, |prefs| prefs.appearance.motion = prefs::Motion::On);
        cx.set_reduce_motion(false);
    });
    let feed = Feed::new("main");
    feed.user("Build it.", Source::App, Delivery::Turn);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let now = hand_clock(&view, cx);
    let message = ids_of(&feed, Kind::User)[0];
    assert_eq!(working_row(&view, cx), None);

    // The turn starts: the last row.
    feed.turn_started();
    settle(cx, window);
    assert_eq!(keys(&view, cx).last(), Some(&RowKey::Working(message)));
    let row = super::row::element_id(RowKey::Working(message));
    cx.update_window(window, |_, window, _| {
        assert_eq!(window.find(row.clone()).label(), Some("Working"));
    })
    .unwrap();

    // Thinking says it's busy, and so does a call that runs.
    think(&feed, "Where to start");
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
    let build = call(&feed, "main", "shell", &[("command", "cargo build".into())]);
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);

    // After the call's result, it waits again, with a worker running too.
    result(&feed, "main", build, "shell", "Finished");
    feed.worker("r1", "Write the fuzz target", WorkerChange::Started);
    settle(cx, window);
    let after_call = *ids_of(&feed, Kind::Call).last().unwrap();
    assert_eq!(keys(&view, cx).last(), Some(&RowKey::Working(after_call)));
    let draws = draws_in_a_second(cx, window, &view);
    assert!((58..=62).contains(&draws), "{draws} draws");
    assert_eq!(cx.read(|cx| view.read(cx).animated), None);

    // Text says it's busy while it comes. Two seconds without more, it
    // waits again.
    say(&feed, "main", "It builds. Now the tests.");
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
    now.set(now.get() + 1000);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
    now.set(now.get() + 1000);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(cx, window);
    let text = *ids_of(&feed, Kind::Text).last().unwrap();
    assert_eq!(working_row(&view, cx), Some(RowKey::Working(text)));

    // A note waits below it, and a question for the user ends it.
    let note = feed.note("Use release mode.", Source::App, 1, Vec::new());
    settle(cx, window);
    assert!(
        keys(&view, cx).ends_with(&[RowKey::Working(text), RowKey::User(note)]),
        "{:?}",
        keys(&view, cx)
    );
    crate::questions::ask(&feed, &[("q1", "Release or debug?")], true);
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
    crate::questions::answer_in_message(&feed, "q1", "Release.");
    settle(cx, window);
    let answer = *ids_of(&feed, Kind::User).last().unwrap();
    assert!(
        keys(&view, cx).ends_with(&[
            RowKey::User(answer),
            RowKey::Working(answer),
            RowKey::User(note)
        ]),
        "{:?}",
        keys(&view, cx)
    );

    // The turn ends.
    feed.reply("It builds. Now the tests.");
    feed.turn_ended("9 s", None);
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
}

/// The role and label a row of `view` gives assistive technology: the
/// transcript is announced as a log is, so a row with a role is heard when
/// it appears and one without is left out.
fn announced_as(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    key: RowKey,
) -> (Option<gpui_kit::Role>, Option<String>) {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let row = window.find(super::row::element_id(key));
        (row.role(), row.label().map(str::to_string))
    })
    .unwrap()
}

/// A screen reader hears "Working…" once for a turn, when it follows what
/// the user said, and not each time the agent waits on the model after its
/// own output, which would repeat it after every call's result. The rows
/// of the calls and the text are heard as they come.
#[gpui_kit::test]
fn working_is_announced_once_for_a_turn(cx: &mut TestAppContext) {
    use gpui_kit::Role;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Build it.", Source::App, Delivery::Turn);
    feed.turn_started();
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let message = ids_of(&feed, Kind::User)[0];
    assert_eq!(working_row(&view, cx), Some(RowKey::Working(message)));
    assert_eq!(
        announced_as(cx, window, RowKey::Working(message)),
        (Some(Role::ListItem), Some("Working".into()))
    );

    // After a call's result, and after text that stopped, it waits again.
    let read = call(&feed, "main", "read_file", &[("path", "a.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
    settle(cx, window);
    let after_call = *ids_of(&feed, Kind::Call).last().unwrap();
    assert_eq!(working_row(&view, cx), Some(RowKey::Working(after_call)));
    assert_eq!(
        announced_as(cx, window, RowKey::Working(after_call)).0,
        None
    );
    assert_eq!(
        announced_as(cx, window, RowKey::Call(after_call)),
        (Some(Role::ListItem), Some("read a.rs".into())),
    );

    // An answer to a question starts the agent again, and that is said.
    crate::questions::ask(&feed, &[("q1", "Release or debug?")], true);
    settle(cx, window);
    crate::questions::answer_in_message(&feed, "q1", "Release.");
    settle(cx, window);
    let answer = *ids_of(&feed, Kind::User).last().unwrap();
    assert_eq!(working_row(&view, cx), Some(RowKey::Working(answer)));
    assert_eq!(
        announced_as(cx, window, RowKey::Working(answer)).0,
        Some(Role::ListItem)
    );
}

/// A worker's own view says "Working…" while the worker runs and waits on
/// the model, and not while it waits for a place.
#[gpui_kit::test]
fn a_workers_view_says_working_while_it_waits_on_the_model(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Fuzz it.", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Write the fuzz target", WorkerChange::Queued);
    let (window, view) = open(cx, &feed, AgentFilter::Worker("r1".into()), 600.);
    assert_eq!(keys(&view, cx), []);

    feed.worker("r1", "Write the fuzz target", WorkerChange::Started);
    settle(cx, window);
    assert_eq!(keys(&view, cx), [RowKey::Working(0)]);
    let read = call(&feed, "r1", "read_file", &[("path", "fuzz.rs".into())]);
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
    result(&feed, "r1", read, "read_file", "1 | fn fuzz() {}\n");
    settle(cx, window);
    let after = *ids_of(&feed, Kind::Call).last().unwrap();
    assert_eq!(keys(&view, cx).last(), Some(&RowKey::Working(after)));

    feed.worker(
        "r1",
        "Write the fuzz target",
        WorkerChange::Finished { report: "Done." },
    );
    settle(cx, window);
    assert_eq!(working_row(&view, cx), None);
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

/// Rows that arrive while the reader is scrolled up count as new wherever
/// they are placed, also above the notes that wait at the end for the agent
/// to read them.
#[gpui_kit::test]
fn the_new_count_includes_rows_above_the_notes_waiting_at_the_end(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    turns(&feed, 30, 1);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 300.);
    load_all(cx, window, &view);
    scroll_to_key(cx, window, &view, user_row(&feed, 10));
    let new = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).new_while_away);
    feed.user("Next", Source::App, Delivery::Turn);
    feed.turn_started();
    settle(cx, window);
    assert_eq!(new(cx), 1, "the message");

    // Two notes wait below what the agent does.
    for (seq, text) in [(1, "Use tabs."), (2, "And the tests.")] {
        feed.note(text, Source::App, seq, Vec::new());
        settle(cx, window);
        assert_eq!(new(cx), 1 + seq as usize, "the note {seq}");
    }
    let rows = keys(&view, cx);
    assert!(
        matches!(rows[rows.len() - 2..], [RowKey::User(_), RowKey::User(_)]),
        "{rows:?}"
    );

    // What the agent does goes above the notes, and counts.
    for (n, count) in [(0, 4), (1, 5)] {
        let id = call(
            &feed,
            "main",
            "read_file",
            &[("path", format!("src/new{n}.rs").into())],
        );
        result(&feed, "main", id, "read_file", "1 | fn a() {}\n");
        settle(cx, window);
        assert_eq!(new(cx), count, "the call {n}");
    }
    // Back to the latest: the count clears.
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    settle(cx, window);
    assert_eq!(new(cx), 0);
}

/// The rows of a Quiet group the reader opens at the end of the transcript,
/// or of a thinking they expand there, are what they asked to see and not
/// news, though they go in below the rows the reader had.
#[gpui_kit::test]
fn opening_the_last_group_or_thinking_while_scrolled_up_is_not_new(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    let feed = Feed::new("main");
    turns(&feed, 30, 1);
    feed.user("Next", Source::App, Delivery::Turn);
    feed.turn_started();
    for n in 0..5 {
        let id = call(
            &feed,
            "main",
            "read_file",
            &[("path", format!("src/x{n}.rs").into())],
        );
        result(&feed, "main", id, "read_file", "1 | fn x() {}\n");
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 300.);
    load_all(cx, window, &view);
    let new = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).new_while_away);
    let group = ids_of(&feed, Kind::Call)[30];
    scroll_to_key(cx, window, &view, user_row(&feed, 29));
    assert!(cx.read(|cx| view.read(cx).list.is_scrolled_up()));
    let folded = keys(&view, cx).len();
    view.update(cx, |view, cx| view.toggle_group(group, cx));
    settle(cx, window);
    assert!(keys(&view, cx).len() >= folded + 5, "the calls show");
    assert_eq!(new(cx), 0, "the group");

    // The same for a thinking, in the Full view, which opens it from a row.
    view.update(cx, |view, cx| view.scroll_to_end(cx));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Full));
    think(&feed, "Weighing the options");
    settle(cx, window);
    let thinking = *ids_of(&feed, Kind::Thinking).last().unwrap();
    scroll_to_key(cx, window, &view, user_row(&feed, 29));
    assert!(cx.read(|cx| view.read(cx).list.is_scrolled_up()));
    view.update(cx, |view, cx| view.set_expanded(thinking, true, cx));
    settle(cx, window);
    assert!(keys(&view, cx).iter().any(block(thinking, 0)));
    assert_eq!(new(cx), 0, "the thinking");
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

/// A note the agent hasn't read waits at the end of the transcript, however
/// far back in the history it came. It shows with the latest entries, not
/// once the older ones are ready, and it shows once.
#[gpui_kit::test]
fn a_note_waiting_from_before_the_first_entries_shows_with_them(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Rename it.", Source::App, Delivery::Turn);
    feed.turn_started();
    let note = feed.note("Keep the old name.", Source::App, 1, Vec::new());
    for ix in 0..450 {
        say(&feed, "main", &format!("Step {ix}.\n\n"));
        let id = call(
            &feed,
            "main",
            "read_file",
            &[("path", format!("src/f{ix}.rs").into())],
        );
        result(&feed, "main", id, "read_file", "1 | fn a() {}\n");
    }
    let opened = feed.clone();
    let (window, view) = cx
        .update(|cx| {
            gpui_kit::open_window(options(800., 400.), cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&opened, AgentFilter::Main, window, cx))
            })
        })
        .unwrap();
    // The history shows from the next frame on: the latest entries first.
    cx.run_until_parked();
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert!(cx.read(|cx| view.read(cx).loaded_from.is_some()));
    let rows = keys(&view, cx);
    let first = *item_ids(&feed)
        .iter()
        .find(|(id, _)| *id > note)
        .map(|(id, _)| id)
        .unwrap();
    assert!(
        !rows.iter().any(|row| row.entry() == first),
        "the entries after the note aren't all ready"
    );
    assert_eq!(rows.last(), Some(&RowKey::User(note)));
    assert!(cx.read(|cx| view.read(cx).ui.contains_key(&note)));

    // Once all are ready, the note is still last and shows once.
    load_all(cx, window, &view);
    let rows = keys(&view, cx);
    assert_eq!(rows.last(), Some(&RowKey::User(note)));
    assert_eq!(
        rows.iter()
            .filter(|row| **row == RowKey::User(note))
            .count(),
        1
    );
    assert!(row_top(cx, window, RowKey::User(note)).is_some());
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
const PNG: &[u8] = include_bytes!(
    "../../packaging/linux/icons/hicolor/256x256/apps/com.mesahillsresearch.ArkoStudio.png"
);

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

#[test]
fn a_label_that_ends_with_the_size_says_it_once() {
    // How `read_file` and Claude Code's `Read` label an image.
    assert_eq!(
        image_caption("chart.png, 457 KB", 467_200),
        "chart.png, 457 KB"
    );
    assert_eq!(
        image_caption("photo.jpg, 3.3 MB", 3_500_000),
        "photo.jpg, 3.3 MB"
    );
    // They round anything under 1 KB up to a kilobyte.
    assert_eq!(image_caption("dot.png, 1 KB", 70), "dot.png, 1 KB");
    assert_eq!(image_caption("empty.png, 0 KB", 0), "empty.png, 0 KB");
    // As Studio says it.
    assert_eq!(image_caption("dot.png · 70 B", 70), "dot.png · 70 B");
}

#[test]
fn a_label_without_a_size_gets_one() {
    assert_eq!(image_caption("chart.png", 12_000), "chart.png · 12 KB");
    assert_eq!(image_caption("chart.png", 1_048_575), "chart.png · 1024 KB");
    assert_eq!(image_caption("photo.jpg", 3_500_000), "photo.jpg · 3.3 MB");
    assert_eq!(image_caption("image", 70), "image · 70 B");
}

#[test]
fn an_empty_label_is_the_size() {
    assert_eq!(image_caption("", 512), "512 B");
    assert_eq!(image_caption("", 12_000), "12 KB");
    assert_eq!(image_caption("", 3_500_000), "3.3 MB");
}

#[test]
fn a_label_that_ends_with_another_size_gets_the_size() {
    assert_eq!(
        image_caption("chart.png, 457 KB", 20_480),
        "chart.png, 457 KB · 20 KB"
    );
    // The size has to start a number, so "20 KB" doesn't end "120 KB".
    assert_eq!(
        image_caption("chart.png, 120 KB", 20_480),
        "chart.png, 120 KB · 20 KB"
    );
    assert_eq!(
        image_caption("photo.jpg, 12.2 MB", 2_306_867),
        "photo.jpg, 12.2 MB · 2.2 MB"
    );
    assert_eq!(image_caption("dot.png, 2 KB", 70), "dot.png, 2 KB · 70 B");
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
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let refresh = item_ids(&feed)[2].0;
    let rows = keys(&view, cx);
    assert_eq!(rows.last(), Some(&RowKey::Refresh(refresh)));
    // A screen reader says what the row reads.
    cx.update_window(window, |_, window, _| {
        let row = super::row::element_id(RowKey::Refresh(refresh));
        assert_eq!(window.find(row).label(), Some("Cache refreshed"));
    })
    .unwrap();
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
    // to it, without the card. It waits on the model now.
    let (_, worker) = open(cx, &feed, AgentFilter::Worker("r1".into()), 720.);
    let rows = keys(&worker, cx);
    assert!(matches!(rows[0], RowKey::Block { .. }));
    assert!(matches!(rows[1], RowKey::Call(_)));
    assert!(matches!(rows[2], RowKey::User(_)));
    assert!(matches!(rows[3], RowKey::Working(_)));
    assert_eq!(rows.len(), 4);
}

/// A worker's worker has its card in the transcript of the worker that
/// started it and not in the main one.
#[gpui_kit::test]
fn a_workers_worker_has_its_card_with_its_starter(cx: &mut TestAppContext) {
    use arko::remote::WorkerInfo;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Review the files", WorkerChange::Started);
    feed.worker_under("r2", "r1");
    feed.worker("r2", "Count the files", WorkerChange::Started);
    let mut info = WorkerInfo::new();
    info.depth = 2;
    feed.worker_info("r2", info);
    let cards: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::WorkerLine)
        .map(|(id, _)| id)
        .collect();
    let (r1_card, r2_card) = (cards[0], cards[1]);

    let (_, main) = open(cx, &feed, AgentFilter::Main, 720.);
    let rows = keys(&main, cx);
    assert!(rows.contains(&RowKey::Worker(r1_card)), "{rows:?}");
    assert!(!rows.contains(&RowKey::Worker(r2_card)), "{rows:?}");

    let (_, starter) = open(cx, &feed, AgentFilter::Worker("r1".into()), 720.);
    let rows = keys(&starter, cx);
    assert!(rows.contains(&RowKey::Worker(r2_card)), "{rows:?}");
    assert!(!rows.contains(&RowKey::Worker(r1_card)), "{rows:?}");
}

/// A worker's failure in the main transcript names the worker that started
/// it, as Arko's notices do, to the eye and to assistive technology. In its
/// own transcript a worker doesn't say whose it is again.
#[gpui_kit::test]
fn a_failure_in_the_main_transcript_names_the_worker_that_started_it(cx: &mut TestAppContext) {
    use arko::remote::WorkerInfo;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Review the files", WorkerChange::Started);
    feed.worker("r3", "Count the words", WorkerChange::Started);
    feed.worker_under("r2", "r1");
    feed.worker("r2", "Count the files", WorkerChange::Started);
    let mut info = WorkerInfo::new();
    info.depth = 2;
    feed.worker_info("r2", info);
    for id in ["r2", "r3"] {
        let error = "the model failed";
        feed.worker(id, "", WorkerChange::Failed { error });
    }
    let labels = |cx: &mut TestAppContext, filter: AgentFilter| {
        let (_, view) = open(cx, &feed, filter, 720.);
        let notices: Vec<RowKey> = keys(&view, cx)
            .into_iter()
            .filter(|key| matches!(key, RowKey::Notice(_)))
            .collect();
        cx.read(|cx| {
            let view = view.read(cx);
            notices
                .into_iter()
                .map(|key| view.row_label(key).to_string())
                .collect::<Vec<_>>()
        })
    };
    // A worker of the main agent is named alone.
    let main = labels(cx, AgentFilter::Main);
    assert_eq!(main.len(), 2, "{main:?}");
    assert!(main[0].starts_with("r2 (under r1), failed ("), "{main:?}");
    assert!(main[1].starts_with("r3, failed ("), "{main:?}");
    let own = labels(cx, AgentFilter::Worker("r2".into()));
    assert_eq!(own.len(), 1, "{own:?}");
    assert!(own[0].starts_with("r2, failed ("), "{own:?}");
}

/// A worker's stop shows in the main transcript with the worker that started
/// the stopped one, also when its starter's stop stopped it.
#[gpui_kit::test]
fn a_stop_in_the_main_transcript_names_the_worker_that_started_it(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Review the files", WorkerChange::Started);
    feed.worker_under("r2", "r1");
    feed.worker("r2", "Count the files", WorkerChange::Started);
    let reason = "its lead r1 was stopped";
    feed.worker("r2", "", WorkerChange::Stopped { reason });
    feed.worker("r1", "", WorkerChange::Stopped { reason: "" });
    let (_, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let labels: Vec<String> = keys(&view, cx)
        .into_iter()
        .filter(|key| matches!(key, RowKey::Notice(_)))
        .map(|key| cx.read(|cx| view.read(cx).row_label(key).to_string()))
        .collect();
    assert_eq!(
        labels,
        [
            "r2 (under r1), stopped (its lead r1 was stopped)",
            "r1, stopped"
        ]
    );
}

/// A worker's card says its profile once the session has recorded it, and
/// the card stays one line tall.
#[gpui_kit::test]
fn a_worker_card_says_its_profile(cx: &mut TestAppContext) {
    use arko::remote::WorkerInfo;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let card = item_ids(&feed)
        .iter()
        .find(|(_, kind)| *kind == Kind::WorkerLine)
        .unwrap()
        .0;
    let setup = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(("worker-setup", card)).is_some()
        })
        .unwrap()
    };
    let height = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.find(("worker-row", card)).bounds().size.height
        })
        .unwrap()
    };
    // Nothing is known of it yet.
    assert!(!setup(cx));
    let before = height(cx);
    let mut info = WorkerInfo::new();
    info.model = Some("gpt-6.1-sol".into());
    info.effort = Some("high".into());
    info.profile = Some("3d-judge".into());
    feed.worker_info("r1", info);
    settle(cx, window);
    assert!(setup(cx));
    assert_eq!(height(cx), before, "one line, as it was");
    // The row's name for assistive technology says it too.
    let label = cx.read(|cx| {
        let view = view.read(cx);
        let row = view
            .rows
            .iter()
            .find(|row| row.key == RowKey::Worker(card))
            .unwrap()
            .key;
        view.row_label(row)
    });
    assert_eq!(label.as_ref(), "Worker r1, Fix the parser, 3d-judge");
}

/// A worker's card names its model and effort only when they aren't the ones
/// workers start with, in the card and in its name for assistive technology.
#[gpui_kit::test]
fn a_worker_card_names_its_model_and_effort_only_when_they_are_not_the_usual(
    cx: &mut TestAppContext,
) {
    use arko::remote::WorkerInfo;

    use crate::workers::Usual;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    let setups = [
        ("r1", Some("gpt-6-luna"), Some("high")),
        ("r2", Some("gpt-6.1-sol"), Some("high")),
        ("r3", Some("gpt-6.1-sol"), Some("xhigh")),
        ("r4", Some("gpt-6-luna"), Some("low")),
        ("r5", None, None),
    ];
    for (id, model, effort) in setups {
        feed.worker(id, "Fix the parser", WorkerChange::Started);
        let mut info = WorkerInfo::new();
        info.model = model.map(str::to_string);
        info.effort = effort.map(str::to_string);
        feed.worker_info(id, info);
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let usual = |model: &str, effort: &str| Usual {
        model: model.into(),
        effort: effort.into(),
    };
    let chips = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            let view = view.read(cx);
            view.rows
                .iter()
                .filter(|row| matches!(row.key, RowKey::Worker(_)))
                .map(|row| view.row_label(row.key).to_string())
                .collect()
        })
    };
    view.update(cx, |view, cx| {
        view.set_usual(&usual("gpt-6-luna", "high"), cx)
    });
    settle(cx, window);
    assert_eq!(
        chips(cx),
        [
            "Worker r1, Fix the parser",
            "Worker r2, Fix the parser, sol",
            "Worker r3, Fix the parser, sol · xhigh",
            "Worker r4, Fix the parser, low effort",
            "Worker r5, Fix the parser",
        ]
    );
    // The card shows the same words, and none for a worker that runs as
    // workers usually do.
    let has_chip = |cx: &mut TestAppContext, card: u64| {
        cx.update_window(window, |_, window, _| {
            window.try_find(("worker-setup", card)).is_some()
        })
        .unwrap()
    };
    let cards: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::WorkerLine)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(
        cards.iter().map(|id| has_chip(cx, *id)).collect::<Vec<_>>(),
        [false, true, true, true, false]
    );
    // Workers that start on another model change which ones stand out.
    view.update(cx, |view, cx| {
        view.set_usual(&usual("gpt-6.1-sol", "xhigh"), cx)
    });
    settle(cx, window);
    assert_eq!(
        chips(cx),
        [
            "Worker r1, Fix the parser, luna · high",
            "Worker r2, Fix the parser, high effort",
            "Worker r3, Fix the parser",
            "Worker r4, Fix the parser, luna · low",
            "Worker r5, Fix the parser",
        ]
    );
}

/// Quiet shows a worker that started at once as its card alone. A start that
/// is still running, queued or refused shows as a call, and Full and
/// Messages keep every call.
#[gpui_kit::test]
fn quiet_shows_a_started_worker_as_its_card_alone(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Split the work", Source::App, Delivery::Turn);
    feed.turn_started();
    let task = |task: &str| [("task", Value::from(task))];
    let started = call(&feed, "main", "start_agent", &task("Fix the parser"));
    result(
        &feed,
        "main",
        started,
        "start_agent",
        "Started r1 (agent `worker`).",
    );
    feed.worker("r1", "Fix the parser", WorkerChange::Started);
    let queued = call(&feed, "main", "start_agent", &task("Review the docs"));
    result(
        &feed,
        "main",
        queued,
        "start_agent",
        "Queued r2 (agent `worker`): 4 agent runs are running, the most this workflow allows, so it starts when one ends.",
    );
    feed.worker("r2", "Review the docs", WorkerChange::Queued);
    let refused = call(&feed, "main", "start_agent", &task("Run the migration"));
    feed.event(
        &event(
            "main",
            AgentEvent::ErrorEncountered {
                error_message: "The worker's billing needs your answer.".into(),
                requires_approval_to_retry: false,
                tool_call_id: Some(refused),
                error_kind: None,
            },
        ),
        None,
    );
    call(&feed, "main", "start_agent", &task("Wait for the card"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let ids = item_ids(&feed);
    let of_kind = |kind: Kind| -> Vec<u64> {
        ids.iter()
            .filter(|(_, found)| *found == kind)
            .map(|(id, _)| *id)
            .collect()
    };
    let calls = of_kind(Kind::Call);
    assert_eq!(calls.len(), 4, "{ids:?}");
    let cards = of_kind(Kind::WorkerLine);
    assert_eq!(cards.len(), 2, "{ids:?}");
    let rows = |cx: &mut TestAppContext, shown: View| {
        cx.update(|cx| prefs::update(cx, |prefs| prefs.view = shown));
        settle(cx, window);
        keys(&view, cx)
            .into_iter()
            .filter(|key| matches!(key, RowKey::Call(_) | RowKey::Worker(_) | RowKey::Count(_)))
            .collect::<Vec<_>>()
    };
    // Only the first call named its worker.
    cx.read(|cx| {
        let view = view.read(cx);
        let started: Vec<Option<String>> = calls
            .iter()
            .map(|id| view.model.entry(*id).unwrap().item.started.clone())
            .collect();
        assert_eq!(started, [Some("r1".to_string()), None, None, None]);
    });
    let (card, other) = (cards[0], cards[1]);
    assert_eq!(
        rows(cx, View::Quiet),
        [
            RowKey::Worker(card),
            RowKey::Call(calls[1]),
            RowKey::Worker(other),
            RowKey::Call(calls[2]),
            RowKey::Call(calls[3]),
        ]
    );
    assert_eq!(
        rows(cx, View::Full),
        [
            RowKey::Call(calls[0]),
            RowKey::Worker(card),
            RowKey::Call(calls[1]),
            RowKey::Worker(other),
            RowKey::Call(calls[2]),
            RowKey::Call(calls[3]),
        ]
    );
    let messages = rows(cx, View::Messages);
    assert_eq!(messages.len(), 5, "{messages:?}");
    assert!(messages.contains(&RowKey::Worker(card)));
}

/// A start the Quiet view leaves out doesn't push a run of calls into a
/// group.
#[gpui_kit::test]
fn quiet_does_not_fold_a_hidden_start_into_a_group(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Look around", Source::App, Delivery::Turn);
    feed.turn_started();
    let read = |feed: &Feed, name: &str| {
        let id = call(feed, "main", "read_file", &[("path", name.into())]);
        result(feed, "main", id, "read_file", "1 | fn x() {}\n");
    };
    read(&feed, "src/a.rs");
    read(&feed, "src/b.rs");
    let started = call(&feed, "main", "start_agent", &[("task", "Count".into())]);
    result(
        &feed,
        "main",
        started,
        "start_agent",
        "Started r1 (agent `worker`).",
    );
    read(&feed, "src/c.rs");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    settle(cx, window);
    let calls: Vec<u64> = item_ids(&feed)
        .into_iter()
        .filter(|(_, kind)| *kind == Kind::Call)
        .map(|(id, _)| id)
        .collect();
    assert_eq!(calls.len(), 4);
    // Three calls show in a row. The fourth is the start.
    let quiet = keys(&view, cx);
    assert!(
        !quiet.iter().any(|key| matches!(key, RowKey::Group(_))),
        "{quiet:?}"
    );
    let shown: Vec<RowKey> = quiet
        .iter()
        .copied()
        .filter(|key| matches!(key, RowKey::Call(_)))
        .collect();
    assert_eq!(
        shown,
        [
            RowKey::Call(calls[0]),
            RowKey::Call(calls[1]),
            RowKey::Call(calls[3])
        ]
    );
    // One more call folds them, and the group counts the four it stands for.
    read(&feed, "src/d.rs");
    settle(cx, window);
    let quiet = keys(&view, cx);
    assert!(quiet.contains(&RowKey::Group(calls[0])), "{quiet:?}");
    let label = cx.read(|cx| view.read(cx).row_label(RowKey::Group(calls[0])));
    assert_eq!(label.as_ref(), "4 calls");
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
    // Copy gives the text as written, whether it shows formatted or not.
    assert_eq!(
        cx.read(|cx| view.read(cx).source_text(reply)),
        Some("**Bold** move.\n\n- a\n- b".to_string())
    );
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

/// A selection across the rows of one long message holds while its rows
/// scroll out of the list and back, a turn of the wheel at a time. In every
/// frame, each row it covers on screen shows its part, and the selection is
/// the same at the end. Copy takes all of it, its paragraphs a blank line
/// apart, also while the row the drag started in is out of the list.
#[gpui_kit::test]
fn a_selection_across_a_long_messages_rows_survives_scrolling(cx: &mut TestAppContext) {
    use gpui_kit::base::TextSelection;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Explain the parser", Source::App, Delivery::Turn);
    let paragraphs: Vec<String> = (1..=80)
        .map(|n| format!("Paragraph {n} says what the parser does with a chunk."))
        .collect();
    feed.reply(&paragraphs.join("\n\n"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let key = |block| RowKey::Block {
        id: reply,
        generation: 0,
        block,
        source: false,
    };
    // The reply's first rows at the top of the screen.
    let first = keys(&view, cx)
        .iter()
        .position(|row| *row == key(0))
        .unwrap();
    scroll_to_row(cx, &view, first);
    settle(cx, window);
    // From the start of the first paragraph into the fourth. Each paragraph
    // is one line, at the bottom of its row, in the reading column.
    let (from, to) = cx
        .update_window(window, |_, window, _| {
            let row = |block| window.find(super::row::element_id(key(block))).bounds();
            let (start, end) = (row(0), row(3));
            let column = start.size.width.min(px(736.));
            let text = start.left() + (start.size.width - column) / 2. + px(16.);
            (
                gpui_kit::point(text + px(1.), start.bottom() - px(6.)),
                gpui_kit::point(text + px(120.), end.bottom() - px(6.)),
            )
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    settle(cx, window);
    let selected = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            TextSelection::selected_text(window, cx)
        })
        .unwrap()
    };
    let whole = selected(cx);
    let middle = format!("{}\n\n{}", paragraphs[1], paragraphs[2]);
    assert!(whole.starts_with(&paragraphs[0]), "{whole:?}");
    assert!(whole.contains(&middle), "{whole:?}");
    assert!(whole.contains("\n\nParagraph 4"), "{whole:?}");
    assert!(!whole.contains(&paragraphs[3]), "{whole:?}");
    // Copy takes it all, its paragraphs a blank line apart.
    cx.update_window(window, |_, window, cx| window.press("secondary-c", cx))
        .unwrap();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, whole.trim());
    // What each covered row shows selected, and the covered rows on screen.
    let painted = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            let doc = view.read(cx).ui[&reply].doc.as_ref().unwrap();
            (0..4)
                .map(|block| {
                    doc.blocks[block]
                        .view
                        .as_ref()
                        .map(|view| view.read(cx).selected_text().trim().to_string())
                        .unwrap_or_default()
                })
                .collect()
        })
    };
    let covered_on_screen = |cx: &mut TestAppContext| -> Vec<usize> {
        cx.read(|cx| {
            let view = view.read(cx);
            view.list
                .on_screen()
                .into_iter()
                .filter_map(|ix| (0..4).find(|block| view.rows[ix].key == key(*block)))
                .collect()
        })
    };
    let shown = painted(cx);
    assert_eq!(shown[1], paragraphs[1]);
    assert_eq!(shown[2], paragraphs[2]);
    assert!(shown.iter().all(|part| !part.is_empty()), "{shown:?}");
    // The selection holds and shows, a turn of the wheel at a time.
    let check = |cx: &mut TestAppContext, step: &str| {
        assert_eq!(selected(cx), whole, "{step}");
        let now = painted(cx);
        for block in covered_on_screen(cx) {
            assert_eq!(now[block], shown[block], "{step}: row {block}");
        }
    };
    // One frame a turn.
    let wheel = |cx: &mut TestAppContext, dy: f32| {
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "transcript",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(dy))),
                cx,
            )
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    };
    for step in 0..15 {
        wheel(cx, -120.);
        check(cx, &format!("away, step {step}"));
    }
    // The first rows are out of the list: nothing of them is drawn.
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find(super::row::element_id(key(0))).is_none());
    })
    .unwrap();
    // The keyboard's Copy still takes all of it, though the row the drag
    // started in, which holds the focus, isn't drawn.
    let focused = cx
        .update_window(window, |_, window, cx| {
            let doc = view.read(cx).ui[&reply].doc.as_ref().unwrap();
            let row = doc.blocks[0].view.as_ref().unwrap();
            row.read(cx).focus_handle().is_focused(window)
        })
        .unwrap();
    assert!(focused, "the first row holds the focus");
    cx.write_to_clipboard(gpui_kit::ClipboardItem::new_string("Nothing yet".into()));
    cx.update_window(window, |_, window, cx| window.press("secondary-c", cx))
        .unwrap();
    assert_eq!(clipboard_text(cx), whole.trim());
    for step in 0..15 {
        wheel(cx, 120.);
        check(cx, &format!("back, step {step}"));
    }
    assert_eq!(cx.read(|cx| view.read(cx).list.top().item_ix), first);
    assert_eq!(painted(cx), shown);
    assert_eq!(covered_on_screen(cx), [0, 1, 2, 3]);
}

/// A right-click at `position`, as the pointer makes one.
fn right_click_at(cx: &mut TestAppContext, window: AnyWindowHandle, position: Point<Pixels>) {
    use gpui_kit::{Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent};

    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseDown(MouseDownEvent {
                button: MouseButton::Right,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.render_frame(cx);
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Right,
                position,
                modifiers: Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
        window.render_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// The labels of the drawn menu that shows, by item, or none.
fn menu_labels(cx: &mut TestAppContext, window: AnyWindowHandle) -> Option<Vec<String>> {
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find("popup-menu")?;
        Some(
            (0..12usize)
                .map(|ix| {
                    window
                        .within("popup-menu")
                        .try_find(ix)
                        .and_then(|item| item.label().map(str::to_string))
                        .unwrap_or_default()
                })
                .collect(),
        )
    })
    .unwrap()
}

/// Presses the item `label` of the drawn menu that shows.
fn press_menu_item(cx: &mut TestAppContext, window: AnyWindowHandle, label: &str) {
    let labels = menu_labels(cx, window).expect("a menu shows");
    let ix = labels
        .iter()
        .position(|item| item == label)
        .unwrap_or_else(|| panic!("{label} in {labels:?}"));
    cx.update_window(window, |_, window, cx| {
        window.within("popup-menu").click(ix, cx)
    })
    .unwrap();
    cx.run_until_parked();
}

fn clipboard_text(cx: &mut TestAppContext) -> String {
    cx.read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default()
}

/// A right-click on selected text of the conversation opens the menu of
/// selected text, whose Copy copies the whole selection, across the rows of
/// a long message, also once its first rows have left the list.
#[gpui_kit::test]
fn copy_in_the_menu_of_selected_text_copies_the_whole_selection(cx: &mut TestAppContext) {
    use gpui_kit::ClipboardItem;
    use gpui_kit::base::TextSelection;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Explain the parser", Source::App, Delivery::Turn);
    let paragraphs: Vec<String> = (1..=40)
        .map(|n| format!("Paragraph {n} says what the parser does with a chunk."))
        .collect();
    feed.reply(&paragraphs.join("\n\n"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    let reply = ids_of(&feed, Kind::Reply)[0];
    let key = |block| RowKey::Block {
        id: reply,
        generation: 0,
        block,
        source: false,
    };
    let first = keys(&view, cx)
        .iter()
        .position(|row| *row == key(0))
        .unwrap();
    scroll_to_row(cx, &view, first);
    settle(cx, window);
    // Where the words of block `block` are: one line at the bottom of its
    // row, in the reading column.
    let words = |cx: &mut TestAppContext, block: usize, x: f32| {
        cx.update_window(window, |_, window, _| {
            let row = window.find(super::row::element_id(key(block))).bounds();
            let column = row.size.width.min(px(736.));
            let text = row.left() + (row.size.width - column) / 2. + px(16.);
            gpui_kit::point(text + px(x), row.bottom() - px(6.))
        })
        .unwrap()
    };
    // From the start of the first paragraph into the sixth.
    let (from, to) = (words(cx, 0, 1.), words(cx, 5, 120.));
    cx.update_window(window, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    settle(cx, window);
    let whole = cx
        .update_window(window, |_, window, cx| {
            TextSelection::selected_text(window, cx)
        })
        .unwrap();
    assert!(whole.starts_with(&paragraphs[0]), "{whole:?}");
    assert!(whole.contains(&paragraphs[4]), "{whole:?}");
    assert!(!whole.contains(&paragraphs[5]), "{whole:?}");

    // On a row in the middle of it.
    cx.write_to_clipboard(ClipboardItem::new_string("Nothing yet".into()));
    let middle = words(cx, 2, 40.);
    right_click_at(cx, window, middle);
    let labels = menu_labels(cx, window).expect("the menu opened");
    assert!(labels.contains(&"Copy".to_string()), "{labels:?}");
    assert!(labels.contains(&"Select All".to_string()), "{labels:?}");
    press_menu_item(cx, window, "Copy");
    assert_eq!(clipboard_text(cx), whole.trim());
    assert!(menu_labels(cx, window).is_none(), "the menu closed");

    // Once the first rows have left the list, on a row still in sight.
    cx.update(|cx| {
        view.update(cx, |view, cx| {
            view.list.scroll_to(first + 3, px(0.));
            cx.notify();
        })
    });
    settle(cx, window);
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find(super::row::element_id(key(0))).is_none());
    })
    .unwrap();
    cx.write_to_clipboard(ClipboardItem::new_string("Nothing yet".into()));
    let still_shown = words(cx, 4, 40.);
    right_click_at(cx, window, still_shown);
    press_menu_item(cx, window, "Copy");
    assert_eq!(clipboard_text(cx), whole.trim());
}

/// A point on the words of the row `key`: one line at the bottom of its row,
/// `x` into the reading column's text.
fn on_words(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    key: RowKey,
    x: f32,
) -> Point<Pixels> {
    cx.update_window(window, |_, window, _| {
        let row = window.find(super::row::element_id(key)).bounds();
        let column = row.size.width.min(px(736.));
        let text = row.left() + (row.size.width - column) / 2. + px(16.);
        gpui_kit::point(text + px(x), row.bottom() - px(6.))
    })
    .unwrap()
}

fn block_of(id: u64, source: bool) -> RowKey {
    RowKey::Block {
        id,
        generation: 0,
        block: 0,
        source,
    }
}

/// A right-click on a reply's text offers Copy, Select All and View source,
/// which switches that reply, and no other, to its source. The menu reads
/// View formatted then, and has no Select All, as plain text has none. The
/// text of a message and of a reply shown as written have no such item.
#[gpui_kit::test]
fn the_menu_of_a_replys_text_offers_view_source(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The first **reply**.");
    feed.user("Two", Source::App, Delivery::Turn);
    feed.reply("The second **reply**.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let replies = ids_of(&feed, Kind::Reply);
    let (first, second) = (replies[0], replies[1]);
    let users = ids_of(&feed, Kind::User);
    let source =
        |cx: &mut TestAppContext, reply: u64| cx.read(|cx| view.read(cx).shows_source(reply));

    let point = on_words(cx, window, block_of(second, false), 20.);
    right_click_at(cx, window, point);
    let labels = menu_labels(cx, window).expect("the menu opened");
    assert_eq!(
        labels
            .iter()
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>(),
        ["Copy", "Select All", "View source"]
    );
    press_menu_item(cx, window, "View source");
    settle(cx, window);
    assert!(menu_labels(cx, window).is_none(), "the menu closed");
    assert!(source(cx, second));
    assert!(!source(cx, first));

    // As source, the text is plain: Copy, and View formatted.
    let point = on_words(cx, window, block_of(second, true), 20.);
    right_click_at(cx, window, point);
    let labels = menu_labels(cx, window).expect("the menu opened");
    assert_eq!(
        labels
            .iter()
            .filter(|label| !label.is_empty())
            .collect::<Vec<_>>(),
        ["Copy", "View formatted"]
    );
    press_menu_item(cx, window, "View formatted");
    settle(cx, window);
    assert!(!source(cx, second));

    // A user's message has Copy alone.
    let point = cx
        .update_window(window, |_, window, _| {
            window.find(("user-message", users[1])).bounds().center()
        })
        .unwrap();
    right_click_at(cx, window, point);
    let labels = menu_labels(cx, window).expect("the menu opened");
    assert!(
        !labels.iter().any(|label| label.starts_with("View")),
        "{labels:?}"
    );
    cx.update_window(window, |_, window, cx| window.press("escape", cx))
        .unwrap();
    settle(cx, window);

    // With Markdown off there is no source to switch to.
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    settle(cx, window);
    let point = on_words(cx, window, block_of(second, true), 20.);
    right_click_at(cx, window, point);
    let labels = menu_labels(cx, window).expect("the menu opened");
    assert!(
        !labels.iter().any(|label| label.starts_with("View")),
        "{labels:?}"
    );
}

/// In the whole window, on a real session, View source in the menu of a
/// reply's text switches that reply.
#[gpui_kit::test]
fn view_source_in_the_menu_of_a_replys_text_works_in_the_window(cx: &mut TestAppContext) {
    use crate::backend::ArkoBackend;
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};

    let mock = Mock::start(|_, _| reply("a1", "Seven **pears**."));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let session = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| session.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply in the transcript", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            session.read(cx).transcript(cx).is_some_and(|transcript| {
                let transcript = transcript.read(cx);
                transcript.row_count() >= 2
                    && transcript.busy_reason().is_none()
                    && transcript
                        .model
                        .entries()
                        .iter()
                        .any(|entry| entry.item.kind == Kind::Reply)
            })
        })
    });
    let transcript = cx.read(|cx| session.read(cx).transcript(cx).cloned().unwrap());
    let reply = cx.read(|cx| {
        let view = transcript.read(cx);
        view.model
            .entries()
            .iter()
            .find(|entry| entry.item.kind == Kind::Reply)
            .map(|entry| entry.item.id)
            .unwrap()
    });
    let point = on_words(cx, window, block_of(reply, false), 20.);
    right_click_at(cx, window, point);
    press_menu_item(cx, window, "View source");
    settle(cx, window);
    assert!(cx.read(|cx| transcript.read(cx).shows_source(reply)));
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// The rows of each reply, as whether they show the source, in order.
fn reply_sources(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> Vec<Vec<bool>> {
    let rows = keys(view, cx);
    cx.read(|cx| {
        let view = view.read(cx);
        view.model
            .entries()
            .iter()
            .filter(|entry| entry.item.kind == Kind::Reply)
            .map(|entry| {
                rows.iter()
                    .filter_map(|key| match key {
                        RowKey::Block { id, source, .. } if *id == entry.item.id => Some(*source),
                        _ => None,
                    })
                    .collect()
            })
            .collect()
    })
}

/// Turning the session's source view on shows every reply as its source,
/// and each reply's menu turns that one reply the other way. Turning the
/// session's view on or off sets every reply again, those turned before
/// included, and a reply that comes later shows the session's way.
#[gpui_kit::test]
fn the_sessions_source_view_sets_every_reply_and_each_reply_can_differ(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    for word in ["first", "second", "third"] {
        feed.user(word, Source::App, Delivery::Turn);
        feed.reply(&format!("The {word} **reply**."));
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 900.);
    let replies = ids_of(&feed, Kind::Reply);
    let sources = |cx: &mut TestAppContext| -> Vec<bool> {
        reply_sources(cx, &view)
            .into_iter()
            .map(|rows| rows[0])
            .collect()
    };
    let pick = |cx: &mut TestAppContext, reply: u64, label: &str| {
        cx.update_window(window, |_, window, cx| {
            window.click(("reply-menu", reply), cx)
        })
        .unwrap();
        settle(cx, window);
        cx.update_window(window, |_, window, cx| {
            assert_eq!(
                window.within("popup-menu").find(0usize).label(),
                Some(label)
            );
            window.within("popup-menu").click(0usize, cx)
        })
        .unwrap();
        settle(cx, window);
    };
    assert_eq!(sources(cx), [false, false, false]);
    assert!(!cx.read(super::show_source));

    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true]);
    // The menu of each says what it does against the session's view.
    pick(cx, replies[0], "View formatted");
    assert_eq!(sources(cx), [false, true, true]);
    pick(cx, replies[0], "View source");
    assert_eq!(sources(cx), [true, true, true]);
    pick(cx, replies[1], "View formatted");
    assert_eq!(sources(cx), [true, false, true]);

    // Off sets all of them, the one turned before included.
    cx.update(|cx| super::set_show_source(false, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [false, false, false]);
    pick(cx, replies[2], "View source");
    assert_eq!(sources(cx), [false, false, true]);
    // On sets all of them again.
    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true]);

    // A reply that comes while it is on shows its source, and its menu
    // brings the formatted text.
    feed.user("fourth", Source::App, Delivery::Turn);
    feed.reply("The fourth **reply**.");
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true, true]);
    let fourth = ids_of(&feed, Kind::Reply)[3];
    pick(cx, fourth, "View formatted");
    assert_eq!(sources(cx), [true, true, true, false]);
    cx.update(|cx| super::set_show_source(false, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [false, false, false, false]);

    // With Markdown off every reply shows as written, and the session's
    // view and the replies' menus have nothing to change.
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = false));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true, true]);
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(replies.iter().all(|reply| !view.shows_source(*reply)));
    });
    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true, true]);
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(replies.iter().all(|reply| !view.shows_source(*reply)));
    });
    cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.render_markdown = true));
    settle(cx, window);
    assert_eq!(sources(cx), [true, true, true, true]);
}

/// Every transcript follows the session's source view, in each window,
/// and one made while it is on starts with it.
#[gpui_kit::test]
fn every_transcript_follows_the_sessions_source_view(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The **reply**.");
    let (first, first_view) = open(cx, &feed, AgentFilter::Main, 400.);
    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, first);
    assert_eq!(reply_sources(cx, &first_view), [[true]]);
    let (second, second_view) = open(cx, &feed, AgentFilter::Main, 400.);
    assert_eq!(reply_sources(cx, &second_view), [[true]]);
    cx.update(|cx| super::set_show_source(false, cx));
    settle(cx, first);
    settle(cx, second);
    assert_eq!(reply_sources(cx, &first_view), [[false]]);
    assert_eq!(reply_sources(cx, &second_view), [[false]]);
}

/// The session's source view can change while a reply streams. The open
/// block goes on growing in either view without a block view made again,
/// the rows above it stay where they are, and the transcript stays at the
/// end.
#[gpui_kit::test]
fn the_sessions_source_view_can_change_while_a_reply_streams(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Plan it", Source::App, Delivery::Turn);
    feed.turn_started();
    let answer = "Here is the plan.\n\n## Steps\n\n- one\n- two\n\nThat's all, **really**.\n";
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let user = ids_of(&feed, Kind::User)[0];
    let mut streamed = String::new();
    let mut stream = |cx: &mut TestAppContext, text: &str| {
        for chunk in text.as_bytes().chunks(4) {
            let chunk = std::str::from_utf8(chunk).unwrap();
            say(&feed, "main", chunk);
            streamed.push_str(chunk);
            settle(cx, window);
        }
    };
    let (first, rest) = answer.split_at(30);
    stream(cx, first);
    let streaming = cx.read(|cx| view.read(cx).model.entries()[1].item.id);
    let block_views = |cx: &mut TestAppContext| -> Vec<_> {
        cx.read(|cx| {
            let view = view.read(cx);
            let doc = view.ui[&streaming].doc.as_ref().unwrap();
            doc.blocks
                .iter()
                .filter_map(|block| block.view.as_ref().map(|view| view.entity_id()))
                .collect()
        })
    };
    // The rows of the streaming entry, as whether they show the source.
    let rows = |cx: &mut TestAppContext| -> Vec<bool> {
        keys(&view, cx)
            .iter()
            .filter_map(|key| match key {
                RowKey::Block { id, source, .. } if *id == streaming => Some(*source),
                _ => None,
            })
            .collect()
    };
    // The top of the user's message, which streaming below never moves.
    let message_top = row_top(cx, window, RowKey::User(user));
    let views = block_views(cx);
    assert_eq!(views.len(), 3, "a paragraph, a heading, and a list begun");
    assert_eq!(rows(cx), [false, false, false]);

    // On in the middle of the stream: the blocks so far are source rows.
    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, window);
    assert_eq!(rows(cx), [true, true, true]);
    assert_eq!(row_top(cx, window, RowKey::User(user)), message_top);
    let streamed_so_far = cx.read(|cx| view.read(cx).source_text(streaming));
    assert_eq!(streamed_so_far.as_deref(), Some(first.trim()));
    // The text streams on in the source view.
    let (second, rest) = rest.split_at(16);
    stream(cx, second);
    assert_eq!(rows(cx).len(), block_views(cx).len());
    assert!(rows(cx).iter().all(|source| *source));
    assert_eq!(row_top(cx, window, RowKey::User(user)), message_top);
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));

    // Off again, with more to come: the same block views, no longer made.
    cx.update(|cx| super::set_show_source(false, cx));
    settle(cx, window);
    assert!(rows(cx).iter().all(|source| !*source));
    assert!(block_views(cx).starts_with(&views));
    stream(cx, rest);
    assert!(block_views(cx).starts_with(&views));
    assert_eq!(
        rows(cx).len(),
        4,
        "a paragraph, a heading, a list, a paragraph"
    );
    assert_eq!(row_top(cx, window, RowKey::User(user)), message_top);
    let streamed_text = cx.read(|cx| view.read(cx).source_text(streaming));
    assert_eq!(streamed_text.as_deref(), Some(answer.trim()));

    // The reply ends, in the session's view.
    feed.reply(answer.trim());
    settle(cx, window);
    assert!(
        keys(&view, cx)
            .iter()
            .any(|key| matches!(key, RowKey::Actions(id) if *id == streaming))
    );
    assert_eq!(reply_sources(cx, &view), [[false; 4]]);
    cx.update(|cx| super::set_show_source(true, cx));
    settle(cx, window);
    assert_eq!(reply_sources(cx, &view), [[true; 4]]);
}

/// The menu finds the reply under the pointer at other zoom levels too.
#[gpui_kit::test]
fn the_menu_of_a_replys_text_finds_the_reply_when_zoomed(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The first **reply**.");
    feed.user("Two", Source::App, Delivery::Turn);
    feed.reply("The second **reply**.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 900.);
    let second = ids_of(&feed, Kind::Reply)[1];
    let first_row = RowKey::User(ids_of(&feed, Kind::User)[0]);
    let mut rems = Vec::new();
    for steps in [1, 1, -4] {
        cx.update(|cx| crate::appearance::zoom(steps, cx));
        settle(cx, window);
        // The first row starts where the list's space above it ends, to the
        // half pixel, at any zoom.
        let rem = cx
            .update_window(window, |_, window, _| window.rem_size())
            .unwrap();
        rems.push(rem);
        let top = row_top(cx, window, first_row).unwrap();
        let at = |y: f32| gpui_kit::point(px(100.), px(top + y));
        cx.read(|cx| {
            let list = &view.read(cx).list;
            assert_eq!(list.row_at(at(0.5), rem), Some(0));
            assert_eq!(list.row_at(at(-0.5), rem), None);
        });
        let point = on_words(cx, window, block_of(second, false), 20.);
        right_click_at(cx, window, point);
        let labels = menu_labels(cx, window).expect("the menu opened");
        assert!(labels.contains(&"View source".to_string()), "{labels:?}");
        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        settle(cx, window);
    }
    assert!(rems.windows(2).all(|pair| pair[0] != pair[1]), "{rems:?}");
}

/// The Menu key and Shift+F10 open the same menu from a reply's text that has
/// the keyboard focus, and Escape closes it.
#[gpui_kit::test]
fn the_menu_key_opens_the_menu_of_a_replys_text(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("One", Source::App, Delivery::Turn);
    feed.reply("The first **reply**.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let reply = ids_of(&feed, Kind::Reply)[0];
    // A click puts the focus in the text.
    let point = on_words(cx, window, block_of(reply, false), 20.);
    cx.update_window(window, |_, window, cx| {
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseDown(gpui_kit::MouseDownEvent {
                button: gpui_kit::MouseButton::Left,
                position: point,
                modifiers: gpui_kit::Modifiers::default(),
                click_count: 1,
                first_mouse: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui_kit::PlatformInput::MouseUp(gpui_kit::MouseUpEvent {
                button: gpui_kit::MouseButton::Left,
                position: point,
                modifiers: gpui_kit::Modifiers::default(),
                click_count: 1,
            }),
            cx,
        );
    })
    .unwrap();
    settle(cx, window);
    for key in ["shift-f10", "menu"] {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        settle(cx, window);
        let labels = menu_labels(cx, window).unwrap_or_else(|| panic!("{key} opened the menu"));
        assert_eq!(
            labels
                .iter()
                .filter(|label| !label.is_empty())
                .collect::<Vec<_>>(),
            ["Copy", "Select All", "View source"],
            "{key}"
        );
        cx.update_window(window, |_, window, cx| window.press("escape", cx))
            .unwrap();
        settle(cx, window);
        assert!(menu_labels(cx, window).is_none(), "{key}");
    }
    cx.read(|cx| assert!(!view.read(cx).shows_source(reply)));
}

/// A selection from the user's message, across the reply and the next
/// message, into a second reply holds while the messages it starts in
/// scroll out of the list and back. Its ends stay where they were, the
/// parts out of sight are copied with the rest, and they show as selected
/// again when they come back.
#[gpui_kit::test]
fn a_selection_across_messages_survives_scrolling(cx: &mut TestAppContext) {
    use gpui_kit::ClipboardItem;
    use gpui_kit::base::TextSelection;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Explain the parser, please.", Source::App, Delivery::Turn);
    let first: Vec<String> = (1..=3)
        .map(|n| format!("The first answer, paragraph {n}."))
        .collect();
    feed.reply(&first.join("\n\n"));
    feed.user("And the lexer?", Source::App, Delivery::Turn);
    let second: Vec<String> = (1..=40)
        .map(|n| format!("The second answer, paragraph {n}, says what the lexer does."))
        .collect();
    feed.reply(&second.join("\n\n"));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    load_all(cx, window, &view);
    let users = ids_of(&feed, Kind::User);
    let replies = ids_of(&feed, Kind::Reply);
    let (question, answer) = (users[0], replies[0]);
    let key = |id, block| RowKey::Block {
        id,
        generation: 0,
        block,
        source: false,
    };
    let top = keys(&view, cx)
        .iter()
        .position(|row| *row == RowKey::User(question))
        .unwrap();
    scroll_to_row(cx, &view, top);
    settle(cx, window);
    // From the start of the question into the third paragraph of the
    // second answer.
    let (from, to) = cx
        .update_window(window, |_, window, _| {
            let bubble = window.find(("user-message", question)).bounds();
            let end = window
                .find(super::row::element_id(key(replies[1], 2)))
                .bounds();
            let column = end.size.width.min(px(736.));
            let text = end.left() + (end.size.width - column) / 2. + px(16.);
            (
                gpui_kit::point(bubble.left() + px(13.), bubble.center().y),
                gpui_kit::point(text + px(120.), end.bottom() - px(6.)),
            )
        })
        .unwrap();
    cx.update_window(window, |_, window, cx| window.drag(from, to, cx))
        .unwrap();
    settle(cx, window);
    let selected = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            TextSelection::selected_text(window, cx)
        })
        .unwrap()
    };
    let whole = selected(cx);
    assert!(
        whole.starts_with("Explain the parser, please."),
        "{whole:?}"
    );
    for part in first
        .iter()
        .chain([&"And the lexer?".to_string(), &second[0], &second[1]])
    {
        assert!(whole.contains(part.as_str()), "{part:?} in {whole:?}");
    }
    // Into the third paragraph, not to its end.
    assert!(
        whole.contains(&format!("{}\n\nThe second", second[1])),
        "{whole:?}"
    );
    assert!(!whole.contains(&second[2]), "{whole:?}");
    // What the first answer's rows show selected, and whether the question
    // does.
    let painted = |cx: &mut TestAppContext| -> (bool, Vec<String>) {
        cx.read(|cx| {
            let view = view.read(cx);
            let question = view.ui[&question].runs[&super::find::Place::Text]
                .handle
                .snapshot(cx)
                .is_some();
            let doc = view.ui[&answer].doc.as_ref().unwrap();
            let rows = (0..3)
                .map(|block| {
                    doc.blocks[block]
                        .view
                        .as_ref()
                        .map(|view| view.read(cx).selected_text().trim().to_string())
                        .unwrap_or_default()
                })
                .collect();
            (question, rows)
        })
    };
    let shown = painted(cx);
    assert_eq!(shown, (true, first.clone()));
    let wheel = |cx: &mut TestAppContext, dy: f32| {
        cx.update_window(window, |_, window, cx| {
            window.scroll(
                "transcript",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(px(0.), px(dy))),
                cx,
            )
        })
        .unwrap();
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    };
    for step in 0..12 {
        wheel(cx, -120.);
        assert_eq!(selected(cx), whole, "away, step {step}");
    }
    // The question and the first answer are out of the list.
    cx.update_window(window, |_, window, _| {
        assert!(window.try_find(("user-message", question)).is_none());
        assert!(
            window
                .try_find(super::row::element_id(key(answer, 2)))
                .is_none()
        );
    })
    .unwrap();
    cx.write_to_clipboard(ClipboardItem::new_string("Nothing yet".into()));
    cx.update_window(window, |_, window, cx| window.press("secondary-c", cx))
        .unwrap();
    assert_eq!(clipboard_text(cx).trim(), whole.trim());
    for step in 0..12 {
        wheel(cx, 120.);
        assert_eq!(selected(cx), whole, "back, step {step}");
    }
    assert_eq!(cx.read(|cx| view.read(cx).list.top().item_ix), top);
    assert_eq!(painted(cx), shown);
}

/// The events `view` sends, as they come.
fn events_of(
    view: &Entity<TranscriptView>,
    cx: &mut TestAppContext,
) -> (
    Rc<std::cell::RefCell<Vec<TranscriptEvent>>>,
    gpui_kit::Subscription,
) {
    let events = Rc::new(std::cell::RefCell::new(Vec::new()));
    let subscription = cx.update({
        let events = events.clone();
        |cx| {
            cx.subscribe(view, move |_, event: &TranscriptEvent, _| {
                events.borrow_mut().push(event.clone());
            })
        }
    });
    (events, subscription)
}

/// Whether the element `id` is drawn.
fn drawn(cx: &mut TestAppContext, window: AnyWindowHandle, id: impl Into<ElementId>) -> bool {
    let id = id.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id).is_some()
    })
    .unwrap()
}

/// A running call that can move to the background offers Move to
/// background, as long as the session's status lists it, and asks for the
/// move when pressed.
#[gpui_kit::test]
fn a_running_call_that_can_move_offers_move_to_background(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Look it up", Source::App, Delivery::Turn);
    call(
        &feed,
        "main",
        "mcp__slow__lookup",
        &[("word", "mesa".into())],
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let item = ids_of(&feed, Kind::Call)[0];
    assert!(!drawn(cx, window, ("move-call", item)), "not movable yet");
    let (events, _subscription) = events_of(&view, cx);
    view.update(cx, |view, cx| view.set_movable(&[item], cx));
    settle(cx, window);
    assert!(drawn(cx, window, ("move-call", item)));
    cx.update_window(window, |_, window, cx| {
        window.click(("move-call", item), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*events.borrow(), [TranscriptEvent::MoveToBackground(item)]);
    // It moved, or ended: the status no longer lists it.
    view.update(cx, |view, cx| view.set_movable(&[], cx));
    settle(cx, window);
    assert!(!drawn(cx, window, ("move-call", item)));
}

/// The result a call that moved to the background gives the agent.
fn moved(job: Uuid) -> String {
    format!(
        "in the background as job {job}, 95.3 s\nThe user moved this call to the background. It goes on running, and its result reaches you as a note when it ends."
    )
}

/// A call that moved to the background says so. While its job runs, its
/// row says for how long and offers Stop, and once the job ends it says how
/// it ended, as Arko words it, without Stop.
#[gpui_kit::test]
fn a_call_in_the_background_follows_its_job(cx: &mut TestAppContext) {
    use super::row::JobLine;
    use arko::remote::{JobKind, JobStart, JobState, MAIN_AGENT};

    start(cx);
    let feed = Feed::new("main");
    feed.user("Index it", Source::App, Delivery::Turn);
    let calls: Vec<(Uuid, Uuid)> = (0..3)
        .map(|_| {
            let job = Uuid::new_v4();
            let call = call(&feed, "main", "index_create", &[("path", ".".into())]);
            result(&feed, "main", call, "index_create", &moved(job));
            (call, job)
        })
        .collect();
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let now = hand_clock(&view, cx);
    let items = ids_of(&feed, Kind::Call);
    let line = |cx: &mut TestAppContext, ix: usize| {
        cx.read(|cx| {
            let view = view.read(cx);
            view.job_line(&view.model.entry(items[ix]).unwrap().item)
        })
    };
    // Moved, and its job not known yet.
    assert_eq!(line(cx, 0), Some(JobLine::Moved));
    assert!(!drawn(cx, window, ("stop-job", items[0])));

    // Its job runs, for 3 minutes so far.
    for (call, job) in &calls {
        let started = now.get() - 3 * 60_000 - 5_000;
        feed.job_started(
            JobStart::new(*job, MAIN_AGENT, JobKind::Call, "index_create")
                .made_by(*call)
                .started(started),
        );
    }
    settle(cx, window);
    let (_, job) = calls[0];
    assert_eq!(
        line(cx, 0),
        Some(JobLine::Running {
            job: job.to_string().into(),
            since: now.get() - 3 * 60_000 - 5_000,
        })
    );
    assert_eq!(super::row::job_time(3 * 60_000 + 5_000), "3 min");
    // Its time goes on, a minute at a time: the next draw is as the fourth
    // minute starts.
    assert!(cx.read(|cx| view.read(cx).ticker.is_some()));
    assert_eq!(
        cx.read(|cx| view.read(cx).tick_every),
        std::time::Duration::from_millis(55_050)
    );
    let (events, _subscription) = events_of(&view, cx);
    cx.update_window(window, |_, window, cx| {
        window.click(("stop-job", items[0]), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [TranscriptEvent::StopJob(job.to_string().into())]
    );

    // The jobs end: one finished, one stopped, one failed.
    feed.job_ended(calls[0].1, JobState::Finished, None);
    feed.job_ended(calls[1].1, JobState::Stopped, None);
    feed.job_ended(calls[2].1, JobState::Failed, Some("exit 1".into()));
    settle(cx, window);
    let Some(JobLine::Outcome {
        text,
        failed: false,
    }) = line(cx, 0)
    else {
        panic!("{:?}", line(cx, 0));
    };
    assert!(
        text.starts_with("Finished in the background · 3 min"),
        "{text}"
    );
    assert_eq!(
        line(cx, 1),
        Some(JobLine::Outcome {
            text: "Stopped".into(),
            failed: false,
        })
    );
    assert_eq!(
        line(cx, 2),
        Some(JobLine::Outcome {
            text: "Failed in the background".into(),
            failed: true,
        })
    );
    for item in &items {
        assert!(!drawn(cx, window, ("stop-job", *item)));
    }
}

/// A call that started a command in the background (`terminal_start`)
/// offers to stop it while it runs, and goes back to its usual line once it
/// ended.
#[gpui_kit::test]
fn a_command_started_in_the_background_can_be_stopped(cx: &mut TestAppContext) {
    use super::row::JobLine;
    use arko::remote::{JobKind, JobStart, JobState, MAIN_AGENT};

    start(cx);
    let feed = Feed::new("main");
    feed.user("Serve it", Source::App, Delivery::Turn);
    let job = Uuid::new_v4();
    let call = call(
        &feed,
        "main",
        "terminal_start",
        &[("command", "npm run dev".into())],
    );
    result(
        &feed,
        "main",
        call,
        "terminal_start",
        &format!("Started job {job}."),
    );
    feed.job_started(JobStart::new(job, MAIN_AGENT, JobKind::Command, "npm run dev").made_by(call));
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let item = ids_of(&feed, Kind::Call)[0];
    let line = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let view = view.read(cx);
            view.job_line(&view.model.entry(item).unwrap().item)
        })
    };
    assert!(matches!(line(cx), Some(JobLine::Running { .. })));
    assert!(drawn(cx, window, ("stop-job", item)));
    feed.job_ended(job, JobState::Failed, Some("exit 1".into()));
    settle(cx, window);
    assert_eq!(line(cx), None);
    assert!(!drawn(cx, window, ("stop-job", item)));
}

/// Whether the element `inner` is drawn inside the element `outer`.
fn drawn_in(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    inner: impl Into<ElementId>,
    outer: impl Into<ElementId>,
) -> bool {
    let (inner, outer) = (inner.into(), outer.into());
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let (Some(inner), Some(outer)) = (window.try_find(inner), window.try_find(outer)) else {
            return false;
        };
        outer.bounds().contains(&inner.bounds().center())
    })
    .unwrap()
}

/// A Quiet group of more than three calls folds them, and its header offers
/// what the row of the call it names would: Move to background for the
/// running call that can move, else Stop for the latest call whose job
/// runs. Opening the group moves the buttons to the calls' rows.
#[gpui_kit::test]
fn a_folded_groups_header_offers_move_and_stop(cx: &mut TestAppContext) {
    use arko::remote::{JobKind, JobState, MAIN_AGENT};

    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    let feed = Feed::new("main");
    feed.user("Serve it, then build", Source::App, Delivery::Turn);
    feed.turn_started();
    let read = call(&feed, "main", "read_file", &[("path", "src/a.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
    let (served, job) = (
        call(
            &feed,
            "main",
            "terminal_start",
            &[("command", "npm run dev".into())],
        ),
        Uuid::new_v4(),
    );
    result(
        &feed,
        "main",
        served,
        "terminal_start",
        &format!("Started job {job}."),
    );
    feed.job_started(
        arko::remote::JobStart::new(job, MAIN_AGENT, JobKind::Command, "npm run dev")
            .made_by(served),
    );
    let read = call(&feed, "main", "read_file", &[("path", "src/b.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn b() {}\n");
    let building = call(&feed, "main", "shell", &[("command", "cargo build".into())]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    let calls = ids_of(&feed, Kind::Call);
    let (serving, building_item) = (calls[1], calls[3]);
    view.update(cx, |view, cx| view.set_movable(&[building_item], cx));
    settle(cx, window);
    let group = super::row::element_id(RowKey::Group(calls[0]));
    let rows = keys(&view, cx);
    assert!(rows.contains(&RowKey::Group(calls[0])), "{rows:?}");
    assert!(!rows.contains(&RowKey::Call(building_item)), "{rows:?}");

    // Folded, the header names the running command and offers to move it.
    let move_button = ("move-call", building_item);
    let stop_button = ("stop-job", serving);
    assert!(drawn_in(cx, window, move_button, group.clone()));
    assert!(!drawn(cx, window, stop_button), "one call at a time");
    let (events, _subscription) = events_of(&view, cx);
    cx.update_window(window, |_, window, cx| window.click(move_button, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [TranscriptEvent::MoveToBackground(building_item)]
    );

    // Opened, the calls' rows offer both, and the header doesn't.
    view.update(cx, |view, cx| view.toggle_group(calls[0], cx));
    settle(cx, window);
    for (button, call) in [(stop_button, serving), (move_button, building_item)] {
        let row = super::row::element_id(RowKey::Call(call));
        assert!(drawn_in(cx, window, button, row), "{button:?}");
        assert!(!drawn_in(cx, window, button, group.clone()), "{button:?}");
    }

    // Folded again, with the command done, the header names the job's call
    // and offers to stop it.
    view.update(cx, |view, cx| view.toggle_group(calls[0], cx));
    result(&feed, "main", building, "shell", "exit code 0, 1.0 s\n");
    view.update(cx, |view, cx| view.set_movable(&[], cx));
    settle(cx, window);
    assert!(drawn_in(cx, window, stop_button, group.clone()));
    assert!(!drawn(cx, window, move_button));
    events.borrow_mut().clear();
    cx.update_window(window, |_, window, cx| window.click(stop_button, cx))
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        *events.borrow(),
        [TranscriptEvent::StopJob(job.to_string().into())]
    );

    // The job ends: nothing to offer.
    feed.job_ended(job, JobState::Stopped, None);
    settle(cx, window);
    assert!(!drawn(cx, window, stop_button));
}

/// A reopened session's transcript shows the turns Arko started for news (a
/// monitor's alert or warning, a background job's end, the workers' news)
/// as the notices the live transcript showed, not as messages of the
/// user's. Only the user's own messages can be edited.
#[gpui_kit::test]
fn a_reopened_sessions_turns_for_news_show_as_notices(cx: &mut TestAppContext) {
    use arko_ai::Message;

    start(cx);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Full));
    let started = |lead: &str| {
        Message::new_user(&format!(
            "{lead} (Arko started this turn for it. The user hasn't written.)"
        ))
    };
    // A stopped run reopened with the user's words, and a monitor's alert
    // that came meanwhile.
    let reopened = format!(
        "# Workflow context update\n\nYour run was stopped before it finished and has now been reopened: continue your task where you were.\n\nGo on.\n\nMonitor alert. Job {} (`npm run dev`, lines that match `ERROR`) printed 1 matching line since 22:47:55 UTC.\n  ERROR boom\nIt ends in 22 min.",
        Uuid::new_v4()
    );
    let messages = [
        Message::new_user("Watch the dev server."),
        Message::new_assistant("Watching it."),
        Message::new_user(&reopened),
        Message::new_assistant("Going on."),
        started("A monitor of yours alerted after your last turn ended. The alert follows."),
        Message::new_assistant("It logged an error."),
        started(
            "A monitor of yours ends in 25 min. Its notice follows and asks whether to keep it.",
        ),
        Message::new_assistant("Kept it."),
        started(
            "A background job of yours finished after your last turn ended. Its update follows.",
        ),
        Message::new_assistant("Built."),
        // As Arko worded the workers' news before.
        Message::new_user(
            "Your workers sent you news as your last turn ended: it follows. (Arko started this turn for it; the user hasn't written.)",
        ),
        Message::new_assistant("Noted."),
    ];
    let feed = Feed::new("main");
    feed.restore(arko::remote::history(
        &messages,
        true,
        &arko::images::Images::default(),
    ));
    let (_window, view) = open(cx, &feed, AgentFilter::Main, 900.);
    let (users, notices) = cx.read(|cx| {
        let view = view.read(cx);
        let text = |id: u64| view.model.entry(id).unwrap().item.text.clone();
        let (mut users, mut notices) = (Vec::new(), Vec::new());
        for row in &view.rows {
            match row.key {
                RowKey::User(id) => users.push(text(id)),
                RowKey::Notice(id) => notices.push(text(id)),
                _ => {}
            }
        }
        (users, notices)
    });
    assert_eq!(users, ["Watch the dev server.", "Go on."]);
    assert_eq!(
        notices,
        [
            "Monitor alerted",
            "Monitor ends in 25 min",
            "Job finished",
            "News from workers",
        ]
    );
    // Alt+↑ edits the user's first message, the one that started a turn of
    // its own, and an edit of it removes the second and the six replies.
    let first = ids_of(&feed, Kind::User)[0];
    assert_eq!(ids_of(&feed, Kind::User).len(), 2);
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.last_message(), Some(first));
        assert!(view.is_editable(first));
        assert_eq!(view.messages_after(first), 7);
    });
}

/// A monitor's call, as Arko feeds it: the call, the monitor's start, then
/// the call's result. Gives the call's ID.
fn monitor_call(feed: &Feed, job: Uuid, started: i64, ends_at: i64) -> Uuid {
    use arko::remote::{JobKind, JobStart, MAIN_AGENT};

    let call = call(
        feed,
        "main",
        "monitor_start",
        &[
            ("command", "npm run dev".into()),
            ("pattern", "ERROR".into()),
        ],
    );
    feed.job_started(
        JobStart::new(job, MAIN_AGENT, JobKind::Monitor, "npm run dev")
            .made_by(call)
            .started(started)
            .monitoring("lines that match ERROR", ends_at),
    );
    result(
        feed,
        "main",
        call,
        "monitor_start",
        &format!("Monitor {job} started. It watches `npm run dev` for lines that match `ERROR`."),
    );
    call
}

/// A call that started a monitor shows it while it runs, with its alerts,
/// when it ends and Stop, and follows its alerts and rearms. Once it ends,
/// the row says how, in the words of the call's own line: after its time,
/// stopped, or with the failure mark when its command failed or Arko
/// stopped.
#[gpui_kit::test]
fn a_monitors_call_follows_the_monitor(cx: &mut TestAppContext) {
    use super::row::{JobLine, monitoring};
    use arko::remote::JobState;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Watch the server", Source::App, Delivery::Turn);
    // They started 30 minutes ago (by the feed's clock, which times their
    // ends), and the rows are read 8 minutes after.
    let started = chrono::Utc::now().timestamp_millis() - 30 * 60_000 - 5_000;
    let ends_at = started + 30 * 60_000;
    let jobs: Vec<Uuid> = (0..4)
        .map(|_| {
            let job = Uuid::new_v4();
            monitor_call(&feed, job, started, ends_at);
            job
        })
        .collect();
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let now = hand_clock(&view, cx);
    now.set(started + 8 * 60_000 + 10_000);
    let items = ids_of(&feed, Kind::Call);
    let line = |cx: &mut TestAppContext, ix: usize| {
        cx.read(|cx| {
            let view = view.read(cx);
            view.job_line(&view.model.entry(items[ix]).unwrap().item)
        })
    };
    let job = |ix: usize| -> gpui_kit::SharedString { jobs[ix].to_string().into() };
    assert_eq!(
        line(cx, 0),
        Some(JobLine::Monitoring {
            job: job(0),
            alerts: 0,
            ends_at: Some(ends_at),
        })
    );
    assert!(drawn(cx, window, ("stop-job", items[0])));

    // It alerted three times: 21 min 50 s are left, which read as 22.
    feed.job_changed(jobs[0], 3, ends_at);
    settle(cx, window);
    assert_eq!(
        line(cx, 0),
        Some(JobLine::Monitoring {
            job: job(0),
            alerts: 3,
            ends_at: Some(ends_at),
        })
    );
    assert_eq!(
        cx.read(|cx| view.read(cx).job_text(items[0])).as_deref(),
        Some("Monitoring · 3 alerts · ends in 22 min")
    );
    // The row draws again as that turns to 21 minutes, 20 s from now.
    assert_eq!(
        cx.read(|cx| view.read(cx).tick_every),
        std::time::Duration::from_millis(20_050)
    );
    now.set(now.get() + 20_050);
    assert_eq!(
        cx.read(|cx| view.read(cx).job_text(items[0])).as_deref(),
        Some("Monitoring · 3 alerts · ends in 21 min")
    );
    assert_eq!(
        monitoring(0, Some(30 * 60_000)),
        "Monitoring · ends in 30 min"
    );
    assert_eq!(
        monitoring(1, Some(59_000)),
        "Monitoring · 1 alert · ends in less than a minute"
    );
    assert_eq!(super::row::monitoring_changes_in(59_000), None);
    // A rearm moves its end.
    feed.job_changed(jobs[1], 0, ends_at + 10 * 60_000);
    settle(cx, window);
    assert!(
        matches!(line(cx, 1), Some(JobLine::Monitoring { ends_at: Some(at), .. }) if at == ends_at + 10 * 60_000)
    );
    let (events, _subscription) = events_of(&view, cx);
    cx.update_window(window, |_, window, cx| {
        window.click(("stop-job", items[0]), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*events.borrow(), [TranscriptEvent::StopJob(job(0))]);

    // They end: one after its time, one stopped, one whose command failed,
    // and one that Arko's end stopped.
    feed.job_changed(jobs[0], 4, ends_at);
    feed.job_ended(jobs[0], JobState::Expired, None);
    feed.job_ended(jobs[1], JobState::Stopped, None);
    feed.job_ended(jobs[2], JobState::Failed, Some("exit 1".into()));
    feed.job_ended(jobs[3], JobState::Interrupted, None);
    settle(cx, window);
    let ended = [
        ("Ended after 30 min · 4 alerts", false),
        ("Stopped", false),
        ("Command exited (exit 1)", true),
        ("Interrupted", true),
    ];
    for (ix, (text, failed)) in ended.into_iter().enumerate() {
        assert_eq!(
            line(cx, ix),
            Some(JobLine::Outcome {
                text: text.into(),
                failed,
            })
        );
        // The call's own line, which the feed keeps, says the same.
        let outcome = cx.read(|cx| {
            let view = view.read(cx);
            view.model.entry(items[ix]).unwrap().item.outcome.clone()
        });
        assert_eq!(
            outcome.map(|outcome| super::row::capitalized(&outcome)),
            Some(text.to_string())
        );
        assert!(!drawn(cx, window, ("stop-job", items[ix])));
    }
    assert!(cx.read(|cx| view.read(cx).ticker.is_none()));
}

/// A monitor's call whose monitor the transcript has no summary of says
/// what its call's line says, without Stop.
#[gpui_kit::test]
fn a_monitors_call_without_its_summary_says_its_line(cx: &mut TestAppContext) {
    use super::row::JobLine;
    use arko::remote::JobState;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Watch the server", Source::App, Delivery::Turn);
    let job = Uuid::new_v4();
    let call = call(
        &feed,
        "main",
        "monitor_start",
        &[("command", "npm run dev".into())],
    );
    result(
        &feed,
        "main",
        call,
        "monitor_start",
        &format!("Monitor {job} started. It watches `npm run dev`."),
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let item = ids_of(&feed, Kind::Call)[0];
    let line = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let view = view.read(cx);
            view.job_line(&view.model.entry(item).unwrap().item)
        })
    };
    assert_eq!(
        line(cx),
        Some(JobLine::Outcome {
            text: "Monitoring".into(),
            failed: false,
        })
    );
    assert!(!drawn(cx, window, ("stop-job", item)));
    feed.job_ended(job, JobState::Stopped, None);
    settle(cx, window);
    assert_eq!(
        line(cx),
        Some(JobLine::Outcome {
            text: "Stopped".into(),
            failed: false,
        })
    );
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
    // The short message on one line, inside the bubble's padding.
    let words = cx
        .update_window(window, |_, window, _| {
            let style = window.text_style();
            let size = style.font_size.to_pixels(window.rem_size());
            let text = "Now fuzz it.";
            let line =
                window
                    .text_system()
                    .layout_line(text, size, &[style.to_run(text.len())], None);
            f32::from(line.width) + 1.5 * rem
        })
        .unwrap();
    // Wide, then narrower, then wide again: the bubbles take their widths
    // back as the column widens.
    let widths: [(f32, Widest); 4] = [
        (800., |column, _| column / 2.),
        (480., |_, rem| 17.5 * rem),
        (300., |column, _| column),
        (800., |column, _| column / 2.),
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
            assert!(
                (short_bubble.size.width - px(words)).abs() < px(1.),
                "{width}: {short_bubble:?}, {words}"
            );
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

/// The left edge of the reading column's text, as [`column_right`] has it.
fn column_left(width: f32, rem: Pixels) -> Pixels {
    let rem = f32::from(rem);
    let column = width.min(46. * rem) - 2. * rem;
    px((width - column) / 2.)
}

/// The line after a turn (its time and cost) starts at the column's edge,
/// where the reply's text does, also with Regenerate at its right.
#[gpui_kit::test]
fn the_line_after_a_turn_starts_where_the_reply_does(cx: &mut TestAppContext) {
    start(cx);
    for (replied, changeable) in [(true, false), (false, true)] {
        let feed = ended_turn(replied, changeable);
        let footer = ids_of(&feed, Kind::Turn)[0];
        let (window, _) = open(cx, &feed, AgentFilter::Main, 600.);
        cx.update_window(window, |_, window, _| {
            let rem = window.rem_size();
            let text = window.find(("turn-text", footer)).bounds();
            assert_eq!(text.left(), column_left(800., rem), "{replied}");
            if changeable {
                let regenerate = window.find(("regenerate", footer)).bounds();
                assert_eq!(regenerate.right(), column_right(800., rem));
            }
        })
        .unwrap();
    }
}

/// Regenerate sits alone at the right end of the latest reply's actions,
/// away from Copy and the menu, in a wide window and in a narrow one. The message it sends again offers it no longer.
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
            let menu = window.find(("reply-menu", reply)).bounds();
            let regenerate = window.find(("regenerate", reply)).bounds();
            assert_eq!(regenerate.right(), column_right(width, rem), "{width}");
            assert!(regenerate.right() < row.right());
            assert!(regenerate.left() - menu.right() >= rem * 2., "{width}");
            assert_eq!(regenerate.top(), menu.top(), "{width}");
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
/// marked "Pending", and the reply goes on in one piece. Once the
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
            Some("You, pending, Keep the old name.")
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

/// In every view, each of the user's messages in `feed` shows once, and
/// the notes the agent hasn't read are the last rows, in the order sent.
fn notes_show_in_every_view(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    feed: &Feed,
) {
    let users: Vec<_> = feed
        .subscribe()
        .0
        .items
        .into_iter()
        .filter(|item| item.kind == Kind::User)
        .collect();
    let waiting: Vec<RowKey> = users
        .iter()
        .filter(|item| item.pending)
        .map(|item| RowKey::User(item.id))
        .collect();
    for shown in [View::Full, View::Quiet, View::Messages] {
        cx.update(|cx| prefs::update(cx, |prefs| prefs.view = shown));
        settle(cx, window);
        let rows = keys(view, cx);
        assert!(
            rows.ends_with(&waiting),
            "{shown:?}: {waiting:?} in {rows:?}"
        );
        for user in &users {
            let count = rows
                .iter()
                .filter(|row| **row == RowKey::User(user.id))
                .count();
            assert_eq!(count, 1, "{shown:?}: {:?} in {rows:?}", user.text);
        }
    }
}

/// A note the agent hasn't read shows at the end of every view, however it
/// came: while a call runs, right after another note, as the agent takes an
/// earlier one, and as the turn ends. Taken with the next message, each
/// shows once, where the agent read it.
#[gpui_kit::test]
fn every_note_not_read_yet_shows_in_every_view(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Rename the parser.", Source::App, Delivery::Turn);
    feed.turn_started();
    say(&feed, "main", "Looking around.");
    let (window, view) = open(cx, &feed, AgentFilter::Main, 720.);
    let pending = |feed: &Feed| {
        feed.subscribe()
            .0
            .items
            .iter()
            .filter(|item| item.pending)
            .map(|item| item.text.clone())
            .collect::<Vec<_>>()
    };
    let path = |file: &str| [("path", Value::from(file))];

    // While a call runs.
    let first = call(&feed, "main", "read_file", &path("a.rs"));
    feed.note("Keep the old name.", Source::App, 1, Vec::new());
    notes_show_in_every_view(cx, window, &view, &feed);

    // Right after it, with the run of calls going on around both.
    result(&feed, "main", first, "read_file", "1 | fn a() {}\n");
    let second = call(&feed, "main", "read_file", &path("b.rs"));
    feed.note("And the tests.", Source::App, 2, Vec::new());
    result(&feed, "main", second, "read_file", "1 | fn b() {}\n");
    let third = call(&feed, "main", "read_file", &path("c.rs"));
    assert_eq!(pending(&feed), ["Keep the old name.", "And the tests."]);
    notes_show_in_every_view(cx, window, &view, &feed);

    // As the agent takes the first, in the same frame.
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
    feed.note("Not the docs.", Source::App, 3, Vec::new());
    result(&feed, "main", third, "read_file", "1 | fn c() {}\n");
    call(&feed, "main", "read_file", &path("d.rs"));
    assert_eq!(pending(&feed), ["And the tests.", "Not the docs."]);
    notes_show_in_every_view(cx, window, &view, &feed);

    // As the turn ends.
    feed.note("Thanks.", Source::App, 4, Vec::new());
    feed.reply("Renamed it.");
    feed.turn_ended("9 s", None);
    assert_eq!(pending(&feed).len(), 3);
    notes_show_in_every_view(cx, window, &view, &feed);

    // The next message takes them.
    feed.notes_sent(&pending(&feed));
    feed.turn_started();
    assert!(pending(&feed).is_empty());
    notes_show_in_every_view(cx, window, &view, &feed);
}

/// A card of the agent's questions is one line while they are open (the
/// answer panel has them). Once one settles, the card is a box with the
/// line as its heading, and each settled question joins the record in it
/// with what became of it, numbered in its ask. The agent's ids, which go
/// on across conversations, are never shown or heard.
#[gpui_kit::test]
fn a_question_card_is_a_line_until_its_questions_settle(cx: &mut TestAppContext) {
    use arko::questions::QuestionState;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Tidy the parser.", Source::App, Delivery::Turn);
    // A later conversation's questions: their ids go on from the first's.
    let card = crate::questions::ask(
        &feed,
        &[
            ("q7", "Tabs or spaces?"),
            ("q8", "Push now?"),
            ("q9", "Which branch?"),
            ("q10", "Keep the old API?"),
            ("q11", "Rename the module?"),
        ],
        false,
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    settle(cx, window);
    assert!(keys(&view, cx).contains(&RowKey::Question(card)));
    assert_eq!(
        announced_as(cx, window, RowKey::Question(card))
            .1
            .as_deref(),
        Some("Arko asked 5 questions")
    );
    let shows = |cx: &mut TestAppContext, n: u64| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(("question", card * 8 + n)).is_some()
        })
        .unwrap()
    };
    assert!(
        (0..5).all(|n| !shows(cx, n)),
        "open questions are the panel's"
    );
    let line_x = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.find(("question-line", card)).bounds().origin.x
        })
        .unwrap()
    };
    let bare = line_x(cx);
    feed.set_question_state(
        "q7",
        QuestionState::Answered {
            answer: "Spaces".into(),
        },
    );
    feed.set_question_state("q8", QuestionState::AnsweredInMessage);
    feed.set_question_state(
        "q9",
        QuestionState::Closed {
            why: Some("Found it in the README".into()),
        },
    );
    feed.set_question_state("q10", QuestionState::Dismissed);
    settle(cx, window);
    assert_eq!(
        announced_as(cx, window, RowKey::Question(card)),
        (
            Some(gpui_kit::Role::ListItem),
            Some(
                "Arko asked 5 questions, 1. Tabs or spaces?, Spaces, 2. Push now?, Answered in your message, 3. Which branch?, Arko closed it. Found it in the README, 4. Keep the old API?, Dismissed"
                    .into()
            )
        )
    );
    assert!((0..4).all(|n| shows(cx, n)));
    assert!(!shows(cx, 4), "still open");
    // A box now, its line inside the padding.
    assert!(line_x(cx) > bare + px(8.));
    // The answer given in the answer panel is the user's bubble under the
    // card, and the other outcomes stay in it.
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        assert!(window.try_find(("question-outcome", card * 8)).is_none());
        assert!(
            window
                .try_find(("question-outcome", card * 8 + 1))
                .is_some()
        );
        assert!(window.try_find(("answer", card * 8)).is_some());
        assert!(window.try_find(("answer", card * 8 + 1)).is_none());
    })
    .unwrap();
}

/// The answers given in the answer panel are a bubble of the user's under
/// the card, at the column's right like the user's messages and tagged
/// like a typed answer. Each carries its question's number when the ask
/// numbered them, and an answer to a lone question has none.
#[gpui_kit::test]
fn answers_given_in_the_panel_are_a_bubble_under_the_card(cx: &mut TestAppContext) {
    use arko::questions::QuestionState;

    start(cx);
    let feed = Feed::new("main");
    feed.user("Tidy the parser.", Source::App, Delivery::Turn);
    let card = crate::questions::ask(
        &feed,
        &[
            ("q1", "Tabs or spaces?"),
            ("q2", "Push now?"),
            ("q3", "Which branch?"),
        ],
        false,
    );
    let lone = crate::questions::ask(&feed, &[("q4", "Rename the module?")], false);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    settle(cx, window);
    let answered = |id: &str, answer: &str| {
        feed.set_question_state(
            id,
            QuestionState::Answered {
                answer: answer.into(),
            },
        )
    };
    answered("q1", "Spaces");
    feed.set_question_state("q2", QuestionState::Dismissed);
    answered("q3", "main, then a pull request against the release branch");
    answered("q4", "Yes");
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        let message = window.find(("user-message", ids_of(&feed, Kind::User)[0]));
        let bubble = window.find(("question-answers", card)).bounds();
        let record = window.find(("question", card * 8)).bounds();
        // At the right edge of the column, as the user's message is, and
        // under the card.
        assert!(
            (bubble.right() - message.bounds().right()).abs() < px(1.),
            "{bubble:?}"
        );
        assert!(bubble.left() > record.left() + px(100.), "{bubble:?}");
        assert!(bubble.top() > record.bottom());
        // Numbered as their questions, past the one dismissed.
        assert!(window.try_find(("answer-number", card * 8)).is_some());
        assert!(window.try_find(("answer", card * 8 + 1)).is_none());
        assert!(window.try_find(("answer-number", card * 8 + 2)).is_some());
        // A lone question's answer has no number.
        assert!(window.try_find(("answer", lone * 8)).is_some());
        assert!(window.try_find(("answer-number", lone * 8)).is_none());
        view.update(cx, |view, cx| view.focus_rows(window, cx));
    })
    .unwrap();
    // Each card and its answers remain one keyboard stop.
    for (key, expected) in [
        ("up", RowKey::Question(card)),
        ("up", RowKey::User(ids_of(&feed, Kind::User)[0])),
        ("down", RowKey::Question(card)),
        ("down", RowKey::Question(lone)),
    ] {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
        settle(cx, window);
        assert_eq!(cx.read(|cx| view.read(cx).cursor), Some(expected));
    }
}

/// The answers given in the answer panel are found, and copied as
/// selected, as the user's words are.
#[gpui_kit::test]
fn answers_given_in_the_panel_are_found_and_copied(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Tidy the parser.", Source::App, Delivery::Turn);
    let card = crate::questions::ask(&feed, &[("q1", "Which branch?")], false);
    feed.set_question_state(
        "q1",
        arko::questions::QuestionState::Answered {
            answer: "The release branch".into(),
        },
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    settle(cx, window);
    view.update(cx, |view, cx| view.find("release", cx));
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).find_status().total), 1);
    // The question's words come first, then the answer under them.
    view.update(cx, |view, cx| view.find("branch", cx));
    settle(cx, window);
    let status = cx.read(|cx| view.read(cx).find_status());
    assert_eq!(status.total, 2);
    view.update(cx, |view, cx| view.find("", cx));
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        let text = window.find(("answer-text", card * 8)).bounds();
        let middle = text.size.height / 2.;
        window.drag(
            text.origin + gpui_kit::point(px(1.), middle),
            text.origin + gpui_kit::point(text.size.width - px(1.), middle),
            cx,
        );
        window.press("secondary-c", cx);
    })
    .unwrap();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, "The release branch");
}

/// The line of a card with questions open asks for the answer panel on
/// that card, by a click or Enter. Once they are all settled it's text.
#[gpui_kit::test]
fn the_line_of_a_card_opens_the_answer_panel(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Tidy the parser.", Source::App, Delivery::Turn);
    let card = crate::questions::ask(&feed, &[("q1", "Tabs or spaces?")], false);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    settle(cx, window);
    let heard = Rc::new(std::cell::RefCell::new(Vec::new()));
    let _events = cx.update(|cx| {
        let heard = heard.clone();
        cx.subscribe(&view, move |_, event: &TranscriptEvent, _| {
            heard.borrow_mut().push(event.clone())
        })
    });
    cx.update_window(window, |_, window, cx| {
        window.click(("question-line", card), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(*heard.borrow(), [TranscriptEvent::OpenQuestions(card)]);
    crate::questions::answer_in_message(&feed, "q1", "Spaces");
    settle(cx, window);
    heard.borrow_mut().clear();
    cx.update_window(window, |_, window, cx| {
        window.click(("question-line", card), cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert!(heard.borrow().is_empty());
}

/// The words of a settled question are found, and copied as selected. An
/// open one's are the answer panel's.
#[gpui_kit::test]
fn a_question_cards_words_are_found_and_copied(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Tidy the parser.", Source::App, Delivery::Turn);
    let card = crate::questions::ask(&feed, &[("q1", "Tabs or spaces?")], false);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    settle(cx, window);
    view.update(cx, |view, cx| view.find("spaces", cx));
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).find_status().total), 0);
    feed.set_question_state("q1", arko::questions::QuestionState::Dismissed);
    view.update(cx, |view, cx| view.find("", cx));
    settle(cx, window);
    view.update(cx, |view, cx| view.find("spaces", cx));
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).find_status().total), 1);
    view.update(cx, |view, cx| view.find("", cx));
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        let text = window.find(("question-text", card * 8)).bounds();
        let middle = text.size.height / 2.;
        window.drag(
            text.origin + gpui_kit::point(px(1.), middle),
            text.origin + gpui_kit::point(text.size.width - px(1.), middle),
            cx,
        );
        window.press("secondary-c", cx);
    })
    .unwrap();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, "Tabs or spaces?");
}
