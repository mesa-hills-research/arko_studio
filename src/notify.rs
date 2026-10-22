//! Notifications that reach the user away from the session: a turn that
//! ends or a question that arrives while Arko Studio is in the background
//! posts an OS notification and asks for the window's attention (each as
//! Settings › Notifications says), and a question in a session no window
//! shows comes up in the app. A sound goes with them when the user wants
//! one.

use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, WindowExt as _, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::{LiveEvent, LiveSession};
use crate::prefs;
use crate::workspace::Workspace;

/// What a notification does to bring its session forward in the window.
pub type Opens = Rc<dyn Fn(&mut Window, &mut App)>;

/// Shows `session` in the window it is called with.
pub fn opens(session: &Entity<LiveSession>) -> Opens {
    let session = session.clone();
    Rc::new(move |window, cx| open_in(&session, window, cx))
}

/// The session a card is about, as the card's first line: its title as a
/// link that brings the session forward. A screen reader says "Open session
/// Fonts".
#[derive(Clone)]
pub struct SessionLink {
    key: u64,
    title: SharedString,
    opens: Opens,
}

impl SessionLink {
    /// A link to the session with `key` and `title`, which runs `opens`.
    pub fn new(key: u64, title: impl Into<SharedString>, opens: Opens) -> Self {
        Self {
            key,
            title: title.into(),
            opens,
        }
    }

    /// What the link does.
    pub fn opens(&self) -> Opens {
        self.opens.clone()
    }

    /// The link for `session`.
    pub fn of(session: &Entity<LiveSession>, cx: &App) -> Self {
        let live = session.read(cx);
        Self::new(live.key(), live.title().clone(), opens(session))
    }

    /// The link's button, which a click or Enter activates. A card that
    /// closes on a click closes on the link's too.
    pub fn button(&self) -> Button {
        let opens = self.opens.clone();
        Button::new(("open-session", self.key))
            .link()
            .small()
            // A long title gives way to the card's width.
            .flex_shrink(1.)
            .min_w_0()
            .child(div().min_w_0().truncate().child(self.title.clone()))
            .accessibility_label(format!("Open session {}", self.title))
            .on_click(move |_, window, cx| opens(window, cx))
    }
}

/// A card's lines under the session's link: the first as it reads, the
/// others smaller and muted. A line break in `text` starts a line.
pub fn lines(text: &str, cx: &App) -> impl IntoElement {
    let muted = cx.theme().muted_foreground;
    v_flex()
        .gap_0p5()
        .children(text.lines().enumerate().map(move |(index, line)| {
            div()
                .text_sm()
                .when(index > 0, |this| this.text_xs().text_color(muted))
                .child(line.to_string())
        }))
}

/// A card about `session`: its link and `text`.
pub fn card(link: &SessionLink, text: &str, cx: &App) -> AnyElement {
    v_flex()
        .min_w_0()
        .gap_0p5()
        .child(div().flex().min_w_0().child(link.button()))
        .child(lines(text, cx))
        .into_any_element()
}

/// An in-app notification about `session` that says `text` under the
/// session's link, of the type `kind`.
pub fn session_note(
    session: &Entity<LiveSession>,
    kind: NotificationType,
    text: impl Into<String>,
    cx: &App,
) -> Notification {
    let link = SessionLink::of(session, cx);
    let text = text.into();
    Notification::new()
        .with_type(kind)
        .content(move |_, _, cx| card(&link, &text, cx))
}

/// Every window's workspace.
pub fn workspaces(cx: &App) -> Vec<(AnyWindowHandle, Entity<Workspace>)> {
    cx.windows()
        .into_iter()
        .filter_map(|handle| {
            let root = handle.downcast::<gpui_kit::base::Root>()?;
            let view = root.read(cx).ok()?.view().clone();
            Some((handle, view.downcast::<Workspace>().ok()?))
        })
        .collect()
}

/// The windows showing `session`.
pub fn windows_showing(session: &Entity<LiveSession>, cx: &App) -> Vec<AnyWindowHandle> {
    workspaces(cx)
        .into_iter()
        .filter(|(_, workspace)| {
            workspace
                .read(cx)
                .live_session()
                .is_some_and(|shown| shown.entity_id() == session.entity_id())
        })
        .map(|(window, _)| window)
        .collect()
}

/// Whether the window in front shows `session`.
pub fn shown_in_front(session: &Entity<LiveSession>, cx: &App) -> bool {
    cx.active_window()
        .is_some_and(|active| windows_showing(session, cx).contains(&active))
}

