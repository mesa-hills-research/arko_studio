//! End-to-end tests: the app's windows on arko's real engine, its sessions
//! answered by arko's mock model server. The engine runs on threads of its
//! own, in real time, and `wait_until` lets the app follow it.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::backend::{ArkoBackend, SessionId};
use crate::live::{LiveSession, LiveSessions, Phase};
use crate::sidebar::Row;
use crate::testing::{self, Answer, Mock, reply, reply_after, wait_until};
use crate::tests::{act, open, open_title};
use crate::transcript::TranscriptView;
use crate::workspace::Workspace;
use crate::{Startup, menus};

/// An app on a real Arko folder in a temporary folder, its sessions on
/// `mock`, a new agent session set to work in `work`.
struct App {
    _temp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
}

fn start(cx: &mut TestAppContext, mock: &Mock) -> App {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    start_in(cx, mock, temp, root, work)
}

fn start_in(
    cx: &mut TestAppContext,
    mock: &Mock,
    temp: tempfile::TempDir,
    root: PathBuf,
    work: PathBuf,
) -> App {
    // The engine's threads wake the app's tasks: the test scheduler lets
    // them, as for real I/O.
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    App {
        _temp: temp,
        root,
        work,
        window,
        workspace,
    }
}

impl App {
    fn shown(&self, cx: &mut TestAppContext) -> Option<Entity<LiveSession>> {
        cx.read(|cx| self.workspace.read(cx).live_session().cloned())
    }

    /// Types `text` in the composer and sends it with Enter.
    fn send(&self, cx: &mut TestAppContext, text: &str) {
        act(cx, self.window, |window, cx| window.input(text, cx));
        act(cx, self.window, |window, cx| window.press("enter", cx));
    }

    /// The texts of the shown session's feed.
    fn feed(&self, cx: &mut TestAppContext) -> Vec<String> {
        self.feed_of(self.shown(cx).as_ref(), cx)
    }

    fn feed_of(
        &self,
        session: Option<&Entity<LiveSession>>,
        cx: &mut TestAppContext,
    ) -> Vec<String> {
        cx.read(|cx| {
            session
                .and_then(|session| session.read(cx).feed().cloned())
                .map(|feed| {
                    feed.subscribe()
                        .0
                        .items
                        .into_iter()
                        .map(|item| item.text)
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn wait_for_text(&self, cx: &mut TestAppContext, text: &str) {
        wait_until(cx, text, |cx| {
            self.feed(cx).iter().any(|item| item.contains(text))
        });
    }

    /// The shown transcript, once the window has drawn it.
    fn transcript(&self, cx: &mut TestAppContext) -> Option<Entity<TranscriptView>> {
        cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            self.workspace
                .read(cx)
                .session_view()
                .read(cx)
                .transcript(cx)
                .cloned()
        })
    }

    /// Waits for the shown transcript to have at least `rows` rows.
    fn wait_for_rows(&self, cx: &mut TestAppContext, rows: usize) {
        wait_until(cx, "the transcript's rows", |cx| {
            self.transcript(cx)
                .is_some_and(|view| cx.read(|cx| view.read(cx).row_count()) >= rows)
        });
    }

    fn composer_text(&self, cx: &mut TestAppContext) -> String {
        cx.read(|cx| {
            self.workspace
                .read(cx)
                .session_view()
                .read(cx)
                .composer()
                .read(cx)
                .text(cx)
        })
    }

    /// Closes every session, as quitting does, and waits for them to end.
    fn close_all(&self, cx: &mut TestAppContext) {
        act(cx, self.window, |window, cx| {
            let sessions = LiveSessions::all(cx);
            crate::quit::close(sessions, false, window, cx);
        });
        wait_until(cx, "the sessions to end", |cx| {
            cx.read(|cx| LiveSessions::all(cx).is_empty())
        });
    }
}

/// The mock model's turn: a reply, or after `delay`.
fn answer(text: &'static str, delay: Duration) -> Answer {
    if delay.is_zero() {
        reply("r1", text)
    } else {
        reply_after("r1", text, delay)
    }
}

#[gpui_kit::test]
fn a_first_message_starts_a_session_and_its_reply_arrives(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("Count the pears.") {
            answer("Seven pears.", Duration::ZERO)
        } else {
            answer("Hm.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Count the pears.");
    // At once: the session shows, named by its first message, and the
    // composer is empty again.
    assert_eq!(
        open_title(&app.workspace, cx).as_deref(),
        Some("Count the pears.")
    );
    assert_eq!(app.composer_text(cx), "");
    app.wait_for_text(cx, "Seven pears.");
    assert!(mock.was_sent("Count the pears."));
    // The transcript shows the message and its reply.
    app.wait_for_rows(cx, 2);
    // The turn's end comes after its reply, and draws the transcript again.
    wait_until(cx, "the turn to end", |cx| {
        app.shown(cx)
            .is_some_and(|session| cx.read(|cx| !session.read(cx).is_working()))
    });
    // Typing in the composer draws the composer, not the transcript.
    let transcript = app.transcript(cx).unwrap();
    let draw = |cx: &mut TestAppContext| {
        cx.run_until_parked();
        cx.update_window(app.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    draw(cx);
    draw(cx);
    let drawn = cx.read(|cx| transcript.read(cx).render_count());
    for key in ["a", "b", "c", "backspace"] {
        // As the platform delivers it (the test's `press` draws the whole
        // window again, around the key).
        cx.update_window(app.window, |_, window, cx| {
            window.dispatch_keystroke(gpui_kit::Keystroke::parse(key).unwrap(), cx)
        })
        .unwrap();
        draw(cx);
    }
    assert_eq!(app.composer_text(cx), "ab");
    assert_eq!(cx.read(|cx| transcript.read(cx).render_count()), drawn);
    // It's in Arko's folder, listed in the sidebar and kept in the history.
    wait_until(cx, "the sidebar to list it", |cx| {
        cx.read(|cx| {
            app.workspace
                .read(cx)
                .sidebar()
                .read(cx)
                .rows()
                .iter()
                .any(|row| matches!(row, Row::Session { title, .. } if title == "Count the pears."))
        })
    });
    let sessions = arko::sessions::list(&app.root);
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].workspace.as_deref(), Some(app.work.as_path()));
    assert_eq!(
        cx.read(|cx| crate::history::InputHistory::entries(cx).to_vec()),
        ["Count the pears."]
    );
    app.close_all(cx);
    assert!(
        arko::sessions::list(&app.root)[0].holder.is_none(),
        "unlocked"
    );
}

#[gpui_kit::test]
fn escape_stops_a_turn_and_send_comes_back(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Too late.", Duration::from_secs(20)));
    let app = start(cx, &mock);
    app.send(cx, "Think for a long time.");
    wait_until(cx, "the turn to start", |cx| {
        app.shown(cx)
            .is_some_and(|session| cx.read(|cx| session.read(cx).is_working()))
    });
    act(cx, app.window, |window, _| {
        assert!(window.try_find("stop").is_some(), "Stop instead of Send");
        assert!(window.try_find("send").is_none());
    });
    // What is written now joins the task.
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    assert_eq!(
        cx.read(|cx| view.read(cx).placeholder(cx)),
        "Add to the task while Arko works"
    );
    act(cx, app.window, |window, cx| window.press("escape", cx));
    wait_until(cx, "the turn to stop", |cx| {
        app.shown(cx)
            .is_some_and(|session| cx.read(|cx| !session.read(cx).is_working()))
    });
    act(cx, app.window, |window, _| {
        assert!(window.try_find("send").is_some());
    });
    assert!(!app.feed(cx).iter().any(|item| item.contains("Too late.")));
    app.close_all(cx);
}

#[gpui_kit::test]
fn the_agents_question_is_answered_from_the_composer(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("Two spaces.") {
            answer("Indented with two spaces.", Duration::ZERO)
        } else {
            let call = arko::testing::tool_call(
                "q1",
                "ask_user",
                serde_json::json!({ "questions": "Tabs or spaces?" }),
            );
            Box::pin(async move { call })
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Indent the file.");
    wait_until(cx, "the question card", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("question-card").is_some()
        })
        .unwrap()
    });
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    assert_eq!(
        cx.read(|cx| view.read(cx).placeholder(cx)),
        "Answer the question above"
    );
    // Later folds it into the composer's footer, and the composer still answers.
    act(cx, app.window, |window, cx| {
        window.click("question-later", cx)
    });
    act(cx, app.window, |window, _| {
        assert!(window.try_find("question-card").is_none());
        assert!(window.try_find("question-chip").is_some());
    });
    app.send(cx, "Two spaces.");
    app.wait_for_text(cx, "Indented with two spaces.");
    // The window follows the session a frame later.
    wait_until(cx, "the question to close", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("question-chip").is_none()
        })
        .unwrap()
    });
    app.close_all(cx);
}

