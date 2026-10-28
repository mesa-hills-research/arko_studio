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
use crate::questions::{asked, part};
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
            "default": "Spaces", "state": "open", "item": 7, "number": 1
        })),
        asked(serde_json::json!({
            "id": "q2", "question": "Which tests should I run?",
            "choices": ["Unit", "Integration", "Screenshots"], "multiple": true,
            "state": "open", "item": 7, "number": 2
        })),
        asked(serde_json::json!({
            "id": "q3", "question": "Which branch?", "default": "main", "state": "open", "item": 7,
            "number": 3
        })),
    ]
}

/// Arko asks to approve the worker profile `name`, as question `id`, the
/// `number`th of its card (`None` alone).
fn approval(id: &str, name: &str, number: Option<u32>) -> arko::questions::AgentQuestion {
    asked(serde_json::json!({
        "id": id,
        "question": format!("The main agent wants to save the worker profile {name}."),
        "choices": ["Save", "Don't save"], "state": "open", "waits": true, "item": 9,
        "number": number,
        "approval": {
            "name": name, "model": "sol", "effort": "high", "scope": "every project",
            "description": "Judges 3D results against a reference image",
            "instructions": "You judge 3D models.\nCompare them with the reference."
        }
    }))
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

    fn label(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) -> Option<String> {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(id).label().map(str::to_string)
        })
        .unwrap()
    }

    /// What the panel's questions are heard as.
    fn question_labels(&self, cx: &mut TestAppContext) -> Vec<String> {
        let ids: Vec<String> = self.panel(cx).read_with(cx, |panel, _| {
            panel.questions().iter().map(|q| q.id.clone()).collect()
        });
        ids.iter()
            .filter_map(|id| self.label(cx, part("answer-question", id, 0)))
            .collect()
    }

    /// The question whose field has the keyboard, by its id.
    fn focused_question(&self, cx: &mut TestAppContext) -> Option<String> {
        let panel = self.panel(cx);
        cx.update_window(self.window, |_, window, cx| {
            panel.read(cx).focused_question(window, cx)
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
    let labels: Vec<Option<String>> = ["q1", "q2", "q3"]
        .into_iter()
        .map(|id| app.label(cx, part("answer-question", id, 0)))
        .collect();
    assert_eq!(
        labels,
        [
            Some("1. Tabs or spaces?".into()),
            Some("2. Which tests should I run?, choose any".into()),
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

/// Choices (one, or several where the question allows), "You decide" and
/// "Other" are sent as one answer, the choices in the question's order.
#[gpui_kit::test]
fn the_panel_sends_what_is_filled_in(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    // q1: Tabs, then Spaces instead.
    app.click(cx, part("answer-choice", "q1", 0));
    app.click(cx, part("answer-choice", "q1", 1));
    // q2: Screenshots and Unit, both kept.
    app.click(cx, part("answer-choice", "q2", 2));
    app.click(cx, part("answer-choice", "q2", 0));
    // q3: You decide, and q1 also says why.
    app.click(cx, part("answer-you-decide", "q3", 0));
    app.fill(cx, 0, "Two of them");
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
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
    assert!(!app.shows(cx, "questions-chip"));
}

/// Send sends what is filled in, and names the questions left unanswered,
/// which stay open in the quiet chip. "You decide" lets go of the choices.
#[gpui_kit::test]
fn a_partial_send_leaves_the_rest_open(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, part("answer-choice", "q1", 0));
    app.click(cx, part("answer-you-decide", "q1", 0));
    app.click(cx, "answer-send");
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![
                QuestionAnswer::new(
                    "q1",
                    Answer {
                        you_decide: true,
                        ..Answer::default()
                    }
                ),
                QuestionAnswer::new("q2", Answer::default()),
                QuestionAnswer::new("q3", Answer::default()),
            ],
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("2 open questions")
    );
    // The panel shows the rest.
    app.click(cx, "questions-open");
    assert!(!app.shows(cx, part("answer-question", "q1", 0)));
    assert!(app.shows(cx, part("answer-question", "q2", 0)));
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
/// to the message box.
#[gpui_kit::test]
fn escape_in_the_panel_attaches_the_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.type_text(cx, "Half a thought");
    app.ask(cx, three());
    // From the transcript.
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(crate::transcript::FocusTranscript), cx)
    });
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

/// A click beside a field takes the keyboard out of it: on the panel's
/// empty space, on a question's words, or on a choice. The keyboard stays
/// in the panel, whose keys still work, so Esc answers in chat.
#[gpui_kit::test]
fn a_click_beside_a_field_takes_the_keyboard_out_of_it(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    let left = |app: &Window, cx: &mut TestAppContext| {
        app.focused_question(cx).is_none() && app.panel_focused(cx)
    };
    app.fill(cx, 0, "Both");
    assert_eq!(app.focused_question(cx).as_deref(), Some("q1"));
    app.click(cx, "answer-panel-title");
    assert!(left(&app, cx));
    // The words of the first question, at their start.
    app.fill(cx, 0, "");
    act(cx, app.window, |window, cx| {
        window.click_at(
            part("answer-question", "q1", 0),
            gpui_kit::point(gpui_kit::px(24.), gpui_kit::px(6.)),
            cx,
        )
    });
    assert!(left(&app, cx));
    app.fill(cx, 2, "");
    app.click(cx, part("answer-choice", "q1", 1));
    assert!(left(&app, cx));
    app.fill(cx, 2, "");
    app.click(cx, part("answer-you-decide", "q2", 0));
    assert!(left(&app, cx));
    // What was typed and chosen stays.
    let panel = app.panel(cx);
    let answers = cx.read(|cx| panel.read(cx).answers(cx));
    assert_eq!(answers.answers[0].answer.text.as_deref(), Some("Both"));
    assert_eq!(answers.answers[0].answer.choices, ["Spaces"]);
    assert!(answers.answers[1].answer.you_decide);
    app.press(cx, "escape");
    assert!(!app.shows(cx, "answer-panel"));
    assert!(app.shows(cx, "questions-replying"));
    assert!(app.composer_focused(cx));
}

/// A choice pressed with a key keeps the keyboard on it.
#[gpui_kit::test]
fn a_choice_pressed_with_a_key_keeps_the_keyboard(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.fill(cx, 0, "");
    // "You decide" comes before the field.
    act(cx, app.window, |window, cx| window.focus_prev(cx));
    app.press(cx, "space");
    let panel = app.panel(cx);
    let answers = cx.read(|cx| panel.read(cx).answers(cx));
    assert!(answers.answers[0].answer.you_decide);
    let focused = cx
        .update_window(app.window, |_, window, cx| {
            window.render_frame(cx);
            window.find(part("answer-you-decide", "q1", 0)).focused()
        })
        .unwrap();
    assert_eq!(focused, Some(true));
}

/// The panel follows the questions: one answered elsewhere leaves it, and
/// it closes when none is open.
#[gpui_kit::test]
fn the_panel_follows_the_open_questions(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.ask(cx, three().into_iter().skip(1).collect());
    assert!(!app.shows(cx, part("answer-question", "q1", 0)));
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

/// The panel numbers each ask's questions from 1, as arko does, whatever
/// the agent's ids: a question with id q7 in a later conversation is 1.
/// A question asked alone has no number.
#[gpui_kit::test]
fn the_panel_numbers_questions_in_their_ask_without_ids(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(
        cx,
        vec![
            asked(serde_json::json!({
                "id": "q7", "question": "Tabs or spaces?", "state": "open", "item": 40, "number": 1
            })),
            asked(serde_json::json!({
                "id": "q8", "question": "Push now?", "state": "open", "item": 40, "number": 2
            })),
            asked(serde_json::json!({
                "id": "q9", "question": "Which branch?", "state": "open", "item": 44
            })),
        ],
    );
    assert_eq!(
        app.question_labels(cx),
        ["1. Tabs or spaces?", "2. Push now?", "Which branch?"]
    );
}

/// "Not now" dismisses the questions its chip shows, and only those: the
/// ones attached to the message box stay.
#[gpui_kit::test]
fn not_now_dismisses_only_the_questions_it_shows(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, "answer-in-chat");
    app.type_text(cx, "Spaces");
    let mut four = three();
    four.push(asked(serde_json::json!({
        "id": "q4", "question": "Push now?", "state": "open", "item": 8
    })));
    app.ask(cx, four);
    assert_eq!(
        app.label(cx, "questions-open").as_deref(),
        Some("Arko asks a question")
    );
    app.click(cx, "questions-not-now");
    assert_eq!(
        app.sent(cx),
        [arko::Input::DismissQuestions(vec!["q4".into()])]
    );
    assert!(app.shows(cx, "questions-replying"));
}

/// An approval shows what it would save, Save and Don't save without "You
/// decide", and its instructions. Save sends Save at once.
#[gpui_kit::test]
fn an_approval_is_saved_with_save(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, vec![approval("x1a2b3c4d", "3d-judge", None)]);
    assert!(app.shows(cx, "answer-panel"));
    assert_eq!(
        app.question_labels(cx),
        ["Save the worker profile 3d-judge for every project?"]
    );
    assert!(app.shows(cx, part("answer-instructions", "x1a2b3c4d", 0)));
    assert!(!app.shows(cx, part("answer-you-decide", "x1a2b3c4d", 0)));
    app.click(cx, part("answer-choice", "x1a2b3c4d", 0));
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![QuestionAnswer::new(
                "x1a2b3c4d",
                Answer {
                    choices: vec!["Save".into()],
                    ..Answer::default()
                }
            )],
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
}

