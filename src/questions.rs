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
use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Selectable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

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
pub fn asks_text(questions: &[AgentQuestion]) -> String {
    match questions.len() {
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

/// The user's answer to a question given in the answer panel, which the
/// transcript shows as the user's words, under the question's card.
pub fn answer(state: &QuestionState) -> Option<&str> {
    match state {
        QuestionState::Answered { answer } => Some(answer.trim()),
        _ => None,
    }
}

/// A question's words without its number.
pub fn text(question: &AgentQuestion) -> String {
    question.question.trim().to_string()
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
}

impl EventEmitter<AnswerPanelEvent> for AnswerPanel {}

/// The answer panel: each open question with its choices as chips, an
/// "Other" field and "You decide", and Send.
pub struct AnswerPanel {
    questions: Vec<AgentQuestion>,
    drafts: HashMap<String, Draft>,
    /// The questions' list, which scrolls to a card's questions.
    scroll: ScrollHandle,
    /// Where the keyboard goes when a click beside a field takes it out of
    /// the field, so that the panel's keys still work.
    focus: FocusHandle,
}

impl AnswerPanel {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            questions: Vec::new(),
            drafts: HashMap::new(),
            scroll: ScrollHandle::default(),
            focus: cx.focus_handle(),
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

    /// What is filled in. Every question the panel shows is named, those
    /// left unanswered with an empty answer.
    pub fn answers(&self, cx: &App) -> Answers {
        let answers = self
            .questions
            .iter()
            .map(|question| {
                let Some(draft) = self.drafts.get(&question.id) else {
                    return QuestionAnswer::new(question.id.clone(), Answer::default());
                };
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
                QuestionAnswer::new(question.id.clone(), answer)
            })
            .collect();
        Answers { answers }
    }

    /// Asks the session view to send what is filled in, if anything is.
    fn send(&mut self, cx: &mut Context<Self>) {
        if !self.answers(cx).is_empty() {
            cx.emit(AnswerPanelEvent::Send);
        }
    }

    /// What was sent is cleared: the drafts of the questions answered.
    pub fn sent(&mut self, answered: &[String], cx: &mut Context<Self>) {
        self.drafts.retain(|id, _| !answered.contains(id));
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

    /// Gives the keyboard to question `index`'s field (tests).
    #[cfg(test)]
    pub fn focus_field(&self, index: usize, window: &mut Window, cx: &mut App) {
        let draft = self.drafts.get(&self.questions[index].id).unwrap();
        draft.other.update(cx, |field, cx| field.focus(window, cx));
    }

    /// Puts `text` in question `index`'s field (tests).
    #[cfg(test)]
    pub fn set_field(&self, index: usize, text: &str, window: &mut Window, cx: &mut App) {
        let draft = self.drafts.get(&self.questions[index].id).unwrap();
        draft.other.update(cx, |field, cx| {
            field.set_value(text.to_string(), window, cx)
        });
    }

    /// Whether the keyboard is in the panel.
    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus.contains_focused(window, cx) || self.focused_question(window, cx).is_some()
    }

    /// A click on a choice takes the keyboard out of the field it is in,
    /// as a click on the panel's empty space does. A key that pressed it
    /// leaves the keyboard on it.
    fn leave_field(&self, click: &ClickEvent, window: &mut Window, cx: &mut App) {
        if !click.is_keyboard() && self.focused_question(window, cx).is_some() {
            window.focus(&self.focus, cx);
        }
    }

    /// The question whose field has the keyboard, by its id.
    pub fn focused_question(&self, window: &Window, cx: &App) -> Option<String> {
        use gpui_kit::Focusable as _;
        self.drafts
            .iter()
            .find(|(_, draft)| {
                draft
                    .other
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            })
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
        // A chosen chip is outlined in the text's color, which stands out
        // from the other chips' outlines in every theme, as the kit's own
        // fill for a chosen outline button is faint in some.
        let chosen_outline = theme.foreground;
        let chips = question.choices.iter().enumerate().map(|(n, choice)| {
            let chosen = draft.choices.contains(choice);
            let (id, choice) = (id.clone(), choice.clone());
            Button::new(part("answer-choice", &id, n))
                .small()
                .label(choice.clone())
                .outline()
                .selected(chosen)
                .toggled(chosen)
                .when(chosen, |this| this.border_color(chosen_outline))
                .on_click(cx.listener(move |this, click, window, cx| {
                    this.leave_field(click, window, cx);
                    this.toggle_choice(&id, &choice, cx)
                }))
                .into_any_element()
        });
        let you_decide = {
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
                .when(chosen, |this| this.border_color(chosen_outline))
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
                .on_click(cx.listener(move |this, click, window, cx| {
                    this.leave_field(click, window, cx);
                    this.toggle_you_decide(&id, cx)
                }))
        };
        // Another ask's questions start below a line. One whose card isn't
        // there yet (item 0) stands on its own.
        let new_batch = index > 0
            && self
                .questions
                .get(index - 1)
                .is_some_and(|before| before.item != question.item || question.item == 0);
        let words = text(question);
        // A question that takes several of its choices says so after its
        // words.
        let choose_any = question.multiple && !question.choices.is_empty();
        Some(
            v_flex()
                .id(part("answer-question", &id, 0))
                .test_support()
                .role(Role::Group)
                .aria_label(if choose_any {
                    format!(
                        "{}, {}",
                        first_line(&question.numbered()),
                        arko::questions::CHOOSE_ANY.to_lowercase()
                    )
                } else {
                    first_line(&question.numbered()).to_string()
                })
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
                                // "Choose any" follows the question's words.
                                .child(
                                    h_flex()
                                        .flex_wrap()
                                        .items_baseline()
                                        .gap_x_1p5()
                                        .child(SelectableText::new(
                                            part("answer-text", &id, 0),
                                            words.clone(),
                                        ))
                                        .when(choose_any, |this| {
                                            this.child(
                                                div()
                                                    .flex_none()
                                                    .text_xs()
                                                    .text_color(theme.muted_foreground)
                                                    .child(arko::questions::CHOOSE_ANY),
                                            )
                                        }),
                                ),
                        ),
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
                        .aria_label(format!("{}, other answer", first_line(&words))),
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
        let title = asks_text(&self.questions);
        let theme = cx.theme();
        v_flex()
            .id("answer-panel")
            .test_support()
            .key_context(PANEL_CONTEXT)
            // A click on its empty space or its words takes the keyboard
            // out of a field to the panel.
            .track_focus(&self.focus)
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
                    .children(questions),
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

/// The user answers the question `id`, which the agent waits for, with
/// `text` typed in the message box, as the feed shows it (tests).
#[cfg(test)]
pub(crate) fn answer_in_message(feed: &arko::remote::Feed, id: &str, text: &str) {
    feed.set_question_state(id, QuestionState::AnsweredInMessage);
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

    #[test]
    fn the_chips_count_the_questions() {
        let question = asked(serde_json::json!({ "id": "q1", "question": "?", "state": "open" }));
        assert_eq!(
            asks_text(std::slice::from_ref(&question)),
            "Arko asks a question"
        );
        assert_eq!(
            asks_text(&[question.clone(), question]),
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
    }
}
