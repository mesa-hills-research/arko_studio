//! The agent's questions to the user: the answer panel above the message
//! box, the words of the chips that stand for it, and what became of each
//! question as the transcript's record says it.
//!
//! The agent asks without stopping its work (unless it waits for the
//! answer), and the questions stay open until the user answers or
//! dismisses them, or the agent settles them. While a question is open only
//! the panel shows it. The panel sends what is filled in as one answer
//! ([`arko::Input::AnswerQuestions`]); the session view decides when it
//! shows (see `SessionView::sync_questions`).

use std::collections::HashMap;

use arko::questions::{AgentQuestion, Answer, Answers, QuestionAnswer, QuestionState};
use gpui_kit::base::SelectableText;
use gpui_kit::component::input::{self, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs;

/// The key context of the answer panel.
pub const PANEL_CONTEXT: &str = "AnswerPanel";

/// The tallest the panel grows, as a share of the window's height, before
/// its questions scroll.
const MAX_HEIGHT: f32 = 0.6;

gpui_kit::actions!(
    questions,
    [
        /// Opens the answer panel, or gives it the keyboard (Alt+Q).
        AnswerQuestions,
        /// Closes the answer panel and attaches the open questions to the
        /// message box, which the next message answers (Esc in the panel).
        AnswerInChat,
    ]
);

/// "Arko asks 2 questions": the panel's title, and the chip that stands for
/// questions that came while the user typed. Approvals alone say what they
/// ask.
pub fn asks_text(questions: &[AgentQuestion]) -> String {
    let approvals =
        !questions.is_empty() && questions.iter().all(|question| question.approval.is_some());
    match (questions.len(), approvals) {
        (1, true) => "Arko asks to save a worker profile".to_string(),
        (count, true) => format!("Arko asks to save {count} worker profiles"),
        (1, false) => "Arko asks a question".to_string(),
        (count, false) => format!("Arko asks {count} questions"),
    }
}

/// "2 open questions": the quiet chip of questions not attached.
pub fn open_text(count: usize) -> String {
    match count {
        1 => "1 open question".to_string(),
        count => format!("{count} open questions"),
    }
}

/// "Replying to 2 questions": the chip of the questions attached to the
/// message box.
pub fn replying_text(count: usize) -> String {
    match count {
        1 => "Replying to a question".to_string(),
        count => format!("Replying to {count} questions"),
    }
}

/// What became of a question, as the record under its card's line says
/// it: the user's answer, or a few words. `None` while it is open, when
/// only the answer panel shows it.
pub fn outcome(state: &QuestionState) -> Option<String> {
    Some(match state {
        QuestionState::Open => return None,
        QuestionState::Answered { answer } => answer.trim().to_string(),
        QuestionState::AnsweredInMessage => "Answered in your message".to_string(),
        QuestionState::Closed { why: Some(why) } => format!("Arko closed it. {}", why.trim()),
        QuestionState::Closed { why: None } => "Arko closed it".to_string(),
        QuestionState::Dismissed => "Dismissed".to_string(),
        // A state a later arko adds.
        _ => "Closed".to_string(),
    })
}

/// A question's words without its number: an approval says what it
/// approves.
pub fn text(question: &AgentQuestion) -> String {
    match &question.approval {
        Some(approval) => approval.title(),
        None => question.question.trim().to_string(),
    }
}

/// What a screen reader hears of a question card: its line ("Arko asked 2
/// questions"), then each settled question with what became of it.
pub fn card_label(line: &str, questions: &[AgentQuestion]) -> String {
    let mut label = line.to_string();
    for question in questions {
        if let Some(outcome) = outcome(&question.state) {
            label.push_str(&format!(
                ", {}, {outcome}",
                first_line(&question.numbered())
            ));
        }
    }
    label
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
}

/// The id of a part of the question `id` in the panel. It follows the
/// question, not its place, so the keyboard stays on its question when one
/// before it is answered elsewhere.
pub(crate) fn part(name: &str, id: &str, n: usize) -> ElementId {
    (SharedString::from(format!("{name}-{id}")), n).into()
}

/// What the user filled in for one question.
struct Draft {
    choices: Vec<String>,
    you_decide: bool,
    /// "Other", or the answer of a question without choices.
    other: Entity<InputState>,
    _other: Subscription,
}

/// What the panel asks of the session view.
#[derive(Debug, Clone, PartialEq)]
pub enum AnswerPanelEvent {
    /// Send what is filled in ([`AnswerPanel::answers`]).
    Send,
    /// Close it and attach the open questions to the message box.
    AnswerInChat,
    /// Close it, the questions left as they are.
    Close,
    /// Send what is filled in at once, with `choice` as the answer to
    /// question `id` (an approval's Save or Don't save).
    Choose { id: String, choice: String },
}

impl EventEmitter<AnswerPanelEvent> for AnswerPanel {}

/// The answer panel: each open question with its choices as chips, an
/// "Other" field and "You decide", one "Anything else" box, and Send. An
/// approval of a worker profile shows the profile, Save and Don't save, an
/// Other field for changes, and its instructions.
pub struct AnswerPanel {
    questions: Vec<AgentQuestion>,
    drafts: HashMap<String, Draft>,
    anything_else: Entity<TextareaState>,
    _anything_else: Subscription,
    /// The questions' list, which scrolls to a card's questions.
    scroll: ScrollHandle,
}

impl AnswerPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let send_key = prefs::get(cx).input.send_key;
        let anything_else = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 4)
                .placeholder("Anything else")
                .submit_keys(crate::composer::submit_keys(send_key))
        });
        let subscription = cx.subscribe_in(
            &anything_else,
            window,
            |this, field: &Entity<TextareaState>, event: &InputEvent, _, cx| match event {
                InputEvent::PressEnter { secondary, shift }
                    if field.read(cx).is_submit_chord(*secondary, *shift) =>
                {
                    this.send(cx)
                }
                InputEvent::Change => cx.notify(),
                _ => {}
            },
        );
        Self {
            questions: Vec::new(),
            drafts: HashMap::new(),
            anything_else,
            _anything_else: subscription,
            scroll: ScrollHandle::new(),
        }
    }

    /// Shows `questions`, keeping what was filled in for those it showed.
    pub fn set_questions(
        &mut self,
        questions: Vec<AgentQuestion>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if questions == self.questions {
            return;
        }
        self.drafts
            .retain(|id, _| questions.iter().any(|question| question.id == *id));
        for question in &questions {
            if self.drafts.contains_key(&question.id) {
                continue;
            }
            let placeholder = if question.approval.is_some() {
                "Tell Arko what to change"
            } else if question.choices.is_empty() {
                "Answer"
            } else {
                "Other"
            };
            let other = cx.new(|cx| InputState::new(window, cx).placeholder(placeholder));
            let subscription = cx.subscribe_in(
                &other,
                window,
                |this, _, event: &InputEvent, _, cx| match event {
                    InputEvent::PressEnter { .. } => this.send(cx),
                    InputEvent::Change => cx.notify(),
                    _ => {}
                },
            );
            self.drafts.insert(
                question.id.clone(),
                Draft {
                    choices: Vec::new(),
                    you_decide: false,
                    other,
                    _other: subscription,
                },
            );
        }
        self.questions = questions;
        cx.notify();
    }

    /// The questions it shows.
    pub fn questions(&self) -> &[AgentQuestion] {
        &self.questions
    }

    /// What is filled in: an answer for each question that has one, and
    /// "Anything else".
    pub fn answers(&self, cx: &App) -> Answers {
        self.answers_choosing(None, cx)
    }

    /// What is filled in, with `chosen` (a question's id and a choice) as
    /// that question's whole answer.
    pub fn answers_choosing(&self, chosen: Option<(&str, &str)>, cx: &App) -> Answers {
        let answers = self
            .questions
            .iter()
            .filter_map(|question| {
                if let Some((_, choice)) = chosen.filter(|(id, _)| *id == question.id) {
                    let answer = Answer {
                        choices: vec![choice.to_string()],
                        ..Answer::default()
                    };
                    return Some(QuestionAnswer::new(question.id.clone(), answer));
                }
                let draft = self.drafts.get(&question.id)?;
                let text = draft.other.read(cx).value().trim().to_string();
                let answer = Answer {
                    // In the question's order, whatever order they were
                    // picked in.
                    choices: question
                        .choices
                        .iter()
                        .filter(|choice| draft.choices.contains(choice))
                        .cloned()
                        .collect(),
                    text: (!text.is_empty()).then_some(text),
                    you_decide: draft.you_decide,
                };
                (!answer.is_empty()).then(|| QuestionAnswer::new(question.id.clone(), answer))
            })
            .collect();
        let anything_else = self.anything_else.read(cx).value().trim().to_string();
        Answers {
            answers,
            anything_else: (!anything_else.is_empty()).then_some(anything_else),
        }
    }

    /// Asks the session view to send what is filled in, if anything is.
    fn send(&mut self, cx: &mut Context<Self>) {
        if !self.answers(cx).is_empty() {
            cx.emit(AnswerPanelEvent::Send);
        }
    }

    /// What was sent is cleared: the drafts of the questions answered, and
    /// "Anything else".
    pub fn sent(&mut self, answered: &[String], window: &mut Window, cx: &mut Context<Self>) {
        self.drafts.retain(|id, _| !answered.contains(id));
        self.anything_else
            .update(cx, |field, cx| field.set_value("", window, cx));
        cx.notify();
    }

    /// Gives the keyboard to the first question's field.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if let Some(draft) = self
            .questions
            .first()
            .and_then(|question| self.drafts.get(&question.id))
        {
            draft.other.update(cx, |field, cx| field.focus(window, cx));
        } else {
            self.anything_else
                .update(cx, |field, cx| field.focus(window, cx));
        }
    }

    /// Scrolls to the first open question of the card `item` and gives it
    /// the keyboard, or to the first question when the card has none open.
    pub fn focus_card(&self, item: u64, window: &mut Window, cx: &mut App) {
        let Some(index) = self
            .questions
            .iter()
            .position(|question| question.item == item)
        else {
            return self.focus(window, cx);
        };
        self.scroll.scroll_to_item(index);
        if let Some(draft) = self.drafts.get(&self.questions[index].id) {
            draft.other.update(cx, |field, cx| field.focus(window, cx));
        }
    }

    /// Gives the keyboard to question `index`'s field, or to "Anything
    /// else" past the last question (tests).
    #[cfg(test)]
    pub fn focus_field(&self, index: usize, window: &mut Window, cx: &mut App) {
        match self
            .questions
            .get(index)
            .and_then(|question| self.drafts.get(&question.id))
        {
            Some(draft) => draft.other.update(cx, |field, cx| field.focus(window, cx)),
            None => self
                .anything_else
                .update(cx, |field, cx| field.focus(window, cx)),
        }
    }

    /// Whether the keyboard is in the panel.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        use gpui_kit::Focusable as _;
        self.drafts.values().any(|draft| {
            draft
                .other
                .read(cx)
                .focus_handle(cx)
                .contains_focused(window, cx)
        }) || self
            .anything_else
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
    }

    /// The question whose field has the keyboard, by its id (tests).
    #[cfg(test)]
    pub fn focused_question(&self, window: &Window, cx: &App) -> Option<String> {
        use gpui_kit::Focusable as _;
        self.drafts
            .iter()
            .find(|(_, draft)| draft.other.read(cx).focus_handle(cx).is_focused(window))
            .map(|(id, _)| id.clone())
    }

    /// Picks `choice` of the question `id`, or lets go of it: one choice,
    /// or several where the question allows.
    fn toggle_choice(&mut self, id: &str, choice: &str, cx: &mut Context<Self>) {
        let multiple = self
            .questions
            .iter()
            .find(|question| question.id == id)
            .is_some_and(|question| question.multiple);
        let Some(draft) = self.drafts.get_mut(id) else {
            return;
        };
        if let Some(at) = draft.choices.iter().position(|chosen| chosen == choice) {
            draft.choices.remove(at);
        } else {
            if !multiple {
                draft.choices.clear();
            }
            draft.choices.push(choice.to_string());
            draft.you_decide = false;
        }
        cx.notify();
    }

    fn toggle_you_decide(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(draft) = self.drafts.get_mut(id) {
            draft.you_decide = !draft.you_decide;
            if draft.you_decide {
                draft.choices.clear();
            }
            cx.notify();
        }
    }

    fn render_question(
        &self,
        index: usize,
        question: &AgentQuestion,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let draft = self.drafts.get(&question.id)?;
        let theme = cx.theme();
        let id: SharedString = question.id.clone().into();
        let approval = question.approval.as_ref();
        let chips = question.choices.iter().enumerate().map(|(n, choice)| {
            let chosen = draft.choices.contains(choice);
            let (id, choice) = (id.clone(), choice.clone());
            let button = Button::new(part("answer-choice", &id, n))
                .small()
                .label(choice.clone());
            // An approval's choices answer at once, and Save is the primary one.
            if approval.is_some() {
                let button = if choice == arko::questions::SAVE {
                    button.primary()
                } else {
                    button.outline()
                };
                return button
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(AnswerPanelEvent::Choose {
                            id: id.to_string(),
                            choice: choice.clone(),
                        })
                    }))
                    .into_any_element();
            }
            button
                .outline()
                .selected(chosen)
                .toggled(chosen)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_choice(&id, &choice, cx)))
                .into_any_element()
        });
        let you_decide = approval.is_none().then(|| {
            let id = id.clone();
            let chosen = draft.you_decide;
            let spoken = match &question.default {
                Some(default) => format!("You decide, Arko goes with {default}"),
                None => "You decide".to_string(),
            };
            Button::new(part("answer-you-decide", &id, 0))
                .small()
                .outline()
                .selected(chosen)
                .toggled(chosen)
                .accessibility_label(spoken)
                .child(h_flex().gap_1().child("You decide").when_some(
                    question.default.clone(),
                    |this, default| {
                        this.child(
                            div()
                                .text_color(theme.muted_foreground)
                                .child(format!("· {default}")),
                        )
                    },
                ))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_you_decide(&id, cx)))
        });
        // Another ask's questions start below a line.
        let new_batch = index > 0
            && self
                .questions
                .get(index - 1)
                .is_some_and(|before| before.item != question.item);
        let words = text(question);
        // The profile an approval would save, under what it asks.
        let profile = approval.map(|approval| {
            v_flex()
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(approval.runs_on()),
                )
                .child(SelectableText::new(
                    part("answer-description", &id, 0),
                    approval.description.trim().to_string(),
                ))
        });
        // Its instructions, which may be long, scroll in a box of their own.
        let instructions = approval
            .filter(|approval| !approval.instructions.trim().is_empty())
            .map(|approval| {
                v_flex()
                    .id(part("answer-instructions", &id, 0))
                    .test_support()
                    .gap_1()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Instructions"),
                    )
                    .child(
                        div()
                            .max_h(rems(10.))
                            .overflow_y_scrollbar()
                            // Each approval's box keeps its own place.
                            .id(part("answer-instructions-box", &id, 0))
                            .p_2()
                            .rounded(theme.radius)
                            .bg(theme.muted)
                            .text_sm()
                            .child(SelectableText::new(
                                part("answer-instructions-text", &id, 0),
                                approval.instructions.trim_end().to_string(),
                            )),
                    )
            });
        Some(
            v_flex()
                .id(part("answer-question", &id, 0))
                .test_support()
                .role(Role::Group)
                .aria_label(first_line(&question.numbered()).to_string())
                .gap_1p5()
                .when(new_batch, |this| {
                    this.pt_4().border_t_1().border_color(theme.border)
                })
                .child(
                    h_flex()
                        .items_baseline()
                        .gap_1p5()
                        .text_sm()
                        .when_some(question.number, |this, number| {
                            // At least a rem, so the texts line up.
                            this.child(
                                div()
                                    .flex_none()
                                    .min_w(rems(1.))
                                    .text_color(theme.muted_foreground)
                                    .child(format!("{number}.")),
                            )
                        })
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(SelectableText::new(
                                    part("answer-text", &id, 0),
                                    words.clone(),
                                ))
                                .children(profile),
                        )
                        .when(question.multiple, |this| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("Choose any"),
                            )
                        }),
                )
                .child(
                    h_flex()
                        .flex_wrap()
                        .gap_1()
                        .children(chips)
                        .children(you_decide),
                )
                .child(
                    Input::new(&draft.other)
                        .small()
                        .aria_label(format!("{}, other answer", first_line(&words))),
                )
                .children(instructions)
                .into_any_element(),
        )
    }
}