/// The question Arko asks before a worker starts on a model billed per
/// token is a plain question with its three choices. The choice picked is
/// its answer, as for the agent's own questions.
#[gpui_kit::test]
fn a_worker_billed_per_token_is_allowed_with_a_choice(cx: &mut TestAppContext) {
    let app = start(cx);
    let question = "Start a worker on Opus 5.5, billed per token? It costs $5 per million input tokens and $25 per million output tokens. The task is “Review the parser.”";
    let choices = ["Allow", "Allow Opus 5.5 for this session", "Deny"];
    let id = "x5e6f7a8b";
    app.ask(
        cx,
        vec![asked(serde_json::json!({
            "id": id, "question": question, "choices": choices,
            "state": "open", "waits": true, "item": 12
        }))],
    );
    assert!(app.shows(cx, "answer-panel"));
    assert_eq!(
        app.label(cx, "answer-panel-title").as_deref(),
        Some("Arko asks a question")
    );
    assert_eq!(app.question_labels(cx), [question]);
    for (n, choice) in choices.into_iter().enumerate() {
        assert_eq!(
            app.label(cx, part("answer-choice", id, n)).as_deref(),
            Some(choice)
        );
    }
    assert!(!app.shows(cx, part("answer-choice", id, 3)));
    app.click(cx, part("answer-choice", id, 1));
    app.click(cx, "answer-send");
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![QuestionAnswer::new(
                id,
                Answer {
                    choices: vec!["Allow Opus 5.5 for this session".into()],
                    ..Answer::default()
                }
            )],
        })]
    );
    assert!(!app.shows(cx, "answer-panel"));
}