/// The question's words can be selected and copied with the keyboard. Esc
/// then gives the keyboard back to the composer, the question still asked,
/// and the answer goes as usual.
#[gpui_kit::test]
fn the_agents_question_can_be_selected_and_copied(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("Two spaces.") {
            answer("Indented with two spaces.", Duration::ZERO)
        } else {
            let call = arko::testing::tool_call(
                "q1",
                "ask_user",
                serde_json::json!({ "questions": "Tabs or spaces?" }),
            );
            Box::pin(async move { call })
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Indent the file.");
    wait_until(cx, "the question card", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("question-text").is_some()
        })
        .unwrap()
    });
    act(cx, app.window, |window, cx| {
        let text = window.find("question-text").bounds();
        let middle = text.size.height / 2.;
        window.drag(
            text.origin + gpui_kit::point(gpui_kit::px(1.), middle),
            text.origin + gpui_kit::point(text.size.width - gpui_kit::px(1.), middle),
            cx,
        );
        window.press("secondary-c", cx);
    });
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap_or_default();
    assert_eq!(copied, "Tabs or spaces?");
    act(cx, app.window, |window, cx| window.press("escape", cx));
    use gpui_kit::Focusable as _;
    let composer = cx.read(|cx| {
        let view = app.workspace.read(cx).session_view().clone();
        view.read(cx).composer().read(cx).textarea().clone()
    });
    act(cx, app.window, |window, cx| {
        assert!(composer.read(cx).focus_handle(cx).is_focused(window));
        assert!(window.try_find("question-card").is_some(), "still asked");
    });
    app.send(cx, "Two spaces.");
    app.wait_for_text(cx, "Indented with two spaces.");
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_long_question_scrolls_inside_its_card(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| {
        let plan: Vec<String> = (1..=60).map(|step| format!("{step}. A step.")).collect();
        let call = arko::testing::tool_call(
            "q1",
            "ask_user",
            serde_json::json!({ "questions": format!("Shall I go ahead?\n\n{}", plan.join("\n")) }),
        );
        Box::pin(async move { call })
    });
    let app = start(cx, &mock);
    app.send(cx, "Plan the work.");
    wait_until(cx, "the question card", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("question-text").is_some()
        })
        .unwrap()
    });
    act(cx, app.window, |window, _| {
        let tallest = window.rem_size() * crate::session_view::QUESTION_MAX_HEIGHT;
        let height = window.find("question-text").bounds().size.height;
        assert!(height <= tallest + gpui_kit::px(0.5), "{height:?}");
        // The composer is still on screen below it.
        let composer = window.find("composer-field").bounds();
        assert!(composer.bottom() <= window.viewport_size().height);
    });
    app.close_all(cx);
}

