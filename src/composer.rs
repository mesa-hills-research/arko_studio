//! The composer's text field: what the user writes to the agent.
//!
//! GPUI Kit's textarea, with the send key from Settings › Input (Mod+Enter
//! always sends too), smooth typing, spell checking and word suggestions
//! from the shared writing tools, long pastes as tokens, the input history
//! on ↑ and ↓ at its edges, and a menu of arko's slash commands. It is a
//! view of its own, so typing never draws the transcript or the rest of the
//! window again. The session view around it decides what a sent message is
//! (a message, a note, an answer, a command) and clears it once taken.

use std::collections::HashMap;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use arko::remote::Reading;
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InlineToken, InputEvent, InputToken, Keymap, MoveDown, MoveUp,
    SubmitKey, SuggestionOptions, Textarea, TextareaState,
};
use gpui_kit::component::{ActiveTheme as _, IconName, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::writing::WordSuggestions;
use gpui_kit::*;

use crate::prefs::{self, SendKey};
use crate::writing::WritingAids;

/// The key context of the composer's field.
pub const CONTEXT: &str = "Composer";

/// The most lines the composer grows to before it scrolls.
pub const MAX_ROWS: usize = 10;

/// A paste this long (or longer than the lines Settings › Input sets)
/// becomes a token.
pub const PASTE_TOKEN_BYTES: usize = 2048;

/// The most rows the slash menu lists at once.
const MENU_ROWS: usize = 8;

/// The screen commands the app runs itself. The rest of them belong to the
/// terminal's full-screen view.
const APP_COMMANDS: &[&str] = &["/view", "/settings"];

/// What the composer asks of the view around it.
#[derive(Debug, Clone, PartialEq)]
pub enum ComposerEvent {
    /// The user sent this text (tokens expanded). The view clears the
    /// composer with [`Composer::sent`] once it takes it.
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

/// Whether a paste of `text` becomes a token.
pub fn pastes_as_token(text: &str, max_lines: u32) -> bool {
    text.len() > PASTE_TOKEN_BYTES || text.lines().count() > max_lines as usize
}

/// A token's label for a paste of `text`: "Pasted 120 lines".
pub fn paste_label(text: &str) -> String {
    let lines = text.lines().count();
    if lines > 1 {
        format!("Pasted {lines} lines")
    } else {
        format!("Pasted {} KB", text.len().div_ceil(1024))
    }
}

/// A row of the slash menu.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    /// What it puts in the field.
    pub insert: String,
    /// The command or the word, as listed.
    pub label: SharedString,
    /// What may follow a command: `on|off`.
    pub hint: SharedString,
    pub about: SharedString,
    /// A space follows it (more may be typed after).
    pub more: bool,
}

/// The slash menu: arko's commands while the first word is typed, then the
/// words an argument may be.
#[derive(Debug, Clone, PartialEq)]
pub struct SlashMenu {
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// The bytes a choice replaces.
    pub range: Range<usize>,
}

/// The commands the menu offers: the session's, and the screen ones the
/// app runs.
fn offered(command: &arko::commands::Command) -> bool {
    command.runs == arko::commands::Runs::Session || APP_COMMANDS.contains(&command.name)
}

/// The slash menu for `text`, from the commands and the session's status
/// `reading`, or `None` when it isn't a command being typed.
pub fn slash_menu(text: &str, reading: &Reading) -> Option<SlashMenu> {
    if !text.starts_with('/') || text.contains('\n') {
        return None;
    }
    let Some(space) = text.find(char::is_whitespace) else {
        let items: Vec<MenuItem> = arko::commands::matching(text, true)
            .into_iter()
            .filter(|command| offered(command))
            .map(|command| MenuItem {
                insert: command.name.to_string(),
                label: command.name.into(),
                hint: command.args.into(),
                about: command.about.into(),
                more: !command.args.is_empty(),
            })
            .collect();
        return (!items.is_empty()).then_some(SlashMenu {
            items,
            selected: 0,
            range: 0..text.len(),
        });
    };
    let name = &text[..space];
    let command = arko::commands::find(&format!("{name} x")).filter(|command| offered(command))?;
    let rest = &text[space..];
    let mut words: Vec<&str> = rest.split_whitespace().collect();
    let partial = if rest.ends_with(char::is_whitespace) {
        ""
    } else {
        words.pop().unwrap_or("")
    };
    let start = text.len() - partial.len();
    let lower = partial.to_lowercase();
    let items: Vec<MenuItem> = arko::commands::dynamic_choices(reading, command.name, &words)
        .into_iter()
        .filter(|word| word.to_lowercase().starts_with(&lower) && *word != partial)
        .map(|word| MenuItem {
            label: word.clone().into(),
            insert: word,
            hint: SharedString::default(),
            about: SharedString::default(),
            more: true,
        })
        .collect();
    (!items.is_empty()).then_some(SlashMenu {
        items,
        selected: 0,
        range: start..text.len(),
    })
}

/// The composer's field.
pub struct Composer {
    textarea: Entity<TextareaState>,
    suggestions: Rc<WordSuggestions>,
    /// Each paste token's text, by the token's id.
    pastes: HashMap<SharedString, String>,
    pasted: u32,
    /// Walking the input history: where, and the draft it started from.
    walk: Option<(usize, String)>,
    menu: Option<SlashMenu>,
    /// The text the menu was closed for with Escape.
    dismissed: Option<String>,
    /// The session's status, for the slash menu's choices.
    reading: Arc<Reading>,
    /// A turn runs: Esc stops it.
    working: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Composer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = prefs::get(cx).input.clone();
        let writing = cx.global::<WritingAids>().0.clone();
        let suggestions = writing.word_suggestions();
        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, MAX_ROWS)
                .placeholder("Message Arko")
                .smooth_caret(input.smooth_typing)
                .keymap(text_keymap(input.keymap))
                .submit_keys(submit_keys(input.send_key))
                .spell_checker(writing.spell_checker())
                .suggestion_provider(suggestions.clone())
                .suggestion_options(SuggestionOptions::default().menu(false).inline(true))
        });
        textarea.update(cx, |textarea, cx| {
            textarea.set_spell_checking(input.spell_check, cx)
        });
        let subscriptions = vec![
            suggestions.follow(&textarea, cx),
            cx.subscribe_in(&textarea, window, Self::on_input_event),
            // The field follows Settings › Input as it changes.
            cx.observe_global::<prefs::PrefsStore>(|this, cx| this.follow_prefs(cx)),
        ];
        Self {
            textarea,
            suggestions,
            pastes: HashMap::new(),
            pasted: 0,
            walk: None,
            menu: None,
            dismissed: None,
            reading: Arc::default(),
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
            if textarea.is_spell_checking() != input.spell_check {
                textarea.set_spell_checking(input.spell_check, cx);
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

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.textarea.read(cx).focus_handle(cx).is_focused(window)
    }

    /// The text as typed, tokens as their labels' text.
    pub fn text(&self, cx: &App) -> String {
        self.textarea.read(cx).value().to_string()
    }

    pub fn is_empty(&self, cx: &App) -> bool {
        self.textarea.read(cx).value().trim().is_empty()
    }

    /// The text with every paste token replaced by what was pasted.
    pub fn expanded_text(&self, cx: &App) -> String {
        let content = self.textarea.read(cx).content();
        let mut text = content.text().to_string();
        for span in content.tokens().iter().rev() {
            if let Some(pasted) = self.pastes.get(span.token().id()) {
                text.replace_range(span.range(), pasted);
            }
        }
        text
    }

    /// Replaces the text with `text` (a draft, a message to edit).
    pub fn set_text(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.walk = None;
        self.menu = None;
        self.dismissed = None;
        self.pastes.clear();
        self.textarea.update(cx, |textarea, cx| {
            textarea.set_value(text.to_string(), window, cx);
            let end = text.len();
            textarea.set_selected_range(end..end, cx);
        });
        self.suggestions.opened_with(text);
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

    /// The session's status, for the slash menu, and whether a turn runs.
    pub fn set_session_state(&mut self, reading: Arc<Reading>, working: bool) {
        self.reading = reading;
        self.working = working;
    }

    /// Tall (`rows` lines) or growing with its text.
    pub fn set_rows(&mut self, rows: Option<usize>, cx: &mut Context<Self>) {
        self.textarea.update(cx, |textarea, cx| match rows {
            Some(rows) => textarea.set_auto_grow(rows, rows, cx),
            None => textarea.set_auto_grow(1, MAX_ROWS, cx),
        });
    }

    pub fn menu(&self) -> Option<&SlashMenu> {
        self.menu.as_ref()
    }

    /// Opens the slash menu for the text as it stands (a command put in the
    /// composer from the palette).
    pub fn show_menu(&mut self, cx: &mut Context<Self>) {
        let text = self.text(cx);
        self.dismissed = None;
        self.update_menu(&text);
        cx.notify();
    }

    /// The message was taken: the composer empties, the history and the
    /// vocabulary learn it.
    pub fn sent(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        crate::history::add(text, cx);
        // The last sentence of a message sent with Enter often has no full
        // stop: learn it now, as typed.
        self.suggestions.learn_rest(self.textarea.read(cx).text());
        crate::writing::save(cx);
        self.set_text("", window, cx);
    }

    fn on_input_event(
        &mut self,
        textarea: &Entity<TextareaState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { secondary, shift } => {
                if !textarea.read(cx).is_submit_chord(*secondary, *shift) {
                    return;
                }
                if self.menu.is_some() && !self.menu_completes_text(cx) {
                    self.accept_menu(window, cx);
                } else {
                    self.submit(cx);
                }
            }
            InputEvent::Change => {
                let text = self.text(cx);
                if let Some((index, _)) = &self.walk {
                    let entries = crate::history::InputHistory::entries(cx);
                    if entries.get(*index) != Some(&text) {
                        self.walk = None;
                    }
                }
                self.update_menu(&text);
                cx.emit(ComposerEvent::Edited);
                cx.notify();
            }
            _ => {}
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.expanded_text(cx);
        if text.trim().is_empty() {
            return;
        }
        self.menu = None;
        cx.emit(ComposerEvent::Submit(text));
    }

    /// Asks to send what is typed (the Send button).
    pub fn send(&mut self, cx: &mut Context<Self>) {
        self.submit(cx);
    }

    fn update_menu(&mut self, text: &str) {
        if self.dismissed.as_deref() == Some(text) {
            return;
        }
        self.dismissed = None;
        let selected = self.menu.as_ref().map(|menu| menu.selected);
        self.menu = slash_menu(text, &self.reading).map(|mut menu| {
            if let Some(selected) = selected {
                menu.selected = selected.min(menu.items.len() - 1);
            }
            menu
        });
    }

    /// The menu's choice is what is typed already: Enter sends it.
    fn menu_completes_text(&self, cx: &App) -> bool {
        let Some(menu) = &self.menu else {
            return false;
        };
        let text = self.text(cx);
        let item = &menu.items[menu.selected];
        text[menu.range.clone()] == item.insert && !item.more
    }

    /// Puts the menu's selected choice in the field.
    pub fn accept_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let item = &menu.items[menu.selected];
        let mut text = self.text(cx);
        let mut insert = item.insert.clone();
        if item.more {
            insert.push(' ');
        }
        text.replace_range(menu.range.clone(), &insert);
        let end = menu.range.start + insert.len();
        let next = text.clone();
        self.textarea.update(cx, |textarea, cx| {
            textarea.replace_all(next, window, cx);
            textarea.set_selected_range(end..end, cx);
        });
        // The words after a command come next.
        self.update_menu(&text);
        cx.notify();
    }

    /// Chooses the menu's row `index` with the mouse.
    pub fn choose(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu
            && index < menu.items.len()
        {
            menu.selected = index;
            self.accept_menu(window, cx);
            self.focus(window, cx);
        }
    }

    fn move_menu(&mut self, step: isize, cx: &mut Context<Self>) {
        if let Some(menu) = &mut self.menu {
            let count = menu.items.len() as isize;
            menu.selected = (menu.selected as isize + step).rem_euclid(count) as usize;
            cx.notify();
        }
    }

    /// Whether the caret is on the field's first (`up`) or last row, with
    /// nothing selected: where ↑ and ↓ walk the history.
    fn at_edge(&self, up: bool, cx: &App) -> bool {
        let textarea = self.textarea.read(cx);
        if !textarea.selected_range().is_empty() {
            return false;
        }
        // The row the caret is drawn on, where the field has been laid out.
        if let (Some((caret, _)), Some(text), Some(line)) = (
            textarea.cursor_layout(),
            textarea.text_bounds(),
            textarea.line_height(),
        ) {
            return if up {
                caret.top() - text.top() < line / 2.
            } else {
                text.bottom() - caret.bottom() < line / 2.
            };
        }
        let position = textarea.cursor_position();
        // (Only before the field is laid out: never in a window the user sees.)
        let lines = textarea.value().split('\n').count();
        if up {
            position.line == 0
        } else {
            position.line as usize + 1 >= lines
        }
    }

    /// Walks the history one step back (`up`) or forward, `false` when
    /// there is nowhere to go.
    fn walk_history(&mut self, up: bool, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let entries = crate::history::InputHistory::entries(cx);
        let next = match (&self.walk, up) {
            (None, false) => return false,
            (None, true) => match entries.len().checked_sub(1) {
                Some(last) => Some((last, self.expanded_text(cx))),
                None => return false,
            },
            (Some((index, draft)), true) => match index.checked_sub(1) {
                Some(index) => Some((index, draft.clone())),
                None => return true,
            },
            (Some((index, draft)), false) => {
                (index + 1 < entries.len()).then(|| (index + 1, draft.clone()))
            }
        };
        let text = match &next {
            Some((index, _)) => entries[*index].clone(),
            None => self.walk.take().map(|(_, draft)| draft).unwrap_or_default(),
        };
        self.set_text(&text, window, cx);
        self.walk = next;
        true
    }

    fn on_move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.move_menu(-1, cx);
            cx.stop_propagation();
        } else if self.at_edge(true, cx) && self.walk_history(true, window, cx) {
            cx.stop_propagation();
        }
    }

    fn on_move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.move_menu(1, cx);
            cx.stop_propagation();
        } else if self.at_edge(false, cx) && self.walk_history(false, window, cx) {
            cx.stop_propagation();
        }
    }

    fn on_tab(&mut self, _: &IndentInline, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.is_some() {
            self.accept_menu(window, cx);
            cx.stop_propagation();
        }
    }

    fn on_enter(&mut self, enter: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        // A chord that sends reaches the view through `PressEnter`. One that
        // breaks a line chooses from the menu instead while it is open.
        let sends = self
            .textarea
            .read(cx)
            .is_submit_chord(enter.secondary, enter.shift);
        if self.menu.is_some() && !sends {
            self.accept_menu(window, cx);
            cx.stop_propagation();
        }
    }

    fn on_escape(&mut self, _: &Escape, _: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.dismissed = Some(self.text(cx));
            cx.notify();
            cx.stop_propagation();
            return;
        }
        let textarea = self.textarea.read(cx);
        // Vim's Escape is its own, and a suggestion on show closes first.
        let vim = textarea.current_keymap() == Keymap::Vim;
        if self.working && !vim && textarea.suggestions().is_empty() {
            cx.emit(ComposerEvent::Stop);
            cx.stop_propagation();
        }
    }

    /// Turns a long paste into a token.
    fn paste_token(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        self.pasted += 1;
        let id = SharedString::from(format!("paste-{}", self.pasted));
        let label = paste_label(&text);
        let token =
            InlineToken::new(id.clone(), format!("[{label} #{}]", self.pasted)).with_label(label);
        let inserted = self.textarea.update(cx, |textarea, cx| {
            textarea.replace_with_token(token, window, cx)
        });
        match inserted {
            Ok(()) => {
                self.pastes.insert(id, text);
            }
            // A token can't go here (an input method is composing): paste
            // the text itself.
            Err(_) => self
                .textarea
                .update(cx, |textarea, cx| textarea.replace(text, window, cx)),
        }
        cx.notify();
    }

    /// Opens a paste token into its text.
    fn expand_token(&mut self, id: &SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = self.pastes.remove(id) else {
            return;
        };
        let range = self
            .textarea
            .read(cx)
            .tokens()
            .iter()
            .find(|span| span.token().id() == id)
            .map(|span| span.range());
        if let Some(range) = range {
            self.textarea.update(cx, |textarea, cx| {
                textarea.set_selected_range(range, cx);
                textarea.replace(text, window, cx);
            });
        }
    }

    fn render_menu(&self, menu: &SlashMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // The rows around the selected one.
        let first = menu
            .selected
            .saturating_sub(MENU_ROWS - 1)
            .min(menu.items.len().saturating_sub(MENU_ROWS));
        let rows = menu
            .items
            .iter()
            .enumerate()
            .skip(first)
            .take(MENU_ROWS)
            .map(|(index, item)| {
                let selected = index == menu.selected;
                div()
                    .id(("slash-item", index))
                    .test_support()
                    .px_2()
                    .py_1()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(theme.radius)
                    .text_sm()
                    .when(selected, |this| {
                        this.bg(theme.accent).text_color(theme.accent_foreground)
                    })
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font_family.clone())
                            .child(item.label.clone()),
                    )
                    .when(!item.hint.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .font_family(theme.mono_font_family.clone())
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(item.hint.clone()),
                        )
                    })
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(if selected {
                                theme.accent_foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(item.about.clone()),
                    )
                    .on_mouse_down(MouseButton::Left, |_, window, _| window.prevent_default())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.choose(index, window, cx)),
                    )
            });
        v_flex()
            .id("slash-menu")
            .test_support()
            .absolute()
            .left_0()
            .right_0()
            .bottom(relative(1.))
            .mb_2()
            .p_1()
            .bg(theme.popover)
            .text_color(theme.popover_foreground)
            .border_1()
            .border_color(theme.border)
            .rounded(theme.radius_lg)
            .shadow_md()
            .children(rows)
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let paste_lines = prefs::get(cx).input.paste_token_lines;
        let menu = self.menu.clone();
        div()
            .id("composer-field")
            .relative()
            .w_full()
            .key_context(CONTEXT)
            .capture_action(cx.listener(Self::on_move_up))
            .capture_action(cx.listener(Self::on_move_down))
            .capture_action(cx.listener(Self::on_tab))
            .capture_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(Self::on_escape))
            .when_some(menu, |this, menu| this.child(self.render_menu(&menu, cx)))
            .child(
                Textarea::new(&self.textarea)
                    .appearance(false)
                    .aria_label("Message")
                    .token(|token, _, _| InputToken::new(token).icon(IconName::FileText))
                    .on_token_click({
                        let this = this.clone();
                        move |event, window, cx| {
                            let id = event.token().id().clone();
                            this.update(cx, |this, cx| this.expand_token(&id, window, cx))
                                .ok();
                        }
                    })
                    .on_paste(move |item, window, cx| {
                        let Some(text) = item.text() else {
                            return false;
                        };
                        if !pastes_as_token(&text, paste_lines) {
                            return false;
                        }
                        let this = this.clone();
                        window.defer(cx, move |window, cx| {
                            this.update(cx, |this, cx| this.paste_token(text, window, cx))
                                .ok();
                        });
                        true
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{PASTE_TOKEN_BYTES, SendKey, SubmitKey, paste_label, pastes_as_token, slash_menu};
    use arko::remote::Reading;

    #[test]
    fn mod_enter_always_sends() {
        use super::submit_keys;
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

    #[test]
    fn long_pastes_become_tokens() {
        assert!(!pastes_as_token("one\ntwo", 20));
        let lines = "line\n".repeat(21);
        assert!(pastes_as_token(&lines, 20));
        assert!(!pastes_as_token(&lines, 30));
        assert!(pastes_as_token(&"x".repeat(PASTE_TOKEN_BYTES + 1), 20));
        assert_eq!(paste_label(&lines), "Pasted 21 lines");
        assert_eq!(paste_label(&"x".repeat(3000)), "Pasted 3 KB");
    }

    #[test]
    fn the_slash_menu_lists_commands_then_their_words() {
        let reading = Reading::default();
        let menu = slash_menu("/s", &reading).unwrap();
        let names: Vec<&str> = menu.items.iter().map(|item| item.label.as_ref()).collect();
        // The session's commands and the app's /settings, without the screen's.
        assert_eq!(names, ["/stop-workers", "/settings", "/skills", "/session"]);
        assert_eq!(menu.range, 0..2);
        assert!(slash_menu("/x", &reading).is_none());
        assert!(slash_menu("/view\nmore", &reading).is_none());
        assert!(slash_menu("hello /s", &reading).is_none());
        assert!(slash_menu("/watch", &reading).is_none(), "the screen's own");

        let menu = slash_menu("/outside ", &reading).unwrap();
        let words: Vec<&str> = menu.items.iter().map(|item| item.label.as_ref()).collect();
        assert_eq!(words, ["on", "off"]);
        assert_eq!(menu.range, 9..9);
        let menu = slash_menu("/outside o", &reading).unwrap();
        assert_eq!(menu.range, 9..10);
        assert_eq!(menu.items.len(), 2);
        let menu = slash_menu("/outside of", &reading).unwrap();
        assert_eq!(menu.items[0].insert, "off");
        // A finished word has nothing more to offer.
        assert!(slash_menu("/outside off", &reading).is_none());
        // Commands without arguments offer none.
        assert!(slash_menu("/cost ", &reading).is_none());
        // Words after a word.
        let menu = slash_menu("/project-instructions workers ", &reading).unwrap();
        assert_eq!(menu.items.len(), 2);
    }
}
