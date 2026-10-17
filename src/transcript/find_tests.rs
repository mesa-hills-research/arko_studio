//! Find in a transcript of a real feed, drawn in a test window.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Delivery, Feed, Source};
use arko_agents::AgentEvent;
use arko_workflows::WorkflowEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};
use serde_json::Value;
use uuid::Uuid;

use super::{FindStatus, Place, find_all};
use crate::Startup;
use crate::backend::FakeBackend;
use crate::prefs::{self, TranscriptView as View};
use crate::transcript::model::RowKey;
use crate::transcript::{AgentFilter, TranscriptView};

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

fn think(feed: &Feed, text: &str) {
    feed.event(&event(AgentEvent::ReasoningChunk(text.into())), None);
}

/// A shell call that printed `output`.
fn command(feed: &Feed, line: &str, output: &str) {
    let id = Uuid::new_v4();
    let parameters: HashMap<String, Value> = [("command".to_string(), Value::from(line))]
        .into_iter()
        .collect();
    feed.event(
        &event(AgentEvent::ToolCallFormatted {
            tool_call_id: id,
            tool_name: "shell".into(),
            parameters,
        }),
        None,
    );
    feed.event(
        &event(AgentEvent::ToolResult {
            tool_call_id: id,
            tool_name: "shell".into(),
            output: format!("exit code 0, 0.1 s\n{output}"),
        }),
        None,
    );
}

/// A turn that mentions the parser in its message, its thinking, a
/// command's output and its reply.
fn feed() -> Feed {
    let feed = Feed::new("main");
    feed.user("Make the parser streaming.", Source::App, Delivery::Turn);
    feed.turn_started();
    think(&feed, "The parser takes a whole string today.");
    command(&feed, "ls src", "lib.rs\nmain.rs");
    command(&feed, "ls src/syntax", "lexer.rs\nparser.rs");
    command(&feed, "ls tests", "parse.rs");
    command(&feed, "ls docs", "streaming.md");
    say(&feed, "The **parser** now streams.");
    feed.reply("The **parser** now streams.");
    feed.turn_ended("12 s", None);
    feed
}

fn open(cx: &mut TestAppContext, view: View) -> (AnyWindowHandle, Entity<TranscriptView>) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = view));
    let feed = feed();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(800.), px(900.)),
        })),
        ..Default::default()
    };
    let opened = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
        })
        .unwrap();
    settle(cx, opened.0);
    opened
}

fn settle(cx: &mut TestAppContext, window: AnyWindowHandle) {
    for _ in 0..4 {
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }
    cx.run_until_parked();
}

fn find(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    query: &str,
) -> FindStatus {
    let status = view.update(cx, |view, cx| view.find(query, cx));
    settle(cx, window);
    status
}

fn next(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    view: &Entity<TranscriptView>,
    forward: bool,
) -> FindStatus {
    let status = view.update(cx, |view, cx| view.find_next(forward, cx));
    settle(cx, window);
    status
}

fn expanded(cx: &mut TestAppContext, view: &Entity<TranscriptView>, id: u64) -> bool {
    cx.read(|cx| {
        view.read(cx)
            .ui
            .get(&id)
            .and_then(|ui| ui.expanded)
            .unwrap_or(false)
    })
}

