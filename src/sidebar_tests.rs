//! The sidebar's dot for news: a session gets it when a turn ends or a
//! question comes while the user looks at another session, and loses it
//! when the session is shown. A closed group shows it for the sessions
//! inside.

use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Activity, Question, Reading};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, ElementId, Entity, TestAppContext};

use crate::Startup;
use crate::backend::{Backend as _, FakeBackend, SessionId};
use crate::live::{Indicator, LiveSession, LiveSessions};
use crate::notifications;
use crate::sidebar::Row;
use crate::tests::{act, open, open_title, row_of};
use crate::workspace::Workspace;

const SPLIT: &str = "20261016-091200-a1b2";
const FONTS: &str = "20261016-080000-a7b8";
const SPLIT_TITLE: &str = "Split the engine out of main.rs";
const FONTS_TITLE: &str = "Variable fonts and the wght axis";

/// A window in front, with the first sample session (in the folder
/// "arko") and the fourth (a chat) running in the app.
struct App {
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    split: Entity<LiveSession>,
    fonts: Entity<LiveSession>,
}

fn start(news: &[&str], cx: &mut TestAppContext) -> App {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let entries = backend.sessions().unwrap();
    cx.update(|cx| crate::init(Startup::for_tests(backend).with_news(news), cx));
    let (window, workspace) = open(cx, 1300.);
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    let split = cx.update(|cx| LiveSession::resume(&entries[0], cx));
    let fonts = cx.update(|cx| LiveSession::resume(&entries[3], cx));
    cx.run_until_parked();
    App {
        window,
        workspace,
        split,
        fonts,
    }
}

/// A turn of `session`, which asks `question` first when given.
fn turn(session: &Entity<LiveSession>, cx: &mut TestAppContext, question: Option<&str>) {
    for working in [true, false] {
        let activity = Activity {
            working,
            question: question.filter(|_| working).map(|text| Question {
                id: 1,
                text: text.into(),
            }),
            since: None,
        };
        session.update(cx, |session, cx| {
            session.set_status(Reading::default(), activity, cx)
        });
        cx.run_until_parked();
    }
}

impl App {
    fn has_news(&self, id: &str, cx: &mut TestAppContext) -> bool {
        cx.read(|cx| notifications::has_news(&SessionId(id.into()), cx))
    }

    fn shows(&self, id: impl Into<ElementId>, cx: &mut TestAppContext) -> bool {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }

    fn label(&self, id: impl Into<ElementId>, cx: &mut TestAppContext) -> Option<String> {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).label().map(String::from)
        })
        .unwrap()
    }

    /// The index of the row of the group `key` in the list.
    fn group_row(&self, key: &str, cx: &mut TestAppContext) -> usize {
        cx.read(|cx| {
            self.workspace
                .read(cx)
                .sidebar()
                .read(cx)
                .rows()
                .iter()
                .position(|row| matches!(row, Row::Group { key: group, .. } if group == key))
                .unwrap_or_else(|| panic!("no group {key}"))
        })
    }
}

/// A turn that ends in a session the window doesn't show gives it a dot,
/// and showing the session takes it away.
#[gpui_kit::test]
fn a_turn_that_ends_out_of_sight_gives_a_dot_until_the_session_is_shown(cx: &mut TestAppContext) {
    let app = start(&[], cx);
    let row = row_of(&app.workspace, SPLIT_TITLE, cx);
    assert!(
        !app.shows(("row-news", row), cx),
        "no decoration without news"
    );
    turn(&app.split, cx, None);
    assert!(app.has_news(SPLIT, cx));
    assert!(app.shows(("row-news", row), cx));
    // The other sessions have none.
    let other = row_of(&app.workspace, FONTS_TITLE, cx);
    assert!(!app.shows(("row-news", other), cx));
    // Clicking the row shows the session, and the dot goes.
    act(cx, app.window, |window, cx| window.click(("row", row), cx));
    assert_eq!(open_title(&app.workspace, cx).as_deref(), Some(SPLIT_TITLE));
    assert!(!app.has_news(SPLIT, cx));
    assert!(!app.shows(("row-news", row), cx));
}

