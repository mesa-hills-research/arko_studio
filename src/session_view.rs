//! The main column: the transcript's reading column and the composer under
//! it, for the session the window shows, or a new one before its first
//! message.
//!
//! The composer is a view of its own (typing draws only it). This view
//! decides what a sent message is: a new session's first message, a
//! message, a note while a turn runs, the answer to the agent's question,
//! or a command the app runs itself.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use gpui_kit::component::input::{self, Input, InputEvent, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    ThemeStyled as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Mode, ModelChoices, NewSession, SessionEntry, SharedBackend};
use crate::composer::{Attached, Composer, ComposerEvent, Outgoing};
use crate::editing::{self, Change, Choice};
use crate::layout::WidthClass;
use crate::live::{LiveEvent, LiveSession, LiveSessions, Phase, PromptKind};
use crate::transcript::{AgentFilter, FindStatus, TranscriptEvent, TranscriptView};
use crate::{drafts, menus, prefs};

/// How many recent folders Change… offers.
const RECENT_FOLDERS: usize = 8;

/// How long typing rests before the draft is kept.
const DRAFT_REST: std::time::Duration = std::time::Duration::from_millis(300);
/// The tallest the question card's text grows, in rem, before it scrolls.
pub const QUESTION_MAX_HEIGHT: f32 = 12.;

/// A new session, before its first message: how it will work, where and on
/// which model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub mode: Mode,
    /// The folder chosen with Change…. `None` follows the most recent one.
    pub folder: Option<PathBuf>,
    /// The model chosen in the chip's popover. `None`: the default.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// What the main column asks of the window.
#[derive(Debug, Clone)]
pub enum SessionViewEvent {
    /// A new session started with its first message: the window shows it.
    Created(Entity<LiveSession>),
    /// Open Settings › Models & accounts, on what the group of models named
    /// here needs ("OpenAI, with your API key"), if one is.
    OpenAccounts(Option<SharedString>),
    /// Show the session's waiting prompt again.
    ShowPrompt,
    /// The reader opened a worker's card in the transcript.
    OpenWorker(SharedString),
    /// A branch of the session was made: the window opens it.
    OpenBranch(Box<Branched>),
}

/// A branch of a session, to open.
#[derive(Debug, Clone)]
pub struct Branched {
    /// It lists with this until the list is read again.
    pub entry: SessionEntry,
    /// Its first message, sent as it opens.
    pub first: Option<arko::Message>,
    /// What its composer shows.
    pub draft: Option<String>,
}

/// The find bar at the top of the transcript (Mod+F).
struct FindBar {
    input: Entity<InputState>,
    /// Where the keyboard was: it goes back there when the bar closes.
    before: Option<FocusHandle>,
    status: FindStatus,
    _input: Subscription,
}

/// The key context of the find bar.
pub const FIND_CONTEXT: &str = "FindBar";

/// A message of the user's being edited in the composer.
struct Editing {
    /// The message, by its item.
    item: u64,
    /// What the composer held before, given back when the edit ends.
    draft: String,
    images: Vec<Attached>,
    /// How many of the message's images the session no longer keeps: the
    /// edit goes without them.
    lost_images: usize,
}

/// "1 image is no longer kept": what an edit or a resend says of the
/// message's images the session dropped.
fn images_not_kept(count: usize) -> Option<String> {
    match count {
        0 => None,
        1 => Some("1 image is no longer kept".to_string()),
        count => Some(format!("{count} images are no longer kept")),
    }
}

/// How many sessions' transcripts the window keeps ready: switching back to
/// one of them shows it at once, where it was.
const KEPT_TRANSCRIPTS: usize = 4;

/// A session's transcript, for the opening of its feed it shows.
struct KeptTranscript {
    session: EntityId,
    opening: u64,
    view: Entity<TranscriptView>,
    _events: Subscription,
}

impl EventEmitter<SessionViewEvent> for SessionView {}

/// What the main column shows: a session, or a new one not started yet.
pub struct SessionView {
    session: Option<Entity<LiveSession>>,
    draft: Draft,
    /// The folders of recent agent sessions, the latest first.
    recent_folders: Vec<PathBuf>,
    /// The user's home folder, the last place a new session falls back on.
    home: Option<PathBuf>,
    /// The models a new session can start on, for the folder they were
    /// listed for.
    models: Option<(Option<PathBuf>, ModelChoices)>,
    composer: Entity<Composer>,
    composer_empty: bool,
    width: WidthClass,
    /// The question collapsed with Later.
    later: Option<u64>,
    /// The composer's rows while expanded.
    expanded_rows: Option<usize>,
    /// The worker the composer writes to (`/message r1 …`), chosen in the
    /// workers panel.
    target: Option<SharedString>,
    /// The message the composer edits.
    editing: Option<Editing>,
    find: Option<FindBar>,
    model_filter: Entity<InputState>,
    /// The Chat | Agent switch in a new session's empty transcript, and the
    /// one in the chip's popover.
    mode_focus: FocusHandle,
    mode_menu_focus: FocusHandle,
    _session: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    _models: Option<Task<()>>,
    _draft: Option<Task<()>>,
    /// The transcripts of the sessions shown last, the latest last.
    transcripts: Vec<KeptTranscript>,
    /// Its window closed: it shows nothing and keeps no more drafts.
    closed: bool,
    /// The first message of the new session that is opening.
    first: Option<FirstMessage>,
    /// The images of a first message whose session didn't open, for the
    /// new session's composer.
    returned_images: Vec<Attached>,
}

/// A new session's first message, kept until the session opens.
struct FirstMessage {
    text: String,
    images: Vec<Attached>,
    _subscription: Subscription,
}

