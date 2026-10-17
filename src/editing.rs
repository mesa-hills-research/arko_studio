//! Changing a conversation: what editing a message or regenerating the
//! reply removes, and the dialog that asks first, with Branch instead.

use std::cell::Cell;
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::dialog::DialogFooter;
use gpui_kit::component::{ActiveTheme as _, Disableable as _, WindowExt as _, v_flex};
use gpui_kit::*;

use crate::prefs;

/// What the user asks to change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// Send an edited message in place of one.
    Edit,
    /// Send the last message again for a new reply.
    Regenerate,
}

/// What the user chose in the dialog.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    /// Change the conversation in place.
    Change,
    /// Leave it, and go on in a new session branched before the message.
    Branch,
}

/// The dialog's title.
pub fn title(change: Change) -> &'static str {
    match change {
        Change::Edit => "Edit this message?",
        Change::Regenerate => "Regenerate the reply?",
    }
}

/// What the change removes (`after`: the messages after the message), and
/// in an agent's session, that files stay as the agent left them.
pub fn description(change: Change, after: usize, agent: bool) -> String {
    let mut text = match (change, after) {
        (Change::Edit, 0) => "It is sent again as you changed it.".to_string(),
        (Change::Edit, 1) => "The message after it will be removed.".to_string(),
        (Change::Edit, after) => format!("The {after} messages after it will be removed."),
        (Change::Regenerate, 0 | 1) => {
            "Your last message is sent again, and the reply to it removed.".to_string()
        }
        (Change::Regenerate, after) => {
            format!("Your last message is sent again, and the {after} messages after it removed.")
        }
    };
    if agent {
        text.push_str(match change {
            Change::Edit => " Files the agent changed after this message stay as they are.",
            Change::Regenerate => " Files the agent changed after it stay as they are.",
        });
    }
    text
}

/// What hears the user's choice.
type Answer = Rc<dyn Fn(Choice, &mut Window, &mut App)>;

/// Why Branch instead can't keep a message that follows a handover: a
/// branch can't start from the handover's summary yet.
pub const BRANCH_LOSES_THE_SUMMARY: &str = "A branch from this message would start without the summary of the work before it. Edit it here, or branch from a later message.";

/// Asks before `change` removes the `after` messages after the message:
/// [Cancel] [Branch instead] [Edit]. "Don't ask again" counts only when the
/// change is chosen. `answer` hears the choice (nothing on Cancel). With
/// `no_branch`, Branch instead is disabled, and its tooltip says why.
pub fn confirm(
    change: Change,
    after: usize,
    agent: bool,
    no_branch: Option<&'static str>,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(Choice, &mut Window, &mut App) + 'static,
) {
    let answer: Answer = Rc::new(answer);
    let dont_ask = Rc::new(Cell::new(false));
    let description = description(change, after, agent);
    let ok = match change {
        Change::Edit => "Edit",
        Change::Regenerate => "Regenerate",
    };
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let muted = cx.theme().muted_foreground;
        let checked = dont_ask.clone();
        let (branch, change_it, on_enter) = (answer.clone(), answer.clone(), answer.clone());
        let (remembered, remembered_on_enter) = (dont_ask.clone(), dont_ask.clone());
        alert
            .title(title(change))
            .description(description.clone())
            // Enter does what the primary button does, as in the app's other
            // dialogs.
            .on_ok(move |_, window, cx| {
                if remembered_on_enter.get() {
                    prefs::update(cx, |prefs| prefs.sessions.ask_before_edit = false);
                }
                on_enter(Choice::Change, window, cx);
                true
            })
            .content(move |content, _, _| {
                let checked = checked.clone();
                content.child(
                    v_flex()
                        .id("edit-dialog")
                        .test_support()
                        .text_sm()
                        .text_color(muted)
                        .child(
                            Checkbox::new("dont-ask-edit")
                                .label("Don’t ask again")
                                .checked(checked.get())
                                .on_click(move |on, window, _| {
                                    checked.set(*on);
                                    window.refresh();
                                }),
                        ),
                )
            })
            .footer(
                DialogFooter::new()
                    .child(
                        Button::new("edit-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("edit-branch")
                            .outline()
                            .label("Branch instead")
                            .disabled(no_branch.is_some())
                            .tooltip(
                                no_branch.unwrap_or(
                                    "Keep this conversation, and go on in a new session",
                                ),
                            )
                            .on_click(move |_, window, cx| {
                                window.close_dialog(cx);
                                branch(Choice::Branch, window, cx);
                            }),
                    )
                    .child(Button::new("edit-confirm").primary().label(ok).on_click(
                        move |_, window, cx| {
                            // Remembered only for the change itself.
                            if remembered.get() {
                                prefs::update(cx, |prefs| prefs.sessions.ask_before_edit = false);
                            }
                            window.close_dialog(cx);
                            change_it(Choice::Change, window, cx);
                        },
                    )),
            )
    });
}

#[cfg(test)]
#[path = "editing_tests.rs"]
mod engine_tests;

#[cfg(test)]
mod tests {
    use super::{Change, description, title};

    #[test]
    fn the_dialog_says_what_goes() {
        assert_eq!(title(Change::Edit), "Edit this message?");
        assert_eq!(
            description(Change::Edit, 3, false),
            "The 3 messages after it will be removed."
        );
        assert_eq!(
            description(Change::Edit, 1, true),
            "The message after it will be removed. Files the agent changed after this message stay as they are."
        );
        assert_eq!(
            description(Change::Regenerate, 1, false),
            "Your last message is sent again, and the reply to it removed."
        );
        assert_eq!(
            description(Change::Regenerate, 3, false),
            "Your last message is sent again, and the 3 messages after it removed."
        );
    }
}
