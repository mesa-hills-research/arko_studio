//! How the transcript shows the agent's work: thoughts as their first line,
//! Quiet's rows of calls, one marker column and the spaces around what is
//! open, drawn from a feed as a session feeds it.

use arko::remote::{Delivery, Feed, Kind, Source};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use super::model::{Gap, Rail, RowKey};
use super::tests::{call, item_ids, keys, open, result, settle, start, think};

use super::{AgentFilter, TranscriptView};
use crate::prefs::{self, TranscriptView as View};

/// The IDs of `feed`'s items of `kind`, in order.
fn ids_of(feed: &Feed, kind: Kind) -> Vec<u64> {
    item_ids(feed)
        .into_iter()
        .filter(|(_, found)| *found == kind)
        .map(|(id, _)| id)
        .collect()
}

fn set_view(cx: &mut TestAppContext, view: View) {
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = view));
}

fn row_of(
    view: &gpui_kit::Entity<TranscriptView>,
    cx: &mut TestAppContext,
    key: RowKey,
) -> super::model::Row {
    cx.read(|cx| {
        *view
            .read(cx)
            .rows
            .iter()
            .find(|row| row.key == key)
            .unwrap_or_else(|| panic!("no row {key:?}"))
    })
}

/// A thought shows its first line, with no label. One that fits on its
/// line has nothing to open. One with more opens to its text, which stands
/// apart from its neighbours, and closes to its line again.
#[gpui_kit::test]
fn a_thought_is_its_first_line_and_opens_when_it_says_more(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Look", Source::App, Delivery::Turn);
    feed.turn_started();
    think(&feed, "Short.");
    let read = call(&feed, "main", "read_file", &[("path", "src/a.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
    think(&feed, "First, the parser.\n\nThen its tests.");
    feed.reply("Done.");
    feed.turn_ended("1 s", None);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let thoughts = ids_of(&feed, Kind::Thinking);
    let (short, long) = (thoughts[0], thoughts[1]);
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(!view.thought_opens(short, false));
        assert!(view.thought_opens(long, false));
    });
    let line = |cx: &mut TestAppContext, id: u64| {
        cx.update_window(window, |_, window, _| {
            window
                .try_find(("thinking-toggle", id))
                .and_then(|toggle| toggle.label().map(str::to_string))
        })
        .unwrap()
    };
    assert_eq!(line(cx, long).as_deref(), Some("First, the parser. …"));
    // A click on the short one does nothing.
    cx.update_window(window, |_, window, cx| {
        window.click(("thinking-row", short), cx)
    })
    .unwrap();
    settle(cx, window);
    assert!(keys(&view, cx).contains(&RowKey::Thinking(short)));
    // A click on the long one opens it, apart from what is around it.
    cx.update_window(window, |_, window, cx| {
        window.click(("thinking-toggle", long), cx)
    })
    .unwrap();
    settle(cx, window);
    let rows = keys(&view, cx);
    assert!(!rows.contains(&RowKey::Thinking(long)), "{rows:?}");
    let first = RowKey::Block {
        id: long,
        generation: 0,
        block: 0,
        source: false,
    };
    let second = RowKey::Block {
        id: long,
        generation: 0,
        block: 1,
        source: false,
    };
    assert_eq!(row_of(&view, cx, first).gap, Gap::Apart);
    assert_eq!(row_of(&view, cx, second).gap, Gap::Small);
    let after = rows.iter().position(|key| *key == second).unwrap() + 1;
    let next = cx.read(|cx| view.read(cx).rows[after]);
    assert_eq!(next.gap, Gap::Apart, "{next:?}");
}

/// The thought that streams is the one live line: the spinner turns beside
/// it, and no "Working…" shows. Once it ends the spinner goes and the line
/// stays.
#[gpui_kit::test]
fn the_thought_that_streams_is_the_live_line(cx: &mut TestAppContext) {
    start(cx);
    cx.update(|cx| {
        prefs::update(cx, |prefs| prefs.appearance.motion = prefs::Motion::On);
        cx.set_reduce_motion(false);
    });
    let feed = Feed::new("main");
    feed.user("Look", Source::App, Delivery::Turn);
    feed.turn_started();
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let working = |cx: &mut TestAppContext| {
        keys(&view, cx)
            .iter()
            .any(|key| matches!(key, RowKey::Working(_)))
    };
    assert!(working(cx));
    think(&feed, "Where to start");
    settle(cx, window);
    let thought = ids_of(&feed, Kind::Thinking)[0];
    assert!(!working(cx));
    let live = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.try_find(("live", thought)).is_some()
        })
        .unwrap()
    };
    assert!(live(cx));
    let read = call(&feed, "main", "read_file", &[("path", "src/a.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
    settle(cx, window);
    assert!(!live(cx));
    assert!(keys(&view, cx).contains(&RowKey::Thinking(thought)));
}

/// In Quiet, the reads and edits of one file are one row whose words name
/// each kind, and a call that runs has no marker until it has something
/// to show.
#[gpui_kit::test]
fn quiet_rows_name_each_kind_and_open_once_they_hold_something(cx: &mut TestAppContext) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    feed.user("Fix it", Source::App, Delivery::Turn);
    feed.turn_started();
    let read = call(&feed, "main", "read_file", &[("path", "src/app.rs".into())]);
    result(&feed, "main", read, "read_file", "1 | fn app() {}\n");
    let edit = call(
        &feed,
        "main",
        "edit_file",
        &[
            ("path", "src/app.rs".into()),
            ("old_string", "fn app() {}".into()),
            ("new_string", "fn app() -> u8 { 1 }".into()),
        ],
    );
    result(
        &feed,
        "main",
        edit,
        "edit_file",
        "Edited src/app.rs at line 1",
    );
    let build = call(&feed, "main", "shell", &[("command", "cargo build".into())]);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    let calls = ids_of(&feed, Kind::Call);
    let label = |cx: &mut TestAppContext, key| cx.read(|cx| view.read(cx).row_label(key));
    assert_eq!(
        label(cx, RowKey::Group(calls[0])).as_ref(),
        "Read app.rs and made 1 edit"
    );
    let opens = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).group_opens(calls[2]));
    assert!(!opens(cx), "a command that runs has nothing to show yet");
    result(
        &feed,
        "main",
        build,
        "shell",
        "exit code 0, 1.0 s\nFinished",
    );
    settle(cx, window);
    assert!(opens(cx));
}

