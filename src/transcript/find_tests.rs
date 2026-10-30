//! Find in a transcript of a real feed, drawn in a test window.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Delivery, Feed, Source};
use arko_agents::AgentEvent;
use arko_workflows::WorkflowEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Pixels, Point, TestAppContext, WindowBounds,
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
    assert!(
        cx.read(|cx| view.read(cx).group_open(group)),
        "the group opened"
    );
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
fn code_regions_are_the_lines_between_fences_also_in_lists() {
    use super::code_regions;
    let top = "```rust\nfn a() {}\n```\n";
    let regions = code_regions(top);
    assert_eq!(regions.len(), 1);
    assert_eq!(&top[regions[0].clone()], "fn a() {}\n");
    let list = "1. Run this:\n\n   ```bash\n   echo a\n   echo b\n   ```\n\n2. Done.\n";
    let regions = code_regions(list);
    assert_eq!(regions.len(), 1);
    assert_eq!(&list[regions[0].clone()], "   echo a\n   echo b\n");
    // Not closed yet (it streams): to the end.
    let open = "> ~~~\n> one\n";
    assert_eq!(&open[code_regions(open)[0].clone()], "> one\n");
    assert!(code_regions("Just words.\n").is_empty());
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
    // Case folds in full, and accents match however they're written.
    assert_eq!(find_all("Straße and STRASSE", "straße"), [0..7, 12..19]);
    assert_eq!(one("Cafe\u{301} au lait", "CAFÉ"), 0..6);
    // Characters whose folding is longer still map back whole.
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
    let rows = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window
                .find(crate::transcript::row::element_id(RowKey::User(1)))
                .bounds()
        })
        .unwrap()
    };
    let before = rows(cx);
    act(cx, window, |window, cx| window.press("secondary-f", cx));
    act(cx, window, |window, _| {
        assert!(window.try_find("find-bar").is_some());
    });
    // It floats over the transcript: the rows stay where they were.
    assert_eq!(rows(cx), before);
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
    // F3 opens the find bar too (Ctrl+F belongs to the Emacs and Vim
    // keymaps), and moves on once it is open, Shift+F3 back.
    act(cx, window, |window, cx| window.press("f3", cx));
    act(cx, window, |window, cx| window.input("pears", cx));
    assert_eq!(status(cx).and_then(|status| status.current), Some(1));
    act(cx, window, |window, cx| window.press("f3", cx));
    assert_eq!(status(cx).and_then(|status| status.current), Some(0));
    act(cx, window, |window, cx| window.press("shift-f3", cx));
    assert_eq!(status(cx).and_then(|status| status.current), Some(1));
    act(cx, window, |window, cx| window.press("escape", cx));
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

/// The find ignores case in full and how accents are written, in the
/// user's text and in a reply's Markdown, and marks the text as it is.
#[gpui_kit::test]
fn the_find_ignores_case_and_how_accents_are_written(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Is the Straße café open?", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.reply("The STRASSE cafe\u{301} opens at **seven**.");
    feed.turn_ended("2 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    assert_eq!(find(cx, window, &view, "strasse").total, 2);
    assert_eq!(find(cx, window, &view, "Café").total, 2);
    assert_eq!(find(cx, window, &view, "STRASSE CAFE\u{301}").total, 2);
    // The latest is the current match. The message's is marked as written.
    let message = entry(cx, &view, "Is the Straße");
    let marked = cx.read(|cx| view.read(cx).match_ranges(message, Place::Text));
    assert_eq!(marked, [(7..20, false)]);
    assert_eq!(find(cx, window, &view, "cafe ").total, 0);
}

#[gpui_kit::test]
fn the_next_match_keeps_its_quiet_group_open(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Go.", Source::App, Delivery::Turn);
    feed.turn_started();
    for number in 0..5 {
        command(&feed, &format!("needle {number}"), "no match");
    }
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Quiet, 900.);
    view.update(cx, |view, cx| view.find("needle", cx));
    settle(cx, window);
    for _ in 0..5 {
        view.update(cx, |view, cx| view.find_next(false, cx));
        settle(cx, window);
        let calls = cx.read(|cx| {
            view.read(cx)
                .rows
                .iter()
                .filter(|row| matches!(row.key, RowKey::Call(_)))
                .count()
        });
        assert_eq!(calls, 5);
    }
}

