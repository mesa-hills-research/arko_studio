//! The agent's questions in the window: the answer panel above the message
//! box, the chips that stand for it, Alt+Q, and the question cards in the
//! transcript. A session that hasn't opened keeps what is sent in its queue.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arko::questions::{Answer, Answers, QuestionAnswer};
use arko::remote::{Activity, Reading};
use gpui_kit::Focusable as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::live::LiveSession;
use crate::questions::asked;
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

/// Tabs or spaces (one choice, with a default), which tests (several), and
/// which branch (no choices).
fn three() -> Vec<arko::questions::AgentQuestion> {
    vec![
        asked(serde_json::json!({
            "id": "q1", "question": "Tabs or spaces?", "choices": ["Tabs", "Spaces"],
            "default": "Spaces", "state": "open", "item": 7
        })),
        asked(serde_json::json!({
            "id": "q2", "question": "Which tests should I run?",
            "choices": ["Unit", "Integration", "Screenshots"], "multiple": true,
            "state": "open", "item": 7
        })),
        asked(serde_json::json!({
            "id": "q3", "question": "Which branch?", "default": "main", "state": "open", "item": 7
        })),
    ]
}

impl Window {
    /// The engine's status lists `questions` as open.
    fn ask(&self, cx: &mut TestAppContext, questions: Vec<arko::questions::AgentQuestion>) {
        let reading = Reading {
            questions,
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

    fn label(&self, cx: &mut TestAppContext, id: &'static str) -> Option<String> {
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).label().map(str::to_string)
        })
        .unwrap()
    }

    fn type_text(&self, cx: &mut TestAppContext, text: &str) {
        act(cx, self.window, |window, cx| window.input(text, cx));
    }

    fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
        let id = id.into();
        act(cx, self.window, |window, cx| window.click(id, cx));
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        act(cx, self.window, |window, cx| window.press(key, cx));
    }

    /// Gives the keyboard to the panel's field `index` and types `text`.
    fn fill(&self, cx: &mut TestAppContext, index: usize, text: &str) {
        let panel = self.panel(cx);
        act(cx, self.window, |window, cx| {
            panel.update(cx, |panel, cx| panel.focus_field(index, window, cx))
        });
        self.type_text(cx, text);
    }

    fn panel(&self, cx: &mut TestAppContext) -> Entity<crate::questions::AnswerPanel> {
        cx.read(|cx| {
            self.workspace
                .read(cx)
                .session_view()
                .read(cx)
                .answer_panel()
                .cloned()
        })
        .expect("the answer panel shows")
    }

    /// What the session sent (it keeps it until it opens).
    fn sent(&self, cx: &mut TestAppContext) -> Vec<arko::Input> {
        cx.read(|cx| self.session.read(cx).queued().to_vec())
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

    fn panel_focused(&self, cx: &mut TestAppContext) -> bool {
        let panel = self.panel(cx);
        cx.update_window(self.window, |_, window, cx| {
            panel.read(cx).contains_focus(window, cx)
        })
        .unwrap()
    }
}

/// Questions that come while the message box is empty, with no key for a
/// moment, open the panel by itself. It leaves the keyboard where it was.
#[gpui_kit::test]
fn questions_open_the_panel_while_the_user_isnt_typing(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    assert!(app.shows(cx, "answer-panel"));
    assert!(!app.shows(cx, "questions-chip"));
    // Numbered in their batch, without the agent's ids.
    let labels: Vec<Option<String>> = (0..3usize)
        .map(|question| {
            cx.update_window(app.window, |_, window, _| {
                window
                    .find(("answer-question", question))
                    .label()
                    .map(str::to_string)
            })
            .unwrap()
        })
        .collect();
    assert_eq!(
        labels,
        [
            Some("1. Tabs or spaces?".into()),
            Some("2. Which tests should I run?".into()),
            Some("3. Which branch?".into())
        ]
    );
    assert!(!app.panel_focused(cx));
}

