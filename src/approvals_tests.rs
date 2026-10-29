//! The approval tray in the window: one request or several, the boxes and
//! the counting Approve button, Reject all, a row's details and its option,
//! the message box's words going with the answer, the shortcuts while the
//! message box has the keyboard, and the tray's own keys. A session that
//! hasn't opened keeps what is sent in its queue.

use std::sync::Arc;
use std::time::SystemTime;

use arko::approvals::{ApprovalAnswers, ApprovalChoice, ApprovalRequest};
use arko::remote::{Activity, Reading};
use gpui_kit::Focusable as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::Startup;
use crate::approvals::{part, request};
use crate::backend::FakeBackend;
use crate::live::LiveSession;
use crate::tests::{act, open};
use crate::workspace::Workspace;

struct Window {
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    session: Entity<LiveSession>,
}

/// A window showing a session that hasn't opened.
fn start(cx: &mut TestAppContext) -> Window {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    act(cx, window, |window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.show_session(session.clone(), window, cx)
        })
    });
    Window {
        window,
        workspace,
        session,
    }
}

/// Arko asks to save the worker profile `name`, as request `id`.
fn profile(id: &str, name: &str) -> ApprovalRequest {
    request(serde_json::json!({
        "id": id, "batch": 1,
        "approval": {
            "kind": "profile", "name": name, "model": "sol", "effort": "high",
            "scope": "every project", "folder": "Arko's profiles folder",
            "description": "Judges 3D results",
            "instructions": "You judge 3D models.\nCompare them with the reference."
        }
    }))
}

/// Arko asks to add the MCP server github.
fn server(id: &str) -> ApprovalRequest {
    request(serde_json::json!({
        "id": id, "batch": 1,
        "approval": {
            "kind": "mcp_server", "name": "github", "scope": "project",
            "command": "npx -y @modelcontextprotocol/server-github",
            "env": [{"name": "GITHUB_TOKEN", "set": false}],
            "why": "Read the issues."
        }
    }))
}

/// Arko asks to start a worker on Opus 5.5, billed per token.
fn billing(id: &str) -> ApprovalRequest {
    request(serde_json::json!({
        "id": id, "batch": 1,
        "approval": {
            "kind": "worker_billing", "model": "Opus 5.5", "input_price": 5.0,
            "output_price": 25.0, "spent": 1.2, "task": "Review the parser"
        }
    }))
}

/// Arko asks to save the skill release-notes, with a script.
fn skill(id: &str) -> ApprovalRequest {
    request(serde_json::json!({
        "id": id, "batch": 1,
        "approval": {
            "kind": "skill", "name": "release-notes", "scope": "user",
            "description": "Writes release notes from the git log",
            "source": {"from": "agent"}, "scripts": ["scripts/log.sh"],
            "why": "You asked for notes for each release.",
            "files": [
                {"path": "SKILL.md", "bytes": 40, "text": "# Release notes\nRead the log."},
                {"path": "scripts/log.sh", "bytes": 20, "text": "git log --oneline"}
            ]
        }
    }))
}

/// The answer naming each of `choices` (an id and whether it is approved),
/// with `words`.
fn answers(choices: &[(&str, bool)], words: Option<&str>) -> arko::Input {
    arko::Input::AnswerApprovals(ApprovalAnswers {
        choices: choices
            .iter()
            .map(|(id, approve)| ApprovalChoice {
                id: id.to_string(),
                approve: *approve,
                option: false,
            })
            .collect(),
        words: words.map(str::to_string),
    })
}

impl Window {
    /// The engine's status lists `requests` in the tray.
    fn request(&self, cx: &mut TestAppContext, requests: Vec<ApprovalRequest>) {
        let reading = Reading {
            approvals: requests,
            ..Reading::default()
        };
        self.session.update(cx, |session, cx| {
            session.set_status(reading, Activity::default(), cx)
        });
        cx.run_until_parked();
    }