/// The agent's request to save a worker profile shows as it is written,
/// not as Markdown that would run its lines together, and long instructions
/// scroll inside the card without pushing the composer off the window.
#[gpui_kit::test]
fn a_request_to_save_a_profile_shows_its_instructions_and_scrolls(cx: &mut TestAppContext) {
    let mut profile = arko::profiles::Profile::new("3d-judge", "Judges 3D results");
    profile.model = Some("sol".into());
    profile.instructions = (1..=60)
        .map(|line| format!("Check that the render matches the reference, point {line}."))
        .collect::<Vec<_>>()
        .join("\n");
    let question = arko::profiles::approval_text(&profile, arko::profiles::Place::User);
    assert!(question.lines().count() > 60, "the whole profile is in it");
    let mock = Mock::start(move |_, _| {
        let call = arko::testing::tool_call(
            "q1",
            "ask_user",
            serde_json::json!({ "questions": question.clone() }),
        );
        Box::pin(async move { call })
    });
    let app = start(cx, &mock);
    app.send(cx, "Save a judge profile.");
    wait_until(cx, "the question card", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("question-text").is_some()
        })
        .unwrap()
    });
    act(cx, app.window, |window, _| {
        assert!(window.try_find("question-plain").is_some());
        let tallest = window.rem_size() * crate::session_view::QUESTION_MAX_HEIGHT;
        let height = window.find("question-text").bounds().size.height;
        assert!(height <= tallest + gpui_kit::px(0.5), "{height:?}");
        let composer = window.find("composer-field").bounds();
        assert!(composer.bottom() <= window.viewport_size().height);
        // The plain text is the long one: far taller than the card.
        let text = window.find("question-plain").bounds().size.height;
        assert!(text > tallest * 4., "{text:?} in {tallest:?}");
    });
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_command_typed_while_a_question_waits_runs_and_the_question_stays(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("Two spaces.") {
            answer("Indented with two spaces.", Duration::ZERO)
        } else {
            let call = arko::testing::tool_call(
                "q1",
                "ask_user",
                serde_json::json!({ "questions": "Tabs or spaces?" }),
            );
            Box::pin(async move { call })
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Indent the file.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the question", |cx| {
        cx.read(|cx| session.read(cx).question().is_some())
    });
    app.send(cx, "/cost");
    wait_until(cx, "the cost", |cx| {
        app.feed(cx).iter().any(|item| item.contains('$'))
    });
    assert!(
        cx.read(|cx| session.read(cx).question().is_some()),
        "still asked"
    );
    assert!(!mock.was_sent("/cost"), "not an answer");
    // Esc folds the question card, and the turn goes on.
    act(cx, app.window, |window, cx| window.press("escape", cx));
    act(cx, app.window, |window, _| {
        assert!(window.try_find("question-card").is_none());
        assert!(window.try_find("question-chip").is_some());
    });
    assert!(cx.read(|cx| session.read(cx).is_working()));
    app.send(cx, "Two spaces.");
    app.wait_for_text(cx, "Indented with two spaces.");
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_slash_command_is_completed_from_the_menu_and_runs(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Hello.", Duration::ZERO));
    let app = start(cx, &mock);
    app.send(cx, "Say hello.");
    app.wait_for_text(cx, "Hello.");
    act(cx, app.window, |window, cx| window.input("/co", cx));
    act(cx, app.window, |window, _| {
        assert!(window.try_find("slash-menu").is_some());
        assert!(window.try_find(("slash-item", 0usize)).is_some());
    });
    act(cx, app.window, |window, cx| window.press("tab", cx));
    assert_eq!(app.composer_text(cx), "/cost");
    // The command is complete: Enter sends it, and the session answers.
    act(cx, app.window, |window, cx| window.press("enter", cx));
    assert_eq!(app.composer_text(cx), "");
    app.wait_for_text(cx, "/cost");
    wait_until(cx, "the cost", |cx| {
        app.feed(cx)
            .iter()
            .any(|item| item.contains('$') || item.to_lowercase().contains("cost"))
    });
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_session_streams_on_while_the_window_shows_another(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("The slow one.") {
            answer("Slow answer.", Duration::from_secs(6))
        } else {
            answer("Quick answer.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "The slow one.");
    let slow = app.shown(cx).unwrap();
    wait_until(cx, "the slow turn to start", |cx| {
        cx.read(|cx| slow.read(cx).is_working() && slow.read(cx).id().is_some())
    });
    let slow_id: SessionId = cx.read(|cx| slow.read(cx).id().cloned().unwrap());

    // A second session in the same window: the first keeps working.
    act(cx, app.window, |window, cx| window.press("secondary-n", cx));
    assert_eq!(open_title(&app.workspace, cx), None);
    app.send(cx, "The quick one.");
    app.wait_for_text(cx, "Quick answer.");
    let indicator = |cx: &mut TestAppContext| cx.read(|cx| LiveSessions::indicator(&slow_id, cx));
    wait_until(cx, "the sidebar's spinner", |cx| {
        indicator(cx).working
            && cx
                .update_window(app.window, |_, window, cx| {
                    window.render_frame(cx);
                    (0..20usize).any(|row| window.try_find(("row-working", row)).is_some())
                })
                .unwrap()
    });
    // It ends out of sight: the sidebar's dot says so until the window shows it.
    wait_until(cx, "the slow reply", |cx| {
        app.feed_of(Some(&slow), cx)
            .iter()
            .any(|item| item.contains("Slow answer."))
    });
    let has_news =
        |cx: &mut TestAppContext| cx.read(|cx| crate::notifications::has_news(&slow_id, cx));
    wait_until(cx, "the sidebar's dot", |cx| has_news(cx));
    act(cx, app.window, |window, cx| {
        let workspace = app.workspace.clone();
        workspace.update(cx, |workspace, cx| {
            workspace.open_session(slow_id.clone(), window, cx)
        });
    });
    assert!(!has_news(cx));
    assert_eq!(cx.read(|cx| LiveSessions::all(cx).len()), 2);
    // Its transcript was kept, with the answer that came meanwhile.
    app.wait_for_rows(cx, 2);
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_projects_mcp_servers_are_approved_in_a_dialog(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Hello.", Duration::ZERO));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        work.join(".mcp.json"),
        r#"{ "mcpServers": { "echo": { "command": "this-program-isnt-there" } } }"#,
    )
    .unwrap();
    let app = start_in(cx, &mock, temp, root, work);
    app.send(cx, "Say hello.");
    wait_until(cx, "the approval dialog", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("mcp-approval").is_some()
        })
        .unwrap()
    });
    act(cx, app.window, |window, cx| window.click("mcp-refuse", cx));
    // The session opens without them, and arko remembers the answer.
    app.wait_for_text(cx, "Hello.");
    let approvals = std::fs::read_to_string(app.root.join("mcp-approvals.json")).unwrap();
    assert!(approvals.contains("echo"), "{approvals}");
    app.close_all(cx);
}

#[gpui_kit::test]
fn a_session_closed_before_it_opened_never_sends_its_first_message(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Hello.", Duration::ZERO));
    let app = start(cx, &mock);
    std::fs::write(
        app.work.join(".mcp.json"),
        r#"{ "mcpServers": { "echo": { "command": "this-program-isnt-there" } } }"#,
    )
    .unwrap();
    app.send(cx, "Say hello.");
    // It waits on the approval, and is closed meanwhile.
    wait_until(cx, "the approval dialog", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("mcp-approval").is_some()
        })
        .unwrap()
    });
    let session = app.shown(cx).unwrap();
    act(cx, app.window, |window, cx| {
        crate::quit::close(LiveSessions::all(cx), false, window, cx);
    });
    // The question is answered for the user: it ends without waiting.
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| !session.read(cx).is_running())
    });
    assert!(cx.read(|cx| LiveSessions::all(cx).is_empty()));
    assert!(
        mock.bodies.lock().unwrap().is_empty(),
        "nothing went to the model"
    );
    // The message it didn't send is back in the composer.
    wait_until(cx, "the message back", |cx| {
        app.composer_text(cx) == "Say hello."
    });
}