/// A find that opens the Quiet group at the end of the transcript shows rows
/// the reader asked to see and not news, though they go in below the rows
/// the reader had.
#[gpui_kit::test]
fn a_find_opening_the_last_group_while_scrolled_up_is_not_new(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    for number in 0..30 {
        feed.user(&format!("message {number}"), Source::App, Delivery::Turn);
    }
    feed.turn_started();
    for number in 0..5 {
        let output = if number == 0 { "needle" } else { "no match" };
        command(&feed, &format!("ls {number}"), output);
    }
    let (window, view) = open_feed(cx, &feed, View::Quiet, 300.);
    view.update(cx, |view, _| view.list.scroll_to(24, px(0.)));
    settle(cx, window);
    assert!(cx.read(|cx| view.read(cx).list.is_scrolled_up()));
    let calls = |cx: &mut TestAppContext| {
        rows(cx, &view)
            .iter()
            .filter(|key| matches!(key, RowKey::Call(_)))
            .count()
    };
    assert_eq!(calls(cx), 0);
    find(cx, window, &view, "needle");
    assert_eq!(calls(cx), 5, "the group opened");
    // The first call is in sight, and the reader still isn't at the end.
    assert!(cx.read(|cx| view.read(cx).list.is_scrolled_up()));
    assert_eq!(cx.read(|cx| view.read(cx).new_while_away), 0);
}

#[gpui_kit::test]
fn a_match_in_thinking_opens_both_the_entry_and_its_code(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Go.", Source::App, Delivery::Turn);
    feed.turn_started();
    think(
        &feed,
        &format!(
            "```rust\n{}let needle = 1;\n```\n",
            "let x = 0;\n".repeat(80)
        ),
    );
    say(&feed, "Done.");
    feed.reply("Done.");
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    view.update(cx, |view, cx| view.find("needle", cx));
    settle(cx, window);
    cx.read(|cx| {
        let view = view.read(cx);
        let id = view.current_match().unwrap().entry;
        let ui = view.ui.get(&id).unwrap();
        assert_eq!(ui.expanded, Some(true));
        assert!(ui.uncapped.contains(&0));
    });
}

#[gpui_kit::test]
fn changing_scope_rejects_an_older_background_search(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Go.", Source::App, Delivery::Turn);
    feed.turn_started();
    for number in 0..10 {
        command(
            &feed,
            &format!("build {number}"),
            &format!("{}needle", "filler ".repeat(5000)),
        );
    }
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    view.update(cx, |view, cx| {
        assert!(view.find("needle", cx).searching);
        let generation = view.find.as_ref().unwrap().generation;
        view.settings.view = View::Messages;
        view.refresh_rows(cx);
        view.find_changed_all(cx);
        // Even a completion already queued on the UI thread is stale.
        view.folded(generation, Vec::new(), cx);
    });
    settle(cx, window);
    let status = cx.read(|cx| view.read(cx).find_status());
    assert!(!status.searching);
    assert_eq!(status.total, 0);
}