/// A question gives the dot too, with the notifications of Settings off.
#[gpui_kit::test]
fn a_question_gives_a_dot_whichever_notifications_are_on(cx: &mut TestAppContext) {
    let app = start(&[], cx);
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.notifications.turn_ended = false;
            prefs.notifications.question = false;
        })
    });
    turn(&app.fonts, cx, Some("Tabs or spaces?"));
    assert!(app.has_news(FONTS, cx));
    let row = row_of(&app.workspace, FONTS_TITLE, cx);
    assert!(app.shows(("row-news", row), cx));
    // Nothing was kept in the history, which follows Settings.
    assert!(cx.read(|cx| notifications::entries(cx).is_empty()));
}

/// A session in view never gets one, and the session shown in a window
/// that was behind loses it when the window comes to the front.
#[gpui_kit::test]
fn a_session_in_view_gets_no_dot_and_a_window_coming_forward_clears_it(cx: &mut TestAppContext) {
    let app = start(&[], cx);
    let row = row_of(&app.workspace, SPLIT_TITLE, cx);
    act(cx, app.window, |window, cx| window.click(("row", row), cx));
    turn(&app.split, cx, None);
    assert!(!app.has_news(SPLIT, cx), "it is in view");
    assert!(!app.shows(("row-news", row), cx));
    // A second window comes to the front, over the first: the session of
    // the first is out of sight, so a turn gives it news. Bringing that
    // window back shows it.
    let (second, _) = open(cx, 1300.);
    cx.update_window(second, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    turn(&app.split, cx, None);
    assert!(app.has_news(SPLIT, cx));
    cx.update_window(app.window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    assert!(!app.has_news(SPLIT, cx));
}

/// The turn that keeps the cache warm with a short message is no news.
#[gpui_kit::test]
fn a_cache_refresh_turn_gives_no_news(cx: &mut TestAppContext) {
    let app = start(&[], cx);
    app.split.update(cx, |session, _| {
        session.answer_refresh_with(Ok(arko::Refreshed::Message))
    });
    app.split
        .update(cx, |session, cx| session.refresh_cache(cx))
        .detach();
    cx.run_until_parked();
    turn(&app.split, cx, None);
    assert!(cx.read(|cx| app.split.read(cx).turn_was_refresh()));
    assert!(!app.has_news(SPLIT, cx));
    // The next turn is news.
    turn(&app.split, cx, None);
    assert!(app.has_news(SPLIT, cx));
}

/// A cache reminder alone gives no dot, though it is an unread notification.
#[gpui_kit::test]
fn a_cache_reminder_alone_gives_no_dot(cx: &mut TestAppContext) {
    use crate::cache_reminders::ReminderClock;
    use arko::remote::CacheStatus;
    use std::time::Duration;

    const NOW: i64 = 1_800_000_000_000;
    let app = start(&[], cx);
    cx.update(|cx| cx.set_global(ReminderClock(std::rc::Rc::new(|| NOW))));
    // 50 s of a 5-minute cache left: the last reminder is due.
    let reading = Reading {
        cache: CacheStatus {
            lifetime: Some(Duration::from_secs(300)),
            refreshed_at: Some(NOW - 250_000),
            ..CacheStatus::default()
        },
        ..Reading::default()
    };
    app.split.update(cx, |session, cx| {
        session.set_touched_at(NOW);
        session.set_status(reading, Activity::default(), cx)
    });
    cx.run_until_parked();
    assert_eq!(cx.read(notifications::unread), 1);
    assert!(!app.has_news(SPLIT, cx));
    let row = row_of(&app.workspace, SPLIT_TITLE, cx);
    assert!(!app.shows(("row-news", row), cx));
}

/// The dot goes with the spinner and the question mark, and the row's name
/// says there is news while the dot itself has no name.
#[gpui_kit::test]
fn the_dot_shows_with_the_other_marks_and_the_row_names_the_news(cx: &mut TestAppContext) {
    let app = start(&[SPLIT, FONTS], cx);
    let split = row_of(&app.workspace, SPLIT_TITLE, cx);
    let fonts = row_of(&app.workspace, FONTS_TITLE, cx);
    cx.update(|cx| {
        LiveSessions::set_indicator(
            &SessionId(SPLIT.into()),
            Indicator {
                working: true,
                question: false,
            },
            cx,
        );
        LiveSessions::set_indicator(
            &SessionId(FONTS.into()),
            Indicator {
                working: false,
                question: true,
            },
            cx,
        );
    });
    cx.run_until_parked();
    assert!(app.shows(("row-working", split), cx));
    assert!(app.shows(("row-news", split), cx));
    assert!(app.shows(("row-question", fonts), cx));
    assert!(app.shows(("row-news", fonts), cx));
    assert_eq!(
        app.label(("row", split), cx).as_deref(),
        Some("Split the engine out of main.rs, working, new activity")
    );
    assert_eq!(
        app.label(("row", fonts), cx).as_deref(),
        Some("Variable fonts and the wght axis, a question waits, new activity")
    );
    // The dot has neither a role nor a name for a screen reader.
    cx.update_window(app.window, |_, window, cx| {
        window.render_frame(cx);
        let dot = window.find(("row-news", split));
        assert_eq!(dot.role(), None);
        assert_eq!(dot.label(), None);
    })
    .unwrap();
}

/// A closed group shows the dot beside its name while a session inside has
/// news, the chats too. An open group shows it on the session's row only.
#[gpui_kit::test]
fn a_closed_group_shows_the_dot_for_the_sessions_inside(cx: &mut TestAppContext) {
    let app = start(&[SPLIT, FONTS], cx);
    let arko = app.group_row("/home/ada/src/arko", cx);
    let chats = app.group_row("chats", cx);
    assert!(
        !app.shows(("group-news", arko), cx),
        "open groups show the rows"
    );
    assert!(!app.shows(("group-news", chats), cx));
    assert_eq!(app.label(("row", arko), cx).as_deref(), Some("arko"));
    // Closing the folder's group.
    act(cx, app.window, |window, cx| window.click(("row", arko), cx));
    let arko = app.group_row("/home/ada/src/arko", cx);
    assert!(app.shows(("group-news", arko), cx));
    assert_eq!(
        app.label(("row", arko), cx).as_deref(),
        Some("arko, new activity")
    );
    // The other group is open still.
    let chats = app.group_row("chats", cx);
    assert!(!app.shows(("group-news", chats), cx));
    act(cx, app.window, |window, cx| {
        window.click(("row", chats), cx)
    });
    let chats = app.group_row("chats", cx);
    assert!(app.shows(("group-news", chats), cx));
    // Showing the session of a closed group takes its dot away, and the
    // session shown stays out of it.
    cx.update_window(app.window, |_, window, cx| {
        app.workspace.update(cx, |workspace, cx| {
            workspace.open_session(SessionId(FONTS.into()), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    assert!(!app.shows(("group-news", app.group_row("chats", cx)), cx));
    assert!(app.shows(("group-news", app.group_row("/home/ada/src/arko", cx)), cx));
}

/// News is there when the app starts again: the history file holds it.
#[gpui_kit::test]
fn news_from_an_earlier_run_shows_at_the_start(cx: &mut TestAppContext) {
    let app = start(&[FONTS], cx);
    let row = row_of(&app.workspace, FONTS_TITLE, cx);
    assert!(app.shows(("row-news", row), cx));
    let other = row_of(&app.workspace, SPLIT_TITLE, cx);
    assert!(!app.shows(("row-news", other), cx));
}

/// A deleted session has no news left and what it told is read, though the
/// history keeps the text.
#[gpui_kit::test]
fn deleting_a_session_reads_what_it_told(cx: &mut TestAppContext) {
    let app = start(&[], cx);
    turn(&app.fonts, cx, Some("Tabs or spaces?"));
    assert!(app.has_news(FONTS, cx));
    assert_eq!(cx.read(notifications::unread), 2);
    let sidebar = cx.read(|cx| app.workspace.read(cx).sidebar().clone());
    sidebar.update(cx, |_, cx| {
        cx.emit(crate::sidebar::SidebarEvent::Deleted(SessionId(
            FONTS.into(),
        )))
    });
    cx.run_until_parked();
    assert!(!app.has_news(FONTS, cx));
    assert_eq!(cx.read(notifications::unread), 0);
    assert_eq!(cx.read(|cx| notifications::entries(cx).len()), 2);
}