#[gpui_kit::test]
fn a_session_open_elsewhere_says_who_has_it(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Done.", Duration::ZERO));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    let id = testing::create_session(&root, &work, "Count the pears.");
    // Another Arko (here, a session spawned beside the app) holds it.
    let mut options = arko::Options {
        root: Some(root.clone()),
        session: arko::SessionChoice::Resume(id.0.clone()),
        app: "arko",
        app_name: "Arko in a terminal",
        ..arko::Options::default()
    };
    (mock.configure())(&mut options);
    let other = arko::spawn(options, Arc::new(arko::NoPrompter));
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let seam = runtime
        .block_on(other.opened)
        .unwrap()
        .unwrap_or_else(|error| panic!("{error}"));
    let app = start_in(cx, &mock, temp, root, work);
    // The window says so, and shows what it showed before.
    act(cx, app.window, |window, cx| {
        let workspace = app.workspace.clone();
        workspace.update(cx, |workspace, cx| {
            workspace.open_session(id.clone(), window, cx)
        });
    });
    wait_until(cx, "the refusal", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            !window.notifications(cx).is_empty()
        })
        .unwrap()
    });
    assert_eq!(open_title(&app.workspace, cx), None);
    // In words: which app, which process.
    let entry = arko::sessions::list(&app.root)
        .into_iter()
        .map(|entry| crate::backend::SessionEntry {
            id: SessionId(entry.id),
            title: entry.title,
            mode: crate::backend::Mode::Agent,
            workspace: entry.workspace,
            model: entry.model,
            created: entry.created.into(),
            last_active: None,
            conversations: Vec::new(),
            holder: None,
            had_workers: false,
        })
        .next()
        .unwrap();
    let session = cx.update(|cx| LiveSession::resume(&entry, cx));
    wait_until(cx, "the session to be refused", |cx| {
        cx.read(|cx| matches!(session.read(cx).phase(), Phase::Failed(_)))
    });
    let Phase::Failed(why) = cx.read(|cx| session.read(cx).phase().clone()) else {
        unreachable!()
    };
    assert!(
        why.starts_with(&format!(
            "It’s open in Arko in a terminal (process {})",
            std::process::id()
        )),
        "{why}"
    );
    seam.input
        .send(arko::Input::Quit(Some(Vec::new())))
        .unwrap();
    runtime.block_on(other.ended).unwrap();
}

#[gpui_kit::test]
fn closing_a_session_with_a_background_job_asks_first(cx: &mut TestAppContext) {
    let mock = Mock::start(|index, _| {
        if index == 0 {
            let call = arko::testing::tool_call(
                "j1",
                "terminal_start",
                serde_json::json!({ "command": "sleep 60", "terminal": "false" }),
            );
            Box::pin(async move { call })
        } else {
            answer("Started it.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Start the long job.");
    app.wait_for_text(cx, "Started it.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the job in the status", |cx| {
        cx.read(|cx| !session.read(cx).reading().jobs.is_empty())
    });
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(menus::CloseSession), cx)
    });
    act(cx, app.window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("quit-work").is_some());
        // Cancel: it runs on.
        window.within("dialog").click("cancel", cx);
    });
    assert!(cx.read(|cx| session.read(cx).is_running()));
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(menus::CloseSession), cx)
    });
    // "Don't ask again" counts once the user agrees.
    act(cx, app.window, |window, cx| window.click("dont-ask", cx));
    assert!(cx.read(|cx| crate::prefs::get(cx).sessions.ask_before_quit));
    act(cx, app.window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    assert!(!cx.read(|cx| crate::prefs::get(cx).sessions.ask_before_quit));
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| *session.read(cx).phase() == Phase::Ended)
    });
    assert_eq!(
        open_title(&app.workspace, cx),
        None,
        "back to a new session"
    );
    // The job stopped with it.
    assert!(cx.read(|cx| LiveSessions::all(cx).is_empty()));
}

#[gpui_kit::test]
fn a_turn_that_ends_after_its_window_closed_gives_the_session_news(cx: &mut TestAppContext) {
    // Slow enough for the window to close first on a loaded machine.
    let mock = Mock::start(|_, _| answer("Slow answer.", Duration::from_secs(6)));
    let app = start(cx, &mock);
    let (second, _) = open(cx, 1300.);
    app.send(cx, "The slow one.");
    let slow = app.shown(cx).unwrap();
    wait_until(cx, "the slow turn", |cx| {
        cx.read(|cx| slow.read(cx).is_working() && slow.read(cx).id().is_some())
    });
    let id = cx.read(|cx| slow.read(cx).id().cloned().unwrap());
    // Its window closes, and another stays open.
    cx.update_window(app.window, |_, window, cx| {
        assert!(crate::chrome::can_close(window, cx));
        window.remove_window();
    })
    .unwrap();
    cx.run_until_parked();
    wait_until(cx, "the reply", |cx| {
        cx.read(|cx| !slow.read(cx).is_working())
    });
    assert!(cx.read(|cx| crate::notifications::has_news(&id, cx)));
    act(cx, second, |window, cx| {
        crate::quit::close(LiveSessions::all(cx), false, window, cx);
    });
    wait_until(cx, "the sessions to end", |cx| {
        cx.read(|cx| LiveSessions::all(cx).is_empty())
    });
}

/// A long command moves to the background from its row, and the agent
/// goes on. The row then follows the command's job, and Stop on it stops
/// the job.
#[cfg(unix)]
#[gpui_kit::test]
fn a_running_command_moves_to_the_background_from_its_row(cx: &mut TestAppContext) {
    let mock = Mock::start(|index, _| {
        if index == 0 {
            let call = arko::testing::tool_call(
                "m1",
                "shell",
                serde_json::json!({ "command": "sleep 60" }),
            );
            Box::pin(async move { call })
        } else {
            answer("It goes on in the background.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Build it.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the command to be movable", |cx| {
        cx.read(|cx| !session.read(cx).reading().movable.is_empty())
    });
    let item = cx.read(|cx| session.read(cx).reading().movable[0]);
    let drawn = |cx: &mut TestAppContext, id: (&'static str, u64)| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    };
    wait_until(cx, "Move to background", |cx| {
        drawn(cx, ("move-call", item))
    });
    act(cx, app.window, |window, cx| {
        window.click(("move-call", item), cx)
    });
    app.wait_for_text(cx, "It goes on in the background.");
    assert!(mock.was_sent("in the background as job "));
    // Its row offers to stop the job while it runs.
    wait_until(cx, "Stop", |cx| drawn(cx, ("stop-job", item)));
    act(cx, app.window, |window, cx| {
        window.click(("stop-job", item), cx)
    });
    let outcome = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let feed = session.read(cx).feed().cloned().unwrap();
            let call = feed.subscribe().0.items.into_iter().find(|i| i.id == item);
            call.and_then(|call| call.outcome)
        })
    };
    wait_until(cx, "the job to stop", |cx| {
        outcome(cx).as_deref() == Some("stopped")
    });
    wait_until(cx, "Stop to go", |cx| !drawn(cx, ("stop-job", item)));
    app.close_all(cx);
}

/// A monitor the agent starts shows on its call's row while it runs, with
/// Stop, and closing the session names it by what it watches. Stop on the
/// row stops it, and the row says so.
#[cfg(unix)]
#[gpui_kit::test]
fn a_monitor_shows_on_its_calls_row_and_stops_from_it(cx: &mut TestAppContext) {
    let mock = Mock::start(|index, _| {
        if index == 0 {
            let call = arko::testing::tool_call(
                "w1",
                "monitor_start",
                serde_json::json!({ "command": "sleep 600", "pattern": "ERROR" }),
            );
            Box::pin(async move { call })
        } else {
            answer("Watching it.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Watch the server.");
    app.wait_for_text(cx, "Watching it.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the monitor in the status", |cx| {
        cx.read(|cx| !session.read(cx).reading().jobs.is_empty())
    });
    // The quit dialog names it by what it runs.
    let work = cx.read(|cx| session.read(cx).running_work());
    assert_eq!(work.jobs.len(), 1);
    assert_eq!(work.jobs[0].command, "monitor · sleep 600");
    let call = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let feed = session.read(cx).feed().cloned().unwrap();
            let items = feed.subscribe().0.items;
            items
                .into_iter()
                .find(|item| item.tool.as_deref() == Some("monitor_start"))
                .unwrap()
        })
    };
    let item = call(cx).id;
    assert_eq!(call(cx).outcome.as_deref(), Some("monitoring"));
    let drawn = |cx: &mut TestAppContext, id: (&'static str, u64)| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    };
    wait_until(cx, "Stop", |cx| drawn(cx, ("stop-job", item)));
    let transcript = app.transcript(cx).unwrap();
    let said = |cx: &mut TestAppContext| cx.read(|cx| transcript.read(cx).job_text(item));
    assert_eq!(said(cx).as_deref(), Some("Monitoring · ends in 30 min"));
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(menus::CloseSession), cx)
    });
    act(cx, app.window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("quit-work").is_some());
        window.within("dialog").click("cancel", cx);
    });
    act(cx, app.window, |window, cx| {
        window.click(("stop-job", item), cx)
    });
    wait_until(cx, "the monitor to stop", |cx| {
        call(cx).outcome.as_deref() == Some("stopped")
    });
    wait_until(cx, "Stop to go", |cx| !drawn(cx, ("stop-job", item)));
    assert_eq!(said(cx).as_deref(), Some("Stopped"));
    app.close_all(cx);
}

