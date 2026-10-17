//! Notifications that reach the user away from the session: a turn that
//! ends or a question that arrives while Arko Studio is in the background
//! posts an OS notification and asks for the window's attention (each as
//! Settings › Notifications says), and a question in a session no window
//! shows comes up in the app. A sound goes with them when the user wants
//! one.

use gpui_kit::component::WindowExt as _;
use gpui_kit::component::notification::Notification;
use gpui_kit::*;

use crate::live::{LiveEvent, LiveSession};
use crate::prefs;
use crate::workspace::Workspace;

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

/// The window to tell the user of `session` in: the active one showing
/// it, else any showing it, else the active one, else the first.
pub fn window_for(session: &Entity<LiveSession>, cx: &App) -> Option<AnyWindowHandle> {
    let showing = windows_showing(session, cx);
    let active = cx.active_window();
    showing
        .iter()
        .find(|window| Some(**window) == active)
        .or(showing.first())
        .copied()
        .or(active)
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

/// What a session's event tells the user, when it does.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub title: String,
    pub body: String,
    /// It goes to the OS's notification center (the app is in the
    /// background), else it shows in the app.
    pub system: bool,
}

/// What `event` from the session titled `title` tells the user: `focused`
/// is whether the app has the focus, `shown` whether its active window
/// shows the session. Questions come up in the app when no window in front
/// shows them, turns that end only in the OS's notifications (the sidebar
/// marks them).
pub fn notice(
    event: &LiveEvent,
    title: &str,
    question: Option<&str>,
    focused: bool,
    shown: bool,
    settings: &prefs::Notifications,
) -> Option<Notice> {
    match event {
        LiveEvent::TurnEnded if settings.turn_ended && !focused => Some(Notice {
            title: title.to_string(),
            body: "Arko finished its turn.".to_string(),
            system: true,
        }),
        LiveEvent::QuestionAsked if settings.question && !(focused && shown) => Some(Notice {
            title: title.to_string(),
            body: match question {
                Some(text) => format!("Arko asks: {}", first_line(text)),
                None => "Arko asks you something.".to_string(),
            },
            system: !focused,
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
        let window = window_for(&session, cx);
        let shown = cx
            .active_window()
            .is_some_and(|active| windows_showing(&session, cx).contains(&active));
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
        let Some(window) = window else {
            return;
        };
        let key = session.read(cx).key();
        let opens = session.clone();
        window
            .update(cx, |_, window, cx| {
                let note = Notification::new()
                    .id1::<TurnNotice>(ElementId::Integer(key))
                    .title(notice.title.clone())
                    .message(notice.body.clone())
                    .on_click(move |_, window, cx| open_in(&opens, window, cx));
                if notice.system {
                    window.push_notification(note.system(), cx);
                    window.request_attention();
                } else {
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
    use super::{Notice, first_line, notice};
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
        assert_eq!(ended(true), None, "the sidebar says it");
        assert_eq!(
            ended(false),
            Some(Notice {
                title: "Fonts".into(),
                body: "Arko finished its turn.".into(),
                system: true,
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
        assert_eq!(asked(true, true), None, "the question card shows it");
        let in_app = asked(true, false).unwrap();
        assert!(!in_app.system);
        assert_eq!(in_app.body, "Arko asks: Tabs or spaces?");
        assert!(asked(false, true).unwrap().system);
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
        cx.update(|cx| super::watch(&session, cx));
        turn(&session, cx, None);
        assert!(cx.shown_system_notifications().is_empty());
        VisualTestContext::from_window(window, cx).deactivate_window();
        turn(&session, cx, None);
        let shown = cx.shown_system_notifications();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].body.as_ref(), "Arko finished its turn.");
    }

    /// A question in a session the window in front doesn't show comes up
    /// in the app.
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
