//! The main column: the transcript's reading column and the composer under
//! it, for the session the window shows, or a new one before its first
//! message.
//!
//! The composer is a view of its own (typing draws only it). This view
//! decides what a sent message is: a new session's first message, a
//! message, a note while a turn runs, the answer to the agent's question,
//! or a command the app runs itself.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::text::TextView;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _, Toggle, ToggleGroup},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Mode, ModelChoices, NewSession, SessionEntry, SharedBackend};
use crate::composer::{Attached, Composer, ComposerEvent, Outgoing};
use crate::layout::WidthClass;
use crate::live::{LiveEvent, LiveSession, LiveSessions, Phase, PromptKind};
use crate::transcript::{AgentFilter, TranscriptEvent, TranscriptView};
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
    /// Open Settings › Models & accounts.
    OpenAccounts,
    /// Show the session's waiting prompt again.
    ShowPrompt,
    /// The reader opened a worker's card in the transcript.
    OpenWorker(SharedString),
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
    model_filter: Entity<InputState>,
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
            draft: Draft::default(),
            recent_folders: Vec::new(),
            home: cx.global::<HomeFolder>().0.clone(),
            models: None,
            composer,
            composer_empty: true,
            width: WidthClass::Wide,
            later: None,
            expanded_rows: None,
            model_filter,
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
            cx.notify();
        }
    }

    /// Runs the new agent session in `folder`.
    pub fn set_draft_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.draft.folder = Some(folder);
        self.list_models(cx);
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
        cx.notify();
    }

    /// The folders Change… offers, from the sessions in the sidebar.
    pub fn set_recent_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.recent_folders != folders {
            let before = self.draft_folder();
            self.recent_folders = folders;
            if self.draft_folder() != before {
                self.list_models(cx);
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
            let text = self.composer.read(cx).expanded_text(cx);
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
        self.keep_draft(cx);
        if let Some(old) = self.session.take() {
            old.update(cx, |old, cx| old.set_shown(false, cx));
        }
        self._session.clear();
        self.later = None;
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
                let events = cx.subscribe(&view, |_, _, event: &TranscriptEvent, cx| {
                    let TranscriptEvent::OpenWorker(id) = event;
                    cx.emit(SessionViewEvent::OpenWorker(id.clone()));
                });
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
    pub fn placeholder(&self, cx: &App) -> &'static str {
        let Some(session) = &self.session else {
            return "Message Arko";
        };
        let session = session.read(cx);
        if let Some(question) = session.question() {
            if self.later == Some(question.id) {
                "Answer Arko’s question"
            } else {
                "Answer the question above"
            }
        } else if session.is_working() {
            "Add to the task — Arko is working"
        } else {
            "Message Arko"
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
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(placeholder, window, cx);
            composer.set_session_state(reading, working, agent);
        });
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

    /// Sends what the composer holds (the Send button).
    pub fn send_composer(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| composer.send(cx));
    }

    /// Sends `text`: runs the app's own commands, starts a new session with
    /// it, or gives it to the open session as a message, a note or an
    /// answer. The composer empties once it is taken.
    pub fn send(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        let Outgoing { text, images } = outgoing;
        if images.is_empty() && self.app_command(text.trim(), window, cx) {
            self.composer
                .update(cx, |composer, cx| composer.sent(&text, window, cx));
            return;
        }
        let attached = images;
        let images: Vec<arko::Attachment> = attached.iter().map(|image| image.to_arko()).collect();
        let message = arko::Message {
            text: text.clone(),
            images,
        };
        // As in the terminal, a command runs while a question waits, and the
        // question stays open. Other text answers it.
        let command = arko::commands::find(&text).is_some();
        let result = match &self.session {
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

    /// Runs the commands the app handles itself, `false` for the others.
    fn app_command(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) -> bool {
        let mut words = text.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("/view"), Some(view), None) => {
                let view = match view {
                    "full" => prefs::TranscriptView::Full,
                    "quiet" => prefs::TranscriptView::Quiet,
                    "messages" => prefs::TranscriptView::Messages,
                    _ => return false,
                };
                prefs::update(cx, |prefs| prefs.view = view);
                true
            }
            (Some("/settings"), None, None) => {
                window.dispatch_action(Box::new(menus::OpenSettings), cx);
                true
            }
            // A new session's model, before it starts.
            (Some("/model"), Some(model), effort)
                if self.session.is_none() && model != "worker" =>
            {
                self.set_draft_model(model.to_string(), effort.map(str::to_string), cx);
                true
            }
            _ => false,
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
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme().clone();
        let mode = self.mode(cx);
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
                            this.child(
                                Button::new(("model-needs", group_ix))
                                    .link()
                                    .xsmall()
                                    .label(format!("Needs {needs}"))
                                    .tooltip("Settings › Models & accounts")
                                    .on_click({
                                        let view = view.clone();
                                        move |_, _, cx| {
                                            view.update(cx, |_, cx| {
                                                cx.emit(SessionViewEvent::OpenAccounts)
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
            .child(
                ToggleGroup::new("mode")
                    .segmented()
                    .small()
                    // An open session switches between turns only.
                    .disabled(!can_switch)
                    .child(
                        Toggle::new("mode-chat")
                            .label("Chat")
                            .checked(mode == Mode::Chat),
                    )
                    .child(
                        Toggle::new("mode-agent")
                            .label("Agent")
                            .checked(mode == Mode::Agent),
                    )
                    .on_click({
                        let view = view.clone();
                        let popover = popover.clone();
                        move |checks, window, cx| {
                            // The toggle whose state changed is the one clicked.
                            let clicked = if checks[0] != (mode == Mode::Chat) {
                                Mode::Chat
                            } else {
                                Mode::Agent
                            };
                            let open = view
                                .update(cx, |view, _| view.session.is_some())
                                .unwrap_or(false);
                            // An open session asks in a dialog.
                            if open {
                                popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                            }
                            view.update(cx, |view, cx| view.choose_mode(clicked, window, cx))
                                .ok();
                        }
                    }),
            )
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
    fn render_state(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(session) = &self.session else {
            return self.render_draft_line(cx).into_any_element();
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

    fn render_composer(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = self.render_mode_chip(cx);
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
        // The transcript spans the column (its rows centre themselves, so
        // the wheel and the scrollbar work across it), and draws again only
        // when it changes.
        let transcript = self.transcript(cx).map(TranscriptView::cached);
        let state = transcript.is_none().then(|| self.render_state(cx));
        let theme = cx.theme();
        v_flex()
            .id("session-view")
            .test_support()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .items_center()
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
                    .child(self.render_composer(window, cx)),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{recent_folders, shown_folder_from};
    use crate::backend::Backend as _;
    use crate::backend::{FakeBackend, Mode, SessionEntry};
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

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