/// The window to tell the user of `session` in: the one in front while the
/// app has the focus (where the user looks), else one showing the session
/// (which a notification's click brings forward), else the first.
pub fn window_for(session: &Entity<LiveSession>, cx: &App) -> Option<AnyWindowHandle> {
    cx.active_window()
        .or_else(|| windows_showing(session, cx).first().copied())
        .or_else(|| cx.windows().first().copied())
}

/// Whether one of the app's windows has the focus.
pub fn app_focused(cx: &App) -> bool {
    cx.active_window().is_some()
}

/// Plays the system's notification sound, when the user wants sounds, off
/// the UI thread.
pub fn play_sound(cx: &App) {
    if !prefs::get(cx).notifications.sound {
        return;
    }
    cx.background_spawn(async move { sound() }).detach();
}

/// The system's sound for a message, where a player for it is found.
fn sound() {
    use std::process::{Command, Stdio};
    let quiet = |command: &mut Command| {
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    };
    if cfg!(target_os = "macos") {
        quiet(Command::new("afplay").arg("/System/Library/Sounds/Glass.aiff"));
    } else if cfg!(target_os = "windows") {
        let mut command = Command::new("powershell");
        command.args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "[System.Media.SystemSounds]::Asterisk.Play()",
        ]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // No console window flashes up.
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        quiet(&mut command);
    } else {
        // libcanberra's player knows the theme's sounds, PulseAudio's plays
        // the freedesktop one.
        let _ = quiet(Command::new("canberra-gtk-play").args(["-i", "message-new-instant"]))
            || quiet(
                Command::new("paplay")
                    .arg("/usr/share/sounds/freedesktop/stereo/message-new-instant.oga"),
            );
    }
}

/// Where a notice shows, besides the history, which keeps every one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Nowhere else: the user sees it in the window already (the sidebar
    /// marks a turn that ended, the question's card asks).
    History,
    /// A card in the app.
    InApp,
    /// The OS's notification center (the app is in the background).
    System,
}

/// What a session's event tells the user, when it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    /// The session's title.
    pub title: String,
    pub body: String,
    pub delivery: Delivery,
}

/// What `event` from the session titled `title` tells the user, when
/// Settings › Notifications has it on: `focused` is whether the app has the
/// focus, `shown` whether its active window shows the session. Questions
/// come up in the app when no window in front shows them, turns that end
/// only in the OS's notifications (the sidebar marks them). What is not
/// shown is still kept in the history.
pub fn notice(
    event: &LiveEvent,
    title: &str,
    question: Option<&str>,
    focused: bool,
    shown: bool,
    settings: &prefs::Notifications,
) -> Option<Notice> {
    match event {
        LiveEvent::TurnEnded if settings.turn_ended => Some(Notice {
            title: title.to_string(),
            body: "Arko finished its turn.".to_string(),
            delivery: if focused {
                Delivery::History
            } else {
                Delivery::System
            },
        }),
        LiveEvent::QuestionAsked if settings.question => Some(Notice {
            title: title.to_string(),
            body: match question {
                Some(text) => format!("Arko asks “{}”", first_line(text)),
                None => "Arko asks you something.".to_string(),
            },
            delivery: match (focused, shown) {
                (true, true) => Delivery::History,
                (true, false) => Delivery::InApp,
                (false, _) => Delivery::System,
            },
        }),
        _ => None,
    }
}

/// The first line of `text`, shortened for a notification.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut short: String = line.chars().take(140).collect();
    if short.len() < line.len() {
        short.push('…');
    }
    short
}

struct TurnNotice;