    fn shows(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> bool {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }

    fn label(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).label().map(str::to_string)
        })
        .unwrap()
    }

    /// Whether the row of request `id` is checked, as a screen reader hears.
    fn checked(&self, cx: &mut TestAppContext, id: &str) -> Option<bool> {
        let id = part("approval-row", id);
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).selected()
        })
        .unwrap()
    }

    fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
        let id = id.into();
        act(cx, self.window, |window, cx| window.click(id, cx));
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        act(cx, self.window, |window, cx| window.press(key, cx));
    }

    fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        act(cx, self.window, |window, cx| window.input(text, cx));
    }

    /// What the session sent (it keeps it until it opens).
    fn sent(&self, cx: &mut TestAppContext) -> Vec<arko::Input> {
        cx.read(|cx| self.session.read(cx).queued().to_vec())
    }

    fn composer_text(&self, cx: &mut TestAppContext) -> String {
        cx.read(|cx| {
            let view = self.workspace.read(cx).session_view().clone();
            view.read(cx).composer().read(cx).text(cx)
        })
    }

    fn composer_focused(&self, cx: &mut TestAppContext) -> bool {
        let textarea = cx.read(|cx| {
            let view = self.workspace.read(cx).session_view().clone();
            view.read(cx).composer().read(cx).textarea().clone()
        });
        cx.update_window(self.window, |_, window, cx| {
            textarea.read(cx).focus_handle(cx).is_focused(window)
        })
        .unwrap()
    }

    fn tray_focused(&self, cx: &mut TestAppContext) -> bool {
        let tray = cx.read(|cx| {
            let view = self.workspace.read(cx).session_view().clone();
            view.read(cx).approval_tray().cloned()
        });
        let Some(tray) = tray else {
            return false;
        };
        cx.update_window(self.window, |_, window, cx| {
            tray.read(cx).contains_focus(window, cx)
        })
        .unwrap()
    }

    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.window, |_, window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.notifications(cx).len()
        })
        .unwrap()
    }
}

/// One request is one row with Reject and Approve, and no box. Approve
/// answers it, and the tray goes.
#[gpui_kit::test]
fn one_request_is_one_row_with_reject_and_approve(cx: &mut TestAppContext) {
    let app = start(cx);
    assert!(!app.shows(cx, "approval-tray"));
    app.request(cx, vec![profile("a1", "3d-judge")]);
    assert!(app.shows(cx, "approval-tray"));
    assert_eq!(
        app.label(cx, "approval-heading").as_deref(),
        Some("1 approval")
    );
    assert_eq!(
        app.label(cx, part("approval-row", "a1")).as_deref(),
        Some("Profile, 3d-judge, every project, sol high")
    );
    assert!(!app.shows(cx, part("approval-check", "a1")));
    assert_eq!(app.checked(cx, "a1"), None);
    assert_eq!(app.label(cx, "approval-reject").as_deref(), Some("Reject"));
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Approve")
    );
    // The shortcut shows on each button.
    let keys = if cfg!(target_os = "macos") {
        ["kbd:cmd-shift-a", "kbd:cmd-shift-r"]
    } else {
        ["kbd:ctrl-shift-a", "kbd:ctrl-shift-r"]
    };
    for key in keys {
        let shown = gpui_kit::VisualTestContext::from_window(app.window, cx).debug_bounds(key);
        assert!(shown.is_some(), "{key}");
    }
    app.click(cx, "approval-approve");
    assert_eq!(app.sent(cx), [answers(&[("a1", true)], None)]);
    // Answered here, it leaves before the status says so.
    assert!(!app.shows(cx, "approval-tray"));
}

/// Several requests share one tray, a box each, all checked at first. The
/// Approve button counts what is checked and rejects the others in the
/// same click.
#[gpui_kit::test]
fn approve_counts_the_checked_and_rejects_the_others(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(
        cx,
        vec![profile("a1", "3d-judge"), server("a2"), billing("a3")],
    );
    assert_eq!(
        app.label(cx, "approval-heading").as_deref(),
        Some("3 approvals")
    );
    for id in ["a1", "a2", "a3"] {
        assert_eq!(app.checked(cx, id), Some(true), "{id}");
    }
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Approve all")
    );
    assert_eq!(
        app.label(cx, "approval-reject").as_deref(),
        Some("Reject all")
    );
    app.click(cx, part("approval-check", "a2"));
    assert_eq!(app.checked(cx, "a2"), Some(false));
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Approve 2, reject 1")
    );
    app.click(cx, "approval-approve");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", true), ("a2", false), ("a3", true)], None)]
    );
}