/// A session running a background job, listed in the sidebar.
fn a_session_with_a_job(cx: &mut TestAppContext) -> (Mock, App, Entity<LiveSession>, SessionId) {
    let mock = Mock::start(|index, _| {
        if index == 0 {
            let call = arko::testing::tool_call(
                "j1",
                "terminal_start",
                serde_json::json!({ "command": "sleep 60", "terminal": "false" }),
            );
            Box::pin(async move { call })
        } else {
            answer("Started it.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Start the long job.");
    app.wait_for_text(cx, "Started it.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the job in the status", |cx| {
        cx.read(|cx| !session.read(cx).reading().jobs.is_empty())
    });
    let id = cx.read(|cx| session.read(cx).id().cloned().unwrap());
    wait_until(cx, "the sidebar to list it", |cx| {
        cx.read(|cx| {
            let sidebar = app.workspace.read(cx).sidebar().read(cx);
            sidebar.sessions().iter().any(|entry| entry.id == id)
        })
    });
    (mock, app, session, id)
}

impl App {
    /// Delete… on `id` in the sidebar, then Delete in the dialog.
    fn delete(&self, id: &SessionId, cx: &mut TestAppContext) {
        act(cx, self.window, |window, cx| {
            let sidebar = self.workspace.read(cx).sidebar().clone();
            sidebar.update(cx, |sidebar, cx| {
                sidebar.confirm_delete(id.clone(), window, cx)
            });
        });
        wait_until(cx, "the delete dialog", |cx| {
            cx.update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window.has_active_dialog(cx)
            })
            .unwrap()
        });
        act(cx, self.window, |window, cx| {
            window.within("dialog").click("ok", cx)
        });
    }
}

#[gpui_kit::test]
fn quitting_with_no_window_open_asks_about_the_work_in_a_new_one(cx: &mut TestAppContext) {
    let (_mock, app, session, _) = a_session_with_a_job(cx);
    // As on macOS: the last window closed, and the sessions run on.
    cx.update_window(app.window, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(cx.read(|cx| cx.windows().is_empty()));
    cx.update(crate::chrome::quit);
    cx.run_until_parked();
    let window = cx
        .read(|cx| cx.windows().first().copied())
        .expect("a window to ask in");
    act(cx, window, |window, cx| {
        assert!(window.try_find("quit-work").is_some());
        window.within("dialog").click("cancel", cx);
    });
    assert!(cx.read(|cx| session.read(cx).is_running()), "kept");
    act(cx, window, |window, cx| {
        crate::quit::close(LiveSessions::all(cx), false, window, cx);
    });
    act(cx, window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| LiveSessions::all(cx).is_empty())
    });
}

#[gpui_kit::test]
fn deleting_a_session_with_running_work_asks_to_stop_it_then_deletes_it(cx: &mut TestAppContext) {
    let (_mock, app, session, id) = a_session_with_a_job(cx);
    app.delete(&id, cx);
    // The Delete dialog gave way to the question about the job.
    act(cx, app.window, |window, cx| {
        assert!(window.try_find("quit-work").is_some());
        window.within("dialog").click("ok", cx);
        assert!(!window.has_active_dialog(cx), "no dialog left behind");
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| *session.read(cx).phase() == Phase::Ended)
    });
    wait_until(cx, "the delete", |_| {
        arko::sessions::list(&app.root).is_empty()
    });
    wait_until(cx, "the sidebar to drop it", |cx| {
        cx.read(|cx| {
            let sidebar = app.workspace.read(cx).sidebar().read(cx);
            !sidebar.sessions().iter().any(|entry| entry.id == id)
        })
    });
}

#[gpui_kit::test]
fn a_cancelled_delete_leaves_nothing_behind_for_a_later_close(cx: &mut TestAppContext) {
    let (_mock, app, session, id) = a_session_with_a_job(cx);
    app.delete(&id, cx);
    act(cx, app.window, |window, cx| {
        assert!(window.try_find("quit-work").is_some());
        window.within("dialog").click("cancel", cx);
        assert!(!window.has_active_dialog(cx), "no dialog left behind");
    });
    assert!(cx.read(|cx| session.read(cx).is_running()));
    // Later, a plain close: the session closes and stays.
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(menus::CloseSession), cx)
    });
    act(cx, app.window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| *session.read(cx).phase() == Phase::Ended)
    });
    for _ in 0..20 {
        cx.run_until_parked();
        cx.executor().advance_clock(crate::live::FRAME);
    }
    assert_eq!(arko::sessions::list(&app.root).len(), 1, "still there");
}