/// Words typed for an approval go back to the agent as changes to make.
#[gpui_kit::test]
fn typed_words_ask_for_changes_to_an_approval(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, vec![approval("x1a2b3c4d", "3d-judge", None)]);
    app.fill(cx, 0, "Run it on opus instead");
    app.press(cx, "enter");
    assert_eq!(
        app.sent(cx),
        [arko::Input::AnswerQuestions(Answers {
            answers: vec![QuestionAnswer::new(
                "x1a2b3c4d",
                Answer {
                    text: Some("Run it on opus instead".into()),
                    ..Answer::default()
                }
            )],
        })]
    );
}

/// Two approvals asked together share a card, numbered, and each takes
/// its own answer. Each answer names the other too, and the panel stays
/// for the one left.
#[gpui_kit::test]
fn two_approvals_in_one_card_take_their_own_answers(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(
        cx,
        vec![
            approval("x00000001", "3d-judge", Some(1)),
            approval("x00000002", "linter", Some(2)),
        ],
    );
    assert_eq!(
        app.label(cx, "answer-panel-title").as_deref(),
        Some("Arko asks to save 2 worker profiles")
    );
    assert_eq!(
        app.question_labels(cx),
        [
            "1. Save the worker profile 3d-judge for every project?",
            "2. Save the worker profile linter for every project?"
        ]
    );
    // The second's Save.
    app.click(cx, part("answer-choice", "x00000002", 0));
    assert!(app.shows(cx, "answer-panel"));
    assert_eq!(
        app.question_labels(cx),
        ["1. Save the worker profile 3d-judge for every project?"]
    );
    // The first's Don't save.
    app.click(cx, part("answer-choice", "x00000001", 1));
    let choice = |id: &str, choice: &str| {
        QuestionAnswer::new(
            id,
            Answer {
                choices: vec![choice.into()],
                ..Answer::default()
            },
        )
    };
    // Each names every question the panel showed.
    assert_eq!(
        app.sent(cx),
        [
            arko::Input::AnswerQuestions(Answers {
                answers: vec![
                    QuestionAnswer::new("x00000001", Answer::default()),
                    choice("x00000002", "Save"),
                ],
            }),
            arko::Input::AnswerQuestions(Answers {
                answers: vec![choice("x00000001", "Don't save")],
            }),
        ]
    );
    assert!(!app.shows(cx, "answer-panel"));
}