/// The find scrolls a plain-text match to the line it is laid out on, so a
/// match after many short lines lands on screen, whatever its share of the
/// row's bytes. That holds with Unicode, wrapping, an attachment above the
/// text and zoom, going down to a match and back up to one.
#[gpui_kit::test]
fn plain_matches_reveal_their_shaped_line_with_wrapping_images_and_zoom(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    let mut item = arko::remote::Item::new(
        arko::remote::Kind::User,
        format!(
            "A first needle.\n{}{}needle\n{}",
            "é\n".repeat(250),
            "uneven Unicode 日本語 ".repeat(30),
            "long tail ".repeat(600)
        ),
    );
    item.id = 1;
    item.delivery = Some(Delivery::Turn);
    item.images.push(arko::images::ImageRef {
        id: 1,
        mime: "image/png".into(),
        label: "diagram.png".into(),
        bytes: 10,
    });
    feed.restore(vec![item]);
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    // Where the current match's line was painted, and how far down its row
    // the screen starts.
    let landed = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let view = view.read(cx);
            let line = view.find.as_ref().unwrap().plain_reveal.line.get();
            (line.unwrap(), view.list.top().offset_in_item)
        })
    };
    let on_screen = |line: Bounds<Pixels>| line.top() >= px(0.) && line.bottom() <= px(600.);
    for (width, zoom) in [(800., 1.), (400., 1.5)] {
        cx.simulate_window_resize(window, size(px(width), px(600.)));
        cx.update(|cx| prefs::update(cx, |prefs| prefs.appearance.zoom = zoom));
        settle(cx, window);
        // From the end of the transcript, the find lands on the match after
        // the short lines, thousands of pixels down the row.
        view.update(cx, |view, cx| {
            view.clear_find(cx);
            view.list.scroll_to_end();
            view.find("needle", cx);
        });
        settle(cx, window);
        let (line, offset) = landed(cx);
        assert!(offset > px(5000.), "{width} {zoom} {offset:?}");
        assert!(on_screen(line), "down {width} {zoom} {line:?}");
        // The reader scrolls away, and the find leaves the screen there.
        view.update(cx, |view, cx| {
            view.list.scroll_by(px(-200.));
            cx.notify();
        });
        settle(cx, window);
        let before = top(cx, &view);
        settle(cx, window);
        let after = top(cx, &view);
        assert_eq!(after.item_ix, before.item_ix);
        assert_eq!(
            after.offset_in_item, before.offset_in_item,
            "the reveal is consumed once"
        );
        // Up to the first match, above the screen, then down again.
        view.update(cx, |view, cx| view.find_next(true, cx));
        settle(cx, window);
        let (line, offset) = landed(cx);
        assert!(offset < px(600.), "{width} {zoom} {offset:?}");
        assert!(on_screen(line), "up {width} {zoom} {line:?}");
        view.update(cx, |view, cx| view.find_next(true, cx));
        settle(cx, window);
        let (line, offset) = landed(cx);
        assert!(offset > px(5000.), "{width} {zoom} {offset:?}");
        assert!(on_screen(line), "down again {width} {zoom} {line:?}");
    }
}

/// Text with matches stays selectable while the find bar is open: a drag
/// across a message of the user's with a highlighted match copies it.
#[gpui_kit::test]
fn found_text_stays_selectable(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("A needle in the hay", Source::App, Delivery::Turn);
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    settle(cx, window);
    view.update(cx, |view, cx| {
        view.find("needle", cx);
    });
    settle(cx, window);
    assert_eq!(cx.read(|cx| view.read(cx).find_status()).total, 1);
    let id = feed.subscribe().0.items[0].id;
    cx.update_window(window, |_, window, cx| {
        let bounds = window.find(("user-message", id)).bounds();
        let middle = bounds.size.height / 2.;
        let from = bounds.origin + Point::new(px(2.), middle);
        let to = bounds.origin + Point::new(bounds.size.width - px(2.), middle);
        window.drag(from, to, cx);
        window.press("secondary-c", cx);
    })
    .unwrap();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, "A needle in the hay");
}