/// Reject all rejects every request, whatever is checked.
#[gpui_kit::test]
fn reject_all_rejects_every_request(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    app.click(cx, part("approval-check", "a1"));
    app.click(cx, "approval-reject");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", false), ("a2", false)], None)]
    );
    assert!(!app.shows(cx, "approval-tray"));
}

/// With none checked, the Approve button rejects them all and says so.
#[gpui_kit::test]
fn approve_with_none_checked_rejects_all(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    app.click(cx, part("approval-check", "a1"));
    app.click(cx, part("approval-check", "a2"));
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Reject all")
    );
    app.click(cx, "approval-approve");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", false), ("a2", false)], None)]
    );
}

/// A request that comes while the tray is open joins it, checked, and what
/// was set for the others stays.
#[gpui_kit::test]
fn new_requests_join_the_open_tray(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge")]);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    app.click(cx, part("approval-check", "a1"));
    app.request(
        cx,
        vec![profile("a1", "3d-judge"), server("a2"), billing("a3")],
    );
    assert_eq!(app.checked(cx, "a1"), Some(false));
    assert_eq!(app.checked(cx, "a3"), Some(true));
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Approve 2, reject 1")
    );
    // One answered elsewhere leaves, and the box of the one left stays.
    app.request(cx, vec![profile("a1", "3d-judge"), billing("a3")]);
    assert_eq!(
        app.label(cx, "approval-approve").as_deref(),
        Some("Approve 1, reject 1")
    );
    app.request(cx, Vec::new());
    assert!(!app.shows(cx, "approval-tray"));
    assert!(app.sent(cx).is_empty());
}

/// What the message box holds goes to the agent with the answer, and the
/// box empties.
#[gpui_kit::test]
fn the_message_box_words_go_with_the_answer(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge")]);
    app.type_text(cx, "Run it on opus instead");
    app.click(cx, "approval-reject");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", false)], Some("Run it on opus instead"))]
    );
    assert_eq!(app.composer_text(cx), "");
}

/// Words too long to reach the agent aren't sent. The tray says why and
/// keeps the requests, and the words stay to be shortened.
#[gpui_kit::test]
fn words_too_long_to_send_stay_in_the_message_box(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge")]);
    let long = "word ".repeat(4000);
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        let composer = view.read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.set_text(&long, window, cx));
    });
    app.click(cx, "approval-approve");
    assert!(app.sent(cx).is_empty());
    assert_eq!(app.notifications(cx), 1);
    assert!(app.shows(cx, "approval-tray"));
    assert_eq!(app.composer_text(cx), long);
}

/// Mod+Shift+A approves those checked and Mod+Shift+R rejects them all
/// while the message box has the keyboard, which keeps it. The words typed
/// go with the answer.
#[gpui_kit::test]
fn the_shortcuts_answer_from_the_message_box(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    app.click(cx, part("approval-check", "a2"));
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        view.update(cx, |view, cx| view.focus_composer(window, cx))
    });
    app.type_text(cx, "Use opus");
    app.press(cx, "secondary-shift-a");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", true), ("a2", false)], Some("Use opus"))]
    );
    assert_eq!(app.composer_text(cx), "");
    assert!(app.composer_focused(cx));
    app.request(cx, vec![billing("a3")]);
    app.type_text(cx, "Too dear");
    app.press(cx, "secondary-shift-r");
    assert_eq!(
        app.sent(cx).last(),
        Some(&answers(&[("a3", false)], Some("Too dear")))
    );
    assert!(app.composer_focused(cx));
    // With no request waiting, the shortcut says so.
    app.request(cx, Vec::new());
    app.press(cx, "secondary-shift-a");
    assert_eq!(app.sent(cx).len(), 2);
    assert_eq!(app.notifications(cx), 1);
}