impl Render for AnswerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let tallest = window.viewport_size().height * MAX_HEIGHT;
        let questions: Vec<AnyElement> = self
            .questions
            .clone()
            .iter()
            .enumerate()
            .filter_map(|(index, question)| self.render_question(index, question, cx))
            .collect();
        let can_send = !self.answers(cx).is_empty();
        let title = asks_text(&self.questions);
        let theme = cx.theme();
        v_flex()
            .id("answer-panel")
            .test_support()
            .key_context(PANEL_CONTEXT)
            .occlude()
            .on_action(
                cx.listener(|_, _: &AnswerInChat, _, cx| cx.emit(AnswerPanelEvent::AnswerInChat)),
            )
            // Esc in a field the field has no use for.
            .on_action(
                cx.listener(|_, _: &input::Escape, _, cx| cx.emit(AnswerPanelEvent::AnswerInChat)),
            )
            .max_h(tallest)
            .w_full()
            .p_3()
            .gap_3()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_md()
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .child(
                        div()
                            .id("answer-panel-title")
                            .test_support()
                            .role(Role::Heading)
                            .aria_label(title.clone())
                            .flex_1()
                            .text_sm()
                            .font_medium()
                            .child(title),
                    )
                    .child(
                        Button::new("answer-close")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(IconName::Close)
                            .tooltip("Close")
                            .accessibility_label("Close")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AnswerPanelEvent::Close))),
                    ),
            )
            .child(
                v_flex()
                    .id("answer-questions")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .gap_4()
                    .children(questions)
                    .child(
                        Textarea::new(&self.anything_else)
                            .small()
                            .aria_label("Anything else"),
                    ),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("answer-in-chat")
                            .ghost()
                            .small()
                            .label("Answer in chat")
                            .tooltip_with_action(
                                "Answer in chat",
                                &AnswerInChat,
                                Some(PANEL_CONTEXT),
                            )
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(AnswerPanelEvent::AnswerInChat)),
                            ),
                    )
                    .child(
                        Button::new("answer-send")
                            .primary()
                            .small()
                            .label("Send")
                            .disabled(!can_send)
                            .on_click(cx.listener(|this, _, _, cx| this.send(cx))),
                    ),
            )
    }
}