/// The matches on a warning take a shade of its color that reads on their
/// highlight, the current one's and the others', and so do the matches of
/// other lines in theirs.
#[gpui_kit::test]
fn matches_take_a_shade_of_their_lines_color_that_reads(cx: &mut TestAppContext) {
    use gpui_kit::component::ActiveTheme as _;

    let feed = Feed::new("main");
    feed.user("Why is it retrying?", Source::App, Delivery::Turn);
    feed.turn_started();
    feed.notice(
        arko::remote::Level::Warn,
        "rate limited, retrying in 4 s, then retrying again",
    );
    feed.turn_ended("2 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    assert_eq!(find(cx, window, &view, "retrying").total, 3);
    let warning = entry(cx, &view, "rate limited");
    let message = entry(cx, &view, "Why is it");
    cx.update(|cx| {
        let theme = cx.theme();
        let amber = super::Ink::new(theme.warning);
        let bubble = super::Ink::on(theme.foreground, super::super::row::bubble_color(theme));
        let (all, current) = super::match_colors(cx);
        let view = view.read(cx);
        let colors = |entry: u64, ink| {
            let text = view.model.entry(entry).unwrap().item.text.clone();
            view.highlights(entry, Place::Text, &text, ink, cx)
                .into_iter()
                .map(|(_, style)| (style.background_color.unwrap(), style.color))
                .collect::<Vec<_>>()
        };
        let shade = |ink, highlight| Some(super::readable_on(ink, highlight, cx));
        // The latest match is the current one.
        assert_eq!(
            colors(warning, amber),
            [(all, shade(amber, all)), (current, shade(amber, current))]
        );
        assert_ne!(
            shade(amber, current),
            Some(amber.text),
            "amber doesn't read on it"
        );
        assert_eq!(colors(message, bubble), [(all, shade(bubble, all))]);
    });
}

/// A reply with each part of Markdown that looks its own way, in blocks
/// that nest them, with "parser" in each.
const MARKDOWN_PARTS: &str = "Spanning [pars](https://x.test)er, *emphasis parser*, \
    `code parser`, [link `parser` code](https://x.test), [the parser][docs], \
    [a quoted parser][quoted] and [a listed parser][listed].\n\n\
    - item parser\n  - nested parser\n    > quoted parser\n\n\
    > outer parser\n>\n> > inner parser\n>\n> - listed `parser`\n\n\
    Ünïcödé PARSER and [naïve parser](https://x.test)\n\n\
    | head parser `code parser` [parser](https://x.test) |\n| --- |\n\
    | body parser `code parser` [parser](https://x.test) |\n\n\
    A footnote parser[^n].\n\n\
    ```\nlet parser = 1;\n```\n\n\
    [^n]: The footnote parser.\n\n\
    [docs]: https://example.com\n\n\
    > [quoted]: https://example.com\n\n\
    - [listed]: https://example.com\n";

/// The reply [`MARKDOWN_PARTS`] after a question, in a window tall enough
/// to draw all of it, with "parser" found.
fn markdown_parts_found(cx: &mut TestAppContext) -> (AnyWindowHandle, Entity<TranscriptView>, u64) {
    let feed = Feed::new("main");
    feed.user("Stream it.", Source::App, Delivery::Turn);
    feed.turn_started();
    say(&feed, MARKDOWN_PARTS);
    feed.reply(MARKDOWN_PARTS);
    feed.turn_ended("2 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 2000.);
    find(cx, window, &view, "parser");
    let reply = entry(cx, &view, "Spanning");
    (window, view, reply)
}

/// A match as its view was given it, with its text, its highlight, and
/// its text in pieces of one color each.
type Drawn = (
    String,
    gpui_kit::Hsla,
    Vec<(String, Option<gpui_kit::Hsla>)>,
);

/// The pieces of a match, each with its look.
type Looks = Vec<(&'static str, super::Look)>;

/// Each match of block `block` of `entry` as its view was given it.
fn installed(view: &TranscriptView, entry: u64, block: usize, cx: &gpui_kit::App) -> Vec<Drawn> {
    let state = view.existing_block_view(entry, block).unwrap();
    let rendered = state.read(cx).rendered_text();
    let text = rendered.as_str();
    let highlights = &view.find.as_ref().unwrap().installed[&(entry, block)];
    highlights
        .iter()
        .filter(|highlight| highlight.background().a > 0.)
        .map(|found| {
            let range = found.range();
            let mut pieces: Vec<(String, Option<gpui_kit::Hsla>)> = Vec::new();
            for (at, char) in text[range.clone()].char_indices() {
                let at = range.start + at;
                let color = highlights
                    .iter()
                    .rev()
                    .find(|highlight| highlight.range().contains(&at))
                    .and_then(|highlight| highlight.text_color());
                match pieces.last_mut() {
                    Some((piece, last)) if *last == color => piece.push(char),
                    _ => pieces.push((char.to_string(), color)),
                }
            }
            (text[range].to_string(), found.background(), pieces)
        })
        .collect()
}

/// The look of each piece of each match of [`MARKDOWN_PARTS`], by block.
fn markdown_parts_looks() -> Vec<(usize, Vec<Looks>)> {
    use super::{Code, Look, TableRow, Text};

    let text = Look::default();
    let quote = Look {
        text: Text::Quote,
        ..text
    };
    let link = Look {
        text: Text::Link,
        ..text
    };
    let code = |look: Look| Look {
        code: Some(Code::Inline),
        ..look
    };
    let body = Look {
        table: Some(TableRow::Body),
        ..text
    };
    let head = Look {
        table: Some(TableRow::Head),
        ..text
    };
    let one = |look: Look| vec![("parser", look)];
    vec![
        (
            0,
            vec![
                vec![("pars", link), ("er", text)],
                one(text),
                one(code(text)),
                one(code(link)),
                one(link),
                one(link),
                one(link),
            ],
        ),
        (1, vec![one(text), one(text), one(quote)]),
        (2, vec![one(quote), one(quote), one(code(quote))]),
        (3, vec![vec![("PARSER", text)], one(link)]),
        (
            4,
            vec![
                one(head),
                one(code(head)),
                one(Look {
                    text: Text::Link,
                    ..head
                }),
                one(body),
                one(code(body)),
                one(Look {
                    text: Text::Link,
                    ..body
                }),
            ],
        ),
        (5, vec![one(text)]),
        (
            6,
            vec![one(Look {
                code: Some(Code::Block),
                ..text
            })],
        ),
        (7, vec![one(text)]),
    ]
}

/// Checks that each match of [`MARKDOWN_PARTS`] was given the highlight of
/// the current match or the others', and its pieces the shade of their
/// look's ink that reads on it, in the theme in force.
fn check_markdown_highlights(cx: &mut TestAppContext, view: &Entity<TranscriptView>, reply: u64) {
    cx.update(|cx| {
        let view = view.read(cx);
        let inks = super::MarkdownInks::of(false, cx);
        let (all, strong) = super::match_colors(cx);
        let current = view.find.as_ref().unwrap().current.unwrap();
        let mut currents = 0;
        for (block, looks) in markdown_parts_looks() {
            let found = installed(view, reply, block, cx);
            assert_eq!(found.len(), looks.len(), "block {block}: {found:?}");
            for (ordinal, ((text, background, pieces), looks)) in
                found.into_iter().zip(looks).enumerate()
            {
                let is_current = current.entry == reply
                    && current.place == Place::Block(block)
                    && current.ordinal == ordinal;
                currents += usize::from(is_current);
                assert_eq!(background, if is_current { strong } else { all });
                assert_eq!(
                    text,
                    looks.iter().map(|(piece, _)| *piece).collect::<String>()
                );
                // Pieces of one color, as the drawn ones are.
                let mut expected: Vec<(String, Option<gpui_kit::Hsla>)> = Vec::new();
                for (piece, look) in looks {
                    let color = Some(super::readable_on(inks.ink(look), background, cx));
                    match expected.last_mut() {
                        Some((text, last)) if *last == color => text.push_str(piece),
                        _ => expected.push((piece.to_string(), color)),
                    }
                }
                assert_eq!(pieces, expected, "block {block}, match {ordinal}");
            }
        }
        assert_eq!(currents, 1, "the current match is in the reply");
    });
}

/// Each part of Markdown that looks its own way has its look, and a part
/// inside another keeps the outer one's where it sets none. The reply has a
/// link a match spans, emphasis, inline code alone and in a link, a link to
/// a definition in another block, nested lists and quotes, text after
/// characters of several bytes, a table's head and body with their code and
/// links, a footnote and a code block.
#[gpui_kit::test]
fn each_part_of_markdown_has_its_own_look(cx: &mut TestAppContext) {
    use super::{Code, Look, TableRow, Text};

    let (_window, view, reply) = markdown_parts_found(cx);
    // Definitions in a quote or a list count, as anywhere.
    let definitions = view.update(cx, |view, _| view.definitions(reply));
    assert_eq!(definitions, "[docs]: x\n\n[quoted]: x\n\n[listed]: x\n\n");
    cx.update(|cx| {
        let view = view.read(cx);
        let parts = |block: usize| {
            let state = view.existing_block_view(reply, block).unwrap();
            let state = state.read(cx);
            let rendered = state.rendered_text();
            let blocks: Vec<_> = state
                .blocks()
                .iter()
                .filter_map(|block| block.source_range())
                .collect();
            super::markdown_parts(&rendered, &blocks, &definitions)
                .into_iter()
                .map(|(range, look)| (rendered.as_str()[range].to_string(), look))
                .collect::<Vec<_>>()
        };
        let link = Look {
            text: Text::Link,
            ..Look::default()
        };
        let head = Look {
            table: Some(TableRow::Head),
            ..Look::default()
        };
        let body = Look {
            table: Some(TableRow::Body),
            ..Look::default()
        };
        let code = |look: Look| Look {
            code: Some(Code::Inline),
            ..look
        };
        let owned = |parts: &[(&str, Look)]| {
            parts
                .iter()
                .map(|(text, look)| (text.to_string(), *look))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            parts(0),
            owned(&[
                ("pars", link),
                ("code parser", code(Look::default())),
                ("link ", link),
                ("parser", code(link)),
                (" code", link),
                ("the parser", link),
                ("a quoted parser", link),
                ("a listed parser", link),
            ])
        );
        let in_head = Look {
            text: Text::Link,
            ..head
        };
        let in_body = Look {
            text: Text::Link,
            ..body
        };
        assert_eq!(
            parts(4),
            owned(&[
                ("head parser ", head),
                ("code parser", code(head)),
                (" ", head),
                ("parser", in_head),
                ("\nbody parser ", body),
                ("code parser", code(body)),
                (" ", body),
                ("parser", in_body),
            ])
        );
    });
    check_markdown_highlights(cx, &view, reply);
}

/// A theme changed while the find goes on paints the matches in Markdown
/// in its colors, leaving the current match where it is and the reader
/// where they are.
#[gpui_kit::test]
fn a_theme_change_recolors_the_matches_in_markdown(cx: &mut TestAppContext) {
    let (window, view, reply) = markdown_parts_found(cx);
    let installed_before = cx.read(|cx| view.read(cx).find.as_ref().unwrap().installed.clone());
    let (current, scrolled) = cx.read(|cx| {
        let view = view.read(cx);
        (view.find.as_ref().unwrap().current, view.list.top())
    });
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.theme_mode = prefs::ThemeMode::Dark;
            prefs.appearance.dark_theme = "Ayu Dark".into();
        })
    });
    settle(cx, window);
    cx.read(|cx| {
        use gpui_kit::component::ActiveTheme as _;
        assert_eq!(cx.theme().theme_name().as_ref(), "Ayu Dark");
        let view = view.read(cx);
        assert_eq!(view.find.as_ref().unwrap().current, current);
        let top = view.list.top();
        assert_eq!(
            (top.item_ix, top.offset_in_item),
            (scrolled.item_ix, scrolled.offset_in_item)
        );
        assert_ne!(view.find.as_ref().unwrap().installed, installed_before);
    });
    check_markdown_highlights(cx, &view, reply);
}