/// What is filled in for a session's questions waits while the window
/// shows another session, and is sent when the user comes back to it.
#[gpui_kit::test]
fn the_panel_keeps_what_is_filled_in_for_each_session(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, part("answer-choice", "q1", 1));
    app.fill(cx, 2, "main");
    let other = cx.new(|cx| LiveSession::for_tests("s2", cx));
    let show = |session: &Entity<LiveSession>, cx: &mut TestAppContext| {
        act(cx, app.window, |window, cx| {
            app.workspace.update(cx, |workspace, cx| {
                workspace.show_session(session.clone(), window, cx)
            })
        });
    };
    show(&other, cx);
    assert!(!app.shows(cx, "answer-panel"));
    show(&app.session.clone(), cx);
    app.click(cx, "questions-open");
    app.click(cx, "answer-send");
    let sent = app.sent(cx);
    let [arko::Input::AnswerQuestions(answers)] = sent.as_slice() else {
        panic!("{sent:?}");
    };
    assert_eq!(
        answers.answers,
        [
            QuestionAnswer::new(
                "q1",
                Answer {
                    choices: vec!["Spaces".into()],
                    ..Answer::default()
                }
            ),
            QuestionAnswer::new("q2", Answer::default()),
            QuestionAnswer::new(
                "q3",
                Answer {
                    text: Some("main".into()),
                    ..Answer::default()
                }
            ),
        ]
    );
}

/// A card's line in the transcript opens the answer panel on that card's
/// questions, with the keyboard in its first.
#[gpui_kit::test]
fn a_cards_line_opens_the_panel_on_that_card(cx: &mut TestAppContext) {
    let app = start(cx);
    app.type_text(cx, "About the split");
    let mut four = three();
    four.push(asked(serde_json::json!({
        "id": "q4", "question": "Push now?", "state": "open", "item": 8
    })));
    app.ask(cx, four);
    assert!(!app.shows(cx, "answer-panel"));
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        view.update(cx, |view, cx| view.open_panel(Some(8), window, cx))
    });
    assert!(app.shows(cx, "answer-panel"));
    assert_eq!(app.focused_question(cx).as_deref(), Some("q4"));
    act(cx, app.window, |window, cx| {
        view.update(cx, |view, cx| view.open_panel(Some(7), window, cx))
    });
    assert_eq!(app.focused_question(cx).as_deref(), Some("q1"));
}