fn rows(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> Vec<RowKey> {
    cx.read(|cx| view.read(cx).rows.iter().map(|row| row.key).collect())
}

/// The id of the first entry whose text holds `text`.
fn entry(cx: &mut TestAppContext, view: &Entity<TranscriptView>, text: &str) -> u64 {
    cx.read(|cx| {
        view.read(cx)
            .model
            .entries()
            .iter()
            .find(|entry| entry.item.text.contains(text))
            .map(|entry| entry.item.id)
            .unwrap()
    })
}

#[gpui_kit::test]
fn find_counts_collapsed_rows_and_opens_the_one_it_lands_on(cx: &mut TestAppContext) {
    let (window, view) = open(cx, View::Full);
    let status = find(cx, window, &view, "PARSER");
    // The message, the thinking, a command's output and the reply.
    assert_eq!(status.total, 4);
    // It starts at the latest: the reply.
    assert_eq!(status.current, Some(3));
    let thinking = entry(cx, &view, "takes a whole string");
    let listing = entry(cx, &view, "ls src/syntax");
    assert!(!expanded(cx, &view, listing));
    // Up the transcript: the command's output, which opens.
    assert_eq!(next(cx, window, &view, false).current, Some(2));
    assert!(expanded(cx, &view, listing));
    // Then the thinking, which opens too.
    assert_eq!(next(cx, window, &view, false).current, Some(1));
    assert!(
        cx.read(|cx| view.read(cx).thinking_open_by_default()) || expanded(cx, &view, thinking)
    );
    assert!(
        rows(cx, &view)
            .iter()
            .any(|key| matches!(key, RowKey::Block { id, .. } if *id == thinking)),
        "the thinking's text shows"
    );
    // The message is marked in its own text.
    assert_eq!(next(cx, window, &view, false).current, Some(0));
    let message = entry(cx, &view, "Make the parser");
    let marked = cx.read(|cx| view.read(cx).match_ranges(message, Place::Text));
    assert_eq!(marked, [(9..15, true)]);
    // Around at the top, down to the reply again.
    assert_eq!(next(cx, window, &view, false).current, Some(3));
    // Fewer words, the same place: no matches, then none at all.
    assert_eq!(find(cx, window, &view, "parsers").total, 0);
    assert_eq!(find(cx, window, &view, "").total, 0);
    view.update(cx, |view, cx| view.clear_find(cx));
    assert!(
        cx.read(|cx| view.read(cx).match_ranges(message, Place::Text))
            .is_empty()
    );
}

#[gpui_kit::test]
fn a_match_in_a_folded_group_of_calls_opens_the_group(cx: &mut TestAppContext) {
    let (window, view) = open(cx, View::Quiet);
    let group = rows(cx, &view)
        .into_iter()
        .find_map(|key| match key {
            RowKey::Group(first) => Some(first),
            _ => None,
        })
        .expect("four calls fold into a group");
    let status = find(cx, window, &view, "streaming.md");
    assert_eq!(status.total, 1);
    let listing = entry(cx, &view, "ls docs");
    assert!(expanded(cx, &view, group), "the group opened");
    assert!(expanded(cx, &view, listing), "the call opened");
    assert!(rows(cx, &view).contains(&RowKey::Call(listing)));
}

#[gpui_kit::test]
fn the_messages_view_finds_only_what_it_shows(cx: &mut TestAppContext) {
    let (window, view) = open(cx, View::Messages);
    // The message and the reply: the calls and the thinking aren't shown.
    assert_eq!(find(cx, window, &view, "parser").total, 2);
}

#[test]
fn matches_ignore_case_and_index_the_text_as_it_is() {
    assert_eq!(find_all("Parse the parser", "parse"), [0..5, 10..15]);
    let one = |text: &str, words: &str| {
        let found = find_all(text, words);
        assert_eq!(found.len(), 1, "{text}");
        found[0].clone()
    };
    assert_eq!(one("aaa", "aa"), 0..2, "one after another");
    assert_eq!(one("Straße and STRASSE", "straße"), 0..7);
    // Characters whose lowercase is longer still map back whole.
    let text = "İstanbul";
    let found = find_all(text, "i̇s");
    assert_eq!(found.len(), 1);
    assert_eq!(&text[found[0].clone()], "İs");
    assert!(find_all("text", "").is_empty());
    assert!(find_all("", "x").is_empty());
}

/// On the real engine: Mod+F opens the find bar over the session's
/// transcript, Enter moves on, and Esc closes it, giving the keyboard back
/// to the composer.
#[gpui_kit::test]
fn mod_f_opens_the_find_bar_and_esc_gives_the_keyboard_back(cx: &mut TestAppContext) {
    use crate::backend::ArkoBackend;
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};

    let mock = Mock::start(|_, _| reply("a1", "Seven pears."));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply in the transcript", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            view.read(cx).transcript(cx).is_some_and(|transcript| {
                transcript.read(cx).row_count() >= 2 && transcript.read(cx).busy_reason().is_none()
            })
        })
    });
    act(cx, window, |window, cx| window.press("secondary-f", cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("find-bar").is_some());
    });
    act(cx, window, |window, cx| window.input("pears", cx));
    let status = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).find_status());
    assert_eq!(
        status(cx),
        Some(FindStatus {
            current: Some(1),
            total: 2,
            ..FindStatus::default()
        }),
        "the message and the reply, on the latest"
    );
    act(cx, window, |window, cx| window.press("enter", cx));
    assert_eq!(status(cx).and_then(|status| status.current), Some(0));
    act(cx, window, |window, cx| window.press("escape", cx));
    assert_eq!(status(cx), None);
    act(cx, window, |window, cx| {
        assert!(window.try_find("find-bar").is_none());
        let composer = view.read(cx).composer().read(cx);
        assert!(composer.is_focused(window, cx), "the keyboard is back");
    });
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// `feed`'s transcript in a window `height` high, its rows all ready.
fn open_feed(
    cx: &mut TestAppContext,
    feed: &Feed,
    view: View,
    height: f32,
) -> (AnyWindowHandle, Entity<TranscriptView>) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = view));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(800.), px(height)),
        })),
        ..Default::default()
    };
    let opened = cx
        .update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(feed, AgentFilter::Main, window, cx))
            })
        })
        .unwrap();
    settle(cx, opened.0);
    opened
}