/// A row opens in place to its details: what it asks, its lines and its
/// warning. A second click closes them.
#[gpui_kit::test]
fn a_row_opens_in_place_to_its_details(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    assert!(!app.shows(cx, part("approval-details", "a2")));
    assert_eq!(
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(part("approval-row", "a2")).expanded()
        })
        .unwrap(),
        Some(false)
    );
    app.click(cx, part("approval-row", "a2"));
    assert!(app.shows(cx, part("approval-details", "a2")));
    assert!(app.shows(cx, part("approval-question", "a2")));
    assert!(!app.shows(cx, part("approval-details", "a1")));
    // One row is open at a time.
    app.click(cx, part("approval-row", "a1"));
    assert!(app.shows(cx, part("approval-details", "a1")));
    assert!(app.shows(cx, part("approval-box", "a1")));
    assert!(!app.shows(cx, part("approval-details", "a2")));
    app.click(cx, part("approval-row", "a1"));
    assert!(!app.shows(cx, part("approval-details", "a1")));
    // Opening a row checks nothing.
    assert!(app.sent(cx).is_empty());
}

/// A skill's files show as chips, and the chosen one shows in the box.
#[gpui_kit::test]
fn a_skills_files_show_one_at_a_time_in_the_box(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![skill("a1")]);
    app.click(cx, part("approval-row", "a1"));
    let file = |n: usize| (gpui_kit::SharedString::from("approval-file-a1"), n);
    assert!(app.shows(cx, file(0)));
    assert!(app.shows(cx, file(1)));
    let shown = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(file(1)).checked()
        })
        .unwrap()
    };
    assert_eq!(shown(cx), Some(false));
    app.click(cx, file(1));
    assert_eq!(shown(cx), Some(true));
}

/// A worker billed per token has its option in its details: approved with
/// it on, the answer says so. Rejected, it never does.
#[gpui_kit::test]
fn the_option_goes_with_its_approval(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![billing("a1")]);
    assert!(!app.shows(cx, part("approval-option", "a1")));
    app.click(cx, part("approval-row", "a1"));
    assert_eq!(
        app.label(cx, part("approval-option", "a1")).as_deref(),
        Some("Allow Opus 5.5 for this session")
    );
    app.click(cx, part("approval-option", "a1"));
    app.click(cx, "approval-approve");
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerApprovals(ApprovalAnswers {
            choices: vec![ApprovalChoice {
                id: "a1".into(),
                approve: true,
                option: true,
            }],
            words: None,
        })]
    );
    // Rejected with the option on, the option doesn't go.
    app.request(cx, vec![billing("a2")]);
    app.click(cx, part("approval-row", "a2"));
    app.click(cx, part("approval-option", "a2"));
    app.click(cx, "approval-reject");
    assert_eq!(app.sent(cx).last(), Some(&answers(&[("a2", false)], None)));
}

/// Alt+A gives the tray the keyboard. There ↑↓ move between the rows,
/// Space checks one, A checks all or none, → opens a row's details, Esc
/// closes them and then gives the keyboard back, and Enter approves.
#[gpui_kit::test]
fn the_tray_keys(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(
        cx,
        vec![profile("a1", "3d-judge"), server("a2"), billing("a3")],
    );
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        view.update(cx, |view, cx| view.focus_composer(window, cx))
    });
    app.press(cx, "alt-a");
    assert!(app.tray_focused(cx));
    app.press(cx, "down");
    app.press(cx, "space");
    assert_eq!(app.checked(cx, "a2"), Some(false));
    // A checks all, then none.
    app.press(cx, "a");
    for id in ["a1", "a2", "a3"] {
        assert_eq!(app.checked(cx, id), Some(true), "{id}");
    }
    app.press(cx, "a");
    for id in ["a1", "a2", "a3"] {
        assert_eq!(app.checked(cx, id), Some(false), "{id}");
    }
    app.press(cx, "a");
    app.press(cx, "up");
    app.press(cx, "space");
    assert_eq!(app.checked(cx, "a1"), Some(false));
    app.press(cx, "right");
    assert!(app.shows(cx, part("approval-details", "a1")));
    app.press(cx, "escape");
    assert!(!app.shows(cx, part("approval-details", "a1")));
    assert!(app.tray_focused(cx));
    app.press(cx, "escape");
    assert!(app.composer_focused(cx));
    // Alt+A again, and Enter approves those checked.
    app.press(cx, "alt-a");
    assert!(app.tray_focused(cx));
    app.press(cx, "enter");
    assert_eq!(
        app.sent(cx),
        [answers(&[("a1", false), ("a2", true), ("a3", true)], None)]
    );
    assert!(app.composer_focused(cx));
}