/// A question as the status gives it (tests).
#[cfg(test)]
pub(crate) fn question(id: &str, text: &str) -> AgentQuestion {
    asked(serde_json::json!({ "id": id, "question": text, "state": "open" }))
}

/// A question from its JSON, as arko's API gives it (tests).
#[cfg(test)]
pub(crate) fn asked(json: serde_json::Value) -> AgentQuestion {
    serde_json::from_value(json).unwrap()
}

/// The main agent asks `questions` (ids and texts) on `feed`, waiting for
/// the answers when `waits` (tests). Returns the card's item.
#[cfg(test)]
pub(crate) fn ask(feed: &arko::remote::Feed, questions: &[(&str, &str)], waits: bool) -> u64 {
    let asked: Vec<arko::questions::AskedQuestion> = questions
        .iter()
        .map(|(id, text)| arko::questions::AskedQuestion {
            id: id.to_string(),
            question: arko_agents::questions::Question {
                question: text.to_string(),
                ..Default::default()
            },
        })
        .collect();
    feed.ask_questions(&asked, waits)
}

/// The user answers the question `id` with `text` in the answer panel, as
/// the feed shows it (tests).
#[cfg(test)]
pub(crate) fn answer(feed: &arko::remote::Feed, id: &str, text: &str) {
    feed.set_question_state(
        id,
        QuestionState::Answered {
            answer: text.to_string(),
        },
    );
    feed.user(
        text,
        arko::remote::Source::App,
        arko::remote::Delivery::Answer,
    );
}