/// Questions that come while the user types wait in a chip, which doesn't
/// take the keyboard, and opens the panel.
#[gpui_kit::test]
fn questions_that_come_while_typing_wait_in_a_chip(cx: &mut TestAppContext) {
    let app = start(cx);
    app.type_text(cx, "Half a thought");
    app.ask(cx, three());
    assert!(!app.shows(cx, "answer-panel"));
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("Arko asks 3 questions")
    );
    assert!(app.composer_focused(cx));
    // Typed a moment ago and emptied since: still typing.
    act(cx, app.window, |window, cx| {
        for _ in 0.."Half a thought".len() {
            window.press("backspace", cx);
        }
    });
    app.ask(
        cx,
        three()
            .into_iter()
            .chain([asked(serde_json::json!({
                "id": "q4", "question": "Push now?", "state": "open"
            }))])
            .collect(),
    );
    assert!(!app.shows(cx, "answer-panel"));
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("Arko asks 4 questions")
    );
    app.click(cx, "questions-open");
    assert!(app.shows(cx, "answer-panel"));
    assert!(!app.shows(cx, "questions-chip"));
}

/// Choices (one, or several where the question allows), "You decide",
/// "Other" and "Anything else" are sent as one answer, the choices in the
/// question's order.
#[gpui_kit::test]
fn the_panel_sends_what_is_filled_in(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    // q1: Tabs, then Spaces instead.
    app.click(cx, ("answer-choice", 0usize));
    app.click(cx, ("answer-choice", 1usize));
    // q2: Screenshots and Unit, both kept.
    app.click(cx, ("answer-choice", 16usize + 2));
    app.click(cx, ("answer-choice", 16usize));
    // q3: You decide, and q1 also says why.
    app.click(cx, ("answer-you-decide", 2usize));
    app.fill(cx, 0, "Two of them");
    app.fill(cx, 3, "Keep the old API one more release.");
    app.click(cx, "answer-send");
    let answer = |choices: &[&str], text: Option<&str>, you_decide: bool| Answer {
        choices: choices.iter().map(|choice| choice.to_string()).collect(),
        text: text.map(str::to_string),
        you_decide,
    };
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![
                QuestionAnswer::new("q1", answer(&["Spaces"], Some("Two of them"), false)),
                QuestionAnswer::new("q2", answer(&["Unit", "Screenshots"], None, false)),
                QuestionAnswer::new("q3", answer(&[], None, true)),
            ],
            anything_else: Some("Keep the old API one more release.".into()),
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
    assert!(!app.shows(cx, "questions-chip"));
}

/// Send sends what is filled in. The questions left stay open, in the
/// quiet chip, and "You decide" lets go of the choices.
#[gpui_kit::test]
fn a_partial_send_leaves_the_rest_open(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, ("answer-choice", 0usize));
    app.click(cx, ("answer-you-decide", 0usize));
    app.click(cx, "answer-send");
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![QuestionAnswer::new(
                "q1",
                Answer {
                    you_decide: true,
                    ..Answer::default()
                }
            )],
            anything_else: None,
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("2 open questions")
    );
    // The panel shows the rest.
    app.click(cx, "questions-open");
    assert!(!app.shows(cx, ("answer-question", 2usize)));
    assert!(app.shows(cx, ("answer-question", 1usize)));
}

/// "Answer in chat" closes the panel and attaches the questions to the
/// message box, and the next message answers them.
#[gpui_kit::test]
fn answer_in_chat_attaches_the_questions_to_the_next_message(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, "answer-in-chat");
    assert!(!app.shows(cx, "answer-panel"));
    assert!(app.shows(cx, "questions-replying"));
    assert!(!app.shows(cx, "questions-chip"));
    assert!(app.composer_focused(cx));
    app.type_text(cx, "Spaces, all of them, main.");
    app.press(cx, "enter");
    assert_eq!(
        app.sent(cx),
        [arko::Input::Message(arko::Message {
            text: "Spaces, all of them, main.".into(),
            answers: vec!["q1".into(), "q2".into(), "q3".into()],
            ..Default::default()
        })]
    );
    assert!(!app.shows(cx, "questions-replying"));
}