#[gpui_kit::test]
fn an_agent_session_switches_to_chat_after_saying_what_it_costs(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, body| {
        if body.contains("Now just talk.") {
            answer("Talking.", Duration::ZERO)
        } else {
            answer("Working.", Duration::ZERO)
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Start working.");
    app.wait_for_text(cx, "Working.");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the turn to end", |cx| {
        cx.read(|cx| !session.read(cx).is_working())
    });
    app.wait_for_rows(cx, 2);
    let before = app.transcript(cx).unwrap();
    act(cx, app.window, |window, cx| window.click("mode-model", cx));
    act(cx, app.window, |window, cx| window.click("mode-chat", cx));
    wait_until(cx, "the switch dialog", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find("switch-mode").is_some()
        })
        .unwrap()
    });
    act(cx, app.window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    // The engine closes the session and the app opens it again, as a chat:
    // the window goes on showing it.
    wait_until(cx, "the session to open again as a chat", |cx| {
        cx.read(|cx| {
            let session = session.read(cx);
            session.mode() == crate::backend::Mode::Chat
                && *session.phase() == Phase::Open
                && session.reading().models.started
        })
    });
    assert_eq!(
        app.shown(cx).map(|shown| shown.entity_id()),
        Some(session.entity_id())
    );
    // The first feed went, with the tasks that drained it.
    let feed_tasks = cx.read(|cx| session.read(cx).feed_tasks());
    assert_eq!(feed_tasks, 4);
    // The reopened session's feed has a transcript of its own, with the
    // conversation so far.
    app.wait_for_rows(cx, 2);
    assert_ne!(
        app.transcript(cx).map(|view| view.entity_id()),
        Some(before.entity_id())
    );
    app.send(cx, "Now just talk.");
    app.wait_for_text(cx, "Talking.");
    assert_eq!(
        arko::sessions::list(&app.root)[0].mode,
        arko::sessions::Mode::Chat
    );
    app.close_all(cx);
}

#[gpui_kit::test]
fn an_image_goes_with_the_message(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("A single pixel.", Duration::ZERO));
    let app = start(cx, &mock);
    let composer = cx.read(|cx| {
        app.workspace
            .read(cx)
            .session_view()
            .read(cx)
            .composer()
            .clone()
    });
    cx.update(|cx| {
        composer.update(cx, |composer, cx| {
            composer.attach_image(
                crate::composer::Attached::new(
                    gpui_kit::ImageFormat::Png,
                    testing::PNG.to_vec(),
                    Some("pixel.png".into()),
                ),
                cx,
            )
        })
    });
    app.send(cx, "What is this?");
    app.wait_for_text(cx, "A single pixel.");
    assert!(mock.was_sent("What is this?"));
    assert!(
        mock.was_sent("data:image/png;base64,"),
        "the image went to the model"
    );
    // The message keeps its image, for the transcript's thumbnail.
    let session = app.shown(cx);
    let images = cx.read(|cx| {
        session
            .and_then(|session| session.read(cx).feed().cloned())
            .map(|feed| {
                feed.subscribe()
                    .0
                    .items
                    .iter()
                    .find(|item| item.text == "What is this?")
                    .map(|item| item.images.len())
            })
    });
    assert_eq!(images, Some(Some(1)));
    assert!(cx.read(|cx| composer.read(cx).images().is_empty()), "sent");
    app.close_all(cx);
}

/// MCP config files set in Settings › Advanced that are gone, or aren't MCP
/// config files, are left out: a new session opens anyway, its message
/// sent, and says which it left out.
#[gpui_kit::test]
fn a_session_opens_without_the_mcp_config_files_it_cant_read(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| answer("Hello.", Duration::ZERO));
    let app = start(cx, &mock);
    let folder = app.root.parent().unwrap().to_path_buf();
    let invalid = folder.join("notes.json");
    std::fs::write(&invalid, "{}").unwrap();
    let files = vec![folder.join("gone.json"), invalid];
    cx.update(|cx| crate::prefs::update(cx, |prefs| prefs.advanced.mcp_config = files));
    app.send(cx, "Say hello.");
    app.wait_for_text(cx, "Hello.");
    let session = app.shown(cx).unwrap();
    assert_eq!(cx.read(|cx| session.read(cx).phase().clone()), Phase::Open);
    assert!(mock.was_sent("Say hello."));
    let notes = cx.read(|cx| {
        session
            .read(cx)
            .feed()
            .unwrap()
            .subscribe()
            .0
            .items
            .into_iter()
            .filter(|item| item.kind == arko::remote::Kind::Notice)
            .map(|item| item.text)
            .collect::<Vec<_>>()
    });
    for file in ["gone.json", "notes.json"] {
        assert_eq!(
            notes.iter().filter(|note| note.contains(file)).count(),
            1,
            "{notes:?}"
        );
    }
    app.close_all(cx);
}