fn top(cx: &mut TestAppContext, view: &Entity<TranscriptView>) -> gpui_kit::ListOffset {
    cx.read(|cx| view.read(cx).list.top())
}

/// The reader scrolls away from the match while text streams with the find
/// bar open: the find searches the new text, and the reader stays where
/// they scrolled.
#[gpui_kit::test]
fn the_find_leaves_the_reader_where_they_scrolled_while_text_streams(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Explain.", Source::App, Delivery::Turn);
    feed.turn_started();
    let mut text = String::from("Intro.\n\n```rust\n");
    for line in 0..200 {
        if line == 100 {
            text.push_str("let needle = 100;\n");
        } else {
            text.push_str(&format!("let line_{line} = {line};\n"));
        }
    }
    text.push_str("```\n\nThen more.\n\n");
    say(&feed, &text);
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    assert_eq!(find(cx, window, &view, "needle").total, 1);
    settle(cx, window);
    view.update(cx, |view, cx| {
        view.list.scroll_by(px(-300.));
        cx.notify();
    });
    settle(cx, window);
    let reading = top(cx, &view);
    for n in 0..3 {
        say(&feed, &format!("More needle text {n}. "));
        settle(cx, window);
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(300));
        settle(cx, window);
        let now = top(cx, &view);
        assert_eq!(
            (now.item_ix, now.offset_in_item),
            (reading.item_ix, reading.offset_in_item),
            "after {n} more"
        );
    }
    // The new words were found as they came.
    assert_eq!(
        cx.read(|cx| view.read(cx).find_status()).total,
        4,
        "the code's and three more"
    );
}

/// The view changes while the find bar is open: the find covers what the
/// rows show now.
#[gpui_kit::test]
fn a_view_change_finds_again_in_what_the_rows_show(cx: &mut TestAppContext) {
    let (window, view) = open(cx, View::Full);
    assert_eq!(find(cx, window, &view, "streaming.md").total, 1);
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Messages));
    settle(cx, window);
    let status = cx.read(|cx| view.read(cx).find_status());
    assert_eq!(status.total, 0, "the Messages view shows no calls");
    assert_eq!(next(cx, window, &view, true).current, None);
}

/// A long transcript's first search lowercases its texts off the UI
/// thread: the bar says it searches, and the count comes once they're
/// ready.
#[gpui_kit::test]
fn a_long_transcripts_first_search_runs_off_the_ui_thread(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    let line = "The parser reads the tokens and the lexer hands them over.\n";
    for _ in 0..40 {
        feed.user(&line.repeat(150), Source::App, Delivery::Note);
    }
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    let status = view.update(cx, |view, cx| view.find("lexer", cx));
    assert!(status.searching);
    assert_eq!(status.total, 0);
    settle(cx, window);
    let status = cx.read(|cx| view.read(cx).find_status());
    assert!(!status.searching);
    assert_eq!(status.total, 40 * 150);
    assert_eq!(status.current, Some(40 * 150 - 1), "on the latest");
    // Words that grow search what matched, at once.
    let status = find(cx, window, &view, "lexer hands");
    assert_eq!(status.total, 40 * 150);
}

/// Words that stream in are found as they come, in the frame that brings
/// them.
#[gpui_kit::test]
fn streamed_words_are_found_as_they_come(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("A needle please.", Source::App, Delivery::Turn);
    feed.turn_started();
    say(&feed, "First a needle. ");
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    assert_eq!(find(cx, window, &view, "needle").total, 2);
    for n in 0..3 {
        say(&feed, &format!("Then needle {n}. "));
        settle(cx, window);
        let status = cx.read(|cx| view.read(cx).find_status());
        assert_eq!(status.total, 3 + n, "with no wait");
        assert_eq!(status.current, Some(1), "where it was");
    }
    // A word split across two pieces.
    say(&feed, "Split nee");
    settle(cx, window);
    say(&feed, "dle here.");
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).find_status()).total, 6);
}