/// A block's parts are found once for its text. Moving through the matches
/// and changing the theme find them again in no block, and new text in the
/// block has its parts found again.
#[gpui_kit::test]
fn a_blocks_parts_are_found_once_for_its_text(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Stream it.", Source::App, Delivery::Turn);
    feed.turn_started();
    say(
        &feed,
        "The [parser](https://x.test) and `parser` and the parser",
    );
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    find(cx, window, &view, "parser");
    let parsed = super::blocks_parsed();
    assert!(parsed > 0);
    for _ in 0..3 {
        next(cx, window, &view, true);
    }
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.appearance.theme_mode = prefs::ThemeMode::Dark;
        })
    });
    settle(cx, window);
    assert_eq!(super::blocks_parsed(), parsed);
    say(&feed, ", then the parser streams.");
    settle(cx, window);
    next(cx, window, &view, true);
    assert!(super::blocks_parsed() > parsed);
}

/// What the find keeps of a block's view goes with the view. Scrolled up
/// through a long conversation with a match in every block, it keeps the
/// parts of as many blocks as have views, not of every block with a match.
#[gpui_kit::test]
fn a_blocks_parts_go_with_its_view(cx: &mut TestAppContext) {
    use super::super::tests::{load_all, open, start};

    start(cx);
    let feed = Feed::new("main");
    for turn in 0..200 {
        feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        feed.reply(&format!(
            "Answer {turn}, first paragraph.\n\nAnswer {turn}, **second** paragraph."
        ));
        feed.turn_ended("3 s", None);
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    load_all(cx, window, &view);
    assert_eq!(find(cx, window, &view, "paragraph").total, 400);
    // Each block whose parts are kept has a view.
    let kept = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let view = view.read(cx);
            let kept = view.find.as_ref().unwrap().kept_parts();
            for (entry, block) in &kept {
                assert!(
                    view.existing_block_view(*entry, *block).is_some(),
                    "block {block} of {entry} has no view"
                );
            }
            kept.len()
        })
    };
    let count = cx.read(|cx| view.read(cx).row_count());
    let mut most = 0;
    for ix in (0..count).rev().step_by(4) {
        view.update(cx, |view, cx| {
            view.list.scroll_to(ix, px(0.));
            cx.notify();
        });
        settle(cx, window);
        most = most.max(kept(cx));
    }
    let bound = super::super::BLOCK_VIEWS + super::super::RELEASE_EVERY + 20;
    assert!(most > 0 && most <= bound, "parts of {most} blocks kept");
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