/// Esc in the panel does what "Answer in chat" does, and the keyboard goes
/// back where it was before Alt+Q.
#[gpui_kit::test]
fn escape_in_the_panel_attaches_the_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.type_text(cx, "Half a thought");
    app.ask(cx, three());
    app.press(cx, "alt-q");
    assert!(app.panel_focused(cx));
    app.press(cx, "escape");
    assert!(!app.shows(cx, "answer-panel"));
    assert!(app.shows(cx, "questions-replying"));
    assert!(app.composer_focused(cx));
    // A command doesn't answer them.
    act(cx, app.window, |window, cx| {
        for _ in 0.."Half a thought".len() {
            window.press("backspace", cx);
        }
        window.input("/cost", cx);
        window.press("enter", cx);
    });
    match app.sent(cx).as_slice() {
        [arko::Input::Message(message)] => assert!(message.answers.is_empty()),
        sent => panic!("{sent:?}"),
    }
    assert!(app.shows(cx, "questions-replying"));
}

/// × takes the questions off the message box: the next message doesn't
/// answer them, and they wait in the quiet chip.
#[gpui_kit::test]
fn the_cross_detaches_the_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, "answer-in-chat");
    app.click(cx, "questions-detach");
    assert!(!app.shows(cx, "questions-replying"));
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("3 open questions")
    );
    app.type_text(cx, "Something else first.");
    app.press(cx, "enter");
    match app.sent(cx).as_slice() {
        [arko::Input::Message(message)] => assert!(message.answers.is_empty()),
        sent => panic!("{sent:?}"),
    }
}

/// "Not now" dismisses the questions the chip stands for.
#[gpui_kit::test]
fn not_now_dismisses_the_open_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, "answer-close");
    assert!(!app.shows(cx, "answer-panel"));
    assert!(app.shows(cx, "questions-chip"));
    app.click(cx, "questions-not-now");
    assert_eq!(
        app.sent(cx),
        [arko::Input::DismissQuestions(vec![
            "q1".into(),
            "q2".into(),
            "q3".into()
        ])]
    );
    assert!(!app.shows(cx, "questions-chip"));
}

/// Alt+Q opens the panel from anywhere and gives it the keyboard. With no
/// questions open, it says so.
#[gpui_kit::test]
fn alt_q_opens_and_focuses_the_panel(cx: &mut TestAppContext) {
    let app = start(cx);
    app.press(cx, "alt-q");
    let notified = cx
        .update_window(app.window, |_, window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.notifications(cx).len()
        })
        .unwrap();
    assert_eq!(notified, 1);
    app.ask(cx, three());
    app.click(cx, "answer-close");
    // From the transcript.
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(crate::transcript::FocusTranscript), cx)
    });
    app.press(cx, "alt-q");
    assert!(app.shows(cx, "answer-panel"));
    assert!(app.panel_focused(cx));
    // Again: it keeps the keyboard where it is in the panel.
    app.press(cx, "alt-q");
    assert!(app.panel_focused(cx));
}

/// The panel follows the questions: one answered elsewhere leaves it, and
/// it closes when none is open.
#[gpui_kit::test]
fn the_panel_follows_the_open_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.ask(cx, three().into_iter().skip(1).collect());
    assert!(!app.shows(cx, ("answer-question", 2usize)));
    app.ask(cx, Vec::new());
    assert!(!app.shows(cx, "answer-panel"));
    assert!(!app.shows(cx, "questions-chip"));
    // Questions that come again later open it again.
    cx.executor().advance_clock(Duration::from_secs(3));
    app.ask(
        cx,
        vec![asked(serde_json::json!({
            "id": "q5", "question": "Push now?", "state": "open"
        }))],
    );
    assert!(app.shows(cx, "answer-panel"));
}
