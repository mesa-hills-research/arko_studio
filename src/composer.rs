//! The composer's text field: what the user writes to the agent.
//!
//! GPUI Kit's textarea, with the send key from Settings › Input (Mod+Enter
//! always sends too), smooth typing, spell checking and word suggestions
//! from the shared writing tools, long pastes as tokens, images pasted,
//! dropped or attached as tiles, the input history on ↑ and ↓ at its edges,
//! and a menu of arko's slash commands. It is a
//! view of its own, so typing never draws the transcript or the rest of the
//! window again. The session view around it decides what a sent message is
//! (a message, a note, an answer, a command) and clears it once taken.

use std::collections::HashMap;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use arko::remote::Reading;
use gpui_kit::component::attachment::{
    Attachment, AttachmentContent, AttachmentMedia, AttachmentStatus, AttachmentTitle,
};
use gpui_kit::component::input::{
    Enter, Escape, IndentInline, InlineToken, InputEvent, InputToken, Keymap, MoveDown, MoveUp,
    SubmitKey, SuggestionOptions, Textarea, TextareaState,
};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::writing::WordSuggestions;
use gpui_kit::*;

use crate::fonts::CodeFont as _;
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

/// The most files an @-mention offers.
const MENTION_ROWS: usize = 50;

/// How long a folder's files listed for @-mentions are used before they
/// are listed again (the agent may make files meanwhile).
const FILES_FRESH: std::time::Duration = std::time::Duration::from_secs(30);

/// The screen commands the app runs itself. The rest of them belong to the
/// terminal's full-screen view.
const APP_COMMANDS: &[&str] = &["/view", "/settings"];

/// An image the next message takes along.
#[derive(Clone)]
pub struct Attached {
    pub name: Option<String>,
    /// The image, which its tile previews.
    image: Arc<Image>,
}

impl std::fmt::Debug for Attached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Attached")
            .field("name", &self.name)
            .field("mime", &self.image.format().mime_type())
            .field("bytes", &self.image.bytes().len())
            .finish()
    }
}

impl Attached {
    pub fn new(format: ImageFormat, bytes: Vec<u8>, name: Option<String>) -> Self {
        Self {
            name,
            image: Arc::new(Image::from_bytes(format, bytes)),
        }
    }

    /// As arko sends it with a message.
    pub fn to_arko(&self) -> arko::Attachment {
        arko::Attachment {
            bytes: Arc::from(self.image.bytes()),
            mime: self.image.format().mime_type().to_string(),
            name: self.name.clone(),
        }
    }
}

/// The image format of the file at `path`, by its extension; `None` for
/// other files.
pub fn image_format(path: &Path) -> Option<ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "png" => ImageFormat::Png,
        "jpg" | "jpeg" => ImageFormat::Jpeg,
        "gif" => ImageFormat::Gif,
        "webp" => ImageFormat::Webp,
        "bmp" => ImageFormat::Bmp,
        "tif" | "tiff" => ImageFormat::Tiff,
        _ => return None,
    })
}

/// What the user sends: the text (tokens expanded) and the images.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub text: String,
    pub images: Vec<Attached>,
}

/// What the composer asks of the view around it.
#[derive(Debug, Clone)]
pub enum ComposerEvent {
    /// The user sent this. The view clears the composer with
    /// [`Composer::sent`] once it takes it.
    Submit(Outgoing),
    /// Esc while a turn runs: stop it.
    Stop,
    /// Esc while the composer is in a mode Esc ends (editing a message,
    /// writing to a worker).
    Cancel,
    /// ↑ in the empty composer while the latest message can be edited.
    EditLast,
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

/// What a menu above the field offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuKind {
    /// Arko's commands, and their words.
    Command,
    /// The session folder's files, for an @-mention.
    Mention,
}