impl SessionView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| Composer::new(window, cx));
        let model_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Find a model"));
        let mut subscriptions = vec![
            cx.subscribe_in(&composer, window, Self::on_composer_event),
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                this.sync_expanded(window, cx)
            }),
            cx.observe_window_bounds(window, |this, window, cx| this.sync_expanded(window, cx)),
            cx.observe(&model_filter, |_, _, cx| cx.notify()),
            cx.on_release(|this, cx| this.window_closed(cx)),
        ];
        // ↑ in the empty composer edits the latest message, when it can.
        let view = cx.entity().downgrade();
        composer.update(cx, |composer, _| {
            composer.set_edits_last(move |cx| {
                view.upgrade()
                    .is_some_and(|view| view.read(cx).edits_last(cx))
            })
        });
        let view = cx.entity().downgrade();
        let window_id = window.window_handle().window_id();
        subscriptions.push(cx.on_window_closed({
            let view = view.clone();
            move |cx, closed| {
                if closed == window_id {
                    view.update(cx, |this, cx| this.window_closed(cx)).ok();
                }
            }
        }));
        drafts::keep_at_quit(
            move |cx| {
                view.update(cx, |this, cx| this.keep_draft(cx)).ok();
            },
            cx,
        );
        let mut this = Self {
            session: None,
            draft: Draft {
                mode: default_mode(cx),
                ..Draft::default()
            },
            recent_folders: Vec::new(),
            home: cx.global::<HomeFolder>().0.clone(),
            models: None,
            composer,
            composer_empty: true,
            width: WidthClass::Wide,
            later: None,
            expanded_rows: None,
            target: None,
            editing: None,
            find: None,
            model_filter,
            mode_focus: cx.focus_handle().tab_stop(true),
            mode_menu_focus: cx.focus_handle().tab_stop(true),
            _session: Vec::new(),
            _subscriptions: subscriptions,
            _models: None,
            _draft: None,
            transcripts: Vec::new(),
            closed: false,
            first: None,
            returned_images: Vec::new(),
        };
        this.restore_draft(window, cx);
        this.sync_expanded(window, cx);
        this.sync_composer(window, cx);
        this.list_models(cx);
        this
    }

    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// Makes the new session a chat or an agent session.
    pub fn set_draft_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.draft.mode != mode {
            self.draft.mode = mode;
            self.list_models(cx);
            self.sync_mentions(cx);
            cx.notify();
        }
    }

    /// Runs the new agent session in `folder`.
    pub fn set_draft_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.draft.folder = Some(folder);
        self.list_models(cx);
        self.sync_mentions(cx);
        cx.notify();
    }

    /// Starts the new session on `model` (as `/model` takes it).
    pub fn set_draft_model(
        &mut self,
        model: String,
        effort: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.draft.model = Some(model);
        self.draft.effort = effort;
        self.sync_image_model(cx);
        cx.notify();
    }

    /// The main agent's model, which the composer's images go to: the shown
    /// session's, or the one a new session would start on.
    fn image_model(&self, cx: &App) -> Option<String> {
        match &self.session {
            Some(session) => {
                let main = &session.read(cx).reading().models.main;
                (!main.is_empty()).then(|| main.clone())
            }
            None => self.draft_model(),
        }
    }

    fn sync_image_model(&mut self, cx: &mut Context<Self>) {
        let model = self.image_model(cx);
        self.composer
            .update(cx, |composer, cx| composer.set_image_model(model, cx));
    }

    /// The folders Change… offers, from the sessions in the sidebar.
    pub fn set_recent_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.recent_folders != folders {
            let before = self.draft_folder();
            self.recent_folders = folders;
            if self.draft_folder() != before {
                self.list_models(cx);
                self.sync_mentions(cx);
            }
            cx.notify();
        }
    }

    /// Where the new agent session will run: the folder chosen, else the
    /// most recent one, else the home folder.
    pub fn draft_folder(&self) -> Option<PathBuf> {
        self.draft
            .folder
            .clone()
            .or_else(|| self.recent_folders.first().cloned())
            .or_else(|| self.home.clone())
    }

    /// The folder whose `.env` files the new session reads: an agent's.
    fn models_folder(&self) -> Option<PathBuf> {
        match self.draft.mode {
            Mode::Agent => self.draft_folder(),
            Mode::Chat => None,
        }
    }

    /// Lists the models a new session can start on, in the background.
    fn list_models(&mut self, cx: &mut Context<Self>) {
        let folder = self.models_folder();
        if self
            .models
            .as_ref()
            .is_some_and(|(listed, _)| *listed == folder)
        {
            return;
        }
        let backend = cx.global::<SharedBackend>().clone();
        let listing = cx.background_spawn({
            let folder = folder.clone();
            async move { backend.models(folder.as_deref()) }
        });
        self._models = Some(cx.spawn(async move |this, cx| {
            let models = listing.await;
            this.update(cx, |this, cx| {
                this.models = Some((folder, models));
                this.sync_image_model(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The model a new session would start on.
    pub fn draft_model(&self) -> Option<String> {
        self.draft.model.clone().or_else(|| {
            self.models
                .as_ref()
                .and_then(|(_, models)| models.default.clone())
        })
    }

    /// The line the empty transcript of a new session shows: "New agent
    /// session in ~/src/arko", or "New chat". `None` once a session is open.
    pub fn draft_line(&self) -> Option<String> {
        if self.session.is_some() {
            return None;
        }
        Some(match self.draft.mode {
            Mode::Chat => "New chat".to_string(),
            Mode::Agent => match self.draft_folder() {
                Some(folder) => format!(
                    "New agent session in {}",
                    shown_folder(&folder, self.home.as_deref())
                ),
                None => "New agent session".to_string(),
            },
        })
    }

    pub fn live_session(&self) -> Option<&Entity<LiveSession>> {
        self.session.as_ref()
    }

    /// The open session's entry, for the window's title and tests.
    pub fn session(&self, cx: &App) -> Option<SessionEntry> {
        let session = self.session.as_ref()?.read(cx);
        Some(SessionEntry {
            id: session
                .id()
                .cloned()
                .unwrap_or_else(|| crate::backend::SessionId(String::new())),
            title: session.title().to_string(),
            mode: session.mode(),
            workspace: session.workspace().cloned(),
            model: session.model().to_string(),
            created: std::time::SystemTime::now(),
            last_active: None,
            conversations: Vec::new(),
            holder: None,
            had_workers: !session.workers().is_empty(),
        })
    }

    /// The key the composer's draft is kept under.
    fn draft_key(&self, cx: &App) -> Option<String> {
        match &self.session {
            None => Some(drafts::NEW_SESSION.to_string()),
            Some(session) => session.read(cx).id().map(|id| id.0.clone()),
        }
    }

    /// Its window closed: the session it showed is no longer shown (and is
    /// marked unseen from then on), and what was typed is kept.
    fn window_closed(&mut self, cx: &mut App) {
        self.keep_draft(cx);
        self.closed = true;
        self._session.clear();
        if let Some(session) = self.session.take() {
            session.update(cx, |session, cx| session.set_shown(false, cx));
        }
    }

    fn keep_draft(&mut self, cx: &mut App) {
        self._draft = None;
        if self.closed {
            return;
        }
        if let Some(key) = self.draft_key(cx) {
            // While a message is edited, the user's draft is the one put
            // aside, not the message in the composer.
            let text = match &self.editing {
                Some(editing) => editing.draft.clone(),
                None => self.composer.read(cx).expanded_text(cx),
            };
            drafts::set(&key, &text, cx);
        }
    }

    fn restore_draft(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .draft_key(cx)
            .and_then(|key| drafts::get(&key, cx))
            .unwrap_or_default();
        let images = if self.session.is_none() {
            std::mem::take(&mut self.returned_images)
        } else {
            Vec::new()
        };
        self.composer_empty = text.is_empty() && images.is_empty();
        self.composer.update(cx, |composer, cx| {
            composer.set_text(&text, window, cx);
            for image in images {
                composer.attach_image(image, cx);
            }
        });
    }

    /// Shows `session`, or a new session when `None`. The composer keeps
    /// the draft of the one it leaves and brings back the one it shows.
    pub fn set_session(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.as_ref().map(Entity::entity_id) == session.as_ref().map(Entity::entity_id) {
            return;
        }
        // The draft kept for the session left is what was there before
        // an edit.
        self.end_edit(window, cx);
        self.close_find(window, cx);
        self.keep_draft(cx);
        if let Some(old) = self.session.take() {
            old.update(cx, |old, cx| old.set_shown(false, cx));
        }
        self._session.clear();
        self.later = None;
        self.target = None;
        self.session = session;
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.set_shown(true, cx));
            self._session
                .push(cx.observe_in(session, window, |this, _, window, cx| {
                    this.sync_composer(window, cx);
                    this.sync_transcript(window, cx);
                    cx.notify();
                }));
        }
        self.restore_draft(window, cx);
        self.sync_composer(window, cx);
        self.sync_transcript(window, cx);
        cx.notify();
    }

    /// Makes the shown session's transcript ready: the one kept for it, or a
    /// new one once its feed is there (and again when it opens anew).
    fn sync_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Sessions that ended don't come back.
        self.transcripts.retain(|kept| {
            LiveSessions::all(cx)
                .iter()
                .any(|session| session.entity_id() == kept.session)
        });
        let Some(session) = self.session.clone() else {
            return;
        };
        let (opening, link) = {
            let session = session.read(cx);
            (session.opening(), session.link().cloned())
        };
        let Some(link) = link else {
            return;
        };
        let found = self
            .transcripts
            .iter()
            .position(|kept| kept.session == session.entity_id() && kept.opening == opening);
        let kept = match found {
            Some(index) => self.transcripts.remove(index),
            None => {
                self.transcripts
                    .retain(|kept| kept.session != session.entity_id());
                let view = cx.new(|cx| {
                    let mut view = TranscriptView::new(&link.feed, AgentFilter::Main, window, cx);
                    // The thumbnails of the session's images.
                    view.set_images(link.images.clone(), cx);
                    view
                });
                let events = cx.subscribe_in(&view, window, Self::on_transcript_event);
                KeptTranscript {
                    session: session.entity_id(),
                    opening,
                    view,
                    _events: events,
                }
            }
        };
        self.transcripts.push(kept);
        if self.transcripts.len() > KEPT_TRANSCRIPTS {
            self.transcripts.remove(0);
        }
    }

    /// The transcript the window shows, once the session's feed is there.
    pub fn transcript(&self, cx: &App) -> Option<&Entity<TranscriptView>> {
        let session = self.session.as_ref()?;
        let opening = session.read(cx).opening();
        self.transcripts
            .iter()
            .rev()
            .find(|kept| kept.session == session.entity_id() && kept.opening == opening)
            .map(|kept| &kept.view)
    }

    pub fn set_width(&mut self, width: WidthClass, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }

    /// The composer.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        let textarea = self.composer.read(cx).textarea().clone();
        textarea.update(cx, |textarea, cx| textarea.focus(window, cx));
    }

    /// The mode the composer's chip shows: the open session's, or the
    /// new session's.
    fn mode(&self, cx: &App) -> Mode {
        self.session
            .as_ref()
            .map_or(self.draft.mode, |session| session.read(cx).mode())
    }

    /// The question the composer answers: the agent's, unless collapsed.
    fn question(&self, cx: &App) -> Option<(u64, SharedString)> {
        let question = self.session.as_ref()?.read(cx).question()?;
        Some((question.id, question.text.clone().into()))
    }

    /// What the composer says while empty.
    pub fn placeholder(&self, cx: &App) -> SharedString {
        let Some(session) = &self.session else {
            return "Message Arko".into();
        };
        if let Some(target) = &self.target {
            return format!("Message {target}").into();
        }
        let session = session.read(cx);
        if let Some(question) = session.question() {
            if self.later == Some(question.id) {
                "Answer Arko’s question".into()
            } else {
                "Answer the question above".into()
            }
        } else if session.is_working() {
            "Add to the task — Arko is working".into()
        } else {
            "Message Arko".into()
        }
    }

    /// The worker the composer writes to, if any.
    pub fn target(&self) -> Option<&SharedString> {
        self.target.as_ref()
    }

    /// Makes the composer write to the worker `target` (`/message r1 …`),
    /// or to the main agent again (`None`).
    pub fn set_target(
        &mut self,
        target: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.target != target {
            // Writing to a worker and editing a message are two modes Esc
            // ends: one at a time.
            if target.is_some() {
                self.end_edit(window, cx);
            }
            self.target = target;
            self.sync_composer(window, cx);
            cx.notify();
        }
    }

    /// Gives the composer the session's state: its placeholder, what the
    /// slash menu completes, whether Esc stops a turn.
    fn sync_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.placeholder(cx);
        let (reading, working) = match &self.session {
            Some(session) => {
                let session = session.read(cx);
                (session.reading().clone(), session.is_working())
            }
            None => (Arc::default(), false),
        };
        let agent = self.mode(cx) == Mode::Agent;
        let cancellable = self.target.is_some() || self.editing.is_some();
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(&placeholder, window, cx);
            composer.set_session_state(reading, working, agent);
            composer.set_cancellable(cancellable);
        });
        self.sync_image_model(cx);
        self.sync_mentions(cx);
    }

    /// "@" in the composer mentions the files of the folder an agent works
    /// in (the session's, or the new session's), and nothing in a chat.
    fn sync_mentions(&mut self, cx: &mut Context<Self>) {
        let folder = match self.mode(cx) {
            Mode::Chat => None,
            Mode::Agent => match &self.session {
                Some(session) => session.read(cx).workspace().cloned(),
                None => self.draft_folder(),
            },
        };
        self.composer
            .update(cx, |composer, _| composer.set_mention_folder(folder));
    }

    /// The composer's height follows the Expand setting: about 45% of the
    /// window, or growing with its text.
    fn sync_expanded(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let rows = prefs::get(cx).input.composer_expanded.then(|| {
            let line = f32::from(window.rem_size()) * 1.5;
            let tall = f32::from(window.viewport_size().height) * 0.45;
            ((tall / line).floor() as usize).max(3)
        });
        if rows != self.expanded_rows {
            self.expanded_rows = rows;
            self.composer
                .update(cx, |composer, cx| composer.set_rows(rows, cx));
            cx.notify();
        }
    }

    fn on_composer_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit(outgoing) => self.send(outgoing.clone(), window, cx),
            ComposerEvent::Stop => match self.question(cx) {
                // Esc backs out of the question card first, as Later does.
                Some((id, _)) if self.later != Some(id) => {
                    self.later = Some(id);
                    self.sync_composer(window, cx);
                    cx.notify();
                }
                _ => self.stop(window, cx),
            },
            ComposerEvent::Cancel => self.cancel(window, cx),
            ComposerEvent::EditLast => {
                if let Some(last) = self
                    .transcript(cx)
                    .and_then(|transcript| transcript.read(cx).last_message())
                {
                    self.edit_message(last, window, cx);
                }
            }
            ComposerEvent::Edited => {
                let composer = self.composer.read(cx);
                let empty =
                    composer.textarea().read(cx).text().len() == 0 && composer.images().is_empty();
                if empty != self.composer_empty {
                    self.composer_empty = empty;
                    cx.notify();
                }
                // The draft is kept once typing rests.
                self._draft = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(DRAFT_REST).await;
                    this.update(cx, |this, cx| this.keep_draft(cx)).ok();
                }));
            }
        }
    }

    /// Esc in the composer, in a mode Esc ends: the edit ends, or the
    /// composer writes to the main agent again.
    fn cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.end_edit(window, cx);
        } else {
            self.set_target(None, window, cx);
        }
    }

    fn on_transcript_event(
        &mut self,
        _: &Entity<TranscriptView>,
        event: &TranscriptEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TranscriptEvent::OpenWorker(id) => cx.emit(SessionViewEvent::OpenWorker(id.clone())),
            TranscriptEvent::Edit(item) => self.edit_message(*item, window, cx),
            TranscriptEvent::Regenerate => self.regenerate(window, cx),
            TranscriptEvent::Branch(item) => self.branch_from(Some(*item), window, cx),
            TranscriptEvent::Leave => self.focus_composer(window, cx),
            TranscriptEvent::FindStatus(status) => {
                if let Some(find) = &mut self.find {
                    find.status = *status;
                    cx.notify();
                }
            }
        }
    }

    /// Exports the conversation shown as Markdown, to a file the user
    /// chooses (Export as Markdown…).
    pub fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (items, starts_partway) = self
            .transcript(cx)
            .map(|transcript| {
                let transcript = transcript.read(cx);
                (transcript.export_items(), transcript.starts_partway())
            })
            .unwrap_or_default();
        let Some(session) = self.session.as_ref().filter(|_| !items.is_empty()) else {
            window.push_notification("There is no conversation to export yet.", cx);
            return;
        };
        let session = session.read(cx);
        let header = crate::export::Header {
            title: session.title().to_string(),
            mode: session.mode().label().to_string(),
            model: session.model().to_string(),
            folder: session
                .workspace()
                .map(|folder| shown_folder(folder, self.home.as_deref())),
            when: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
            starts_partway,
        };
        let export = Export {
            text: crate::export::markdown(&header, &items),
            starts_partway,
        };
        let folder = crate::export::folder(
            session.workspace().map(PathBuf::as_path),
            self.home.as_deref(),
        );
        let name = crate::export::file_name(session.title());
        let chosen = cx.prompt_for_new_path(&folder, Some(&name));
        cx.spawn_in(window, async move |this, cx| {
            let answer = chosen
                .await
                .map_err(|_| "it closed without an answer".to_string());
            let path = match answer.and_then(|chosen| chosen.map_err(|error| error.to_string())) {
                Ok(Some(path)) => path,
                // Cancelled.
                Ok(None) => return,
                Err(why) => {
                    let instead = crate::export::free_path(&folder, &name);
                    this.update_in(cx, |this, window, cx| {
                        this.offer_export_to(export, instead, why, window, cx)
                    })
                    .ok();
                    return;
                }
            };
            cx.update(|window, cx| write_export(export, path, window, cx))
                .ok();
        })
        .detach();
    }

    /// The save dialog couldn't open (on Linux, without a desktop portal):
    /// asks whether to save the export as `path` instead.
    pub(crate) fn offer_export_to(
        &mut self,
        export: Export,
        path: PathBuf,
        why: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let description = format!(
            "The save dialog couldn’t open ({why}). Save the conversation as {} instead?",
            path.display()
        );
        let export = Rc::new(RefCell::new(Some(export)));
        window.open_alert_dialog(cx, move |dialog, _, _| {
            let (export, path) = (export.clone(), path.clone());
            dialog
                .title("Export as Markdown")
                .description(description.clone())
                .confirm()
                .ok_text("Save")
                .on_ok(move |_, window, cx| {
                    if let Some(export) = export.borrow_mut().take() {
                        write_export(export, path.clone(), window, cx);
                    }
                    true
                })
        });
    }

    /// The keyboard goes to the transcript's rows, or from them back to the
    /// composer (F6).
    pub fn focus_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(transcript) = self.transcript(cx).cloned() else {
            return;
        };
        if transcript
            .read(cx)
            .focus_handle(cx)
            .contains_focused(window, cx)
        {
            self.focus_composer(window, cx);
        } else {
            transcript.update(cx, |transcript, cx| transcript.focus_rows(window, cx));
        }
    }

    /// Opens the find bar at the top of the transcript, or gives it the
    /// keyboard again with its words selected.
    pub fn open_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.transcript(cx).is_none() {
            return;
        }
        if let Some(find) = &self.find {
            find.input.update(cx, |input, cx| {
                input.focus(window, cx);
                input.select_all(window, cx);
            });
            return;
        }
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Find in the conversation"));
        let subscription = cx.subscribe_in(&input, window, Self::on_find_input);
        let before = window.focused(cx);
        input.update(cx, |input, cx| input.focus(window, cx));
        self.find = Some(FindBar {
            input,
            before,
            status: FindStatus::default(),
            _input: subscription,
        });
        cx.notify();
    }

    /// Closes the find bar: the highlights go, and the keyboard goes back
    /// where it was.
    pub fn close_find(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(find) = self.find.take() else {
            return;
        };
        if let Some(transcript) = self.transcript(cx).cloned() {
            transcript.update(cx, |transcript, cx| transcript.clear_find(cx));
        }
        match find.before {
            Some(before) => before.focus(window, cx),
            None => self.focus_composer(window, cx),
        }
        cx.notify();
    }

    /// Whether the find bar shows, and where its find is.
    pub fn find_status(&self) -> Option<FindStatus> {
        self.find.as_ref().map(|find| find.status)
    }

    /// The next match (`forward`) or the previous one, from anywhere in
    /// the window (F3, Shift+F3). Find opens when it isn't open.
    pub fn find_again(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.find.is_some() {
            self.find_next(forward, cx);
        } else {
            self.open_find(window, cx);
        }
    }

    /// The next match down the transcript (`forward`), or the previous.
    pub fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(transcript) = self.transcript(cx).cloned() else {
            return;
        };
        let status = transcript.update(cx, |transcript, cx| transcript.find_next(forward, cx));
        if let Some(find) = &mut self.find {
            find.status = status;
        }
        cx.notify();
    }

    fn on_find_input(
        &mut self,
        input: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                let query = input.read(cx).value().to_string();
                let Some(transcript) = self.transcript(cx).cloned() else {
                    return;
                };
                let status = transcript.update(cx, |transcript, cx| transcript.find(&query, cx));
                if let Some(find) = &mut self.find {
                    find.status = status;
                }
                cx.notify();
            }
            InputEvent::PressEnter { shift, .. } => self.find_next(!*shift, cx),
            _ => {}
        }
    }

    /// The find bar: the words, where the find is, previous and next, and
    /// close.
    fn render_find(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = cx.theme();
        let has_words = !find.input.read(cx).value().trim().is_empty();
        let status = find_status_text(find.status, has_words);
        let key_tip = |label: &'static str, key: &'static str| {
            move |window: &mut Window, cx: &mut App| {
                Tooltip::new(label)
                    .key_binding(Keystroke::parse(key).ok().map(Kbd::new))
                    .build(window, cx)
            }
        };
        let none = find.status.total == 0;
        Some(
            h_flex()
                .id("find-bar")
                .test_support()
                .key_context(FIND_CONTEXT)
                .capture_action(cx.listener(|this, _: &input::Escape, window, cx| {
                    this.close_find(window, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|this, _: &input::MoveUp, _, cx| {
                    this.find_next(false, cx);
                    cx.stop_propagation();
                }))
                .capture_action(cx.listener(|this, _: &input::MoveDown, _, cx| {
                    this.find_next(true, cx);
                    cx.stop_propagation();
                }))
                .w_full()
                .max_w(rems(46.))
                .px_4()
                .pt_2()
                .child(
                    h_flex()
                        .w_full()
                        .gap_1()
                        .p_1()
                        .rounded(theme.radius_lg)
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.background)
                        .shadow_sm()
                        .child(
                            div().flex_1().min_w_0().child(
                                Input::new(&find.input).small().appearance(false).prefix(
                                    Icon::new(IconName::Search)
                                        .xsmall()
                                        .text_color(theme.muted_foreground),
                                ),
                            ),
                        )
                        .child(
                            div()
                                .id("find-status")
                                .test_support()
                                .flex_none()
                                .px_1()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .font_features(FontFeatures(Arc::new(vec![("tnum".into(), 1)])))
                                .child(status),
                        )
                        // A match past the lines its call's details show:
                        // the whole output opens at it.
                        .when(find.status.in_full_output, |this| {
                            this.child(
                                Button::new("find-show-full")
                                    .ghost()
                                    .xsmall()
                                    .label("Show")
                                    .tooltip("Open the full output at this match")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        if let Some(transcript) = this.transcript(cx).cloned() {
                                            transcript.update(cx, |transcript, cx| {
                                                transcript.show_match_in_full(window, cx)
                                            });
                                        }
                                    })),
                            )
                        })
                        .child(
                            div()
                                .id("find-previous-tip")
                                .tooltip(key_tip("Previous match", "shift-enter"))
                                .child(
                                    Button::new("find-previous")
                                        .ghost()
                                        .xsmall()
                                        .compact()
                                        .icon(IconName::ChevronUp)
                                        .disabled(none)
                                        .accessibility_label("Previous match")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.find_next(false, cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("find-next-tip")
                                .tooltip(key_tip("Next match", "enter"))
                                .child(
                                    Button::new("find-next")
                                        .ghost()
                                        .xsmall()
                                        .compact()
                                        .icon(IconName::ChevronDown)
                                        .disabled(none)
                                        .accessibility_label("Next match")
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.find_next(true, cx)),
                                        ),
                                ),
                        )
                        .child(
                            div()
                                .id("find-close-tip")
                                .tooltip(key_tip("Close", "escape"))
                                .child(
                                    Button::new("find-close")
                                        .ghost()
                                        .xsmall()
                                        .compact()
                                        .icon(IconName::Close)
                                        .accessibility_label("Close")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_find(window, cx)
                                        })),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Whether the composer edits a message.
    pub fn is_editing(&self) -> bool {
        self.editing.is_some()
    }

    /// Whether ↑ in the empty composer edits the latest message now: the
    /// session is between turns, no worker runs, and there is one.
    fn edits_last(&self, cx: &App) -> bool {
        let idle = self
            .session
            .as_ref()
            .is_some_and(|session| !session.read(cx).is_working());
        idle && self.editing.is_none()
            && self.target.is_none()
            && self.transcript(cx).is_some_and(|transcript| {
                let transcript = transcript.read(cx);
                transcript.busy_reason().is_none() && transcript.last_message().is_some()
            })
    }

    /// The images `images` of the session's store, as the composer takes
    /// them: those it still keeps. Also how many it no longer keeps.
    fn stored_images(&self, images: &[arko::images::ImageRef], cx: &App) -> (Vec<Attached>, usize) {
        let store = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).link())
            .map(|link| link.images.clone());
        let kept: Vec<Attached> = images
            .iter()
            .filter_map(|image| {
                let stored = store.as_ref()?.get(image.id)?;
                let format = ImageFormat::from_mime_type(&stored.mime)?;
                let name = Some(stored.label.clone()).filter(|label| !label.is_empty());
                Some(Attached::new(format, stored.bytes.to_vec(), name))
            })
            .collect();
        let lost = images.len() - kept.len();
        (kept, lost)
    }

    /// Puts the user's message `item` in the composer to edit: sending it
    /// cuts the conversation back to just before the message and sends the
    /// new text. Esc gives back what the composer held.
    pub fn edit_message(&mut self, item: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(transcript) = self.transcript(cx).cloned() else {
            return;
        };
        let (message, busy) = {
            let transcript = transcript.read(cx);
            if !transcript.is_editable(item) {
                return;
            }
            (transcript.message(item), transcript.busy_reason())
        };
        if let Some(busy) = busy {
            window.push_notification(format!("{busy}."), cx);
            return;
        }
        let Some((text, images)) = message else {
            return;
        };
        let (images, lost_images) = self.stored_images(&images, cx);
        match &mut self.editing {
            Some(editing) => {
                editing.item = item;
                editing.lost_images = lost_images;
            }
            None => {
                let composer = self.composer.read(cx);
                self.editing = Some(Editing {
                    item,
                    draft: composer.expanded_text(cx),
                    images: composer.images().to_vec(),
                    lost_images,
                });
            }
        }
        self.target = None;
        self.composer.update(cx, |composer, cx| {
            composer.set_text(&text, window, cx);
            composer.set_images(images, cx);
        });
        self.sync_composer(window, cx);
        self.focus_composer(window, cx);
        cx.notify();
    }

    /// Ends the edit: the composer gets back what it held.
    pub fn end_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(editing) = self.editing.take() else {
            return;
        };
        self.composer.update(cx, |composer, cx| {
            composer.set_text(&editing.draft, window, cx);
            composer.set_images(editing.images, cx);
        });
        self.sync_composer(window, cx);
        cx.notify();
    }

    /// Sends the edited message: after asking (unless the user said not to)
    /// whether to cut the conversation back or branch instead.
    fn send_edit(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = self.editing.as_ref().map(|editing| editing.item) else {
            return;
        };
        // A turn that started meanwhile (from the web page, the workers'
        // news): the edit waits in the composer, and the user knows why.
        if let Some(busy) = self
            .transcript(cx)
            .and_then(|transcript| transcript.read(cx).busy_reason())
        {
            window.push_notification(format!("{busy}."), cx);
            return;
        }
        let message = arko::Message {
            text: outgoing.text,
            images: outgoing.images.iter().map(Attached::to_arko).collect(),
        };
        if !prefs::get(cx).sessions.ask_before_edit {
            self.finish_edit(Choice::Change, item, message, window, cx);
            return;
        }
        let (after, no_branch) = self.transcript(cx).map_or((0, None), |transcript| {
            let transcript = transcript.read(cx);
            (
                transcript.messages_after(item),
                transcript
                    .branch_loses_context(item)
                    .then_some(editing::BRANCH_LOSES_THE_SUMMARY),
            )
        });
        let agent = self.mode(cx) == Mode::Agent;
        let view = cx.entity().downgrade();
        editing::confirm(
            Change::Edit,
            after,
            agent,
            no_branch,
            window,
            cx,
            move |choice, window, cx| {
                let message = message.clone();
                view.update(cx, |view, cx| {
                    view.finish_edit(choice, item, message, window, cx)
                })
                .ok();
            },
        );
    }

    fn finish_edit(
        &mut self,
        choice: Choice,
        item: u64,
        message: arko::Message,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let text = message.text.clone();
        match choice {
            Choice::Change => {
                let editing = session.read(cx).edit(item, message, cx);
                cx.spawn_in(window, async move |this, cx| {
                    let edited = editing.await;
                    this.update_in(cx, |this, window, cx| match edited {
                        Ok(()) => this.edited(&text, window, cx),
                        Err(why) => window.push_notification(why, cx),
                    })
                    .ok();
                })
                .detach();
            }
            Choice::Branch => {
                // The edit ends once the branch exists: refused, it stays.
                self.branch(
                    session,
                    Some(item),
                    Some(message),
                    None,
                    Some(text),
                    window,
                    cx,
                );
            }
        }
    }

    /// The edit was taken: the history keeps it, and the composer gets back
    /// what it held before.
    fn edited(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.composer
            .update(cx, |composer, cx| composer.sent(text, window, cx));
        self.end_edit(window, cx);
    }

    /// Regenerate: sends the user's last message again for a new reply,
    /// after asking (unless the user said not to) whether to branch
    /// instead.
    pub fn regenerate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(transcript) = self.transcript(cx).cloned() else {
            return;
        };
        let (last, busy) = {
            let transcript = transcript.read(cx);
            (transcript.last_message(), transcript.busy_reason())
        };
        if let Some(busy) = busy {
            window.push_notification(format!("{busy}."), cx);
            return;
        }
        let Some(last) = last else {
            return;
        };
        if !prefs::get(cx).sessions.ask_before_edit {
            self.finish_regenerate(Choice::Change, last, window, cx);
            return;
        }
        let after = transcript.read(cx).messages_after(last);
        let no_branch = transcript
            .read(cx)
            .branch_loses_context(last)
            .then_some(editing::BRANCH_LOSES_THE_SUMMARY);
        let agent = self.mode(cx) == Mode::Agent;
        let view = cx.entity().downgrade();
        editing::confirm(
            Change::Regenerate,
            after,
            agent,
            no_branch,
            window,
            cx,
            move |choice, window, cx| {
                view.update(cx, |view, cx| {
                    view.finish_regenerate(choice, last, window, cx)
                })
                .ok();
            },
        );
    }

    fn finish_regenerate(
        &mut self,
        choice: Choice,
        last: u64,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        match choice {
            Choice::Change => {
                let regenerating = session.read(cx).regenerate(cx);
                cx.spawn_in(window, async move |_, cx| {
                    if let Err(why) = regenerating.await {
                        cx.update(|window, cx| window.push_notification(why, cx))
                            .ok();
                    }
                })
                .detach();
            }
            Choice::Branch => {
                // The new session sends the message again, images and all.
                let message = self
                    .transcript(cx)
                    .and_then(|transcript| transcript.read(cx).message(last));
                let first = message.map(|(text, images)| {
                    let (images, lost) = self.stored_images(&images, cx);
                    if let Some(note) = images_not_kept(lost) {
                        let them = if lost == 1 { "it" } else { "them" };
                        window.push_notification(
                            format!("{note}, so the branch sends the message without {them}."),
                            cx,
                        );
                    }
                    arko::Message {
                        text,
                        images: images.iter().map(Attached::to_arko).collect(),
                    }
                });
                self.branch(session, Some(last), first, None, None, window, cx);
            }
        }
    }

    /// Branch: a new session with the conversation before the user's
    /// message `before` (or all of it), which the window opens. Branching
    /// from a message puts its text in the new session's composer.
    pub fn branch_from(
        &mut self,
        before: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        let transcript = self.transcript(cx).map(|transcript| transcript.read(cx));
        if let Some(busy) = transcript.and_then(|transcript| transcript.busy_reason()) {
            window.push_notification(format!("{busy}."), cx);
            return;
        }
        let draft = before
            .and_then(|item| transcript.and_then(|transcript| transcript.message(item)))
            .map(|(text, _)| text);
        self.branch(session, before, None, draft, None, window, cx);
    }

    /// Branches `session` before the message `before` (or all of it), and
    /// opens the branch, with `first` as its first message or `draft` in its
    /// composer. `edit` is the edited text the branch goes on with: the
    /// edit ends once the branch exists, and stays if it can't be made.
    #[allow(clippy::too_many_arguments)]
    fn branch(
        &mut self,
        session: Entity<LiveSession>,
        before: Option<u64>,
        first: Option<arko::Message>,
        draft: Option<String>,
        edit: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Before the first message there is nothing to copy: the message
        // starts a new session of the same kind instead.
        let from_start = before.is_some_and(|item| {
            self.transcript(cx)
                .is_some_and(|transcript| transcript.read(cx).message_index(item) == Some(0))
        });
        if from_start && let Some(first) = first {
            let new = {
                let session = session.read(cx);
                let models = &session.reading().models;
                NewSession {
                    mode: session.mode(),
                    folder: session.workspace().cloned(),
                    model: Some(models.main.clone()).filter(|model| !model.is_empty()),
                    effort: Some(models.main_effort.clone()).filter(|effort| !effort.is_empty()),
                }
            };
            if let Some(text) = &edit {
                self.edited(text, window, cx);
            }
            let created = LiveSession::create(new, arko::Input::Message(first), cx);
            cx.emit(SessionViewEvent::Created(created));
            return;
        }
        let branching = session.read(cx).branch(before, cx);
        cx.spawn_in(window, async move |this, cx| {
            let branched = branching.await;
            this.update_in(cx, |this, window, cx| match branched {
                Ok(id) => {
                    if let Some(text) = &edit {
                        this.edited(text, window, cx);
                    }
                    let entry = session.read(cx).branch_entry(id);
                    cx.emit(SessionViewEvent::OpenBranch(Box::new(Branched {
                        entry,
                        first,
                        draft,
                    })));
                }
                Err(why) => window.push_notification(why, cx),
            })
            .ok();
        })
        .detach();
    }

    /// Sends what the composer holds (the Send button).
    pub fn send_composer(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| composer.send(cx));
    }

    /// Sends `text`: runs the app's own commands, starts a new session with
    /// it, or gives it to the open session as a message, a note or an
    /// answer. The composer empties once it is taken.
    pub fn send(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.send_edit(outgoing, window, cx);
            return;
        }
        let Outgoing { text, images } = outgoing;
        if images.is_empty() {
            match self.app_command(text.trim(), window, cx) {
                Ok(true) => {
                    self.composer
                        .update(cx, |composer, cx| composer.sent(&text, window, cx));
                    return;
                }
                Ok(false) => {}
                // Left in the composer to put right.
                Err(why) => {
                    window.push_notification(why, cx);
                    return;
                }
            }
        }
        let attached = images;
        // Images the model can't take stay, with the message, until the
        // user removes them or picks another model.
        if !attached.is_empty()
            && let Some(why) = self.composer.read(cx).image_refusal()
        {
            window.push_notification(
                format!(
                    "The message wasn't sent: {why}. Send it without images, or switch to a model that takes them."
                ),
                cx,
            );
            return;
        }
        let images: Vec<arko::Attachment> = attached.iter().map(|image| image.to_arko()).collect();
        let message = arko::Message {
            text: text.clone(),
            images,
        };
        // As in the terminal, a command runs while a question waits, and the
        // question stays open. Other text answers it.
        let command = arko::commands::find(&text).is_some();
        let result = match (&self.session, &self.target) {
            (Some(_), Some(_)) if !message.images.is_empty() => Err(
                "A message to a worker takes text only. Remove the images, or send them to Arko."
                    .to_string(),
            ),
            (Some(session), Some(worker)) => {
                // As `/message r1 …` typed in the terminal.
                let input = arko::Input::Message(arko::Message {
                    text: format!("/message {worker} {}", text.trim()),
                    images: Vec::new(),
                });
                session.update(cx, |session, cx| session.send(input, cx))
            }
            (session, _) => match session {
                None => self.start_session(message, attached, cx),
                Some(session) => match session.read(cx).question() {
                    Some(_) if command => session.update(cx, |session, cx| {
                        session.send(arko::Input::Message(message), cx)
                    }),
                    Some(_) if !message.images.is_empty() => Err(
                        "An answer takes text only. Send the images once the question is answered."
                            .to_string(),
                    ),
                    Some(question) => {
                        let input = arko::Input::Answer {
                            question: question.id,
                            text: text.clone(),
                        };
                        session.update(cx, |session, cx| session.send(input, cx))
                    }
                    None => session.update(cx, |session, cx| {
                        session.send(arko::Input::Message(message), cx)
                    }),
                },
            },
        };
        match result {
            Ok(()) => {
                self.composer
                    .update(cx, |composer, cx| composer.sent(&text, window, cx));
                self.keep_draft(cx);
                self.composer_empty = true;
                cx.notify();
            }
            Err(why) => window.push_notification(why, cx),
        }
    }

    /// Runs Arko's command `name` as if typed: at once when it takes no
    /// arguments and a session is open, else it waits in the composer for
    /// its words (or for the new session's first message).
    pub fn run_slash(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let takes_words = arko::commands::find(&format!("{name} x")).is_some();
        if self.session.is_some() && !takes_words {
            self.send(
                Outgoing {
                    text: name.to_string(),
                    images: Vec::new(),
                },
                window,
                cx,
            );
            return;
        }
        let text = if takes_words {
            format!("{name} ")
        } else {
            name.to_string()
        };
        self.composer.update(cx, |composer, cx| {
            composer.set_text(&text, window, cx);
            composer.show_menu(cx);
        });
        self.focus_composer(window, cx);
    }

    /// Runs the commands the app handles itself, `false` for the others,
    /// and says why when one can't run.
    fn app_command(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, String> {
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("/view"), Some(view), None) => {
                let view = match view {
                    "full" => prefs::TranscriptView::Full,
                    "quiet" => prefs::TranscriptView::Quiet,
                    "messages" => prefs::TranscriptView::Messages,
                    _ => return Ok(false),
                };
                prefs::update(cx, |prefs| prefs.view = view);
                Ok(true)
            }
            (Some("/settings"), None, None) => {
                window.dispatch_action(Box::new(menus::OpenSettings), cx);
                Ok(true)
            }
            // A new session's model, before it starts: one of those listed,
            // by its name or its alias, once the list is known.
            (Some("/model"), Some(model), effort)
                if self.session.is_none() && model != "worker" =>
            {
                let name = match &self.models {
                    Some((_, choices)) => choices
                        .groups
                        .iter()
                        .flat_map(|group| &group.models)
                        .find(|choice| choice.name == model || choice.alias == model)
                        .map(|choice| choice.name.clone())
                        .ok_or_else(|| format!("Arko has no model called “{model}”."))?,
                    None => model.to_string(),
                };
                self.set_draft_model(name, effort.map(str::to_string), cx);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Starts the new session with `message` as its first.
    fn start_session(
        &mut self,
        message: arko::Message,
        images: Vec<Attached>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let folder = match self.draft.mode {
            Mode::Agent => Some(
                self.draft_folder()
                    .ok_or("Choose a folder for the agent to work in first.")?,
            ),
            Mode::Chat => None,
        };
        let new = NewSession {
            mode: self.draft.mode,
            folder,
            model: self.draft.model.clone(),
            effort: self.draft.effort.clone(),
        };
        let text = message.text.clone();
        let session = LiveSession::create(new, arko::Input::Message(message), cx);
        drafts::set(drafts::NEW_SESSION, "", cx);
        // Should it not open, the first message comes back to the composer.
        let subscription = cx.subscribe(&session, |this, _, event: &LiveEvent, cx| match event {
            LiveEvent::Failed(_) => {
                if let Some(first) = this.first.take() {
                    drafts::set(drafts::NEW_SESSION, &first.text, cx);
                    this.returned_images = first.images;
                }
            }
            LiveEvent::Opened => this.first = None,
            _ => {}
        });
        self.first = Some(FirstMessage {
            text,
            images,
            _subscription: subscription,
        });
        self.draft.model = None;
        self.draft.effort = None;
        // The next new session starts as Settings say.
        self.set_draft_mode(default_mode(cx), cx);
        cx.emit(SessionViewEvent::Created(session));
        Ok(())
    }

    /// Stops the turn that runs (Esc, Stop, Mod+.).
    pub fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.read(cx).is_working() {
            return;
        }
        if let Err(why) = session.update(cx, |session, cx| session.send(arko::Input::Interrupt, cx))
        {
            window.push_notification(why, cx);
        }
    }

    /// Moves the shell command the main agent runs to the background
    /// (Ctrl+B).
    pub fn move_to_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let running = session
            .read(cx)
            .reading()
            .commands
            .iter()
            .any(|agent| agent == arko::remote::MAIN_AGENT);
        if !running {
            window.push_notification("No command is running in the foreground.", cx);
            return;
        }
        let input = arko::Input::Background(arko::remote::MAIN_AGENT.to_string());
        if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
            window.push_notification(why, cx);
        }
    }

    /// Attach: images go with the message; in an agent session other files
    /// go in the text as their paths.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let agent = self.mode(cx) == Mode::Agent;
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: agent,
            multiple: true,
            prompt: Some("Attach".into()),
        });
        let composer = self.composer.downgrade();
        cx.spawn_in(window, async move |_, cx| {
            if let Ok(Ok(Some(paths))) = paths.await {
                composer
                    .update_in(cx, |composer, window, cx| {
                        composer.attach_paths(paths, window, cx);
                        composer.focus(window, cx);
                    })
                    .ok();
            }
        })
        .detach();
    }

    /// The chip's text: the mode, and the model where there's room ("Agent
    /// · Opus 5.5 · high"), on narrow windows the model alone.
    pub fn chip_label(&self, cx: &App) -> String {
        let mode = self.mode(cx).label();
        let (model, effort) = match &self.session {
            Some(session) => {
                let session = session.read(cx);
                let models = &session.reading().models;
                (
                    Some(session.model().to_string()).filter(|model| !model.is_empty()),
                    Some(models.main_effort.clone()).filter(|effort| !effort.is_empty()),
                )
            }
            None => (
                self.draft_model()
                    .map(|model| arko::models::display_name(&model)),
                self.draft.effort.clone(),
            ),
        };
        match (model, self.width) {
            (Some(model), WidthClass::Narrow) => model,
            (Some(model), _) => {
                let mut label = format!("{mode} · {model}");
                if let Some(effort) = effort {
                    label.push_str(&format!(" · {effort}"));
                }
                label
            }
            (None, _) => mode.to_string(),
        }
    }

    /// The chip's popover: Chat or Agent, then the models by the way they
    /// are reached, then the effort of the chosen one.
    fn render_mode_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        Popover::new("mode-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(
                Button::new("mode-model")
                    .ghost()
                    .small()
                    .label(self.chip_label(cx))
                    .dropdown_caret(true),
            )
            .content(move |_, window, cx| {
                let popover = cx.entity();
                let Some(view) = view.upgrade() else {
                    return div().into_any_element();
                };
                view.update(cx, |view, cx| view.render_mode_popover(popover, window, cx))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_mode_popover(
        &mut self,
        popover: Entity<gpui_kit::component::popover::PopoverState>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let started = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).has_started());
        let view = cx.entity().downgrade();
        let filter = self.model_filter.read(cx).value().to_lowercase();

        // The models: the new session's, or the open session's from its
        // status (the workers' only, once its conversation started).
        let (choices, current, workers_only) = match &self.session {
            None => (
                self.models
                    .as_ref()
                    .map(|(_, models)| models.clone())
                    .unwrap_or_default(),
                self.draft_model(),
                false,
            ),
            Some(session) => {
                let models = session.read(cx).reading().models.clone();
                let groups = arko::models::menu(&models, |name| {
                    arko::models::needs(&models.reachable, name)
                });
                let current = if started {
                    Some(models.workers.clone())
                } else {
                    Some(models.main.clone())
                };
                (
                    ModelChoices::from_arko(groups, None),
                    current.filter(|model| !model.is_empty()),
                    started,
                )
            }
        };
        let has_workers = match &self.session {
            Some(session) => session.read(cx).reading().models.has_workers,
            None => true,
        };
        let can_switch = match &self.session {
            None => true,
            Some(session) => {
                let session = session.read(cx);
                *session.phase() == Phase::Open && !session.is_working()
            }
        };
        let session_without_models = self.session.is_some() && choices.groups.is_empty();

        let rows = choices
            .groups
            .iter()
            .enumerate()
            .filter_map(|(group_ix, group)| {
                let models: Vec<_> = group
                    .models
                    .iter()
                    .filter(|model| {
                        filter.is_empty()
                            || model.name.to_lowercase().contains(&filter)
                            || model.alias.to_lowercase().contains(&filter)
                            || arko::models::display_name(&model.name)
                                .to_lowercase()
                                .contains(&filter)
                    })
                    .cloned()
                    .collect();
                if models.is_empty() {
                    return None;
                }
                let usable = group.needs.is_none();
                let needs = group.needs.clone();
                Some(
                    v_flex()
                        .gap_0p5()
                        .child(
                            div()
                                .pt_1()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(group.name.clone()),
                        )
                        .when_some(needs.clone(), |this, needs| {
                            let way = SharedString::from(group.name.clone());
                            this.child(
                                Button::new(("model-needs", group_ix))
                                    .link()
                                    .xsmall()
                                    .label(format!("Needs {needs}"))
                                    .tooltip("Settings › Models & accounts")
                                    .on_click({
                                        let view = view.clone();
                                        move |_, _, cx| {
                                            let way = way.clone();
                                            view.update(cx, |_, cx| {
                                                cx.emit(SessionViewEvent::OpenAccounts(Some(way)))
                                            })
                                            .ok();
                                        }
                                    }),
                            )
                        })
                        .children(models.into_iter().enumerate().map(|(ix, model)| {
                            let checked = current.as_deref() == Some(model.name.as_str());
                            let name = model.name.clone();
                            let view = view.clone();
                            let popover = popover.clone();
                            Button::new(SharedString::from(format!("model-{group_ix}-{ix}")))
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .disabled(!usable || (started && !workers_only))
                                .when(checked, |this| this.icon(IconName::Check))
                                .label(arko::models::display_name(&model.name))
                                .on_click(move |_, window, cx| {
                                    let name = name.clone();
                                    view.update(cx, |view, cx| {
                                        view.choose_model(name, workers_only, window, cx)
                                    })
                                    .ok();
                                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                                })
                        }))
                        .into_any_element(),
                )
            });
        let rows: Vec<AnyElement> = rows.collect();

        let effort_row = (!workers_only)
            .then(|| current.clone())
            .flatten()
            .map(|model| {
                let efforts = arko::commands::dynamic_choices(
                    &arko::remote::Reading::default(),
                    "/model",
                    &[model.as_str()],
                );
                let chosen = match &self.session {
                    None => self.draft.effort.clone(),
                    Some(session) => Some(session.read(cx).reading().models.main_effort.clone()),
                };
                (model, efforts, chosen)
            })
            .filter(|(_, efforts, _)| !efforts.is_empty());

        v_flex()
            .id("mode-choice")
            .w(rems(18.))
            .max_h(rems(28.))
            .gap_2()
            .child(self.render_mode_switch(
                "mode",
                &self.mode_menu_focus,
                Rc::new({
                    let view = view.clone();
                    let popover = popover.clone();
                    move |to, window, cx| {
                        let open = view
                            .update(cx, |view, _| view.session.is_some())
                            .unwrap_or(false);
                        // An open session asks in a dialog.
                        if open {
                            popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        }
                        view.update(cx, |view, cx| view.choose_mode(to, window, cx))
                            .ok();
                    }
                }),
                window,
                cx,
            ))
            .when(self.session.is_some() && !can_switch, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Switching between Chat and Agent waits for the turn to end."),
                )
            })
            .when(started && !session_without_models, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("The main agent keeps its model once the conversation has started."),
                )
            })
            .when(workers_only && has_workers, |this| {
                this.child(div().text_xs().font_medium().child("Model for new workers"))
            })
            .when(!workers_only || has_workers, |this| {
                this.child(Input::new(&self.model_filter).small().cleanable(true))
                    .child(
                        v_flex()
                            .id("model-list")
                            .test_support()
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(rows),
                    )
            })
            .when_some(effort_row, |this, (model, efforts, chosen)| {
                let view = view.clone();
                this.child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Effort"),
                        )
                        .children(efforts.into_iter().map(move |effort| {
                            let checked = chosen.as_deref() == Some(effort.as_str());
                            let view = view.clone();
                            let model = model.clone();
                            Button::new(SharedString::from(format!("effort-{effort}")))
                                .xsmall()
                                .when(checked, |this| this.primary())
                                .when(!checked, |this| this.ghost())
                                .disabled(started)
                                .label(effort.clone())
                                .on_click(move |_, window, cx| {
                                    let (model, effort) = (model.clone(), effort.clone());
                                    view.update(cx, |view, cx| {
                                        view.choose_effort(model, effort, window, cx)
                                    })
                                    .ok();
                                })
                        })),
                )
            })
            .into_any_element()
    }

    /// Chat or Agent: a new session just switches, an open one asks first
    /// (what changes, what it costs, the folder), between turns.
    pub fn choose_mode(&mut self, to: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.session.clone() else {
            self.set_draft_mode(to, cx);
            return;
        };
        if session.read(cx).mode() == to {
            return;
        }
        if session.read(cx).is_working() {
            window.push_notification("Wait for the turn to end.", cx);
            return;
        }
        // The folder an agent works in: where the session worked before,
        // else the recent ones.
        let mut folders: Vec<PathBuf> = session.read(cx).workspace().cloned().into_iter().collect();
        for folder in self.recent_folders.iter().chain(self.home.iter()) {
            if !folders.contains(folder) {
                folders.push(folder.clone());
            }
        }
        // Told not to ask, a switch to Chat just happens, and one to Agent
        // still asks where the agent works.
        let ask = prefs::get(cx).sessions.ask_before_switch;
        if !ask && to == Mode::Chat {
            self.switch_mode(session, to, None, window, cx);
            return;
        }
        let cost = if ask {
            session.read(cx).switch_cost(to, cx)
        } else {
            Task::ready(None)
        };
        let home = self.home.clone();
        cx.spawn_in(window, async move |this, cx| {
            let cost = cost.await;
            this.update_in(cx, |_, window, cx| {
                let view = cx.entity().downgrade();
                crate::prompts::confirm_switch(
                    to,
                    cost,
                    !ask,
                    folders,
                    home,
                    window,
                    cx,
                    move |folder, window, cx| {
                        let session = session.clone();
                        view.update(cx, |view, cx| {
                            view.switch_mode(session, to, folder, window, cx)
                        })
                        .ok();
                    },
                );
            })
            .ok();
        })
        .detach();
    }

    fn switch_mode(
        &mut self,
        session: Entity<LiveSession>,
        to: Mode,
        folder: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let switching = session.update(cx, |session, cx| session.switch_mode(to, folder, cx));
        cx.spawn_in(window, async move |_, cx| {
            if let Err(why) = switching.await {
                cx.update(|window, cx| window.push_notification(why, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn choose_model(
        &mut self,
        model: String,
        workers: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.session {
            None => self.set_draft_model(model, None, cx),
            Some(session) => {
                let command = if workers {
                    format!("/model worker {model}")
                } else {
                    format!("/model {model}")
                };
                self.run_command(session.clone(), command, window, cx);
            }
        }
    }

    fn choose_effort(
        &mut self,
        model: String,
        effort: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.session {
            None => self.set_draft_model(model, Some(effort), cx),
            Some(session) => self.run_command(
                session.clone(),
                format!("/model {model} {effort}"),
                window,
                cx,
            ),
        }
    }

    /// Sends a command line to the session, as the terminal would.
    fn run_command(
        &mut self,
        session: Entity<LiveSession>,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = arko::Input::Message(arko::Message {
            text: command,
            images: Vec::new(),
        });
        if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
            window.push_notification(why, cx);
        }
    }

    /// Chat | Agent: an outlined track with the chosen mode filled, each
    /// with its icon, and a line on what the chosen mode does. ←/→ switch
    /// while it has focus. `on_pick` hears the mode chosen. An open session
    /// switches between turns only.
    fn render_mode_switch(
        &self,
        prefix: &'static str,
        focus: &FocusHandle,
        on_pick: PickMode,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let theme = cx.theme();
        let mode = self.mode(cx);
        let (can_switch, started) = match &self.session {
            None => (true, false),
            Some(session) => {
                let session = session.read(cx);
                (
                    *session.phase() == Phase::Open && !session.is_working(),
                    session.has_started(),
                )
            }
        };
        let focused = focus.is_focused(window) && window.last_input_was_keyboard();
        // The chosen mode stands out from the track: the window's background
        // on a light theme, lighter than the track on a dark one.
        let chosen = if theme.is_dark() {
            theme.foreground.opacity(0.16)
        } else {
            theme.background
        };
        let segment = |value: Mode, label: &'static str, icon: gpui_kit::assets::IconName| {
            let checked = mode == value;
            let on_pick = on_pick.clone();
            h_flex()
                .id(SharedString::from(format!(
                    "{prefix}-{}",
                    label.to_lowercase()
                )))
                .test_support()
                .role(Role::RadioButton)
                .aria_label(label)
                .aria_toggled(if checked {
                    Toggled::True
                } else {
                    Toggled::False
                })
                .aria_selected(checked)
                .when(checked, |this| this.aria_active_descendant())
                .h(rems(1.75))
                .px_3()
                .gap_1p5()
                .items_center()
                .justify_center()
                .rounded(theme.radius)
                .border_1()
                .text_sm()
                .child(Icon::new(icon).small())
                .child(label)
                .map(|this| {
                    if checked {
                        this.bg(chosen)
                            .border_color(theme.border)
                            .text_color(theme.foreground)
                            .font_medium()
                            .shadow_xs()
                    } else {
                        this.border_color(theme.transparent)
                            .text_color(theme.muted_foreground)
                    }
                })
                .when(!can_switch, |this| this.opacity(0.6))
                .when(can_switch && !checked, |this| {
                    this.cursor_pointer()
                        .hover(|style| style.text_color(theme.foreground))
                        .on_click(move |_, window, cx| on_pick(value, window, cx))
                })
        };
        v_flex()
            .items_center()
            .gap_1()
            .child(
                h_flex()
                    .id(SharedString::from(format!("{prefix}-switch")))
                    .test_support()
                    .track_focus(focus)
                    .role(Role::RadioGroup)
                    .aria_label("Mode")
                    .p(px(2.))
                    .gap(px(2.))
                    .rounded(theme.radius + px(2.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.muted)
                    .on_key_down({
                        let on_pick = on_pick.clone();
                        move |event: &KeyDownEvent, window, cx| {
                            let to = match event.keystroke.key.as_str() {
                                "left" => Mode::Chat,
                                "right" => Mode::Agent,
                                _ => return,
                            };
                            cx.stop_propagation();
                            if can_switch && to != mode {
                                on_pick(to, window, cx);
                            }
                        }
                    })
                    .when(focused, |this| this.focus_ring_style(window, cx))
                    .child(segment(
                        Mode::Chat,
                        "Chat",
                        gpui_kit::assets::IconName::MessageSquare,
                    ))
                    .child(segment(
                        Mode::Agent,
                        "Agent",
                        gpui_kit::assets::IconName::Bot,
                    )),
            )
            .child(
                div()
                    .id(SharedString::from(format!("{prefix}-about")))
                    .test_support()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(mode_description(mode, started)),
            )
    }

    /// The one quiet line in a new session's empty transcript, with
    /// Change… for an agent session's folder.
    fn render_draft_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let folder = self.draft_folder();
        let recent = self.recent_folders.clone();
        let home = self.home.clone();
        let view = cx.entity().downgrade();
        h_flex()
            .id("draft-line")
            .test_support()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .map(|this| match self.draft.mode {
                Mode::Chat => this.child("New chat"),
                Mode::Agent => this
                    .child(match &folder {
                        Some(_) => "New agent session in",
                        None => "New agent session",
                    })
                    .when_some(folder.clone(), |this, folder| {
                        this.child(
                            div()
                                .text_color(theme.foreground)
                                .child(shown_folder(&folder, home.as_deref())),
                        )
                    })
                    .child(
                        Button::new("change-folder")
                            .link()
                            .small()
                            .label("Change…")
                            .dropdown_menu(move |menu, _, _| {
                                let menu = recent.iter().fold(menu, |menu, path| {
                                    let view = view.clone();
                                    let choice = path.clone();
                                    menu.item(
                                        PopupMenuItem::new(shown_folder(path, home.as_deref()))
                                            .checked(folder.as_ref() == Some(path))
                                            .on_click(move |_, _, cx| {
                                                let choice = choice.clone();
                                                view.update(cx, |view, cx| {
                                                    view.set_draft_folder(choice, cx)
                                                })
                                                .ok();
                                            }),
                                    )
                                });
                                let view = view.clone();
                                menu.when(!recent.is_empty(), |menu| menu.separator()).item(
                                    PopupMenuItem::new("Browse…").on_click(move |_, _, cx| {
                                        browse(view.clone(), cx);
                                    }),
                                )
                            }),
                    ),
            })
    }

    /// What the transcript's place shows while there is no transcript: a new
    /// session's line, or the session's state.
    fn render_state(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(session) = &self.session else {
            // The first choice of a new session: Chat or Agent, then where.
            let view = cx.entity().downgrade();
            return v_flex()
                .items_center()
                .gap_3()
                .child(self.render_mode_switch(
                    "empty-mode",
                    &self.mode_focus,
                    Rc::new(move |to, window, cx| {
                        view.update(cx, |view, cx| view.choose_mode(to, window, cx))
                            .ok();
                    }),
                    window,
                    cx,
                ))
                .child(self.render_draft_line(cx))
                .into_any_element();
        };
        let session = session.read(cx);
        let text: SharedString = match session.phase() {
            Phase::Opening if session.id().is_none() => "Starting the session…".into(),
            Phase::Opening | Phase::Open | Phase::Closing => "Opening the session…".into(),
            Phase::Sample => "A sample session: it shows, but doesn’t run.".into(),
            Phase::Ended => "The session has closed.".into(),
            Phase::Failed(why) => why.clone().into(),
        };
        div()
            .id("transcript-state")
            .test_support()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(text)
            .into_any_element()
    }

    /// The question card, pinned above the composer while the agent waits
    /// on an answer.
    fn render_question(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (id, text) = self.question(cx)?;
        if self.later == Some(id) {
            return None;
        }
        let theme = cx.theme();
        Some(
            v_flex()
                .id("question-card")
                .test_support()
                .mb_2()
                .p_3()
                .gap_2()
                .bg(theme.secondary)
                .text_color(theme.secondary_foreground)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(
                            Icon::new(gpui_kit::assets::IconName::MessageCircleQuestionMark)
                                .text_color(theme.warning)
                                .flex_none(),
                        )
                        .child(
                            // A long question (a plan to approve) scrolls
                            // and leaves room for the transcript.
                            div()
                                .id("question-text")
                                .test_support()
                                .flex_1()
                                .min_w_0()
                                .max_h(rems(QUESTION_MAX_HEIGHT))
                                .overflow_y_scroll()
                                .text_sm()
                                .child(TextView::markdown(("question", id as usize), text)),
                        )
                        .child(
                            Button::new("question-later")
                                .ghost()
                                .xsmall()
                                .label("Later")
                                .tooltip("Keep the question in the composer’s footer")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.later = Some(id);
                                    cx.notify();
                                })),
                        ),
                )
                .into_any_element(),
        )
    }

    /// A strip above the composer while the session waits for credentials
    /// or an approval the user put off.
    fn render_prompt(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let session = self.session.as_ref()?.read(cx);
        let text = match session.prompt()? {
            PromptKind::Credentials(need) => crate::prompts::need_title(need),
            PromptKind::Mcp(_) => "The project’s MCP servers wait for your approval.".to_string(),
        };
        let theme = cx.theme();
        Some(
            h_flex()
                .id("prompt-strip")
                .test_support()
                .mb_2()
                .px_3()
                .py_2()
                .gap_2()
                .text_sm()
                .border_1()
                .border_color(theme.warning)
                .rounded(theme.radius_lg)
                .child(div().flex_1().child(text))
                .child(
                    Button::new("prompt-show")
                        .small()
                        .label("Continue…")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(SessionViewEvent::ShowPrompt))),
                )
                .into_any_element(),
        )
    }

    /// The strip above the composer while it edits a message.
    fn render_editing(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let lost = images_not_kept(self.editing.as_ref()?.lost_images);
        let theme = cx.theme();
        Some(
            h_flex()
                .id("editing-strip")
                .test_support()
                .mb_2()
                .px_3()
                .py_1()
                .gap_2()
                .text_sm()
                .rounded(theme.radius_lg)
                .bg(theme.secondary)
                .text_color(theme.secondary_foreground)
                .child(Icon::new(gpui_kit::assets::IconName::SquarePen).xsmall())
                .child(div().flex_1().min_w_0().truncate().child(
                    if crate::prefs::get(cx).input.keymap == crate::prefs::Keymap::Vim {
                        "Editing message — Esc in normal mode cancels"
                    } else {
                        "Editing message — Esc cancels"
                    },
                ))
                .children(lost.map(|lost| {
                    div()
                        .id("edit-lost-images")
                        .test_support()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.warning)
                        .child(lost)
                }))
                .child(
                    Button::new("cancel-edit")
                        .ghost()
                        .xsmall()
                        .label("Cancel")
                        .on_click(cx.listener(|this, _, window, cx| this.end_edit(window, cx))),
                )
                .into_any_element(),
        )
    }

    /// "To r1 ×": the worker the composer writes to, and the way back to
    /// the main agent.
    fn render_target(&self, target: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .id("composer-target")
            .test_support()
            .flex_none()
            .h_6()
            .pl_2()
            .gap_1()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .text_xs()
            .text_color(theme.secondary_foreground)
            .child(Icon::new(gpui_kit::assets::IconName::Bot).xsmall())
            .child(format!("To {target}"))
            .child(
                div()
                    .id("clear-target-tip")
                    .tooltip(|window, cx| {
                        Tooltip::new("Write to Arko again")
                            .key_binding(Keystroke::parse("escape").ok().map(Kbd::new))
                            .build(window, cx)
                    })
                    .child(
                        Button::new("clear-target")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(IconName::Close)
                            .accessibility_label("Write to Arko again")
                            .on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.set_target(None, window, cx)
                                }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_composer(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = self.render_mode_chip(cx);
        let target = self
            .target
            .clone()
            .map(|target| self.render_target(target, cx));
        let theme = cx.theme();
        let working = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_working());
        let later_question = self
            .question(cx)
            .filter(|(id, _)| self.later == Some(*id))
            .map(|(id, _)| id);
        let expanded = prefs::get(cx).input.composer_expanded;
        let send_key = prefs::get(cx).input.send_key;
        let send_chord = Keystroke::parse(crate::composer::send_chord(send_key)).ok();
        let _ = window;
        v_flex()
            .id("composer")
            .test_support()
            .relative()
            .w_full()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(theme.input)
            .rounded(theme.radius_lg)
            .bg(theme.background)
            .child(
                h_flex()
                    .items_start()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(self.composer.clone()))
                    .child(
                        Button::new("expand-composer")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(Icon::new(if expanded {
                                gpui_kit::assets::IconName::Minimize2
                            } else {
                                gpui_kit::assets::IconName::Maximize2
                            }))
                            .tooltip_with_action(
                                if expanded { "Shrink" } else { "Expand" },
                                &menus::ExpandComposer,
                                None,
                            )
                            .accessibility_label(if expanded {
                                "Shrink composer"
                            } else {
                                "Expand composer"
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(menus::ExpandComposer), cx)
                            }),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        Button::new("attach")
                            .ghost()
                            .small()
                            .compact()
                            .icon(Icon::new(gpui_kit::assets::IconName::Paperclip))
                            .tooltip("Attach")
                            .accessibility_label("Attach")
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    )
                    .child(chip)
                    .children(target)
                    .when_some(later_question, |this, _| {
                        this.child(
                            Button::new("question-chip")
                                .ghost()
                                .small()
                                .icon(
                                    Icon::new(
                                        gpui_kit::assets::IconName::MessageCircleQuestionMark,
                                    )
                                    .text_color(theme.warning),
                                )
                                .label("Question")
                                .tooltip("Show the agent’s question")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.later = None;
                                    cx.notify();
                                })),
                        )
                    })
                    .child(div().flex_1())
                    .map(|this| {
                        // The tooltips name the keys: Esc stops, the send key sends.
                        let (label, chord) = if working {
                            ("Stop", Keystroke::parse("escape").ok())
                        } else {
                            ("Send", send_chord.clone())
                        };
                        let button = if working {
                            Button::new("stop")
                                .primary()
                                .small()
                                .compact()
                                .icon(IconName::Square)
                                .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
                        } else {
                            Button::new("send")
                                .primary()
                                .small()
                                .compact()
                                .icon(IconName::ArrowUp)
                                .disabled(self.composer_empty)
                                .on_click(cx.listener(|this, _, _, cx| this.send_composer(cx)))
                        };
                        this.child(
                            div()
                                .id("send-or-stop")
                                .tooltip(move |window, cx| {
                                    Tooltip::new(label)
                                        .key_binding(chord.clone().map(Kbd::new))
                                        .build(window, cx)
                                })
                                .child(button.accessibility_label(label)),
                        )
                    }),
            )
    }
}