/// A long transcript's first search folds its texts off the UI
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

/// Each letter typed lands somewhere and opens what hides the match. What
/// the find opened closes as it moves on: only the match on show keeps its
/// row open once the find closes.
#[gpui_kit::test]
fn what_the_find_opened_closes_as_it_moves_on(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Go.", Source::App, Delivery::Turn);
    feed.turn_started();
    command(&feed, "ls a", "pa.rs");
    command(&feed, "ls b", "par.rs");
    command(&feed, "ls c", "p.rs");
    say(&feed, "Done.");
    feed.reply("Done.");
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    let open = |cx: &mut TestAppContext| {
        let mut open: Vec<u64> = cx.read(|cx| {
            view.read(cx)
                .ui
                .iter()
                .filter(|(_, ui)| ui.expanded == Some(true))
                .map(|(id, _)| *id)
                .collect()
        });
        open.sort_unstable();
        open
    };
    let (a, b, c) = (
        entry(cx, &view, "ls a"),
        entry(cx, &view, "ls b"),
        entry(cx, &view, "ls c"),
    );
    find(cx, window, &view, "p.");
    assert_eq!(open(cx), [c]);
    find(cx, window, &view, "pa");
    assert_eq!(open(cx), [b]);
    find(cx, window, &view, "pa.");
    assert_eq!(open(cx), [a]);
    // The reader opens b themselves: it stays.
    view.update(cx, |view, cx| view.set_expanded(b, true, cx));
    view.update(cx, |view, cx| view.clear_find(cx));
    settle(cx, window);
    assert_eq!(open(cx), [a, b]);
}

