//! What a line of work says and opens to, what the find reads of it, and
//! how an open thought, an open count and Messages' live line behave.

use arko::remote::{Delivery, Feed, Kind, Source};
use arko_agents::AgentEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use super::model::RowKey;
use super::tests::{call, event, item_ids, keys, open, result, say, settle, start, think};
use super::{AgentFilter, TranscriptView};
use crate::prefs::{self, TranscriptView as View};

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

/// Fails the call `id` of `tool` with `reason`, as a session does.
fn fail(feed: &Feed, id: uuid::Uuid, tool: &str, reason: &str) {
    feed.event(
        &event(
            "main",
            AgentEvent::ErrorEncountered {
                error_message: format!("Tool '{tool}' execution failed: {reason}"),
                requires_approval_to_retry: false,
                tool_call_id: Some(id),
                error_kind: None,
            },
        ),
        None,
    );
}

/// An edit of `src/a.rs`.
fn edit(feed: &Feed) -> uuid::Uuid {
    call(
        feed,
        "main",
        "edit_file",
        &[
            ("path", "src/a.rs".into()),
            ("old_string", "fn a() {}".into()),
            ("new_string", "fn a() -> u8 { 1 }".into()),
        ],
    )
}

fn drawn(cx: &mut TestAppContext, window: AnyWindowHandle, id: (&'static str, u64)) -> bool {
    cx.update_window(window, |_, window, _| window.try_find(id).is_some())
        .unwrap()
}

/// A Quiet row says only that its call failed, so it opens to why, even
/// where the reason is short enough for Full's line to say it whole: a
/// file that isn't there, an edit whose text isn't found.
#[gpui_kit::test]
fn a_failed_quiet_row_opens_to_why_it_failed(cx: &mut TestAppContext) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    feed.user("Fix it", Source::App, Delivery::Turn);
    feed.turn_started();
    let read = call(
        &feed,
        "main",
        "read_file",
        &[("path", "src/stream.rs".into())],
    );
    fail(&feed, read, "read_file", "src/stream.rs doesn't exist.");
    say(&feed, "main", "Not there. The edit, then.");
    let edited = edit(&feed);
    fail(
        &feed,
        edited,
        "edit_file",
        "old_string was not found in src/a.rs.",
    );
    feed.reply("Neither worked.");
    feed.turn_ended("1 s", None);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    for id in ids_of(&feed, Kind::Call) {
        assert!(cx.read(|cx| view.read(cx).group_opens(id)), "{id}");
        view.update(cx, |view, cx| view.set_row_open(id, true, cx));
        settle(cx, window);
        assert!(
            drawn(cx, window, ("call-output", id)),
            "{id} shows why it failed"
        );
    }
}

/// A Full line whose outcome is cut at the reader's width opens to the
/// outcome whole.
#[gpui_kit::test]
fn a_full_line_that_cuts_its_outcome_opens_to_it(cx: &mut TestAppContext) {
    start(cx);
    let feed = Feed::new("main");
    feed.user("Read it", Source::App, Delivery::Turn);
    feed.turn_started();
    let read = call(
        &feed,
        "main",
        "read_file",
        &[("path", "src/stream.rs".into())],
    );
    fail(
        &feed,
        read,
        "read_file",
        "src/stream.rs doesn't exist, and the tokenizer reads no stream.",
    );
    let (window, view) = open(cx, &feed, AgentFilter::Main, 600.);
    cx.simulate_window_resize(
        window,
        gpui_kit::size(gpui_kit::px(400.), gpui_kit::px(600.)),
    );
    settle(cx, window);
    let id = ids_of(&feed, Kind::Call)[0];
    assert!(cx.read(|cx| view.read(cx).call_opens(id, false)));
    view.update(cx, |view, cx| view.set_expanded(id, true, cx));
    settle(cx, window);
    assert!(drawn(cx, window, ("call-output", id)));
}

/// The thoughts a reader meets, one line each, cut at word ends to lengths
/// around what fits the column. Drawn in the app's fonts, each whose line
/// is cut opens.
#[cfg(target_os = "linux")]
#[test]
fn a_thought_whose_line_is_cut_opens() {
    use super::height_tests::real;
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{FontStyle, TextRun, font};

    const THOUGHTS: [&str; 4] = [
        "The user wants the stale cache test fixed, so I should read the test first and then look at how the cache decides that a value has expired before changing anything else in the module.",
        "Now WAIT for the build, then run cargo test --locked --lib transcript:: and look at which of the FOUR goldens changed, since the MAIN one should stay as it was before this.",
        "Wow, the migration will add MANY NEW COLUMNS: OWNER, GROUP, MODE, MTIME and CTIME, which the old rows lack, so a default must come from somewhere, and NULL is the honest one.",
        "Reading the README again to confirm the install command and the minimum supported version before I write the docs page, since the changelog and the README disagree about it today.",
    ];
    let mut cx = real::app();
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    let feed = Feed::new("main");
    feed.user("Go", Source::App, Delivery::Turn);
    feed.turn_started();
    let mut texts = Vec::new();
    for thought in THOUGHTS {
        let ends = thought
            .char_indices()
            .filter(|(at, c)| *c == ' ' && (70..=150).contains(at))
            .map(|(at, _)| at);
        for end in ends {
            think(&feed, &thought[..end]);
            say(&feed, "main", "ok");
            texts.push(thought[..end].to_string());
        }
    }
    let width = 800.;
    let (window, view) = real::open(&mut cx, &feed, (width, 600.), true);
    real::settle(&mut cx, window);
    let thoughts = ids_of(&feed, Kind::Thinking);
    let mut cut = 0;
    for (id, text) in thoughts.iter().zip(&texts) {
        // Each line drawn, so its width is known.
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let ix = view
                    .rows
                    .iter()
                    .position(|row| row.key == RowKey::Thinking(*id))
                    .unwrap();
                view.list.scroll_to(ix, gpui_kit::px(0.));
                cx.notify();
            })
        });
        real::settle(&mut cx, window);
        let (opens, room, shaped) = cx
            .update_window(window, |_, window, cx| {
                let view = view.read(cx);
                let m = view.metrics.clone().unwrap();
                let mut italic = font(cx.theme().font_family.clone());
                italic.style = FontStyle::Italic;
                let run = TextRun {
                    len: text.len(),
                    font: italic,
                    color: gpui_kit::Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let size = gpui_kit::px(m.rem * super::row::THOUGHT_SIZE);
                let shaped = window.text_system().layout_line(text, size, &[run], None);
                (
                    view.thought_opens(*id, false),
                    super::row::room(width, false, m.rem),
                    f32::from(shaped.width),
                )
            })
            .unwrap();
        if shaped > room {
            cut += 1;
            assert!(opens, "{text:?} is cut ({shaped} px in {room}) and opens");
        }
    }
    assert!(cut > 0);
}