/// Tells the user of `session`'s turns and questions as they come, as
/// Settings › Notifications says. Called once for each session the app
/// opens.
pub fn watch(session: &Entity<LiveSession>, cx: &mut App) {
    cx.subscribe(session, |session, event: &LiveEvent, cx| {
        // The turn that kept the cache warm is no news.
        if *event == LiveEvent::TurnEnded && session.read(cx).turn_was_refresh() {
            return;
        }
        let window = window_for(&session, cx);
        let shown = shown_in_front(&session, cx);
        // A turn that ended or a question that came out of the user's sight
        // gives the session news, for the sidebar's dot, whichever
        // notifications Settings has on.
        if matches!(event, LiveEvent::TurnEnded | LiveEvent::QuestionAsked)
            && !shown
            && let Some(id) = session.read(cx).id().cloned()
        {
            crate::notifications::mark_news(&id, cx);
        }
        let (title, question) = {
            let live = session.read(cx);
            (
                live.title().to_string(),
                live.question().map(|question| question.text.clone()),
            )
        };
        let Some(notice) = notice(
            event,
            &title,
            question.as_deref(),
            app_focused(cx),
            shown,
            &prefs::get(cx).notifications,
        ) else {
            return;
        };
        // Kept whether or not it shows: the user may want it later. Read
        // already when they are looking at the session.
        let id = session.read(cx).id().cloned();
        crate::notifications::record(id.as_ref(), &notice.title, &notice.body, shown, cx);
        let (Some(window), true) = (window, notice.delivery != Delivery::History) else {
            return;
        };
        let key = session.read(cx).key();
        let link = SessionLink::of(&session, cx);
        let open = link.opens();
        window
            .update(cx, |_, window, cx| {
                let note = Notification::new()
                    .id1::<TurnNotice>(ElementId::Integer(key))
                    .on_click(move |_, window, cx| open(window, cx));
                if notice.delivery == Delivery::System {
                    // The OS shows the session's title, then what happened.
                    let note = note
                        .title(notice.title.clone())
                        .message(notice.body.clone());
                    window.push_notification(note.system(), cx);
                    window.request_attention();
                } else {
                    let body = notice.body.clone();
                    let note = note.content(move |_, _, cx| card(&link, &body, cx));
                    window.push_notification(note, cx);
                }
            })
            .ok();
        play_sound(cx);
    })
    .detach();
}