/// While two workers run, the user writes to r1 from the workers panel
/// (its Message, then the composer). The message reaches r1 only, as the
/// user's: r2 never sees it, and the main agent gets only the copy its
/// news brings. It shows in r1's own transcript, and in no view of the
/// main transcript, nor in r2's.
#[gpui_kit::test]
fn a_message_to_a_worker_reaches_it_and_shows_in_its_transcript_only(cx: &mut TestAppContext) {
    use crate::prefs::TranscriptView as View;

    const RUN: &str = "You are agent run";
    let (go, gone) = tokio::sync::watch::channel(false);
    let mock = Mock::start(move |_, body| {
        // Each worker waits for the user's message, takes a step, and
        // reports.
        for (run, step, report) in [("r1", "w", "Indented."), ("r2", "v", "Tested.")] {
            if body.contains(&format!("{RUN} {run}")) {
                if body.contains(&format!("call_{step}1")) {
                    let done = arko::testing::complete(&format!("{step}2"), report);
                    return Box::pin(async move { done });
                }
                let mut gone = gone.clone();
                let call = arko::testing::tool_call(
                    &format!("{step}1"),
                    "shell",
                    serde_json::json!({ "command": "echo hi" }),
                );
                return Box::pin(async move {
                    gone.wait_for(|go| *go).await.unwrap();
                    call
                });
            }
        }
        let start = |id: &str, task: &str| {
            let call = arko::testing::tool_call(
                id,
                "start_agent",
                serde_json::json!({ "agent": "worker", "task": task }),
            );
            Box::pin(async move { call }) as Answer
        };
        if body.contains("Indented.") || body.contains("Tested.") {
            reply(&format!("m4_{}", body.matches("Noted.").count()), "Noted.")
        } else if body.contains("call_m2") {
            reply("m3", "Started them.")
        } else if body.contains("call_m1") {
            start("m2", "Test it.")
        } else {
            start("m1", "Indent it.")
        }
    });
    let app = start(cx, &mock);
    // A new session sends no copies unless the preference asks for them.
    cx.update(|cx| crate::prefs::update(cx, |prefs| prefs.new_sessions.message_copies = true));
    app.send(cx, "Indent and test.");
    app.wait_for_text(cx, "Started them.");
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx)
    });
    let bodies = |run: &str| -> Vec<String> {
        let head = format!("{RUN} {run}");
        let bodies = mock.bodies.lock().unwrap();
        bodies
            .iter()
            .filter(|b| b.contains(&head))
            .cloned()
            .collect()
    };
    wait_until(cx, "the workers to run", |_| {
        !bodies("r1").is_empty() && !bodies("r2").is_empty()
    });

    // r1's Message in the panel: the composer writes to r1.
    let panel = cx.read(|cx| app.workspace.read(cx).workers_panel().clone());
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        panel.update(cx, |panel, cx| panel.show_worker("r1".into(), window, cx))
    });
    act(cx, app.window, |window, cx| {
        window.click("worker-message", cx)
    });
    assert_eq!(
        cx.read(|cx| view.read(cx).target().cloned()).as_deref(),
        Some("r1")
    );
    app.send(cx, "Use tabs.");
    let session = app.shown(cx).unwrap();
    let items = |cx: &mut TestAppContext| {
        cx.read(|cx| session.read(cx).feed().unwrap().subscribe().0.items)
    };
    wait_until(cx, "the message in the feed", |cx| {
        items(cx).iter().any(|item| item.text == "Use tabs.")
    });
    // The docked panel shows r1's transcript, which shows the message, so
    // no notification says it went.
    act(cx, app.window, |window, cx| {
        assert!(gpui_kit::component::WindowExt::notifications(window, cx).is_empty())
    });
    go.send(true).unwrap();
    // (Closing asks first while the status says workers run.)
    wait_until(cx, "the lead to hear both reports", |cx| {
        let lead = mock.bodies.lock().unwrap().iter().any(|body| {
            !body.contains(RUN) && body.contains("Indented.") && body.contains("Tested.")
        });
        lead && cx.read(|cx| {
            let session = session.read(cx);
            !session.is_working() && session.running_work().workers.is_empty()
        })
    });

    // Only r1's conversation has it, as the user's.
    let read = bodies("r1").pop().unwrap();
    assert!(
        read.contains("The user sent you a message directly.") && read.contains(":\\n  Use tabs."),
        "{read}"
    );
    for body in bodies("r2") {
        assert!(!body.contains("Use tabs."), "{body}");
    }
    // The main agent's has it once, in the copy its news brings.
    let lead = mock
        .bodies
        .lock()
        .unwrap()
        .iter()
        .rfind(|body| !body.contains(RUN))
        .unwrap()
        .clone();
    assert_eq!(lead.matches("Use tabs.").count(), 1, "{lead}");
    let at = lead.find("Use tabs.").unwrap();
    let copy = "The user sent r1 a message directly (a copy for you):";
    let heading = lead[..at].rfind(copy).expect(&lead);
    assert!(at - heading - copy.len() <= "\\n\\n".len(), "{lead}");
    // The feed has it as r1's.
    let sent: Vec<_> = items(cx)
        .into_iter()
        .filter(|item| item.text.contains("Use tabs."))
        .map(|item| (item.agent, item.kind))
        .collect();
    assert_eq!(sent, [("r1".to_string(), arko::remote::Kind::User)]);

    // It shows in r1's transcript, in no view of the main one, and not in r2's.
    let shown = |cx: &mut TestAppContext, worker: Option<&str>| {
        if let Some(worker) = worker {
            act(cx, app.window, |window, cx| {
                panel.update(cx, |panel, cx| panel.show_worker(worker.into(), window, cx))
            });
        }
        cx.update_window(app.window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            let transcript = match worker {
                Some(_) => panel.read(cx).transcript().unwrap().clone(),
                None => view.read(cx).transcript(cx).unwrap().clone(),
            };
            transcript.read(cx).shown_user_messages()
        })
    };
    for each in [View::Full, View::Quiet, View::Messages] {
        cx.update(|cx| crate::prefs::update(cx, |prefs| prefs.view = each));
        assert_eq!(shown(cx, None), ["Indent and test."], "{each:?}");
        assert_eq!(shown(cx, Some("r1")), ["Use tabs."], "{each:?}");
        assert!(shown(cx, Some("r2")).is_empty(), "{each:?}");
    }
    let exported = cx.read(|cx| {
        view.read(cx)
            .transcript(cx)
            .unwrap()
            .read(cx)
            .export_items()
    });
    assert!(!exported.iter().any(|item| item.text.contains("Use tabs.")));
    app.close_all(cx);
}

/// What frames cost around a long session, and how often each view draws,
/// as the sidebar, the status bar, the composer and the transcript ask for
/// them. It runs 40 turns, so it is ignored by default.
///
/// ```sh
/// cargo test --lib frames_around_a_long_session -- --ignored --nocapture
/// ```
///
/// Each step asks for a frame and draws one more, so a time is for two draws
/// of the window, on the test platform, which doesn't shape text. The times are
/// of the fastest batch, as they vary with the load.
#[gpui_kit::test]
#[ignore]
fn frames_around_a_long_session(cx: &mut TestAppContext) {
    use std::time::Instant;

    let mock = Mock::start(|_, body| {
        let n = body.matches("Question ").count();
        let text = format!(
            "Answer {n}, the first paragraph, with `code` and **bold** words in it.\n\nThe second paragraph of answer {n} goes on a while, so that it wraps over a couple of lines in the column.\n\n- a list item\n- another one\n\n```rust\nfn answer_{n}() {{}}\n```\n\nAnd a last paragraph."
        );
        Box::pin(async move { arko::testing::reply("r1", &text) })
    });
    let app = start(cx, &mock);
    for n in 0..40 {
        app.send(cx, &format!("Question {n}"));
        app.wait_for_text(cx, &format!("Answer {}", n + 1));
        wait_until(cx, "the turn to end", |cx| {
            app.shown(cx)
                .is_some_and(|session| cx.read(|cx| !session.read(cx).is_working()))
        });
    }
    let transcript = app.transcript(cx).unwrap();
    let (sidebar, status, view) = cx.read(|cx| {
        let workspace = app.workspace.read(cx);
        (
            workspace.sidebar().clone(),
            workspace.status_bar().clone(),
            workspace.session_view().clone(),
        )
    });
    let composer = cx.read(|cx| view.read(cx).composer().clone());
    let draw = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    };
    for _ in 0..5 {
        cx.run_until_parked();
        draw(cx);
    }
    eprintln!("{} rows", cx.read(|cx| transcript.read(cx).row_count()));
    let draws = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            (
                view.read(cx).render_count(),
                transcript.read(cx).render_count(),
            )
        })
    };
    let frames = |cx: &mut TestAppContext, name: &str, ask: &dyn Fn(&mut TestAppContext)| {
        const BATCHES: usize = 15;
        const STEPS: usize = 20;
        let (session_view, rows) = draws(cx);
        let mut times = Vec::new();
        for _ in 0..BATCHES {
            let start = Instant::now();
            for _ in 0..STEPS {
                ask(cx);
                draw(cx);
            }
            times.push(start.elapsed());
        }
        times.sort();
        let (session_view_after, rows_after) = draws(cx);
        eprintln!(
            "{name}: {:.2} ms a step (the fastest of {BATCHES} batches), the session view drew {} times and the transcript {} times in {} steps",
            times[0].as_secs_f64() * 1000. / STEPS as f64,
            session_view_after - session_view,
            rows_after - rows,
            BATCHES * STEPS
        );
    };
    for _ in 0..2 {
        frames(cx, "the workspace", &|cx| {
            app.workspace.update(cx, |_, cx| cx.notify())
        });
        frames(cx, "the sidebar", &|cx| {
            sidebar.update(cx, |_, cx| cx.notify())
        });
        frames(cx, "the status bar", &|cx| {
            status.update(cx, |_, cx| cx.notify())
        });
        frames(cx, "the composer", &|cx| {
            composer.update(cx, |_, cx| cx.notify())
        });
        frames(cx, "the session view", &|cx| {
            view.update(cx, |_, cx| cx.notify())
        });
        frames(cx, "the transcript", &|cx| {
            transcript.update(cx, |_, cx| cx.notify())
        });
    }
    app.close_all(cx);
}