/// Landing on a match on screen moves nothing, and landing deep in a tall
/// message of the user's shows the match, not the message's top.
#[gpui_kit::test]
fn landing_scrolls_only_as_far_as_the_match(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    let long: String = (0..300)
        .map(|line| match line {
            250 => "the needle line\n".to_string(),
            _ => format!("pasted line {line}\n"),
        })
        .collect();
    feed.user(&long, Source::App, Delivery::Turn);
    feed.turn_started();
    say(&feed, "Seen.");
    feed.reply("Seen.");
    feed.turn_ended("1 s", None);
    for n in 0..60 {
        feed.user(&format!("Note number {n}."), Source::App, Delivery::Note);
    }
    let (window, view) = open_feed(cx, &feed, View::Full, 600.);
    let on_screen = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).list.on_screen());
    let before = on_screen(cx);
    let note = entry(cx, &view, "Note number 58.");
    let row = cx.read(|cx| view.read(cx).row_of(note).unwrap());
    assert!(before.contains(&row));
    assert_eq!(find(cx, window, &view, "number 58.").total, 1);
    assert_eq!(on_screen(cx), before, "nothing moved");
    // The needle, 250 lines down a message far above.
    assert_eq!(find(cx, window, &view, "needle line").total, 1);
    settle(cx, window);
    let message = entry(cx, &view, "the needle line");
    let row = cx.read(|cx| view.read(cx).row_of(message).unwrap());
    let top = top(cx, &view);
    assert_eq!(top.item_ix, row);
    assert!(top.offset_in_item > px(3000.), "{top:?}");
}