/// The reader scrolled up, a thought's line is the first row on screen,
/// and they open it. Its first paragraph stays where its line was.
#[gpui_kit::test]
fn the_thought_opened_at_the_top_of_the_screen_stays_there(cx: &mut TestAppContext) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    for turn in 0..20 {
        feed.user(&format!("Question {turn}"), Source::App, Delivery::Turn);
        feed.turn_started();
        think(
            &feed,
            &format!("Turn {turn}, the first paragraph.\n\nThe second.\n\nThe third.\n\nThe last."),
        );
        let read = call(&feed, "main", "read_file", &[("path", "src/a.rs".into())]);
        result(&feed, "main", read, "read_file", "1 | fn a() {}\n");
        feed.reply("Answer.\n\nMore of it.");
        feed.turn_ended("1 s", None);
    }
    let (window, view) = open(cx, &feed, AgentFilter::Main, 400.);
    super::tests::load_all(cx, window, &view);
    let thought = ids_of(&feed, Kind::Thinking)[10];
    let ix = keys(&view, cx)
        .iter()
        .position(|key| *key == RowKey::Thinking(thought))
        .unwrap();
    view.update(cx, |view, cx| {
        view.list.scroll_to(ix, gpui_kit::px(0.));
        cx.notify();
    });
    settle(cx, window);
    cx.update_window(window, |_, window, cx| {
        window.click(("thinking-toggle", thought), cx)
    })
    .unwrap();
    settle(cx, window);
    let first = keys(&view, cx)
        .iter()
        .position(|key| matches!(key, RowKey::Block { id, block: 0, .. } if *id == thought))
        .unwrap();
    assert_eq!(cx.read(|cx| view.read(cx).list.top().item_ix), first);
}

/// An open thought with a long code block, and the thought's rows.
fn open_thought(
    cx: &mut TestAppContext,
    text: &str,
) -> (u64, AnyWindowHandle, Entity<TranscriptView>) {
    start(cx);
    set_view(cx, View::Quiet);
    let feed = Feed::new("main");
    feed.user("Look", Source::App, Delivery::Turn);
    feed.turn_started();
    think(&feed, text);
    feed.reply("Done.");
    feed.turn_ended("1 s", None);
    let (window, view) = open(cx, &feed, AgentFilter::Main, 1600.);
    let thought = ids_of(&feed, Kind::Thinking)[0];
    cx.update_window(window, |_, window, cx| {
        window.click(("thinking-toggle", thought), cx)
    })
    .unwrap();
    settle(cx, window);
    (thought, window, view)
}

fn is_open(cx: &mut TestAppContext, view: &Entity<TranscriptView>, thought: u64) -> bool {
    keys(view, cx)
        .iter()
        .any(|key| matches!(key, RowKey::Block { id, .. } if *id == thought))
}

/// Show all, under a long code block of an open thought, shows the code
/// whole and leaves the thought open.
#[gpui_kit::test]
fn show_all_in_an_open_thought_keeps_it_open(cx: &mut TestAppContext) {
    let code: String = (1..=60).map(|n| format!("line {n}\n")).collect();
    let (thought, window, view) =
        open_thought(cx, &format!("The listing I mean.\n\n```text\n{code}```\n"));
    assert!(is_open(cx, &view, thought));
    cx.update_window(window, |_, window, cx| window.click("show-all", cx))
        .unwrap();
    settle(cx, window);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(cx, window);
    let uncapped = cx.read(|cx| {
        view.read(cx)
            .ui
            .get(&thought)
            .is_some_and(|ui| !ui.uncapped.is_empty())
    });
    assert!(uncapped);
    assert!(is_open(cx, &view, thought));
}

/// A double-click on an open thought, as a reader makes one to select a
/// word, leaves it open. A single click closes it.
#[gpui_kit::test]
fn a_double_click_on_an_open_thought_leaves_it_open(cx: &mut TestAppContext) {
    let (thought, window, view) = open_thought(
        cx,
        "First, the parser and how it takes its input.\n\nThen its tests, one at a time.",
    );
    let block = super::row::element_id(RowKey::Block {
        id: thought,
        generation: 0,
        block: 0,
        source: false,
    });
    cx.update_window(window, |_, window, cx| {
        window.click(block.clone(), cx);
        window.double_click(block.clone(), cx);
    })
    .unwrap();
    settle(cx, window);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(cx, window);
    assert!(is_open(cx, &view, thought));
    cx.update_window(window, |_, window, cx| window.click(block.clone(), cx))
        .unwrap();
    settle(cx, window);
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    settle(cx, window);
    assert!(!is_open(cx, &view, thought));
}