/// A session of 40 turns, and a mock model that answers "The last question"
/// with a shell command that runs for a while.
fn a_long_session(cx: &mut TestAppContext) -> (App, Mock) {
    let mock = Mock::start(|_, body| {
        if body.contains("The last question") {
            if body.contains("call_m1") {
                return reply("r1", "Done.");
            }
            let call = arko::testing::tool_call(
                "m1",
                "shell",
                serde_json::json!({ "command": "sleep 30" }),
            );
            return Box::pin(async move { call });
        }
        let n = body.matches("Question ").count();
        let text = format!(
            "Answer {n}, the first paragraph, with `code` and **bold** words in it.\n\nThe second paragraph of answer {n} goes on a while, so that it wraps over a couple of lines in the column.\n\n- a list item\n- another one\n\n```rust\nfn answer_{n}() {{}}\n```\n\nAnd a last paragraph."
        );
        Box::pin(async move { arko::testing::reply("r1", &text) })
    });
    let app = start(cx, &mock);
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::On
        });
        cx.set_reduce_motion(false);
    });
    for n in 0..40 {
        app.send(cx, &format!("Question {n}"));
        app.wait_for_text(cx, &format!("Answer {}", n + 1));
        wait_until(cx, "the turn to end", |cx| {
            app.shown(cx)
                .is_some_and(|session| cx.read(|cx| !session.read(cx).is_working()))
        });
    }
    (app, mock)
}

/// What a second of time costs with the spinners turning, and how often the
/// session view and the transcript drew in it. Time is stepped in sixtieths of
/// a second, with a display frame at each step, on the test platform, which
/// doesn't shape text. A time is of the processor, for the fastest of 12
/// seconds.
fn spinner_seconds(cx: &mut TestAppContext, app: &App, name: &str) {
    // The time this thread has run, which other programs using the processor
    // don't stretch, where the clock on the wall would.
    fn cpu_time() -> Duration {
        std::fs::read_to_string("/proc/thread-self/schedstat")
            .ok()
            .and_then(|stat| stat.split_whitespace().next()?.parse::<u64>().ok())
            .map(Duration::from_nanos)
            .unwrap_or_default()
    }

    let transcript = app.transcript(cx).unwrap();
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    for _ in 0..30 {
        cx.run_until_parked();
        cx.executor().advance_clock(Duration::from_millis(17));
    }
    let draws = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            (
                view.read(cx).render_count(),
                transcript.read(cx).render_count(),
            )
        })
    };
    const SECONDS: usize = 12;
    crate::testing::count_renders();
    let (session_view, rows_drawn) = draws(cx);
    let mut times = Vec::new();
    for _ in 0..SECONDS {
        let start = cpu_time();
        for _ in 0..60 {
            cx.executor().advance_clock(Duration::from_micros(16_667));
            // The display's next frame, for the spinners that turn in each one.
            cx.update_window(app.window, |_, window, cx| {
                window.simulate_next_frame(cx);
            })
            .unwrap();
            cx.run_until_parked();
        }
        times.push(cpu_time() - start);
    }
    times.sort();
    let (session_view_after, rows_drawn_after) = draws(cx);
    let per_second = |name: &str| {
        crate::testing::renders()
            .get(name)
            .copied()
            .unwrap_or_default() as f64
            / SECONDS as f64
    };
    eprintln!(
        "{name}: {:.2} ms a second (the fastest of {SECONDS}). A second draws the workspace {:.1} times, the sidebar {:.1}, the session view {:.1} and the transcript {:.1}",
        times[0].as_secs_f64() * 1000.,
        per_second("Workspace"),
        per_second("Sidebar"),
        (session_view_after - session_view) as f64 / SECONDS as f64,
        (rows_drawn_after - rows_drawn) as f64 / SECONDS as f64,
    );
}

/// What the running indicator of a command costs in a long session, which
/// turns in each frame of the display. It runs 40 turns, so it is ignored by
/// default.
///
/// ```sh
/// cargo test --lib frames_while_a_command_runs -- --ignored --nocapture
/// ```
#[gpui_kit::test]
#[ignore]
fn frames_while_a_command_runs(cx: &mut TestAppContext) {
    let (app, _mock) = a_long_session(cx);
    app.send(cx, "The last question");
    let session = app.shown(cx).unwrap();
    wait_until(cx, "the command to run", |cx| {
        cx.read(|cx| !session.read(cx).reading().movable.is_empty())
    });
    spinner_seconds(cx, &app, "a command running in the shown session");
    // The command runs on: the session ends with the app.
    app.close_all(cx);
}

/// What the spinner of a session that works in the background costs next to
/// a long session, in the sidebar, in each frame of the display. It runs 40
/// turns, so it is ignored by default.
///
/// ```sh
/// cargo test --lib frames_while_another_session_works -- --ignored --nocapture
/// ```
#[gpui_kit::test]
#[ignore]
fn frames_while_another_session_works(cx: &mut TestAppContext) {
    let (app, _mock) = a_long_session(cx);
    let long = app.shown(cx).unwrap();
    // A second session starts a command, and the window goes back to the long one.
    act(cx, app.window, |window, cx| window.click("new-session", cx));
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(app.work.clone(), cx)));
    app.send(cx, "The last question");
    let other = app.shown(cx).unwrap();
    wait_until(cx, "the command to run", |cx| {
        cx.read(|cx| !other.read(cx).reading().movable.is_empty())
    });
    // The sidebar lists the group and both sessions.
    wait_until(cx, "the sidebar's rows", |cx| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(("row", 2usize)).is_some()
        })
        .unwrap()
    });
    for row in 1..3usize {
        act(cx, app.window, |window, cx| window.click(("row", row), cx));
        if app.shown(cx).map(|shown| shown.entity_id()) == Some(long.entity_id()) {
            break;
        }
    }
    assert_eq!(
        app.shown(cx).map(|shown| shown.entity_id()),
        Some(long.entity_id())
    );
    app.wait_for_rows(cx, 40);
    let spinning = cx
        .update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            (0..3usize).any(|row| window.try_find(("row-working", row)).is_some())
        })
        .unwrap();
    assert!(spinning, "the sidebar shows no spinner");
    spinner_seconds(
        cx,
        &app,
        "a spinner in the sidebar next to the long session",
    );
    app.close_all(cx);
}
