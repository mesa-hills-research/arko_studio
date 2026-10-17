//! The composer's text field: what the user writes to the agent.
//!
//! GPUI Kit's textarea, with the send key from Settings › Input (Mod+Enter
//! always sends too) and smooth typing. It is a view of its own, so typing
//! never draws the transcript or the rest of the window again. The session
//! view around it decides what a sent message is (a message, a note) and
//! clears it once taken.

use gpui_kit::component::input::{Escape, InputEvent, Keymap, SubmitKey, Textarea, TextareaState};
use gpui_kit::*;

use crate::prefs::{self, SendKey};

/// The key context of the composer's field.
pub const CONTEXT: &str = "Composer";

/// The most lines the composer grows to before it scrolls.
pub const MAX_ROWS: usize = 10;

/// What the composer asks of the view around it.
#[derive(Debug, Clone, PartialEq)]
pub enum ComposerEvent {
    /// The user sent this text. The view clears the composer with
    /// [`Composer::sent`] once it takes it.
    Submit(String),
    /// Esc while a turn runs: stop it.
    Stop,
    /// The text changed.
    Edited,
}

/// The chords that send, as Settings › Input chose: Mod+Enter always sends
/// too, so the same keys work whatever the setting.
pub fn submit_keys(send_key: SendKey) -> Vec<SubmitKey> {
    match send_key {
        SendKey::Enter => vec![SubmitKey::Enter, SubmitKey::SecondaryEnter],
        SendKey::ModEnter => vec![SubmitKey::SecondaryEnter],
        SendKey::ShiftEnter => vec![SubmitKey::ShiftEnter, SubmitKey::SecondaryEnter],
    }
}

/// The chord shown for sending, in GPUI's notation.
pub fn send_chord(send_key: SendKey) -> &'static str {
    match send_key {
        SendKey::Enter => "enter",
        SendKey::ModEnter => "secondary-enter",
        SendKey::ShiftEnter => "shift-enter",
    }
}

/// GPUI Kit's keybinding scheme for the composer.
pub fn text_keymap(keymap: prefs::Keymap) -> Keymap {
    match keymap {
        prefs::Keymap::Standard => Keymap::Cua,
        prefs::Keymap::Emacs => Keymap::Emacs,
        prefs::Keymap::Vim => Keymap::Vim,
    }
}

/// The composer's field.
pub struct Composer {
    textarea: Entity<TextareaState>,
    /// A turn runs: Esc stops it.
    working: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = prefs::get(cx).input.clone();
        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .placeholder("Message Arko")
                .smooth_caret(input.smooth_typing)
                .keymap(text_keymap(input.keymap))
                .submit_keys(submit_keys(input.send_key))
        });
        let subscriptions = vec![
            cx.subscribe_in(&textarea, window, Self::on_input_event),
            // The field follows Settings › Input as it changes.
            cx.observe_global::<prefs::PrefsStore>(|this, cx| this.follow_prefs(cx)),
        ];
        Self {
            textarea,
            working: false,
            _subscriptions: subscriptions,
        }
    }

    fn follow_prefs(&mut self, cx: &mut Context<Self>) {
        let input = prefs::get(cx).input.clone();
        self.textarea.update(cx, |textarea, cx| {
            if textarea.has_smooth_caret() != input.smooth_typing {
                textarea.set_smooth_caret(input.smooth_typing, cx);
            }
            let keymap = text_keymap(input.keymap);
            if textarea.current_keymap() != keymap {
                textarea.set_keymap(keymap, cx);
            }
            let keys = submit_keys(input.send_key);
            if textarea.current_submit_keys() != keys.as_slice() {
                textarea.set_submit_keys(keys, cx);
            }
        });
    }

    /// The text field's state.
    pub fn textarea(&self) -> &Entity<TextareaState> {
        &self.textarea
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.textarea
            .update(cx, |textarea, cx| textarea.focus(window, cx));
    }

    /// The text as typed.
    pub fn text(&self, cx: &App) -> String {
        self.textarea.read(cx).value().to_string()
    }

    /// Replaces the text with `text`.
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.textarea.update(cx, |textarea, cx| {
            textarea.set_value(text.to_string(), window, cx);
            let end = text.len();
            textarea.set_selected_range(end..end, cx);
        });
        cx.notify();
    }

    pub fn set_placeholder(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        let text = SharedString::from(text.to_string());
        self.textarea.update(cx, |textarea, cx| {
            if textarea.presentation().placeholder() != &text {
                textarea.set_placeholder(text, window, cx);
            }
        });
    }

    /// Whether a turn runs (Esc stops it).
    pub fn set_working(&mut self, working: bool) {
        self.working = working;
    }

    /// The message was taken: the composer empties.
    pub fn sent(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.set_text("", window, cx);
    }

    fn on_input_event(
        &mut self,
        textarea: &Entity<TextareaState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { secondary, shift } => {
                if textarea.read(cx).is_submit_chord(*secondary, *shift) {
                    self.submit(cx);
                }
            }
            InputEvent::Change => {
                cx.emit(ComposerEvent::Edited);
                cx.notify();
            }
            _ => {}
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        if text.trim().is_empty() {
            return;
        }
        cx.emit(ComposerEvent::Submit(text));
    }

    /// Asks to send what is typed (the Send button).
    pub fn send(&mut self, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        let textarea = self.textarea.read(cx);
        // Vim's Escape is its own.
        let vim = textarea.current_keymap() == Keymap::Vim;
        if self.working && !vim {
            cx.emit(ComposerEvent::Stop);
            cx.stop_propagation();
        }
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("composer-field")
            .relative()
            .w_full()
            .key_context(CONTEXT)
            .capture_action(cx.listener(Self::on_escape))
            .child(
                Textarea::new(&self.textarea)
                    .appearance(false)
                    .aria_label("Message"),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{SendKey, SubmitKey, submit_keys};

    #[test]
    fn mod_enter_always_sends() {
        for key in [SendKey::Enter, SendKey::ModEnter, SendKey::ShiftEnter] {
            assert!(
                submit_keys(key).contains(&SubmitKey::SecondaryEnter),
                "{key:?}"
            );
        }
        assert_eq!(
            submit_keys(SendKey::ShiftEnter),
            [SubmitKey::ShiftEnter, SubmitKey::SecondaryEnter]
        );
        assert!(!submit_keys(SendKey::ModEnter).contains(&SubmitKey::Enter));
    }
}
