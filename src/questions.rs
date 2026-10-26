//! The agent's questions to the user: the answer panel above the message
//! box, the words of the chips that stand for it, and what each question's
//! state reads as in the transcript's card.
//!
//! The agent asks without stopping its work (unless it waits for the
//! answer), and the questions stay open until the user answers or
//! dismisses them, or the agent settles them. The panel sends what is
//! filled in as one answer ([`arko::Input::AnswerQuestions`]); the session
//! view decides when it shows (see `SessionView::sync_questions`).

use std::collections::HashMap;

use arko::questions::{AgentQuestion, Answer, Answers, QuestionAnswer, QuestionState};
use gpui_kit::component::input::{self, Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
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
/// questions that came while the user typed.
pub fn asks_text(count: usize) -> String {
    match count {
        1 => "Arko asks a question".to_string(),
        count => format!("Arko asks {count} questions"),
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

/// What a question's state reads as in its card: a word or two, and what
/// it came to (the answer, or why the agent settled it).
pub fn state_words(state: &QuestionState) -> (&'static str, Option<String>) {
    match state {
        QuestionState::Open => ("Open", None),
        QuestionState::Answered { answer } => ("Answered", Some(answer.clone())),
        QuestionState::AnsweredInMessage => ("Answered in a message", None),
        QuestionState::Closed { why } => ("Settled by Arko", why.clone()),
        QuestionState::Dismissed => ("Dismissed", None),
        // A state a later arko adds.
        _ => ("Closed", None),
    }
}

/// The number a question shows with: its place in the questions asked
/// together (`batch` of them), from 1, and none for a question asked alone.
/// The agent's ids (`q7`) stay out of sight.
pub fn number(place: usize, batch: usize) -> Option<usize> {
    (batch > 1).then_some(place + 1)
}

/// The numbers of `questions` (all of a conversation's, in the order
/// asked) by their ids: each one's place among those of its card.
pub fn numbers(questions: &[AgentQuestion]) -> HashMap<String, usize> {
    let mut batches: HashMap<u64, Vec<&str>> = HashMap::new();
    for question in questions.iter().filter(|question| question.item != 0) {
        batches.entry(question.item).or_default().push(&question.id);
    }
    batches
        .values()
        .flat_map(|ids| {
            ids.iter()
                .enumerate()
                .filter_map(|(place, id)| Some((id.to_string(), number(place, ids.len())?)))
        })
        .collect()
}

/// What a screen reader hears of a question card.
pub fn card_label(questions: &[AgentQuestion]) -> String {
    let mut label = String::from("Arko asks");
    for (place, question) in questions.iter().enumerate() {
        label.push_str(", ");
        if let Some(number) = number(place, questions.len()) {
            label.push_str(&format!("{number}. "));
        }
        label.push_str(first_line(&question.question));
        let (state, detail) = state_words(&question.state);
        label.push_str(", ");
        // In the sentence: "answered", "settled by Arko".
        let mut state = state.chars();
        label.extend(state.next().map(|first| first.to_ascii_lowercase()));
        label.push_str(state.as_str());
        if let Some(detail) = detail {
            label.push(' ');
            label.push_str(&detail);
        }
    }
    label
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
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
}

impl EventEmitter<AnswerPanelEvent> for AnswerPanel {}

/// The answer panel: each open question with its choices as chips, an
/// "Other" field and "You decide", one "Anything else" box, and Send.
pub struct AnswerPanel {
    questions: Vec<AgentQuestion>,
    /// Each question's number in its batch, by id ([`numbers`]).
    numbers: HashMap<String, usize>,
    drafts: HashMap<String, Draft>,
    anything_else: Entity<TextareaState>,
    _anything_else: Subscription,
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
            numbers: HashMap::new(),
            drafts: HashMap::new(),
            anything_else,
            _anything_else: subscription,
        }
    }

    /// Shows `questions`, numbered by `numbers`, keeping what was filled in
    /// for those it showed.
    pub fn set_questions(
        &mut self,
        questions: Vec<AgentQuestion>,
        numbers: HashMap<String, usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if questions == self.questions && numbers == self.numbers {
            return;
        }
        self.numbers = numbers;
        self.drafts
            .retain(|id, _| questions.iter().any(|question| question.id == *id));
        for question in &questions {
            if self.drafts.contains_key(&question.id) {
                continue;
            }
            let placeholder = if question.choices.is_empty() {
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
        let answers = self
            .questions
            .iter()
            .filter_map(|question| {
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
        let chips = question.choices.iter().enumerate().map(|(n, choice)| {
            let chosen = draft.choices.contains(choice);
            let (id, choice) = (id.clone(), choice.clone());
            Button::new(("answer-choice", index * 16 + n))
                .small()
                .outline()
                .label(choice.clone())
                .selected(chosen)
                .toggled(chosen)
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_choice(&id, &choice, cx)))
                .into_any_element()
        });
        let you_decide = {
            let id = id.clone();
            let chosen = draft.you_decide;
            let spoken = match &question.default {
                Some(default) => format!("You decide, Arko goes with {default}"),
                None => "You decide".to_string(),
            };
            Button::new(("answer-you-decide", index))
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
        };
        // Another batch of questions starts below a line.
        let new_batch = index > 0
            && self
                .questions
                .get(index - 1)
                .is_some_and(|before| before.item != question.item);
        let number = self.numbers.get(&question.id).copied();
        let spoken = match number {
            Some(number) => format!("{number}. {}", first_line(&question.question)),
            None => first_line(&question.question).to_string(),
        };
        Some(
            v_flex()
                .id(("answer-question", index))
                .test_support()
                .role(Role::Group)
                .aria_label(spoken)
                .gap_1p5()
                .when(new_batch, |this| {
                    this.pt_4().border_t_1().border_color(theme.border)
                })
                .child(
                    h_flex()
                        .items_baseline()
                        .gap_1p5()
                        .text_sm()
                        .when_some(number, |this, number| {
                            this.child(
                                div()
                                    .flex_none()
                                    .text_color(theme.muted_foreground)
                                    .child(number.to_string()),
                            )
                        })
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(question.question.trim().to_string()),
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
                        .child(you_decide),
                )
                .child(
                    Input::new(&draft.other)
                        .small()
                        .aria_label(format!("{}, other answer", first_line(&question.question))),
                )
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
                            .flex_1()
                            .text_sm()
                            .font_medium()
                            .child(asks_text(self.questions.len())),
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
    use super::{asked, asks_text, card_label, numbers, open_text, replying_text, state_words};
    use arko::questions::QuestionState;

    /// Questions are numbered in their batch, from 1, and a question asked
    /// alone has no number.
    #[test]
    fn questions_are_numbered_in_their_batch() {
        let question = |id: &str, item: u64| {
            asked(serde_json::json!({
                "id": id, "question": "?", "state": "open", "item": item
            }))
        };
        let numbered = numbers(&[
            question("q1", 4),
            question("q2", 4),
            question("q3", 9),
            question("q4", 12),
            question("q5", 12),
            question("q6", 12),
        ]);
        let number = |id: &str| numbered.get(id).copied();
        assert_eq!(
            ["q1", "q2", "q3", "q4", "q5", "q6"].map(number),
            [Some(1), Some(2), None, Some(1), Some(2), Some(3)]
        );
    }

    #[test]
    fn the_chips_count_the_questions() {
        assert_eq!(asks_text(1), "Arko asks a question");
        assert_eq!(asks_text(2), "Arko asks 2 questions");
        assert_eq!(open_text(1), "1 open question");
        assert_eq!(open_text(3), "3 open questions");
        assert_eq!(replying_text(1), "Replying to a question");
        assert_eq!(replying_text(2), "Replying to 2 questions");
    }

    #[test]
    fn each_state_reads_in_a_word_or_two() {
        assert_eq!(state_words(&QuestionState::Open), ("Open", None));
        assert_eq!(
            state_words(&QuestionState::Answered {
                answer: "Spaces".into()
            }),
            ("Answered", Some("Spaces".into()))
        );
        assert_eq!(
            state_words(&QuestionState::AnsweredInMessage),
            ("Answered in a message", None)
        );
        assert_eq!(
            state_words(&QuestionState::Closed {
                why: Some("Found it in the README".into())
            }),
            ("Settled by Arko", Some("Found it in the README".into()))
        );
        assert_eq!(state_words(&QuestionState::Dismissed), ("Dismissed", None));
    }

    /// A screen reader hears each question with its number in the card and
    /// its state, and never the agent's ids.
    #[test]
    fn a_card_is_read_with_each_questions_state() {
        let questions = [
            asked(serde_json::json!({
                "id": "q3", "question": "Tabs or spaces?", "state": "answered", "answer": "Spaces"
            })),
            asked(serde_json::json!({ "id": "q4", "question": "Push now?", "state": "open" })),
            asked(serde_json::json!({
                "id": "q5", "question": "Save the profile?", "state": "dismissed"
            })),
        ];
        assert_eq!(
            card_label(&questions),
            "Arko asks, 1. Tabs or spaces?, answered Spaces, 2. Push now?, open, 3. Save the profile?, dismissed"
        );
        assert_eq!(card_label(&questions[1..2]), "Arko asks, Push now?, open");
    }
}