/// A Quiet row's line is one line at any width: where it cuts its words it
/// opens, to the call as written.
#[gpui_kit::test]
fn a_quiet_row_that_cuts_its_words_opens_to_them(cx: &mut TestAppContext) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    feed.user("Wait", Source::App, Delivery::Turn);
    feed.turn_started();
    let command = format!("echo {}", "done ".repeat(7));
    call(
        &feed,
        "main",
        "shell",
        &[("command", command.trim().into())],
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    let first = ids_of(&feed, Kind::Call)[0];
    let opens = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).group_opens(first));
    // 800 px wide: the words fit.
    assert!(!opens(cx));
    cx.simulate_window_resize(
        window,
        gpui_kit::size(gpui_kit::px(360.), gpui_kit::px(400.)),
    );
    settle(cx, window);
    assert!(opens(cx));
    let height = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window
                .find(super::row::element_id(RowKey::Group(first)))
                .bounds()
                .size
                .height
        })
        .unwrap()
    };
    let closed = height(cx);
    view.update(cx, |view, cx| view.toggle_group(first, cx));
    settle(cx, window);
    assert!(height(cx) > closed);
}

/// Opening a row among closed ones keeps its line where it is on screen:
/// what is above moves up to make the gap above it.
#[gpui_kit::test]
fn an_opened_row_keeps_its_line_in_place(cx: &mut TestAppContext) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    for turn in 0..8 {
        feed.user(&format!("Turn {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        let read = call(&feed, "main", "read_file", &[("path", "src/a.rs".into())]);
        result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
        let command = call(&feed, "main", "shell", &[("command", "ls".into())]);
        result(&feed, "main", command, "shell", "exit code 0\na.rs\n");
        feed.reply("Done.");
        feed.turn_ended("1 s", None);
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    let calls = ids_of(&feed, Kind::Call);
    let ls = calls[calls.len() - 1];
    let top = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            f32::from(
                window
                    .find(super::row::element_id(RowKey::Group(ls)))
                    .bounds()
                    .origin
                    .y,
            )
        })
        .unwrap()
    };
    let before = top(cx);
    let row = row_of(&view, cx, RowKey::Group(ls));
    assert_eq!((row.gap, row.rail), (Gap::Stack, Rail::No));
    view.update(cx, |view, cx| view.toggle_group(ls, cx));
    settle(cx, window);
    assert_eq!(row_of(&view, cx, RowKey::Group(ls)).gap, Gap::Apart);
    // (The row's own top includes the gap above it.)
    let gap = cx.read(|cx| view.read(cx).metrics.as_ref().unwrap().rem) * 0.5;
    assert_eq!(top(cx), before - gap);
}