#[cfg(test)]
mod tests {
    use super::{asked, asks_text, card_label, open_text, outcome, replying_text};
    use arko::questions::QuestionState;

    /// An approval of a worker profile, as Arko asks it for the agent.
    fn approval(id: &str, name: &str, state: &str) -> arko::questions::AgentQuestion {
        asked(serde_json::json!({
            "id": id, "question": format!("Save {name}?"), "choices": ["Save", "Don't save"],
            "state": state, "waits": true, "item": 9,
            "approval": {"name": name, "model": "sol", "scope": "every project",
                         "description": "Judges 3D results"}
        }))
    }

    #[test]
    fn the_chips_count_the_questions() {
        let question = asked(serde_json::json!({ "id": "q1", "question": "?", "state": "open" }));
        assert_eq!(
            asks_text(std::slice::from_ref(&question)),
            "Arko asks a question"
        );
        assert_eq!(
            asks_text(&[question.clone(), question.clone()]),
            "Arko asks 2 questions"
        );
        let saves = [
            approval("x1", "3d-judge", "open"),
            approval("x2", "linter", "open"),
        ];
        assert_eq!(asks_text(&saves[..1]), "Arko asks to save a worker profile");
        assert_eq!(asks_text(&saves), "Arko asks to save 2 worker profiles");
        assert_eq!(
            asks_text(&[saves[0].clone(), question]),
            "Arko asks 2 questions"
        );
        assert_eq!(open_text(1), "1 open question");
        assert_eq!(open_text(3), "3 open questions");
        assert_eq!(replying_text(1), "Replying to a question");
        assert_eq!(replying_text(2), "Replying to 2 questions");
    }