/// Shows `session` in `window` (a notification about it was clicked).
pub fn open_in(session: &Entity<LiveSession>, window: &mut Window, cx: &mut App) {
    let Some(workspace) = gpui_kit::base::Root::read(window, cx)
        .view()
        .clone()
        .downcast::<Workspace>()
        .ok()
    else {
        return;
    };
    if let Some(id) = session.read(cx).id().cloned() {
        workspace.update(cx, |workspace, cx| workspace.open_session(id, window, cx));
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{Delivery, Notice, first_line, notice};
    use crate::live::{LiveEvent, LiveSession};
    use crate::prefs::Notifications;
    use crate::tests::open;
    use arko::remote::{Activity, Question, Reading};
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
    use std::sync::Arc;
    use std::time::SystemTime;

    #[test]
    fn turns_that_end_notify_only_in_the_background() {
        let on = Notifications::default();
        let ended = |focused| notice(&LiveEvent::TurnEnded, "Fonts", None, focused, false, &on);
        assert_eq!(
            ended(true).map(|notice| notice.delivery),
            Some(Delivery::History),
            "the sidebar says it, the history keeps it"
        );
        assert_eq!(
            ended(false),
            Some(Notice {
                title: "Fonts".into(),
                body: "Arko finished its turn.".into(),
                delivery: Delivery::System,
            })
        );
        let off = Notifications {
            turn_ended: false,
            ..Notifications::default()
        };
        assert_eq!(
            notice(&LiveEvent::TurnEnded, "Fonts", None, false, false, &off),
            None
        );
    }

    #[test]
    fn questions_notify_where_the_user_doesnt_see_them() {
        let on = Notifications::default();
        let asked = |focused, shown| {
            notice(
                &LiveEvent::QuestionAsked,
                "Fonts",
                Some("Tabs or spaces?\nThe file mixes both."),
                focused,
                shown,
                &on,
            )
        };
        assert_eq!(
            asked(true, true).map(|notice| notice.delivery),
            Some(Delivery::History),
            "the question card shows it"
        );
        let in_app = asked(true, false).unwrap();
        assert_eq!(in_app.delivery, Delivery::InApp);
        assert_eq!(in_app.body, "Arko asks “Tabs or spaces?”");
        assert_eq!(asked(false, true).unwrap().delivery, Delivery::System);
        let off = Notifications {
            question: false,
            ..Notifications::default()
        };
        assert_eq!(
            notice(&LiveEvent::QuestionAsked, "Fonts", None, false, false, &off),
            None
        );
    }

    fn start(cx: &mut TestAppContext) {
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(crate::Startup::for_tests(backend), cx));
    }

    /// Brings `window` to the front: the app has the focus.
    fn activate(window: gpui_kit::AnyWindowHandle, cx: &mut TestAppContext) {
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        cx.run_until_parked();
        assert_eq!(cx.update(|cx| cx.active_window()), Some(window));
    }

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

    /// A turn that ends while the app is in the background posts an OS
    /// notification. In front, the sidebar's mark says it.
    #[gpui_kit::test]
    fn a_turn_that_ends_in_the_background_posts_an_os_notification(cx: &mut TestAppContext) {
        start(cx);
        let (window, _) = open(cx, 1300.);
        activate(window, cx);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        session.update(cx, |session, cx| {
            let reading = Reading {
                title: "Fonts".into(),
                ..Reading::default()
            };
            session.set_status(reading, Activity::default(), cx)
        });
        cx.update(|cx| super::watch(&session, cx));
        turn(&session, cx, None);
        assert!(cx.shown_system_notifications().is_empty());
        VisualTestContext::from_window(window, cx).deactivate_window();
        turn(&session, cx, None);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        // The OS shows the session's title, then what happened.
        assert_eq!(shown[0].title.as_ref(), "Fonts");
        assert_eq!(shown[0].body.as_ref(), "Arko finished its turn.");
    }

    /// Clicking the OS notification of a session shows that session in the
    /// window that posted it.
    #[gpui_kit::test]
    fn clicking_the_os_notification_shows_its_session(cx: &mut TestAppContext) {
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(crate::Startup::for_tests(backend.clone()), cx));
        let (window, workspace) = open(cx, 1300.);
        activate(window, cx);
        let entry = crate::backend::Backend::sessions(backend.as_ref()).unwrap()[0].clone();
        let session = cx.update(|cx| LiveSession::resume(&entry, cx));
        VisualTestContext::from_window(window, cx).deactivate_window();
        turn(&session, cx, None);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title.as_ref(), entry.title);
        assert_eq!(crate::tests::open_title(&workspace, cx), None);
        cx.simulate_system_notification_response(gpui_kit::SystemNotificationResponse {
            tag: shown[0].tag.clone(),
            action_id: None,
        });
        cx.run_until_parked();
        assert_eq!(
            crate::tests::open_title(&workspace, cx),
            Some(entry.title.clone())
        );
    }

    /// A question in a session the window in front doesn't show comes up as
    /// a card with the session's title as a link that shows the session.
    #[gpui_kit::test]
    fn a_card_names_its_session_in_a_link_that_shows_it(cx: &mut TestAppContext) {
        use gpui_kit::test::TestWindowExt as _;

        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(crate::Startup::for_tests(backend.clone()), cx));
        let (window, workspace) = open(cx, 1300.);
        activate(window, cx);
        let entry = crate::backend::Backend::sessions(backend.as_ref()).unwrap()[1].clone();
        let session = cx.update(|cx| LiveSession::resume(&entry, cx));
        turn(&session, cx, Some("Tabs or spaces?"));
        let key = cx.read(|cx| session.read(cx).key());
        let label = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                window.find(("open-session", key)).label().map(String::from)
            })
            .unwrap();
        assert_eq!(label, Some(format!("Open session {}", entry.title)));
        assert_eq!(crate::tests::open_title(&workspace, cx), None);
        let cards = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| window.notifications(cx).len())
                .unwrap()
        };
        let before = cards(cx);
        crate::tests::act(cx, window, |window, cx| {
            window.click(("open-session", key), cx)
        });
        assert_eq!(
            crate::tests::open_title(&workspace, cx),
            Some(entry.title.clone())
        );
        // The card has done its work.
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(cards(cx), before - 1);
    }

    /// A question in a session the window in front doesn't show comes up
    /// in the app.
    /// What isn't shown is still kept: a turn that ends with the app in
    /// front, where only the sidebar marks it, is unread in the history
    /// until its session is opened. In sight, it is read at once, and what
    /// Settings has off isn't kept.
    #[gpui_kit::test]
    fn what_is_not_shown_is_still_kept_in_the_history(cx: &mut TestAppContext) {
        use crate::notifications::{entries, unread};

        start(cx);
        let (window, workspace) = open(cx, 1300.);
        activate(window, cx);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        session.update(cx, |session, cx| {
            let reading = Reading {
                title: "Fonts".into(),
                ..Reading::default()
            };
            session.set_status(reading, Activity::default(), cx)
        });
        cx.update(|cx| super::watch(&session, cx));
        turn(&session, cx, None);
        assert!(cx.shown_system_notifications().is_empty());
        cx.read(|cx| {
            let kept = entries(cx);
            assert_eq!(kept.len(), 1);
            assert_eq!(kept[0].title, "Fonts");
            assert_eq!(kept[0].message, "Arko finished its turn.");
            assert_eq!(kept[0].session.as_deref(), Some("s1"));
            assert!(!kept[0].read);
            assert_eq!(unread(cx), 1);
        });
        // Showing the session reads what was said of it.
        cx.update_window(window, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_session(session.clone(), window, cx)
            })
        })
        .unwrap();
        assert_eq!(cx.read(unread), 0);
        // In sight, it is read as it comes.
        turn(&session, cx, Some("Tabs or spaces?"));
        cx.read(|cx| {
            assert_eq!(entries(cx).len(), 3);
            assert_eq!(unread(cx), 0);
        });
        // Off in Settings, nothing is kept.
        cx.update(|cx| {
            crate::prefs::update(cx, |prefs| {
                prefs.notifications.turn_ended = false;
                prefs.notifications.question = false;
            })
        });
        turn(&session, cx, Some("Tabs or spaces?"));
        assert_eq!(cx.read(|cx| entries(cx).len()), 3);
    }

    /// Opening a session from its card's link reads what was said of it.
    #[gpui_kit::test]
    fn opening_the_session_reads_what_was_said_of_it(cx: &mut TestAppContext) {
        use crate::notifications::{entries, unread};
        use gpui_kit::test::TestWindowExt as _;

        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(crate::Startup::for_tests(backend.clone()), cx));
        let (window, _) = open(cx, 1300.);
        activate(window, cx);
        let entry = crate::backend::Backend::sessions(backend.as_ref()).unwrap()[0].clone();
        let session = cx.update(|cx| LiveSession::resume(&entry, cx));
        turn(&session, cx, Some("Tabs or spaces?"));
        cx.read(|cx| {
            assert_eq!(entries(cx)[0].message, "Arko asks “Tabs or spaces?”");
            assert_eq!(entries(cx)[0].session.as_deref(), Some(entry.id.as_str()));
            // The question, and the turn that ended with it.
            assert_eq!(unread(cx), 2);
        });
        let key = cx.read(|cx| session.read(cx).key());
        crate::tests::act(cx, window, |window, cx| {
            window.click(("open-session", key), cx)
        });
        assert_eq!(cx.read(unread), 0);
    }

    #[gpui_kit::test]
    fn a_question_elsewhere_comes_up_in_the_app(cx: &mut TestAppContext) {
        start(cx);
        let (window, _) = open(cx, 1300.);
        activate(window, cx);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        cx.update(|cx| super::watch(&session, cx));
        let before = cx
            .update_window(window, |_, window, cx| window.notifications(cx).len())
            .unwrap();
        turn(&session, cx, Some("Tabs or spaces?"));
        let after = cx
            .update_window(window, |_, window, cx| window.notifications(cx).len())
            .unwrap();
        assert_eq!(after, before + 1);
        assert!(cx.shown_system_notifications().is_empty());
    }

    /// With the session on screen in a window behind the one in front, its
    /// cache reminder comes up in front, where the user looks.
    #[gpui_kit::test]
    fn a_reminder_comes_up_in_the_window_in_front(cx: &mut TestAppContext) {
        use crate::cache_reminders::ReminderClock;
        use arko::remote::CacheStatus;
        use std::time::Duration;

        const NOW: i64 = 1_800_000_000_000;
        start(cx);
        let (front, _) = open(cx, 1300.);
        let (behind, workspace) = open(cx, 1300.);
        cx.update(|cx| cx.set_global(ReminderClock(std::rc::Rc::new(|| NOW))));
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        cx.update_window(behind, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_session(session.clone(), window, cx)
            })
        })
        .unwrap();
        // 50 s of a 5-minute cache left: the last reminder is due.
        let reading = Reading {
            cache: CacheStatus {
                lifetime: Some(Duration::from_secs(300)),
                refreshed_at: Some(NOW - 250_000),
                ..CacheStatus::default()
            },
            ..Reading::default()
        };
        session.update(cx, |session, cx| {
            session.set_touched_at(NOW);
            session.set_status(reading, Activity::default(), cx)
        });
        activate(front, cx);
        let count = |window, cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| window.notifications(cx).len())
                .unwrap()
        };
        let before = (count(front, cx), count(behind, cx));
        cx.update(|cx| crate::cache_reminders::watch(&session, cx));
        cx.run_until_parked();
        assert_eq!(
            (count(front, cx), count(behind, cx)),
            (before.0 + 1, before.1)
        );
        assert!(cx.shown_system_notifications().is_empty());
    }

    #[test]
    fn long_first_lines_are_shortened() {
        assert_eq!(
            first_line("\n  Count the pears.  \nThen"),
            "Count the pears."
        );
        let long = "a".repeat(300);
        assert_eq!(first_line(&long).chars().count(), 141);
    }
}