/// Words typed for an approval ask for changes, so Save waits until they
/// are cleared.
#[gpui_kit::test]
fn save_waits_while_changes_are_typed(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, vec![approval("x1a2b3c4d", "3d-judge", None)]);
    app.fill(cx, 0, "Use opus");
    app.click(cx, part("answer-choice", "x1a2b3c4d", 0));
    assert!(app.sent(cx).is_empty());
    act(cx, app.window, |window, cx| {
        for _ in 0.."Use opus".len() {
            window.press("backspace", cx);
        }
    });
    app.click(cx, part("answer-choice", "x1a2b3c4d", 0));
    assert_eq!(app.sent(cx).len(), 1);
}

/// When the question being typed in is answered elsewhere, the panel
/// closes and says so, and the typing doesn't move to another question.
/// Another question leaving changes nothing for it.
#[gpui_kit::test]
fn the_question_typed_in_leaving_closes_the_panel(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.fill(cx, 2, "ma");
    // q1 answered on the web page.
    app.ask(cx, three().into_iter().skip(1).collect());
    assert!(app.shows(cx, "answer-panel"));
    assert_eq!(app.focused_question(cx).as_deref(), Some("q3"));
    // Then q3, the one typed in.
    app.ask(cx, three().into_iter().skip(1).take(1).collect());
    assert!(!app.shows(cx, "answer-panel"));
    let notified = cx
        .update_window(app.window, |_, window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.notifications(cx).len()
        })
        .unwrap();
    assert_eq!(notified, 1);
    assert!(app.composer_focused(cx));
    assert!(app.sent(cx).is_empty());
}

/// "Choose any" follows a question that takes several of its choices, and
/// only such a question.
#[gpui_kit::test]
fn choose_any_follows_a_question_that_takes_several(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(
        cx,
        vec![
            asked(serde_json::json!({
                "id": "q1", "question": "Which tests?", "choices": ["Unit", "Screenshots"],
                "multiple": true, "state": "open", "item": 7, "number": 1
            })),
            asked(serde_json::json!({
                "id": "q2", "question": "Which files?", "multiple": true, "state": "open",
                "item": 7, "number": 2
            })),
        ],
    );
    assert_eq!(
        app.question_labels(cx),
        ["1. Which tests?, choose any", "2. Which files?"]
    );
}

impl Window {
    fn notifications(&self, cx: &mut TestAppContext) -> usize {
        cx.update_window(self.window, |_, window, cx| {
            use gpui_kit::component::WindowExt as _;
            window.notifications(cx).len()
        })
        .unwrap()
    }
}

/// Answers too long to reach the agent in one note aren't sent. The panel
/// says so and keeps them, to be shortened.
#[gpui_kit::test]
fn answers_too_long_to_send_stay_in_the_panel(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    let long = "word ".repeat(3500);
    let panel = app.panel(cx);
    act(cx, app.window, |window, cx| {
        panel.update(cx, |panel, cx| panel.set_field(0, &long, window, cx))
    });
    app.click(cx, "answer-send");
    assert!(app.sent(cx).is_empty());
    assert_eq!(app.notifications(cx), 1);
    assert!(app.shows(cx, "answer-panel"));
    let kept = app.panel(cx).read_with(cx, |panel, cx| panel.answers(cx));
    assert_eq!(kept.answers[0].answer.text.as_deref(), Some(long.trim()));
}

/// A message too long to send as the answer to the questions attached
/// isn't sent. It stays in the message box with the questions attached.
#[gpui_kit::test]
fn a_reply_too_long_to_send_stays_in_the_message_box(cx: &mut TestAppContext) {
    let app = start(cx);
    app.ask(cx, three());
    app.click(cx, "answer-in-chat");
    let long = "word ".repeat(3500);
    let view = cx.read(|cx| app.workspace.read(cx).session_view().clone());
    act(cx, app.window, |window, cx| {
        let composer = view.read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.set_text(&long, window, cx));
    });
    app.press(cx, "enter");
    assert!(app.sent(cx).is_empty());
    assert_eq!(app.notifications(cx), 1);
    assert!(app.shows(cx, "questions-replying"));
    let text = cx.read(|cx| view.read(cx).composer().read(cx).text(cx));
    assert_eq!(text.trim(), long.trim());
}