    #[test]
    fn each_outcome_reads_in_a_few_words() {
        assert_eq!(outcome(&QuestionState::Open), None);
        assert_eq!(
            outcome(&QuestionState::Answered {
                answer: "You decide".into()
            })
            .as_deref(),
            Some("You decide")
        );
        assert_eq!(
            outcome(&QuestionState::AnsweredInMessage).as_deref(),
            Some("Answered in your message")
        );
        assert_eq!(
            outcome(&QuestionState::Closed {
                why: Some("Found it in the README".into())
            })
            .as_deref(),
            Some("Arko closed it. Found it in the README")
        );
        assert_eq!(
            outcome(&QuestionState::Dismissed).as_deref(),
            Some("Dismissed")
        );
    }

    /// A screen reader hears a card's line, then each settled question
    /// with its number in its ask and what became of it. The open ones are
    /// the panel's, and the agent's ids are never heard.
    #[test]
    fn a_card_is_read_with_each_settled_question() {
        let questions = [
            asked(serde_json::json!({
                "id": "q3", "question": "Tabs or spaces?", "state": "answered", "answer": "Spaces",
                "number": 1
            })),
            asked(serde_json::json!({
                "id": "q4", "question": "Push now?", "state": "open", "number": 2
            })),
            asked(serde_json::json!({
                "id": "q5", "question": "Rename the module?", "state": "dismissed", "number": 3
            })),
        ];
        let label = card_label("Arko asked 3 questions", &questions);
        assert_eq!(
            label,
            "Arko asked 3 questions, 1. Tabs or spaces?, Spaces, 3. Rename the module?, Dismissed"
        );
        assert!(!label.contains("q3"));
        let open = approval("x1a2b3c4d", "3d-judge", "open");
        assert_eq!(
            card_label("Arko asked to save a worker profile", &[open]),
            "Arko asked to save a worker profile"
        );
        let mut saved = approval("x1a2b3c4d", "3d-judge", "open");
        saved.state = QuestionState::Answered {
            answer: "Save".into(),
        };
        assert_eq!(
            card_label("Arko asked to save a worker profile", &[saved]),
            "Arko asked to save a worker profile, Save the worker profile 3d-judge for every project?, Save"
        );
    }
}