/// A long code block in a list item is capped like one at the top level:
/// landing past its first lines shows it whole.
#[gpui_kit::test]
fn landing_in_a_long_code_block_in_a_list_shows_it_whole(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Steps?", Source::App, Delivery::Turn);
    feed.turn_started();
    let mut text = String::from("1. Run this:\n\n   ```bash\n");
    for line in 0..100 {
        if line == 80 {
            text.push_str("   echo needle\n");
        } else {
            text.push_str(&format!("   echo line {line}\n"));
        }
    }
    text.push_str("   ```\n\n2. Done.\n");
    say(&feed, &text);
    feed.reply(&text);
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    assert_eq!(find(cx, window, &view, "needle").total, 1);
    let steps = entry(cx, &view, "Run this");
    let uncapped = cx.read(|cx| view.read(cx).ui.get(&steps).unwrap().uncapped.len());
    assert_eq!(uncapped, 1, "the list item's block shows whole");
    // The find moves on: it shows the first lines again.
    assert_eq!(find(cx, window, &view, "Done").total, 1);
    let uncapped = cx.read(|cx| view.read(cx).ui.get(&steps).unwrap().uncapped.len());
    assert_eq!(uncapped, 0);
}

/// Matches past the lines a call's details show (its first 40, each cut at
/// 500 characters) are found: the find bar says they're in the full
/// output, which opens at the match.
#[gpui_kit::test]
fn matches_past_an_outputs_first_lines_are_found_in_the_full_output(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Run it.", Source::App, Delivery::Turn);
    feed.turn_started();
    let output: String = (0..100)
        .map(|line| match line {
            80 => "line 80 error[E0308]: mismatched types\n".to_string(),
            _ => format!("line {line}\n"),
        })
        .collect();
    command(&feed, "cargo build", &output);
    // The engine keeps a command's whole output when it has more lines
    // than its first few.
    let long_line = format!(
        "warning: first\nwarning: second\nwarning: third\nwarning: fourth\n{}error[E0599] at the end\n",
        "x".repeat(3000)
    );
    command(&feed, "cargo check", &long_line);
    command(&feed, "cargo fmt", "error[E0425] early\n");
    feed.turn_ended("1 s", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    for code in ["E0308", "E0599"] {
        let status = find(cx, window, &view, code);
        assert_eq!(status.total, 1, "{code}");
        assert!(status.in_full_output, "{code}");
        // Its row shows the first lines, without the match.
        let build = entry(cx, &view, "cargo build");
        let check = entry(cx, &view, "cargo check");
        for id in [build, check] {
            assert!(
                cx.read(|cx| view.read(cx).match_ranges(id, Place::Output))
                    .is_empty()
            );
        }
    }
    // One the row shows.
    let status = find(cx, window, &view, "E0425");
    assert_eq!(status.total, 1);
    assert!(!status.in_full_output);
    // The full output opens at the match.
    find(cx, window, &view, "E0308");
    cx.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| view.show_match_in_full(window, cx));
        use gpui_kit::component::WindowExt as _;
        assert!(window.has_active_dialog(cx));
    })
    .unwrap();
}

/// The find covers what a row shows beside its text: why a call failed,
/// a worker card's title, a turn's footer.
#[gpui_kit::test]
fn the_find_covers_a_failure_a_workers_title_and_a_turns_footer(cx: &mut TestAppContext) {
    let feed = Feed::new("main");
    feed.user("Go.", Source::App, Delivery::Turn);
    feed.turn_started();
    let id = Uuid::new_v4();
    let parameters: HashMap<String, Value> = [("command".to_string(), Value::from("cargo test"))]
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
            output: "exit code 101, 0.2 s\nthe tests failed".into(),
        }),
        None,
    );
    feed.worker(
        "r1",
        "Profile the renderer",
        arko::remote::WorkerChange::Started,
    );
    feed.turn_ended("48 s · $0.21", None);
    let (window, view) = open_feed(cx, &feed, View::Full, 900.);
    let call = entry(cx, &view, "cargo test");
    assert_eq!(find(cx, window, &view, "exit 101").total, 1);
    assert_eq!(
        cx.read(|cx| view.read(cx).match_ranges(call, Place::Outcome))
            .len(),
        1
    );
    assert_eq!(find(cx, window, &view, "renderer").total, 1);
    assert_eq!(find(cx, window, &view, "$0.21").total, 1);
}