/// The slash menu: arko's commands while the first word is typed, then the
/// words an argument may be. Also the files an "@" mentions.
#[derive(Debug, Clone, PartialEq)]
pub struct SlashMenu {
    pub items: Vec<MenuItem>,
    pub selected: usize,
    /// The bytes a choice replaces.
    pub range: Range<usize>,
    pub kind: MenuKind,
    /// What is typed is a whole command already (`/cost`, `/stop-workers
    /// r1` with r10 running too): Enter sends it as it is.
    pub complete: bool,
    /// Enter takes the selected choice: always a command's, and a file
    /// that matches strongly (its letters in a row, or each starting a
    /// word) or that the user moved to. Otherwise Enter does what it does
    /// without a menu. Tab always takes the choice.
    pub takes_enter: bool,
    /// What the menu says while it has no choices yet: the folder is being
    /// listed.
    pub note: Option<SharedString>,
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
        let complete = items.iter().any(|item| item.insert == text && !item.more);
        return (!items.is_empty()).then_some(SlashMenu {
            items,
            selected: 0,
            range: 0..text.len(),
            kind: MenuKind::Command,
            complete,
            takes_enter: true,
            note: None,
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
    let choices = arko::commands::dynamic_choices(reading, command.name, &words);
    let complete = !partial.is_empty() && choices.iter().any(|word| word == partial);
    let items: Vec<MenuItem> = choices
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
        kind: MenuKind::Command,
        complete,
        takes_enter: true,
        note: None,
    })
}

/// The menu of the files `ranked` for an @-mention over `range`, if any.
fn mention_list(ranked: Vec<crate::mentions::Ranked>, range: Range<usize>) -> Option<SlashMenu> {
    // An ordinary word after an "@" ("@here") can hold a file's letters
    // here and there: Enter sends it then, as typed.
    let strong = ranked.first().is_some_and(|ranked| ranked.strong);
    let items: Vec<MenuItem> = ranked
        .into_iter()
        .map(|ranked| {
            let (folder, name) = match ranked.path.rsplit_once('/') {
                Some((folder, name)) => (folder.to_string(), name.to_string()),
                None => (String::new(), ranked.path.clone()),
            };
            MenuItem {
                insert: ranked.path,
                label: name.into(),
                hint: SharedString::default(),
                about: folder.into(),
                more: true,
            }
        })
        .collect();
    (!items.is_empty()).then_some(SlashMenu {
        items,
        selected: 0,
        range,
        kind: MenuKind::Mention,
        complete: false,
        takes_enter: strong,
        note: None,
    })
}

/// Whether ↑ in the empty composer edits the latest message.
type EditsLast = Rc<dyn Fn(&App) -> bool>;

/// The composer's field.
pub struct Composer {
    textarea: Entity<TextareaState>,
    suggestions: Rc<WordSuggestions>,
    /// Each paste token's text, by the token's id.
    pastes: HashMap<SharedString, String>,
    /// The images the message takes along.
    images: Vec<Attached>,
    /// The session works in a folder: files other than images go in as
    /// their paths.
    agent: bool,
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
    /// Esc ends a mode first ([`ComposerEvent::Cancel`]).
    cancellable: bool,
    /// Whether ↑ in the empty composer edits the latest message now.
    edits_last: Option<EditsLast>,
    /// The folder whose files "@" mentions: an agent session's.
    mention_folder: Option<PathBuf>,
    /// The files listed there, and when.
    files: Option<(PathBuf, std::time::Instant, Arc<crate::mentions::Files>)>,
    /// The files each recent query matched, by its lowercase words (the
    /// latest last): a query that extends one is ranked among those only.
    narrowed: Vec<(String, Arc<Vec<u32>>)>,
    listing: Option<Task<()>>,
    /// The files ranked for what follows the "@", off the UI thread. A
    /// newer query replaces it, so the latest one's files show.
    ranking: Option<Task<()>>,
    /// Mentions inserted so far, for their tokens' ids.
    mentioned: u32,
    /// The model the images go to, which says which it can't take.
    image_model: Option<String>,
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
            images: Vec::new(),
            agent: true,
            pasted: 0,
            walk: None,
            menu: None,
            dismissed: None,
            reading: Arc::default(),
            working: false,
            cancellable: false,
            edits_last: None,
            mention_folder: None,
            files: None,
            listing: None,
            ranking: None,
            narrowed: Vec::new(),
            mentioned: 0,
            image_model: None,
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