/// Asks for a folder for the new agent session.
fn browse(view: WeakEntity<SessionView>, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await
            && let Some(folder) = paths.into_iter().next()
        {
            view.update(cx, |view, cx| view.set_draft_folder(folder, cx))
                .ok();
        }
    })
    .detach();
}

/// How a new session works until the user picks: as Settings › Models &
/// accounts says.
fn default_mode(cx: &App) -> Mode {
    match prefs::get(cx).new_sessions.mode {
        prefs::NewSessionMode::Chat => Mode::Chat,
        prefs::NewSessionMode::Agent => Mode::Agent,
    }
}

/// The folders of `sessions` that ran agents, the most recently active
/// first, each once.
pub fn recent_folders(sessions: &[SessionEntry]) -> Vec<PathBuf> {
    let mut agents: Vec<&SessionEntry> = sessions
        .iter()
        .filter(|session| session.mode == Mode::Agent && session.workspace.is_some())
        .collect();
    agents.sort_by_key(|session| std::cmp::Reverse(session.activity()));
    let mut folders: Vec<PathBuf> = Vec::new();
    for folder in agents
        .into_iter()
        .filter_map(|session| session.workspace.clone())
    {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders.truncate(RECENT_FOLDERS);
    folders
}

/// The user's home folder, as the app found it at start-up.
pub struct HomeFolder(pub Option<PathBuf>);

impl Global for HomeFolder {}

/// The user's home folder, from the environment.
pub fn home_from_env() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// Hears the mode picked on a Chat | Agent switch.
type PickMode = Rc<dyn Fn(Mode, &mut Window, &mut App)>;

/// What `mode` does, as the line under the Chat | Agent switch says it,
/// and for a `started` conversation what switching costs.
pub fn mode_description(mode: Mode, started: bool) -> String {
    let what = match mode {
        Mode::Chat => "Chat: conversation only, no tools.",
        Mode::Agent => "Agent: works in a folder with tools and workers.",
    };
    if started {
        format!("{what} Switching re-reads the conversation without its cache.")
    } else {
        what.to_string()
    }
}

/// `folder` as people read it: under the home folder as `~/…` on macOS and
/// Linux, in full on Windows.
pub fn shown_folder(folder: &Path, home: Option<&Path>) -> String {
    shown_folder_from(folder, home.filter(|_| !cfg!(windows)))
}

fn shown_folder_from(folder: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// Says that a part of the app comes later.
pub fn not_yet(what: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(format!("{what} comes with a later version."), cx);
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = self.width == WidthClass::Narrow;
        let question = self.render_question(cx);
        let prompt = self.render_prompt(cx);
        let editing = self.render_editing(cx);
        let find = self.render_find(cx);
        // The transcript spans the column (its rows centre themselves, so
        // the wheel and the scrollbar work across it), and draws again only
        // when it changes.
        let transcript = self.transcript(cx).map(TranscriptView::cached);
        let state = transcript.is_none().then(|| self.render_state(window, cx));
        let theme = cx.theme();
        v_flex()
            .id("session-view")
            .test_support()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .items_center()
            .children(find)
            .child(
                div()
                    .id("transcript")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .children(transcript)
                    .when_some(state, |this, state| {
                        this.flex()
                            .items_center()
                            .justify_center()
                            .px_4()
                            .child(state)
                    }),
            )
            .child(
                // The composer's column: centred and at most ~46 rem wide, full
                // width on narrow windows.
                v_flex()
                    .w_full()
                    .when(!narrow, |this| this.max_w(rems(46.)))
                    .px_4()
                    .pb_3()
                    .children(prompt)
                    .children(question)
                    .children(editing)
                    .child(self.render_composer(window, cx)),
            )
    }
}

/// What the find bar says of where its find is.
fn find_status_text(status: FindStatus, has_words: bool) -> String {
    match (status.current, status.total) {
        _ if !has_words => String::new(),
        _ if status.searching => "Searching…".to_string(),
        (_, 0) => "No matches".to_string(),
        (Some(current), total) if status.in_full_output => {
            format!("{} of {total} in the full output", current + 1)
        }
        (Some(current), total) => format!("{} of {total}", current + 1),
        // Matches, none of them on show (words that matched nothing
        // before).
        (None, 1) => "1 match".to_string(),
        (None, total) => format!("{total} matches"),
    }
}

/// A conversation exported as Markdown, ready to write.
pub(crate) struct Export {
    pub text: String,
    /// It starts partway through the conversation.
    pub starts_partway: bool,
}

/// Writes `export` to `path` off the UI thread, and says how it went. It
/// goes to a file beside `path` first, which then takes its name, so a
/// failed write leaves a file that was there whole.
fn write_export(export: Export, path: PathBuf, window: &mut Window, cx: &mut App) {
    let written = cx.background_spawn({
        let path = path.clone();
        async move { crate::export::write(&path, &export.text) }
    });
    let starts_partway = export.starts_partway;
    window
        .spawn(cx, async move |cx| {
            let written = written.await;
            cx.update(|window, cx| match written {
                Ok(()) => {
                    let mut said = format!("Exported to {}.", path.display());
                    if starts_partway {
                        said.push_str(" It starts partway, without the earliest messages.");
                    }
                    window.push_notification(said, cx)
                }
                Err(error) => window.push_notification(
                    gpui_kit::component::notification::Notification::error(format!(
                        "Couldn’t export to {}. {}",
                        path.display(),
                        crate::export::why(&error)
                    )),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        FindStatus, find_status_text, images_not_kept, mode_description, recent_folders,
        shown_folder_from,
    };
    use crate::backend::Backend as _;
    use crate::backend::{FakeBackend, Mode, SessionEntry};
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    #[test]
    fn the_mode_line_says_what_the_mode_does_and_what_switching_costs() {
        assert_eq!(
            mode_description(Mode::Chat, false),
            "Chat: conversation only, no tools."
        );
        assert_eq!(
            mode_description(Mode::Agent, true),
            "Agent: works in a folder with tools and workers. Switching re-reads the conversation without its cache."
        );
    }

    #[test]
    fn the_find_bar_says_where_the_find_is() {
        let status = |current: Option<usize>, total: usize| FindStatus {
            current,
            total,
            ..FindStatus::default()
        };
        assert_eq!(find_status_text(status(Some(2), 12), true), "3 of 12");
        assert_eq!(find_status_text(status(None, 12), true), "12 matches");
        assert_eq!(find_status_text(status(None, 1), true), "1 match");
        assert_eq!(find_status_text(status(None, 0), true), "No matches");
        assert_eq!(find_status_text(status(None, 12), false), "");
    }

    #[test]
    fn an_edit_says_how_many_of_its_images_are_gone() {
        assert_eq!(images_not_kept(0), None);
        assert_eq!(
            images_not_kept(1).as_deref(),
            Some("1 image is no longer kept")
        );
        assert_eq!(
            images_not_kept(3).as_deref(),
            Some("3 images are no longer kept")
        );
    }

    #[test]
    fn recent_folders_are_agent_folders_latest_first_each_once() {
        let backend = FakeBackend::sample("/arko", SystemTime::now());
        let sessions: Vec<SessionEntry> = backend.sessions().unwrap();
        assert!(sessions.iter().any(|session| session.mode == Mode::Chat));
        assert_eq!(
            recent_folders(&sessions),
            [
                PathBuf::from("/home/ada/src/arko"),
                PathBuf::from("/home/ada/src/gpui_kit")
            ]
        );
    }

    #[test]
    fn folders_under_home_read_from_a_tilde() {
        let home = Some(Path::new("/home/ada"));
        assert_eq!(
            shown_folder_from(Path::new("/home/ada/src/arko"), home),
            "~/src/arko"
        );
        assert_eq!(shown_folder_from(Path::new("/home/ada"), home), "~");
        assert_eq!(shown_folder_from(Path::new("/srv/data"), home), "/srv/data");
        assert_eq!(
            shown_folder_from(Path::new("/home/adam"), home),
            "/home/adam"
        );
        assert_eq!(shown_folder_from(Path::new("/srv"), None), "/srv");
    }
}
