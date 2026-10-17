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
    AnyWindowHandle, AppContext as _, Bounds, Context, Entity, IntoElement, ParentElement as _,
    Point, Render, SharedString, Styled as _, TestAppContext, Window, WindowBounds, WindowOptions,
    div, px, size,
};
use serde_json::Value;
use uuid::Uuid;

use super::model::RowKey;
use super::{AgentFilter, TranscriptEvent, TranscriptView};
use crate::Startup;
use crate::backend::FakeBackend;
use crate::prefs::{self, TranscriptView as View};

fn start(cx: &mut TestAppContext) {
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
fn open(
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
fn settle(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    cx.run_until_parked();
}

fn keys(view: &Entity<TranscriptView>, cx: &mut TestAppContext) -> Vec<RowKey> {
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

fn call(feed: &Feed, agent: &str, tool: &str, parameters: &[(&str, Value)]) -> Uuid {
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

fn result(feed: &Feed, agent: &str, id: Uuid, tool: &str, output: &str) {
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
fn item_ids(feed: &Feed) -> Vec<(u64, Kind)> {
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
    let scroller = cx.read(|cx| view.read(cx).scroller.clone());
    scroller.update(cx, |scroller, cx| {
        scroller.scroll_to_item(5, cx);
    });
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
    let rows = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).row_count());
    // The latest 200 at once (and a hundred more with each frame drawn
    // since: opening the window drew one).
    let first = rows(cx);
    assert!((200..450).contains(&first), "{first}");
    assert_eq!((first - 200) % 100, 0);
    cx.update_window(window, |_, window, cx| window.render_frame(cx))
        .unwrap();
    assert_eq!(rows(cx), first + 100, "a hundred more each frame");
    settle(cx, window);
    assert_eq!(rows(cx), 450);
    // The reader stayed at the latest message all along.
    assert!(cx.read(|cx| view.read(cx).is_following(cx)));
    assert!(row_top(cx, window, RowKey::User(item_ids(&feed)[449].0)).is_some());
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
    let tile = |entry: u64, id: u64| SharedString::from(format!("image-{entry}-{id}"));
    cx.update_window(window, |_, window, cx| {
        for (entry, id) in [(1, 1), (2, 2), (2, 3)] {
            assert!(window.try_find(tile(entry, id)).is_some(), "{entry} {id}");
        }
        assert!(!window.has_active_image_viewer(cx));
        window.click(tile(2, 2), cx);
        assert!(window.has_active_image_viewer(cx));
    })
    .unwrap();
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
    cx.update_window(window, |_, window, cx| {
        window.click(SharedString::from(format!("show-all-{reply}-1")), cx);
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
    let first = bounds(cx, RowKey::User(user));
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
            first.origin + gpui_kit::point(px(30.), first.size.height / 2.),
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