    /// The session's status, for the slash menu, whether a turn runs, and
    /// whether it works in a folder.
    pub fn set_session_state(&mut self, reading: Arc<Reading>, working: bool, agent: bool) {
        self.reading = reading;
        self.working = working;
        self.agent = agent;
    }

    /// Lets "@" mention the files of `folder` (an agent session's), or
    /// nothing (`None`: a chat).
    pub fn set_mention_folder(&mut self, folder: Option<PathBuf>) {
        if self.mention_folder != folder {
            self.mention_folder = folder;
            self.files = None;
            self.narrowed.clear();
            self.listing = None;
        }
    }

    /// Whether Esc ends a mode of the composer before anything else.
    pub fn set_cancellable(&mut self, cancellable: bool) {
        self.cancellable = cancellable;
    }

    /// Tells whether ↑ in the empty composer edits the latest message (else
    /// it walks the input history), asked as the key comes.
    pub fn set_edits_last(&mut self, edits_last: impl Fn(&App) -> bool + 'static) {
        self.edits_last = Some(Rc::new(edits_last));
    }

    /// Gives the model the images go to: the main agent's.
    pub fn set_image_model(&mut self, model: Option<String>, cx: &mut Context<Self>) {
        if self.image_model != model {
            self.image_model = model;
            cx.notify();
        }
    }

    /// Why the model can't take `image`, if it can't.
    fn refusal(&self, image: &Attached) -> Option<String> {
        let model = self.image_model.as_deref()?;
        arko::models::image_refusal(model, image.image.format().mime_type())
    }

    /// Why the message can't go with its images, if it can't.
    pub fn image_refusal(&self) -> Option<String> {
        self.images.iter().find_map(|image| self.refusal(image))
    }

    /// The images the message takes along.
    pub fn images(&self) -> &[Attached] {
        &self.images
    }

    /// Replaces the images the message takes along.
    pub fn set_images(&mut self, images: Vec<Attached>, cx: &mut Context<Self>) {
        self.images = images;
        cx.emit(ComposerEvent::Edited);
        cx.notify();
    }

    /// Adds an image to the message.
    pub fn attach_image(&mut self, image: Attached, cx: &mut Context<Self>) {
        self.images.push(image);
        cx.emit(ComposerEvent::Edited);
        cx.notify();
    }

    fn remove_image(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.images.len() {
            self.images.remove(index);
            cx.emit(ComposerEvent::Edited);
            cx.notify();
        }
    }

