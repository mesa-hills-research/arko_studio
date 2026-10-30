//! The approval tray in the window: one request or several, the boxes and
//! the counting Approve button, Reject all, a row's details and its option,
//! the message box's words going with the answer, the shortcuts while the
//! message box has the keyboard, and the tray's own keys. A session that
//! hasn't opened keeps what is sent in its queue.

use std::sync::Arc;
use std::time::SystemTime;

use arko::approvals::{
    Approval, ApprovalAnswers, ApprovalChoice, ApprovalRequest, Scope, SkillApproval, SkillFile,
    SkillUpdate,
};
use arko::remote::{Activity, Reading};
use gpui_kit::Focusable as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, px};

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

/// Arko asks to update the skill release-notes an agent saved, whose
/// SKILL.md and script change and which gains a checklist.
fn update(id: &str) -> ApprovalRequest {
    let file = |path: &str, text: &str| SkillFile {
        path: path.into(),
        bytes: text.len() as u64,
        text: Some(text.into()),
    };
    ApprovalRequest {
        id: id.into(),
        batch: 1,
        approval: Approval::Skill(SkillApproval {
            name: "release-notes".into(),
            scope: Scope::Project,
            description: "Writes release notes from the git log".into(),
            update: Some(SkillUpdate {
                saved: "26 Oct".into(),
                diff: "  # Release notes\n- Read the log.\n+ Read the log since the last tag."
                    .into(),
                added: vec!["checklist.md".into()],
                changed: vec!["scripts/log.sh".into()],
                ..SkillUpdate::default()
            }),
            why: "The notes missed the tag.".into(),
            files: vec![
                file(
                    "SKILL.md",
                    "# Release notes\nRead the log since the last tag.",
                ),
                file("checklist.md", "- Tag it."),
                file("scripts/log.sh", "git log --oneline \"$1\"..HEAD"),
            ],
            ..SkillApproval::default()
        }),
    }
}

/// [`update`] with 60 changed lines, more than their box shows at once.
fn long_update(id: &str) -> ApprovalRequest {
    let mut long = update(id);
    if let Approval::Skill(skill) = &mut long.approval {
        let diff: Vec<String> = (1..=60).map(|n| format!("+ Line {n}.")).collect();
        skill.update.as_mut().unwrap().diff = diff.join("\n");
    }
    long
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

/// A long update's changed lines are read from the keyboard. Tab reaches
/// their box, ↓ and Page Down scroll it, End shows its last line and Home
/// its first, and Esc closes the details with the tray keeping the keyboard.
#[gpui_kit::test]
fn a_long_diff_scrolls_to_its_end_by_keyboard(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![long_update("a1")]);
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        view.update(cx, |view, cx| view.focus_composer(window, cx))
    });
    app.press(cx, "alt-a");
    app.press(cx, "right");
    app.press(cx, "tab");
    let diff = part("approval-diff", "a1");
    let place = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            let shown = window.find(diff.clone());
            assert_eq!(shown.focused(), Some(true));
            let lines = window.find(part("approval-diff-lines", "a1")).bounds();
            (shown.bounds(), lines)
        })
        .unwrap()
    };
    let (shown, start) = place(cx);
    assert_eq!(
        app.label(cx, diff.clone()).as_deref(),
        Some("Changed lines")
    );
    assert!(start.bottom() > shown.bottom() + px(500.), "{start:?}");
    let top = |cx: &mut TestAppContext| place(cx).1.top();
    app.press(cx, "down");
    let row = start.size.height / 60.;
    let after_row = start.top() - top(cx);
    assert!((after_row - row).abs() < px(1.), "{after_row:?} {row:?}");
    app.press(cx, "pagedown");
    let page = start.top() - top(cx) - after_row;
    assert!(
        page > row * 5. && page < shown.size.height,
        "{page:?} {shown:?}"
    );
    app.press(cx, "end");
    let (shown, end) = place(cx);
    // Its last line shows at the foot of the box, inside the focus ring.
    let below = shown.bottom() - end.bottom();
    assert!(below >= px(0.) && below < px(2.), "{below:?}");
    app.press(cx, "home");
    assert_eq!(top(cx), start.top());
    // ↑ and ↓ scrolled the box and stayed on the row.
    assert!(app.shows(cx, part("approval-details", "a1")));
    app.press(cx, "escape");
    assert!(!app.shows(cx, part("approval-details", "a1")));
    assert!(app.tray_focused(cx));
}

/// The wheel over a box of text scrolls the box alone, and over the rest
/// of the details it scrolls the tray's rows.
#[gpui_kit::test]
fn the_wheel_over_a_box_scrolls_the_box_alone(cx: &mut TestAppContext) {
    use gpui_kit::{InputEvent as _, ScrollDelta, ScrollWheelEvent, point};

    let app = start(cx);
    app.request(cx, vec![long_update("a1")]);
    app.click(cx, part("approval-row", "a1"));
    let tops = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            let top = |name: &str| window.find(part(name, "a1")).bounds().top();
            (top("approval-question"), top("approval-diff-lines"))
        })
        .unwrap()
    };
    let wheel = |cx: &mut TestAppContext, name: &str| {
        let at = cx
            .update_window(app.window, |_, window, cx| {
                window.render_frame(cx);
                window.find(part(name, "a1")).bounds().center()
            })
            .unwrap();
        cx.update_window(app.window, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: at,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
    };
    let (question, lines) = tops(cx);
    wheel(cx, "approval-diff");
    let (box_question, box_lines) = tops(cx);
    assert_eq!(box_question, question);
    assert!(box_lines < lines, "{box_lines:?} {lines:?}");
    wheel(cx, "approval-question");
    let (rows_question, rows_lines) = tops(cx);
    assert!(rows_question < box_question, "the rows scroll");
    assert_eq!(rows_question - box_question, rows_lines - box_lines);
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
    // A new skill's SKILL.md shows once, in the files' box.
    assert!(app.shows(cx, part("approval-box", "a1")));
    assert!(!app.shows(cx, part("approval-diff", "a1")));
}

/// An update of a skill says so in its row, and its details show SKILL.md's
/// changed lines in a box of their own, above the files and their box.
#[gpui_kit::test]
fn an_update_shows_the_lines_it_changes(cx: &mut TestAppContext) {
    let app = start(cx);
    app.request(cx, vec![update("a1")]);
    assert_eq!(
        app.label(cx, part("approval-row", "a1")).as_deref(),
        Some("Update, release-notes, this project, changes SKILL.md and 2 more files")
    );
    app.click(cx, part("approval-row", "a1"));
    let top = |app: &Window, cx: &mut TestAppContext, id| {
        cx.update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(part(id, "a1")).bounds().top()
        })
        .unwrap()
    };
    let diff = top(&app, cx, "approval-diff");
    let files = top(&app, cx, "approval-box");
    assert!(diff < files, "{diff:?} {files:?}");
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
