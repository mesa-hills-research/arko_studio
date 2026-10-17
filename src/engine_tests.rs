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
    // It ends unseen: the sidebar says so until the window shows it.
    wait_until(cx, "the slow reply", |cx| {
        app.feed_of(Some(&slow), cx)
            .iter()
            .any(|item| item.contains("Slow answer."))
    });
    wait_until(cx, "the unseen mark", |cx| indicator(cx).unseen);
    act(cx, app.window, |window, cx| {
        let workspace = app.workspace.clone();
        workspace.update(cx, |workspace, cx| {
            workspace.open_session(slow_id.clone(), window, cx)
        });
    });
    assert!(!indicator(cx).unseen);
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
fn a_turn_that_ends_after_its_window_closed_marks_the_session_unseen(cx: &mut TestAppContext) {
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
    assert!(cx.read(|cx| LiveSessions::indicator(&id, cx)).unseen);
    act(cx, second, |window, cx| {
        crate::quit::close(LiveSessions::all(cx), false, window, cx);
    });
    wait_until(cx, "the sessions to end", |cx| {
        cx.read(|cx| LiveSessions::all(cx).is_empty())
    });
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