/// Enter and Space on a button or a box in the tray press it, and only it.
#[gpui_kit::test]
fn enter_on_a_button_in_the_tray_presses_that_button(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![billing("a1")]);
    app.click(cx, part("approval-row", "a1"));
    // The option's box, by the keyboard.
    act(cx, app.window, |window, cx| {
        window.focus_next(cx);
    });
    let option_focused = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(part("approval-option", "a1")).focused()
        })
        .unwrap()
    };
    assert_eq!(option_focused(cx), Some(true));
    app.press(cx, "space");
    assert!(app.sent(cx).is_empty());
    act(cx, app.window, |window, cx| {
        window.focus_next(cx);
    });
    // Reject, pressed with Enter.
    app.press(cx, "enter");
    assert_eq!(app.sent(cx), [answers(&[("a1", false)], None)]);
}

/// What is checked for a session's requests waits while the window shows
/// another session.
#[gpui_kit::test]
fn the_tray_keeps_what_is_checked_for_each_session(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![profile("a1", "3d-judge"), server("a2")]);
    app.click(cx, part("approval-check", "a1"));
    let other = cx.new(|cx| LiveSession::for_tests("s2", cx));
    let show = |session: &Entity<LiveSession>, cx: &mut TestAppContext| {
        act(cx, app.window, |window, cx| {
            app.workspace.update(cx, |workspace, cx| {
                workspace.show_session(session.clone(), window, cx)
            })
        });
    };
    show(&other, cx);
    assert!(!app.shows(cx, "approval-tray"));
    show(&app.session.clone(), cx);
    assert_eq!(app.checked(cx, "a1"), Some(false));
    assert_eq!(app.checked(cx, "a2"), Some(true));
}

/// An answer that can't go (a sample session doesn't run) leaves the tray
/// and the message box as they were, and says why.
#[gpui_kit::test]
fn an_answer_that_cant_go_leaves_the_tray(cx: &mut TestAppContext) {
    let app = start(cx);
    act(cx, app.window, |window, cx| {
        let id = crate::backend::SessionId("20261016-091200-a1b2".into());
        app.workspace
            .update(cx, |workspace, cx| workspace.open_session(id, window, cx));
    });
    let sample = cx.read(|cx| app.workspace.read(cx).live_session().unwrap().clone());
    sample.update(cx, |session, cx| {
        session.show_approvals(vec![profile("a1", "3d-judge")], cx)
    });
    cx.run_until_parked();
    app.type_text(cx, "Use opus");
    app.click(cx, "approval-approve");
    assert_eq!(app.notifications(cx), 1);
    assert!(app.shows(cx, "approval-tray"));
    assert_eq!(app.composer_text(cx), "Use opus");
}

/// The answer panel floats above the tray, so both show.
#[gpui_kit::test]
fn the_answer_panel_shows_above_the_tray(cx: &mut TestAppContext) {
    let app = start(cx);
    let reading = Reading {
        approvals: vec![profile("a1", "3d-judge")],
        questions: vec![crate::questions::question("q1", "Tabs or spaces?")],
        ..Reading::default()
    };
    app.session.update(cx, |session, cx| {
        session.set_status(reading, Activity::default(), cx)
    });
    cx.run_until_parked();
    assert!(app.shows(cx, "answer-panel"));
    assert!(app.shows(cx, "approval-tray"));
    let (panel, tray) = cx
        .update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            (
                window.find("answer-panel").bounds(),
                window.find("approval-tray").bounds(),
            )
        })
        .unwrap();
    assert!(panel.bottom() <= tray.top(), "{panel:?} {tray:?}");
}