    /// Files given to the composer (dropped, pasted, attached): images go
    /// with the message, read off the UI thread. In an agent session other
    /// files go in the text as their paths.
    pub fn attach_paths(
        &mut self,
        paths: Vec<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (images, others): (Vec<PathBuf>, Vec<PathBuf>) = paths
            .into_iter()
            .partition(|path| image_format(path).is_some());
        if !others.is_empty() {
            if self.agent {
                let text: Vec<String> = others
                    .iter()
                    .map(|path| path.display().to_string())
                    .collect();
                self.textarea.update(cx, |textarea, cx| {
                    textarea.insert(format!("{} ", text.join(" ")), window, cx)
                });
            } else {
                window.push_notification(
                    "A chat takes images. Other files go to an agent session.",
                    cx,
                );
            }
        }
        if images.is_empty() {
            return;
        }
        let reading = cx.background_spawn(async move {
            images
                .into_iter()
                .map(|path| {
                    let format = image_format(&path)?;
                    let bytes = std::fs::read(&path).ok()?;
                    let name = path
                        .file_name()
                        .map(|name| name.to_string_lossy().into_owned());
                    Some(Attached::new(format, bytes, name))
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let read = reading.await;
            this.update_in(cx, |this, window, cx| {
                if read.iter().any(Option::is_none) {
                    window.push_notification("An image couldn’t be read.", cx);
                }
                for image in read.into_iter().flatten() {
                    this.attach_image(image, cx);
                }
            })
            .ok();
        })
        .detach();
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
        self.images.clear();
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
                // Mod+Enter always sends the text as typed. The send key
                // takes the menu's choice, unless what is typed is whole or
                // the choice is a file that only weakly matches.
                if !*secondary
                    && self
                        .menu
                        .as_ref()
                        .is_some_and(|menu| !menu.complete && menu.takes_enter)
                {
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
                self.refresh_menu(&text, cx);
                cx.emit(ComposerEvent::Edited);
                cx.notify();
            }
            _ => {}
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let text = self.expanded_text(cx);
        if text.trim().is_empty() && self.images.is_empty() {
            return;
        }
        self.menu = None;
        cx.emit(ComposerEvent::Submit(Outgoing {
            text,
            images: self.images.clone(),
        }));
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

    /// The menu for the text as it stands: a command's, else the files an
    /// "@" before the caret mentions.
    fn refresh_menu(&mut self, text: &str, cx: &mut Context<Self>) {
        // A mention's list stays up while the files for what is typed now
        // are ranked.
        let shown = self.menu.take_if(|menu| menu.kind == MenuKind::Mention);
        self.update_menu(text);
        if self.menu.is_some() || self.dismissed.as_deref() == Some(text) {
            self.ranking = None;
            return;
        }
        self.menu = self.mention_menu(text, shown, cx);
    }

    /// The files an "@" before the caret mentions, once the session's
    /// folder is listed (in the background, the first time). They are
    /// ranked off the UI thread: until they come, `shown` (the list of
    /// what was typed before) stays, over what is typed now.
    fn mention_menu(
        &mut self,
        text: &str,
        shown: Option<SlashMenu>,
        cx: &mut Context<Self>,
    ) -> Option<SlashMenu> {
        let Some((folder, range, typed)) = self.mention_typed(text, cx) else {
            self.ranking = None;
            return None;
        };
        let listed = self
            .files
            .as_ref()
            .filter(|(listed, _, _)| *listed == folder)
            .map(|(_, at, files)| (at.elapsed() > FILES_FRESH, files.clone()));
        let files = match listed {
            Some((stale, files)) => {
                // Shown while the folder is listed again.
                if stale {
                    self.list_files(folder, cx);
                }
                files
            }
            None => {
                // The first time, the folder is listed off the UI thread
                // (seconds on a cold disk): the menu says so meanwhile.
                self.list_files(folder, cx);
                return Some(SlashMenu {
                    items: Vec::new(),
                    selected: 0,
                    range,
                    kind: MenuKind::Mention,
                    complete: false,
                    takes_enter: false,
                    note: Some("Listing files…".into()),
                });
            }
        };
        self.rank_mentions(files, typed, range.clone(), cx);
        shown.map(|mut menu| {
            menu.range = range;
            menu
        })
    }

    /// The folder, the bytes from the "@" to the caret, and what follows
    /// the "@", while a mention is typed.
    fn mention_typed(&self, text: &str, cx: &App) -> Option<(PathBuf, Range<usize>, String)> {
        let folder = self.mention_folder.clone()?;
        let caret = self.textarea.read(cx).selected_range();
        if !caret.is_empty() {
            return None;
        }
        let (range, typed) = crate::mentions::typed(text, caret.end)?;
        Some((folder, range, typed.to_string()))
    }

    /// Ranks `files` for `typed` on a background thread, then shows them,
    /// if the same mention is still being typed. A newer call drops this
    /// one's task, so only the latest query's files ever show.
    fn rank_mentions(
        &mut self,
        files: Arc<crate::mentions::Files>,
        typed: String,
        range: Range<usize>,
        cx: &mut Context<Self>,
    ) {
        // The latest query this one extends: its files hold this one's.
        let words = crate::mentions::lowercase(&typed);
        self.narrowed
            .retain(|(earlier, _)| !earlier.is_empty() && words.starts_with(earlier.as_str()));
        let among = self.narrowed.last().map(|(_, matched)| matched.clone());
        let ranking = cx.background_spawn({
            let typed = typed.clone();
            async move {
                crate::mentions::rank(
                    &files,
                    among.as_deref().map(Vec::as_slice),
                    &typed,
                    MENTION_ROWS,
                )
            }
        });
        self.ranking = Some(cx.spawn(async move |this, cx| {
            let ranking = ranking.await;
            this.update(cx, |this, cx| {
                this.ranking = None;
                if !words.is_empty()
                    && this
                        .narrowed
                        .last()
                        .is_none_or(|(earlier, _)| *earlier != words)
                {
                    this.narrowed
                        .push((words.clone(), Arc::new(ranking.matched)));
                    if this.narrowed.len() > 8 {
                        this.narrowed.remove(0);
                    }
                }
                let ranked = ranking.ranked;
                let text = this.text(cx);
                let now = this.mention_typed(&text, cx);
                let same = now.is_some_and(|(_, now, words)| now == range && words == typed);
                let command = this
                    .menu
                    .as_ref()
                    .is_some_and(|menu| menu.kind == MenuKind::Command);
                if !same || command || this.dismissed.as_deref() == Some(text.as_str()) {
                    return;
                }
                this.menu = mention_list(ranked, range);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Lists the files of `folder` off the UI thread, then shows the menu
    /// for what is typed.
    fn list_files(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if self.listing.is_some() {
            return;
        }
        let listing = cx.background_spawn({
            let folder = folder.clone();
            async move { crate::mentions::list(&folder) }
        });
        self.listing = Some(cx.spawn(async move |this, cx| {
            let files = listing.await;
            this.update(cx, |this, cx| {
                this.listing = None;
                if this.mention_folder.as_ref() != Some(&folder) {
                    return;
                }
                this.files = Some((
                    folder,
                    std::time::Instant::now(),
                    Arc::new(crate::mentions::Files::new(files)),
                ));
                this.narrowed.clear();
                let text = this.text(cx);
                let waiting = this.menu.as_ref().is_none_or(|menu| menu.note.is_some());
                if waiting && this.dismissed.as_deref() != Some(text.as_str()) {
                    this.menu = this.mention_menu(&text, None, cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Puts the menu's selected choice in the field.
    pub fn accept_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        let Some(item) = menu.items.get(menu.selected) else {
            // Nothing to choose yet.
            self.menu = Some(menu);
            return;
        };
        if menu.kind == MenuKind::Mention {
            self.insert_mention(menu.range.clone(), item.insert.clone(), window, cx);
            return;
        }
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

    /// Puts the file `path` (relative to the session's folder) in place of
    /// `range` as a token showing its name (with its folder's, when another
    /// file has the name), which sends as the path.
    fn insert_mention(
        &mut self,
        range: Range<usize>,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.mentioned += 1;
        let name = path.rsplit('/').next().unwrap_or(&path).to_string();
        let shared = self
            .files
            .as_ref()
            .is_some_and(|(_, _, files)| files.named(&name) > 1);
        let label = match path.rsplit('/').nth(1) {
            Some(folder) if shared => format!("{folder}/{name}"),
            _ => name,
        };
        // A path with spaces reads as one in backticks.
        let sent = if path.contains(char::is_whitespace) {
            format!("`{path}`")
        } else {
            path.clone()
        };
        let token = InlineToken::new(format!("file-{}", self.mentioned), sent).with_label(label);
        self.textarea.update(cx, |textarea, cx| {
            match textarea.replace_range_with_token(range.clone(), token, window, cx) {
                Ok(()) => textarea.insert(" ".to_string(), window, cx),
                // A token can't go here (an input method is composing): the
                // path itself.
                Err(_) => {
                    textarea.set_selected_range(range, cx);
                    textarea.replace(format!("{path} "), window, cx);
                }
            }
        });
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
        if let Some(menu) = self.menu.as_mut().filter(|menu| !menu.items.is_empty()) {
            let count = menu.items.len() as isize;
            menu.selected = (menu.selected as isize + step).rem_euclid(count) as usize;
            menu.takes_enter = true;
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
        } else if self.edits_last.as_ref().is_some_and(|edits| edits(cx))
            && self.walk.is_none()
            && self.images.is_empty()
            && self.textarea.read(cx).value().is_empty()
        {
            cx.emit(ComposerEvent::EditLast);
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
        if self
            .menu
            .as_ref()
            .is_some_and(|menu| !menu.items.is_empty())
        {
            self.accept_menu(window, cx);
            cx.stop_propagation();
        }
    }

    fn on_enter(&mut self, enter: &Enter, window: &mut Window, cx: &mut Context<Self>) {
        // A chord that sends reaches the view through `PressEnter`. One that
        // breaks a line chooses from the menu instead while it is open, or
        // closes it over a whole command, which it leaves as typed.
        let sends = self
            .textarea
            .read(cx)
            .is_submit_chord(enter.secondary, enter.shift);
        let Some(menu) = &self.menu else {
            return;
        };
        // A file that only weakly matches leaves the line break to Enter.
        if !sends && menu.takes_enter {
            if menu.complete {
                self.menu = None;
                self.dismissed = Some(self.text(cx));
                cx.notify();
            } else {
                self.accept_menu(window, cx);
            }
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
        // Vim's Escape is its own (see `on_vim_escape`), and a suggestion on
        // show closes first.
        let vim = textarea.current_keymap() == Keymap::Vim;
        if self.cancellable && !vim && textarea.suggestions().is_empty() {
            cx.emit(ComposerEvent::Cancel);
            cx.stop_propagation();
            return;
        }
        if self.working && !vim && textarea.suggestions().is_empty() {
            cx.emit(ComposerEvent::Stop);
            cx.stop_propagation();
        }
    }

    /// Escape with the Vim keymap, once Vim let it through: in normal mode
    /// with nothing pending (Vim keeps the others, out of insert mode or
    /// of a half-typed command). It ends what the strip or the chip above
    /// says Esc ends. It doesn't stop a turn: Vim's users press Escape out
    /// of habit.
    fn on_vim_escape(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let escape = event.keystroke.key == "escape" && !event.keystroke.modifiers.modified();
        if escape && self.cancellable && self.textarea.read(cx).current_keymap() == Keymap::Vim {
            cx.emit(ComposerEvent::Cancel);
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

    /// The images the message takes along, as tiles above the text, each
    /// with its remove control.
    fn render_images(&self, cx: &mut Context<Self>) -> AnyElement {
        h_flex()
            .id("composer-images")
            .test_support()
            .flex_wrap()
            .gap_2()
            .pb_2()
            .children(self.images.iter().enumerate().map(|(index, attached)| {
                let name: SharedString = attached
                    .name
                    .clone()
                    .unwrap_or_else(|| "Pasted image".to_string())
                    .into();
                let tile = Attachment::new()
                    .id(("image", index))
                    .small()
                    .media(AttachmentMedia::new().src(attached.image.clone()))
                    .content(AttachmentContent::new().title(AttachmentTitle::new(name.clone())))
                    .on_remove(cx.listener(move |this, _, _, cx| this.remove_image(index, cx)));
                // A tile the model can't take says why.
                match self.refusal(attached) {
                    Some(why) => tile
                        .status(AttachmentStatus::Failed)
                        .tooltip(crate::backend::sentence(&why)),
                    None => tile.tooltip(name),
                }
            }))
            .into_any_element()
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
                // The choice Enter takes stands out. One only Tab takes
                // shows quieter.
                let selected = index == menu.selected;
                let (background, foreground) = if menu.takes_enter {
                    (theme.accent, theme.accent_foreground)
                } else {
                    (theme.secondary, theme.secondary_foreground)
                };
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
                    .when(selected, |this| this.bg(background).text_color(foreground))
                    .child(div().flex_none().code_font(theme).child(item.label.clone()))
                    .when(!item.hint.is_empty(), |this| {
                        this.child(
                            div()
                                .flex_none()
                                .code_font(theme)
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
                                foreground
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
            .children(menu.note.clone().map(|note| {
                div()
                    .id("slash-note")
                    .test_support()
                    .px_2()
                    .py_1()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(note)
            }))
    }
}

impl Render for Composer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let this = cx.entity().downgrade();
        let paste_lines = prefs::get(cx).input.paste_token_lines;
        let menu = self.menu.clone();
        let drop_target = cx.theme().drop_target;
        let tiles = (!self.images.is_empty()).then(|| self.render_images(cx));
        div()
            .id("composer-field")
            .test_support()
            .relative()
            .w_full()
            .key_context(CONTEXT)
            .capture_action(cx.listener(Self::on_move_up))
            .capture_action(cx.listener(Self::on_move_down))
            .capture_action(cx.listener(Self::on_tab))
            .capture_action(cx.listener(Self::on_enter))
            .capture_action(cx.listener(Self::on_escape))
            .on_key_down(cx.listener(Self::on_vim_escape))
            // Files dropped on the composer: images go with the message.
            .drag_over::<ExternalPaths>(move |style, _, _, _| style.bg(drop_target))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, window, cx| {
                this.attach_paths(paths.paths().to_vec(), window, cx)
            }))
            .when_some(menu, |this, menu| this.child(self.render_menu(&menu, cx)))
            .children(tiles)
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
                        // Images go with the message; copied files as dropped.
                        let mut images = Vec::new();
                        let mut paths = Vec::new();
                        for entry in item.entries() {
                            match entry {
                                ClipboardEntry::Image(image) => images.push(Attached::new(
                                    image.format(),
                                    image.bytes().to_vec(),
                                    None,
                                )),
                                ClipboardEntry::ExternalPaths(external) => {
                                    paths.extend(external.paths().iter().cloned())
                                }
                                _ => {}
                            }
                        }
                        if !images.is_empty() || !paths.is_empty() {
                            let this = this.clone();
                            window.defer(cx, move |window, cx| {
                                this.update(cx, |this, cx| {
                                    for image in images {
                                        this.attach_image(image, cx);
                                    }
                                    if !paths.is_empty() {
                                        this.attach_paths(paths, window, cx);
                                    }
                                })
                                .ok();
                            });
                            return true;
                        }
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

    #[test]
    fn the_slash_menu_knows_a_command_typed_in_full() {
        let reading = Reading {
            workers_running: vec!["r1".into(), "r10".into(), "r11".into()],
            ..Reading::default()
        };
        let menu = slash_menu("/stop-workers r1", &reading).unwrap();
        let words: Vec<&str> = menu.items.iter().map(|item| item.label.as_ref()).collect();
        assert_eq!(words, ["r10", "r11"]);
        assert!(menu.complete, "r1 runs too");
        assert!(!slash_menu("/stop-workers r", &reading).unwrap().complete);
        assert!(slash_menu("/handover", &reading).unwrap().complete);
        assert!(!slash_menu("/hand", &reading).unwrap().complete);
    }
}
