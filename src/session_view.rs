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
use gpui_kit::component::tooltip::{ManagedTooltipExt as _, Tooltip};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, ThemeStyled as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    concentric_radius, h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::approvals::{ApprovalTray, TrayEvent};
use crate::backend::{Mode, ModelChoices, NewSession, SessionEntry, SharedBackend};
use crate::composer::{Attached, Composer, ComposerEvent, Outgoing};
use crate::editing::{self, Change, Choice};
use crate::layout::WidthClass;
use crate::live::{LiveEvent, LiveSession, LiveSessions, Phase, PromptKind};
use crate::questions::{AnswerPanel, AnswerPanelEvent};
use crate::transcript::{AgentFilter, FindStatus, TranscriptEvent, TranscriptView};
use crate::workers::{self, Usual};
use crate::{drafts, menus, prefs, segmented};

/// How many recent folders Change… offers.
const RECENT_FOLDERS: usize = 8;

/// How long typing rests before the draft is kept.
const DRAFT_REST: std::time::Duration = std::time::Duration::from_millis(300);
/// How long after the last key the user still counts as typing: questions
/// that come then wait in a chip instead of opening the answer panel.
const TYPING_REST: std::time::Duration = std::time::Duration::from_secs(2);

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
    /// A message went to the worker named here. It shows in that worker's
    /// transcript only.
    SentToWorker(SharedString),
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

/// The key context of the chip's popover, Chat or Agent and the models.
const MODE_CHOICE_CONTEXT: &str = "ModeChoice";

/// Enter and Space press the button that has the keyboard in the chip's
/// popover. GPUI Kit's popover binds them to close itself, which would
/// close the list without choosing.
pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", NoAction, Some(MODE_CHOICE_CONTEXT)),
        KeyBinding::new("space", NoAction, Some(MODE_CHOICE_CONTEXT)),
    ]);
}

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

/// A session's answer panel, with what is filled in.
struct KeptPanel {
    session: WeakEntity<LiveSession>,
    view: Entity<AnswerPanel>,
    _events: Subscription,
}

/// A session's approval tray, with what is checked.
struct KeptTray {
    session: WeakEntity<LiveSession>,
    view: Entity<ApprovalTray>,
    _events: Subscription,
}

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
    /// How many times it has drawn (for tests and measurements).
    renders: usize,
    session: Option<Entity<LiveSession>>,
    draft: Draft,
    /// The folders of recent agent sessions, the latest first.
    recent_folders: Vec<PathBuf>,
    /// The user's home folder, the last place a new session falls back on.
    home: Option<PathBuf>,
    /// The models a new session can start on, for the folder they were
    /// listed for.
    models: Option<(Option<PathBuf>, ModelChoices)>,
    /// The model and effort Settings set for new sessions, as last drawn.
    defaults_shown: (Option<String>, Option<String>),
    composer: Entity<Composer>,
    composer_empty: bool,
    width: WidthClass,
    /// Each session's answer panel, while it has open questions: what is
    /// filled in waits there while the window shows another session.
    panels: Vec<KeptPanel>,
    /// Each session's approval tray, while requests wait in it: what is
    /// checked waits there while the window shows another session.
    trays: Vec<KeptTray>,
    /// Whether the answer panel shows.
    panel_open: bool,
    /// Where the keyboard was before Alt+Q gave it to the panel: it goes
    /// back there when the panel closes.
    panel_return: Option<FocusHandle>,
    /// When the user last changed the composer's text.
    last_key: Option<std::time::Instant>,
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
    /// Main agent | New workers in the chip's popover.
    model_for_focus: FocusHandle,
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
    /// What the model list knows of OpenRouter.
    openrouter: OpenRouterSeen,
    _openrouter: Option<Task<()>>,
    /// The effort each model last ran at, as arko keeps it, by model.
    last_efforts: std::collections::HashMap<String, Option<String>>,
    /// The latest read of each model's last effort since the model list
    /// last opened, so an older read that ends later is dropped. A model
    /// without one is read again.
    last_effort_reads: std::collections::HashMap<String, u64>,
    reads: u64,
    /// The models the open session can switch to, listed off the UI thread
    /// (it may run `claude`) for its status's models as they were then.
    session_models: Option<(arko::remote::Models, ModelChoices)>,
    /// What reaches the models changed since they were listed.
    session_models_stale: bool,
    _session_models: Option<(arko::remote::Models, Task<()>)>,
    /// Once the conversation has started, the model list is the new
    /// workers' (`true`) or the main agent's.
    lists_worker_models: bool,
    /// The latest message given to the shown session for the main agent or
    /// a worker, which comes back to the composer if arko didn't send it
    /// (it came as the old model began its report for a switch, or the
    /// worker's cache expired before arko continued it).
    unsent: Option<Unsent>,
    /// A worker arko didn't continue because its cache expired, with arko's
    /// warning. Arko takes the same message again as confirmed, so a
    /// message to the worker asks with this warning until Continue, until
    /// what the window knows of the worker says so too, or until the window
    /// shows another session.
    continue_refused: Option<(String, String)>,
}

/// A message given to the session, as the composer held it, and whom it
/// went to.
struct Unsent {
    text: String,
    images: Vec<Attached>,
    /// The worker the composer wrote to, or none when it wrote to Arko.
    target: Option<SharedString>,
    /// The worker it continues: the composer's, or the one a `/message`
    /// typed to Arko names.
    worker: Option<String>,
    /// The questions it answered.
    answers: Vec<String>,
}

impl Unsent {
    /// Whether arko's notice repeats this message's `words`: its text on one
    /// line, cut with "…" when long.
    fn is(&self, words: &str) -> bool {
        let line = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        match words.strip_suffix('…') {
            Some(start) => line.starts_with(start),
            None => line == words,
        }
    }
}

/// Where the effort a new session starts with comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffortFrom {
    /// Chosen in the model list or with `/model`.
    Chosen,
    /// The effort the model last ran at.
    LastUsed,
    /// The effort Settings set for new sessions.
    Settings,
    /// xhigh, or the nearest the model takes.
    Default,
}

/// Arko's efforts, lowest first.
const EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The effort of `efforts` (lowest first) a model runs at when `wanted`
/// is asked for, as arko fits it: `wanted` when it's there, else the
/// highest below it, else the lowest.
pub fn fit_effort(efforts: &[String], wanted: &str) -> Option<String> {
    if efforts.iter().any(|effort| effort == wanted) {
        return Some(wanted.to_string());
    }
    let rank = |effort: &str| EFFORTS.iter().position(|known| *known == effort);
    let wanted = rank(wanted);
    efforts
        .iter()
        .filter(|effort| rank(effort) < wanted)
        .max_by_key(|effort| rank(effort))
        .or_else(|| efforts.first())
        .cloned()
}

/// What the model list knows of OpenRouter: the models picked lately and
/// the catalog arko keeps, for their details and the search, and the
/// routing settings in a few words.
#[derive(Default)]
struct OpenRouterSeen {
    recent: Vec<crate::backend::Listed>,
    catalog: crate::backend::Catalog,
    summary: Option<String>,
    /// The catalog is being fetched.
    loading: bool,
    /// When it was last read, to read it again only after a while.
    read_at: Option<std::time::Instant>,
}

/// How long the model list goes before it reads OpenRouter's catalog
/// again (arko fetches it only when what it keeps is stale).
const OPENROUTER_REREAD: std::time::Duration = std::time::Duration::from_secs(30);
/// How many of OpenRouter's models a search in the model list adds.
pub const OPENROUTER_FOUND: usize = 5;

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
                this.sync_expanded(window, cx);
                this.sync_defaults(cx);
            }),
            cx.observe_window_bounds(window, |this, window, cx| this.sync_expanded(window, cx)),
            // What reaches the models changed: a key or a login in
            // Settings. Coming back to the window, the terminal app may
            // have changed them.
            cx.observe_global::<crate::backend::AccountsSeen>(|this, cx| {
                this.session_models_stale = true;
                this.list_models_again(cx)
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && this.models.is_some() {
                    this.list_models_again(cx);
                }
                if window.is_window_active() {
                    this.forget_last_efforts(cx);
                    this.session_models_stale = true;
                }
            }),
            cx.observe(&model_filter, |_, _, cx| cx.notify()),
            // OpenRouter's settings, saved in Settings: the gear's tooltip
            // says so.
            cx.observe_global::<crate::backend::OpenRouterSaved>(|this, cx| {
                if let Some(saved) = cx.global::<crate::backend::OpenRouterSaved>().settings() {
                    this.openrouter.summary = saved.summary();
                    this.openrouter.recent = saved.recent();
                    cx.notify();
                }
            }),
            cx.on_release(|this, cx| this.window_closed(cx)),
        ];
        // Alt+↑ in the empty composer edits the latest message, when it can.
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
            renders: 0,
            session: None,
            draft: Draft {
                mode: default_mode(cx),
                ..Draft::default()
            },
            recent_folders: Vec::new(),
            home: cx.global::<HomeFolder>().0.clone(),
            models: None,
            defaults_shown: (settings_model(cx), settings_effort(cx)),
            composer,
            composer_empty: true,
            width: WidthClass::Wide,
            panels: Vec::new(),
            trays: Vec::new(),
            panel_open: false,
            panel_return: None,
            last_key: None,
            expanded_rows: None,
            target: None,
            editing: None,
            find: None,
            model_filter,
            mode_focus: cx.focus_handle().tab_stop(true),
            mode_menu_focus: cx.focus_handle().tab_stop(true),
            model_for_focus: cx.focus_handle().tab_stop(true),
            _session: Vec::new(),
            _subscriptions: subscriptions,
            _models: None,
            _draft: None,
            transcripts: Vec::new(),
            closed: false,
            first: None,
            returned_images: Vec::new(),
            openrouter: OpenRouterSeen::default(),
            _openrouter: None,
            last_efforts: std::collections::HashMap::new(),
            last_effort_reads: std::collections::HashMap::new(),
            reads: 0,
            session_models: None,
            session_models_stale: false,
            _session_models: None,
            lists_worker_models: false,
            unsent: None,
            continue_refused: None,
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
            None => self.draft_model(cx),
        }
    }

    fn sync_image_model(&mut self, cx: &mut Context<Self>) {
        self.see_last_effort(cx);
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

    /// Lists the models a new session can start on, in the background,
    /// unless they're listed for its folder already.
    fn list_models(&mut self, cx: &mut Context<Self>) {
        let folder = self.models_folder();
        if self
            .models
            .as_ref()
            .is_some_and(|(listed, _)| *listed == folder)
        {
            return;
        }
        self.list_models_again(cx);
    }

    /// Lists the models a new session can start on again, in the
    /// background: what reaches them may have changed (a key added in
    /// Settings or in the terminal app). The list shown stays until then.
    fn list_models_again(&mut self, cx: &mut Context<Self>) {
        let folder = self.models_folder();
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

    /// Reads what the model list shows of OpenRouter, in the background:
    /// the settings and what arko keeps at once, then the catalog fetched
    /// again when it is stale. Once in a while, as the list opens.
    fn see_openrouter(&mut self, cx: &mut Context<Self>) {
        let due = self
            .openrouter
            .read_at
            .is_none_or(|at| at.elapsed() >= OPENROUTER_REREAD);
        if !due || self._openrouter.is_some() {
            return;
        }
        self.openrouter.read_at = Some(std::time::Instant::now());
        self.openrouter.loading = true;
        let backend = cx.global::<SharedBackend>().clone();
        let kept = cx.background_spawn({
            let backend = backend.clone();
            async move { (backend.openrouter(), backend.openrouter_models_kept()) }
        });
        self._openrouter = Some(cx.spawn(async move |this, cx| {
            let (settings, catalog) = kept.await;
            this.update(cx, |this, cx| {
                this.openrouter.recent = settings.recent();
                this.openrouter.summary = settings.summary();
                this.openrouter.catalog = catalog;
                cx.notify();
            })
            .ok();
            let catalog = cx
                .background_spawn(async move { backend.openrouter_models(false) })
                .await;
            this.update(cx, |this, cx| {
                this.openrouter.catalog = catalog;
                this.openrouter.loading = false;
                this._openrouter = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// OpenRouter's model `name` as the models picked lately or the
    /// catalog have it, with its name ("GLM 5") and details.
    fn openrouter_model(&self, name: &str) -> Option<&crate::backend::Listed> {
        self.openrouter
            .recent
            .iter()
            .find(|model| model.name == name)
            .or_else(|| self.openrouter.catalog.get(name))
    }

    /// The model a new session would start on: the one chosen in the chip's
    /// popover, else the one Settings set, else the one arko would choose.
    /// Settings are read as they are now, since they can change while the
    /// window is open.
    pub fn draft_model(&self, cx: &App) -> Option<String> {
        self.draft
            .model
            .clone()
            .or_else(|| settings_model(cx))
            .or_else(|| {
                self.models
                    .as_ref()
                    .and_then(|(_, models)| models.default.clone())
            })
    }

    /// The effort the chip names for a new session: the one it starts
    /// with, unless that's merely the default.
    pub fn draft_effort(&self, cx: &App) -> Option<String> {
        let model = self.draft_model(cx)?;
        self.effort_for(&model, cx)
            .filter(|(_, from)| *from != EffortFrom::Default)
            .map(|(effort, _)| effort)
    }

    /// The effort a new session on `model` starts with, and where it comes
    /// from: the one chosen, else the one the model last ran at, else the
    /// one Settings set, else xhigh, each as near as the model takes it.
    /// `None` for a model that doesn't reason.
    pub fn effort_for(&self, model: &str, cx: &App) -> Option<(String, EffortFrom)> {
        let efforts =
            arko::commands::dynamic_choices(&arko::remote::Reading::default(), "/model", &[model]);
        if efforts.is_empty() {
            return None;
        }
        let last = self.last_efforts.get(model).cloned().flatten();
        let (wanted, from) = match (&self.draft.effort, last, settings_effort(cx)) {
            (Some(chosen), _, _) => (chosen.clone(), EffortFrom::Chosen),
            (None, Some(last), _) => (last, EffortFrom::LastUsed),
            (None, None, Some(set)) => (set, EffortFrom::Settings),
            (None, None, None) => ("xhigh".to_string(), EffortFrom::Default),
        };
        fit_effort(&efforts, &wanted).map(|effort| (effort, from))
    }

    /// Reads the effort the new session's model last ran at, in the
    /// background, unless it was read since the model list last opened.
    /// The effort read before stays meanwhile.
    fn see_last_effort(&mut self, cx: &mut Context<Self>) {
        if self.session.is_some() {
            return;
        }
        let Some(model) = self.draft_model(cx) else {
            return;
        };
        if self.last_effort_reads.contains_key(&model) {
            return;
        }
        self.reads += 1;
        let read = self.reads;
        self.last_effort_reads.insert(model.clone(), read);
        let backend = cx.global::<SharedBackend>().clone();
        let reading = cx.background_spawn({
            let model = model.clone();
            async move { backend.last_effort(&model) }
        });
        cx.spawn(async move |this, cx| {
            let effort = reading.await;
            this.update(cx, |this, cx| {
                let latest = this.last_effort_reads.get(&model) == Some(&read);
                if latest && this.last_efforts.get(&model) != Some(&effort) {
                    this.last_efforts.insert(model, effort);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Lists the models the open session can switch to, in the background,
    /// unless they're listed for `models` already. Those listed before
    /// show meanwhile.
    fn see_session_models(&mut self, models: &arko::remote::Models, cx: &mut Context<Self>) {
        let listed = self
            .session_models
            .as_ref()
            .is_some_and(|(listed, _)| listed == models);
        if listed && !self.session_models_stale {
            return;
        }
        let listing = self
            ._session_models
            .as_ref()
            .is_some_and(|(listing, _)| listing == models);
        if listing {
            return;
        }
        self.session_models_stale = false;
        let backend = cx.global::<SharedBackend>().clone();
        let reading = cx.background_spawn({
            let models = models.clone();
            async move { backend.session_models(&models) }
        });
        let key = models.clone();
        let task = cx.spawn(async move |this, cx| {
            let choices = reading.await;
            this.update(cx, |this, cx| {
                this.session_models = Some((key, choices));
                this._session_models = None;
                cx.notify();
            })
            .ok();
        });
        self._session_models = Some((models.clone(), task));
    }

    /// Reads the models' last efforts again from now on: a session ran
    /// since, here or elsewhere.
    fn forget_last_efforts(&mut self, cx: &mut Context<Self>) {
        self.last_effort_reads.clear();
        self.see_last_effort(cx);
    }

    /// Draws again when the model or effort Settings set for new sessions
    /// changed, since a new session's chip shows them.
    fn sync_defaults(&mut self, cx: &mut Context<Self>) {
        let defaults = (settings_model(cx), settings_effort(cx));
        if defaults != self.defaults_shown {
            self.defaults_shown = defaults;
            if self.session.is_none() {
                self.sync_image_model(cx);
                cx.notify();
            }
        }
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
            main_model: session.reading().models.main.clone(),
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

    /// Its window closed: the session it showed is no longer shown (and a
    /// turn that ends gives it news from then on), and what was typed is
    /// kept.
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
        self.close_panel(window, cx);
        self.target = None;
        self.unsent = None;
        self.continue_refused = None;
        self.session = session;
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.set_shown(true, cx));
            self._session
                .push(cx.observe_in(session, window, |this, _, window, cx| {
                    this.sync_questions(window, cx);
                    this.sync_approvals(window, cx);
                    this.sync_composer(window, cx);
                    this.sync_transcript(window, cx);
                    cx.notify();
                }));
            self._session.push(cx.subscribe_in(
                session,
                window,
                |this, _, event: &LiveEvent, window, cx| match event {
                    LiveEvent::NotSent(words) => {
                        let to_arko = |unsent: &Unsent| unsent.worker.is_none() && unsent.is(words);
                        this.restore_unsent(to_arko, window, cx);
                    }
                    LiveEvent::NotContinued { worker, warning } => {
                        this.not_continued(worker, warning, window, cx)
                    }
                    _ => {}
                },
            ));
        }
        self.restore_draft(window, cx);
        self.sync_questions(window, cx);
        self.sync_approvals(window, cx);
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
        // Its running calls that can move to the background offer to, and
        // its workers' cards name the models that aren't the usual ones.
        let reading = session.read(cx).reading().clone();
        let usual = Usual::of(&reading.models);
        kept.view.update(cx, |view, cx| {
            view.set_movable(&reading.movable, cx);
            view.set_usual(&usual, cx);
        });
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

    /// How many times the view has drawn (for tests and measurements).
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// The composer.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The model list's search field (tests).
    #[cfg(test)]
    pub fn model_filter(&self) -> &Entity<InputState> {
        &self.model_filter
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

    /// The answer panel while it shows (tests and screenshots).
    #[doc(hidden)]
    pub fn answer_panel(&self) -> Option<&Entity<AnswerPanel>> {
        self.panel().filter(|_| self.panel_open)
    }

    /// The shown session's answer panel, open or not.
    fn panel(&self) -> Option<&Entity<AnswerPanel>> {
        let session = self.session.as_ref()?.entity_id();
        self.panels
            .iter()
            .find(|kept| kept.session.entity_id() == session)
            .map(|kept| &kept.view)
    }

    /// The open questions attached to the composer, which the next message
    /// answers.
    fn attached(&self, cx: &App) -> Vec<String> {
        self.session
            .as_ref()
            .map(|session| session.read(cx).attached_questions())
            .unwrap_or_default()
    }

    /// The open questions neither attached nor in the panel, which a chip
    /// above the composer stands for, and whether one of them is new.
    fn waiting_questions(&self, cx: &App) -> (Vec<arko::questions::AgentQuestion>, bool) {
        let Some(session) = &self.session else {
            return (Vec::new(), false);
        };
        if self.panel_open {
            return (Vec::new(), false);
        }
        let session = session.read(cx);
        let attached = session.attached_questions();
        let unseen = session.unseen_questions();
        let waiting: Vec<arko::questions::AgentQuestion> = session
            .open_questions()
            .into_iter()
            .filter(|question| !attached.contains(&question.id))
            .cloned()
            .collect();
        let new = waiting.iter().any(|question| unseen.contains(&question.id));
        (waiting, new)
    }

    /// Whether the user is typing: the composer has text, or had a key a
    /// moment ago.
    fn typing(&self, cx: &App) -> bool {
        let now = cx.background_executor().now();
        !self.composer_empty
            || self.editing.is_some()
            || self
                .last_key
                .is_some_and(|key| now.saturating_duration_since(key) < TYPING_REST)
    }

    /// Follows the session's open questions: the panel shows them, and opens
    /// by itself for new ones while the user isn't typing.
    fn sync_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The panels of sessions that ended go with them.
        self.panels.retain(|kept| {
            kept.session
                .upgrade()
                .is_some_and(|session| *session.read(cx).phase() != Phase::Ended)
        });
        let Some(session) = self.session.clone() else {
            return;
        };
        let open: Vec<arko::questions::AgentQuestion> = session
            .read(cx)
            .open_questions()
            .into_iter()
            .cloned()
            .collect();
        // The question with the keyboard (its field or a chip), if it is no
        // longer open.
        let left = self
            .answer_panel()
            .and_then(|panel| panel.read(cx).focused_question(window, cx))
            .filter(|id| !open.iter().any(|question| question.id == *id));
        // Settled elsewhere (another front end, or the agent), not by what
        // was just sent here: the typing doesn't move to another question.
        let elsewhere = left.as_ref().is_some_and(|id| {
            !session
                .read(cx)
                .reading()
                .questions
                .iter()
                .any(|question| question.id == *id)
        });
        if elsewhere {
            self.close_panel(window, cx);
            window.push_notification("That question is no longer open.", cx);
        }
        if open.is_empty() {
            self.close_panel(window, cx);
            self.panels
                .retain(|kept| kept.session.entity_id() != session.entity_id());
            return;
        }
        let panel = match self.panel() {
            Some(panel) => panel.clone(),
            None => {
                let panel = cx.new(AnswerPanel::new);
                self.panels.push(KeptPanel {
                    session: session.downgrade(),
                    view: panel.clone(),
                    _events: cx.subscribe_in(&panel, window, Self::on_panel_event),
                });
                panel
            }
        };
        panel.update(cx, |panel, cx| panel.set_questions(open, window, cx));
        if !self.panel_open && !session.read(cx).unseen_questions().is_empty() && !self.typing(cx) {
            self.panel_open = true;
            self.sync_composer(window, cx);
            cx.notify();
        }
        if self.panel_open {
            session.update(cx, |session, _| session.see_questions());
        }
    }

    /// Opens the answer panel, or gives it the keyboard (Alt+Q).
    pub fn answer_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_panel(None, window, cx);
    }

    /// Opens the answer panel on the open questions of the card `item`
    /// (its line in the transcript was clicked), or on the first question,
    /// and gives it the keyboard.
    pub(crate) fn open_panel(
        &mut self,
        item: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let open = self
            .session
            .as_ref()
            .is_some_and(|session| !session.read(cx).open_questions().is_empty());
        if !open {
            window.push_notification("No open questions.", cx);
            return;
        }
        self.panel_open = true;
        self.sync_questions(window, cx);
        self.sync_composer(window, cx);
        let Some(panel) = self.panel().cloned() else {
            return;
        };
        if !panel.read(cx).contains_focus(window, cx) {
            self.panel_return = window.focused(cx);
            if item.is_none() {
                panel.update(cx, |panel, cx| panel.focus(window, cx));
            }
        }
        if let Some(item) = item {
            panel.update(cx, |panel, cx| panel.focus_card(item, window, cx));
        }
        cx.notify();
    }

    /// Hides the answer panel, and gives the keyboard back if it had it.
    fn close_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.panel_open {
            return;
        }
        self.panel_open = false;
        let had_focus = self
            .panel()
            .is_some_and(|panel| panel.read(cx).contains_focus(window, cx));
        let back = self.panel_return.take();
        if had_focus {
            match back {
                Some(back) => window.focus(&back, cx),
                None => self.focus_composer(window, cx),
            }
        }
        self.sync_composer(window, cx);
        cx.notify();
    }

    /// Closes the panel and attaches the open questions to the composer,
    /// which the next message answers ("Answer in chat", Esc). The keyboard
    /// goes to the composer, wherever the panel was opened from.
    fn answer_in_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.attach_questions(cx));
        }
        self.panel_return = None;
        self.panel_open = false;
        self.focus_composer(window, cx);
        self.sync_composer(window, cx);
        cx.notify();
    }

    fn on_panel_event(
        &mut self,
        panel: &Entity<AnswerPanel>,
        event: &AnswerPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        match event {
            AnswerPanelEvent::Send => {
                let answers = panel.read(cx).answers(cx);
                let ids = answers.ids();
                let sent = session.update(cx, |session, cx| {
                    session.send(arko::Input::AnswerQuestions(answers), cx)
                });
                match sent {
                    Ok(()) => {
                        panel.update(cx, |panel, cx| panel.sent(&ids, cx));
                        self.close_panel(window, cx);
                    }
                    Err(why) => window.push_notification(why, cx),
                }
            }
            AnswerPanelEvent::AnswerInChat => self.answer_in_chat(window, cx),
            AnswerPanelEvent::Close => self.close_panel(window, cx),
        }
    }

    /// The approval tray while requests wait in it (tests and screenshots).
    #[doc(hidden)]
    pub fn approval_tray(&self) -> Option<&Entity<ApprovalTray>> {
        let session = self.session.as_ref()?.entity_id();
        self.trays
            .iter()
            .find(|kept| kept.session.entity_id() == session)
            .map(|kept| &kept.view)
    }

    /// Follows the requests waiting in the session's approval tray: the
    /// tray shows them, and goes when none wait.
    fn sync_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The trays of sessions that ended go with them.
        self.trays.retain(|kept| {
            kept.session
                .upgrade()
                .is_some_and(|session| *session.read(cx).phase() != Phase::Ended)
        });
        let Some(session) = self.session.clone() else {
            return;
        };
        let requests: Vec<arko::approvals::ApprovalRequest> =
            session.read(cx).approvals().into_iter().cloned().collect();
        if requests.is_empty() {
            let had_focus = self
                .approval_tray()
                .is_some_and(|tray| tray.read(cx).contains_focus(window, cx));
            self.trays
                .retain(|kept| kept.session.entity_id() != session.entity_id());
            if had_focus {
                self.focus_composer(window, cx);
            }
            return;
        }
        let tray = match self.approval_tray() {
            Some(tray) => tray.clone(),
            None => {
                let tray = cx.new(ApprovalTray::new);
                self.trays.push(KeptTray {
                    session: session.downgrade(),
                    view: tray.clone(),
                    _events: cx.subscribe_in(&tray, window, Self::on_tray_event),
                });
                tray
            }
        };
        tray.update(cx, |tray, cx| tray.set_requests(requests, cx));
    }

    /// Answers every request in the approval tray (Mod+Shift+A approves
    /// those checked and rejects the others, Mod+Shift+R rejects them all).
    /// What the message box holds goes with the answer, and the box
    /// empties. A message being edited or written to a worker stays.
    pub fn answer_approvals(&mut self, approve: bool, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(tray)) = (self.session.clone(), self.approval_tray().cloned())
        else {
            window.push_notification("No approvals wait.", cx);
            return;
        };
        let takes_words = self.editing.is_none() && self.target.is_none();
        let words = if takes_words {
            self.composer.read(cx).expanded_text(cx)
        } else {
            String::new()
        };
        let answers = tray.read(cx).answers(approve, &words);
        let sent = session.update(cx, |session, cx| {
            session.send(arko::Input::AnswerApprovals(answers.clone()), cx)
        });
        if let Err(why) = sent {
            window.push_notification(why, cx);
            return;
        }
        if answers.words.is_some() {
            self.composer
                .update(cx, |composer, cx| composer.set_text("", window, cx));
            self.keep_draft(cx);
        }
        // The tray goes, and the keyboard goes back to the message box if
        // the tray had it.
        self.sync_approvals(window, cx);
        cx.notify();
    }

    /// Gives the approval tray the keyboard, or gives it back to the
    /// message box (Alt+A).
    pub fn focus_approvals(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tray) = self.approval_tray().cloned() else {
            window.push_notification("No approvals wait.", cx);
            return;
        };
        if tray.read(cx).contains_focus(window, cx) {
            self.focus_composer(window, cx);
        } else {
            tray.update(cx, |tray, cx| tray.focus(window, cx));
        }
    }

    fn on_tray_event(
        &mut self,
        _: &Entity<ApprovalTray>,
        event: &TrayEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            TrayEvent::Answer { approve } => self.answer_approvals(*approve, window, cx),
            TrayEvent::Leave => self.focus_composer(window, cx),
        }
    }

    /// "Not now": the questions the chip stands for are dismissed, and the
    /// agent goes on with its defaults.
    fn dismiss_questions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (shown, _) = self.waiting_questions(cx);
        let ids: Vec<String> = shown.into_iter().map(|question| question.id).collect();
        let Some(session) = self.session.as_ref().filter(|_| !ids.is_empty()) else {
            return;
        };
        if let Err(why) = session.update(cx, |session, cx| {
            session.send(arko::Input::DismissQuestions(ids), cx)
        }) {
            window.push_notification(why, cx);
        }
        cx.notify();
    }

    /// What the composer says while empty.
    pub fn placeholder(&self, cx: &App) -> SharedString {
        let Some(session) = &self.session else {
            return "Message Arko".into();
        };
        if let Some(target) = &self.target {
            return format!("Message {target}").into();
        }
        let attached = session.read(cx).attached_questions().len();
        let session = session.read(cx);
        if attached == 1 {
            "Answer Arko’s question".into()
        } else if attached > 1 {
            "Answer Arko’s questions".into()
        } else if session.is_working() && !session.is_reporting() {
            "Add to the task while Arko works".into()
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
        let cancellable = self.target.is_some() || self.editing.is_some() || self.panel_open;
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
            ComposerEvent::Stop => self.stop(window, cx),
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
                // (Sending empties it, which isn't typing.)
                if !empty {
                    self.last_key = Some(cx.background_executor().now());
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
        // The answer panel over the composer closes first, as Esc in it
        // does.
        if self.panel_open {
            self.answer_in_chat(window, cx);
        } else if self.editing.is_some() {
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
            TranscriptEvent::OpenQuestions(item) => self.open_panel(Some(*item), window, cx),
            TranscriptEvent::MoveToBackground(item) => {
                if let Some(session) = &self.session {
                    session.update(cx, |session, cx| session.move_call(*item, cx));
                }
            }
            TranscriptEvent::StopJob(job) => {
                if let Some(session) = &self.session {
                    session.update(cx, |session, cx| session.stop_job(job, cx));
                }
            }
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
        let shown = self
            .transcript(cx)
            .is_some_and(|transcript| !transcript.read(cx).export_items().is_empty());
        let Some(live) = self.session.clone().filter(|_| shown) else {
            window.push_notification("There is no conversation to export yet.", cx);
            return;
        };
        let session = live.read(cx);
        let header = crate::export::Header {
            title: session.title().to_string(),
            mode: session.mode().label().to_string(),
            model: session.model().to_string(),
            folder: session
                .workspace()
                .map(|folder| shown_folder(folder, self.home.as_deref())),
            when: crate::figures::date_and_time(
                crate::figures::now_ms(),
                &crate::figures::Times::of(cx),
            ),
            starts_partway: false,
        };
        let root = cx
            .global::<crate::backend::SharedBackend>()
            .data_root()
            .to_path_buf();
        // The session reads it once nothing can still add to it, or says
        // when to export instead.
        let reading = session.saved_conversation(root, cx);
        let epoch = session.input_epoch();
        let exporting = cx.background_spawn(async move {
            let saved = reading.await?;
            Ok::<_, String>(crate::export::with_saved(header, saved))
        });
        let folder = crate::export::folder(
            session.workspace().map(PathBuf::as_path),
            self.home.as_deref(),
        );
        let name = crate::export::file_name(session.title());
        cx.spawn_in(window, async move |this, cx| {
            let exported = exporting.await.and_then(|export| {
                // The view went to another session, or this app sent the
                // session something since it was read.
                let current = this
                    .read_with(cx, |view, cx| {
                        view.session.as_ref() == Some(&live) && live.read(cx).input_epoch() == epoch
                    })
                    .unwrap_or(false);
                current.then_some(export).ok_or_else(|| {
                    "The conversation changed while it was read. Export it again.".to_string()
                })
            });
            let (text, starts_partway) = match exported {
                Ok(export) => export,
                Err(why) => {
                    cx.update(|window, cx| {
                        window.push_notification(
                            gpui_kit::component::notification::Notification::error(format!(
                                "Couldn’t export the conversation. {}",
                                crate::backend::sentence(&why)
                            )),
                            cx,
                        );
                    })
                    .ok();
                    return;
                }
            };
            let Ok(chosen) = cx.update(|_, cx| cx.prompt_for_new_path(&folder, Some(&name))) else {
                return;
            };
            let export = Export {
                text,
                starts_partway,
            };
            let answer = chosen
                .await
                .map_err(|_| "it closed without an answer".to_string());
            let path = match answer.and_then(|chosen| chosen.map_err(|error| error.to_string())) {
                Ok(Some(path)) => path,
                // Cancelled.
                Ok(None) => return,
                Err(why) => {
                    let instead = cx
                        .background_spawn(async move { crate::export::free_path(&folder, &name) })
                        .await;
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
    fn render_find(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let find = self.find.as_ref()?;
        let theme = cx.theme();
        // Close sits the box's padding (p_1) and 1 px border inside its
        // right edge, and its corners follow the box's. The other buttons
        // match it.
        let button_radius = concentric_radius(theme.radius_lg, window.rem_size() * 0.25 + px(1.));
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
                        .id("find-box")
                        .occlude()
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
                                    .rounded(button_radius)
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
                                .managed_tooltip(key_tip("Previous match", "shift-enter"))
                                .child(
                                    Button::new("find-previous")
                                        .ghost()
                                        .xsmall()
                                        .rounded(button_radius)
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
                                .managed_tooltip(key_tip("Next match", "enter"))
                                .child(
                                    Button::new("find-next")
                                        .ghost()
                                        .xsmall()
                                        .rounded(button_radius)
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
                                .managed_tooltip(key_tip("Close", "escape"))
                                .child(
                                    Button::new("find-close")
                                        .ghost()
                                        .xsmall()
                                        .rounded(button_radius)
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

    /// Whether Alt+↑ in the empty composer edits the latest message now: the
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
            answers: Vec::new(),
            continue_uncached: false,
        };
        if !prefs::get(cx).sessions.ask_before_edit {
            self.finish_edit(Choice::Change, item, message, window, cx);
            return;
        }
        let after = self
            .transcript(cx)
            .map_or(0, |transcript| transcript.read(cx).messages_after(item));
        let agent = self.mode(cx) == Mode::Agent;
        let view = cx.entity().downgrade();
        editing::confirm(
            Change::Edit,
            after,
            agent,
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
        let agent = self.mode(cx) == Mode::Agent;
        let view = cx.entity().downgrade();
        editing::confirm(
            Change::Regenerate,
            after,
            agent,
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
                        answers: Vec::new(),
                        continue_uncached: false,
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
    /// answer. The composer empties once it is taken. A message that
    /// continues a worker whose prompt cache expired asks first.
    pub fn send(&mut self, outgoing: Outgoing, window: &mut Window, cx: &mut Context<Self>) {
        self.send_as(outgoing, false, window, cx);
    }

    /// Sends `outgoing` as [`send`](Self::send) does. With
    /// `continue_uncached` the user agreed to continue the worker, so it
    /// goes without asking.
    fn send_as(
        &mut self,
        outgoing: Outgoing,
        continue_uncached: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
        // A line that reads as a command but isn't one stays in the
        // composer to put right. The session would say so too, but only
        // once the composer had emptied, and a worker would get the line
        // as a message. A space before the slash sends it as one.
        if let Some(why) = arko::commands::unknown(&text, crate::composer::offered_commands()) {
            window.push_notification(why, cx);
            return;
        }
        // While the old model writes its report for a switch, a message to
        // the main agent waits in the composer, as the strip above it says.
        // Commands and messages to a worker go.
        let command = arko::commands::find(&text).is_some();
        if !command && self.target.is_none() && self.is_reporting(cx) {
            return;
        }
        let attached = images;
        // Images the model can't take stay, with the message, until the
        // user removes them or picks another model.
        if !attached.is_empty()
            && let Some(why) = self.composer.read(cx).image_refusal()
        {
            window.push_notification(
                format!(
                    "The message wasn’t sent because {why}. Send it without images, or switch to a model that takes them."
                ),
                cx,
            );
            return;
        }
        let worker = self.session.as_ref().and(self.target.clone());
        if worker.is_some() && !attached.is_empty() {
            window.push_notification(
                "A message to a worker takes text only. Remove the images, or send them to Arko.",
                cx,
            );
            return;
        }
        // As `/message r1 …` typed in the terminal.
        let to_worker = worker
            .as_ref()
            .map(|worker| format!("/message {worker} {}", text.trim()));
        let line = to_worker.as_deref().unwrap_or(&text);
        let continues = arko::worker_cache::message_parts(line).map(|(to, _)| to.to_string());
        // Continuing a worker whose prompt cache expired re-reads its whole
        // conversation, so the message waits for the user's answer.
        if !continue_uncached && let Some((worker, warning)) = self.continue_question(line, cx) {
            let outgoing = Outgoing {
                text,
                images: attached,
            };
            self.ask_continue(&worker, &warning, outgoing, window, cx);
            return;
        }
        let images: Vec<arko::Attachment> = attached.iter().map(|image| image.to_arko()).collect();
        // A command runs and leaves the questions attached. Other text
        // answers them.
        let message = arko::Message {
            text: text.clone(),
            images,
            answers: if command || self.target.is_some() {
                Vec::new()
            } else {
                self.attached(cx)
            },
            continue_uncached,
        };
        let kept = continues.is_some() || worker.is_none() && !command;
        let unsent = (self.session.is_some() && kept).then(|| Unsent {
            text: text.clone(),
            images: attached.clone(),
            target: worker.clone(),
            worker: continues,
            answers: message.answers.clone(),
        });
        let result = match (&self.session, to_worker) {
            (Some(session), Some(line)) => {
                let input = arko::Input::Message(arko::Message {
                    text: line,
                    continue_uncached,
                    ..Default::default()
                });
                session.update(cx, |session, cx| session.send(input, cx))
            }
            (None, _) => self.start_session(message, attached, cx),
            (Some(session), None) => session.update(cx, |session, cx| {
                session.send(arko::Input::Message(message), cx)
            }),
        };
        match result {
            Ok(()) => {
                self.composer
                    .update(cx, |composer, cx| composer.sent(&text, window, cx));
                self.keep_draft(cx);
                self.composer_empty = true;
                self.unsent = unsent;
                if let Some(worker) = worker {
                    cx.emit(SessionViewEvent::SentToWorker(worker));
                }
                cx.notify();
            }
            Err(why) => window.push_notification(why, cx),
        }
    }

    /// Asks, with Arko's `warning`, before `outgoing` continues `worker`, a
    /// finished worker whose prompt cache expired. It stays in the composer
    /// meanwhile. Continue sends it while the view shows the same session
    /// and the composer still holds it for the same worker. Otherwise
    /// nothing goes, since the question was about another message.
    fn ask_continue(
        &mut self,
        worker: &str,
        warning: &str,
        outgoing: Outgoing,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self.session.as_ref().map(Entity::entity_id);
        let target = self.target.clone();
        let view = cx.entity().downgrade();
        let confirmed = worker.to_string();
        crate::prompts::confirm_continue(worker, warning, window, cx, move |window, cx| {
            view.update(cx, |view, cx| {
                let holds = view.session.as_ref().map(Entity::entity_id) == session
                    && view.target == target
                    && view.editing.is_none()
                    && view.composer.read(cx).expanded_text(cx) == outgoing.text;
                if holds {
                    view.forget_refusal(&confirmed);
                    view.send_as(outgoing.clone(), true, window, cx);
                }
            })
            .ok();
        });
    }

    /// The worker the message `line` continues, and the warning to ask
    /// with, when it asks first. It asks when what the window knows of the
    /// worker says its cache expired, or when arko didn't continue the
    /// worker and the window doesn't know that yet.
    fn continue_question(&mut self, line: &str, cx: &App) -> Option<(String, String)> {
        let session = self.session.as_ref()?;
        let now = chrono::Utc::now();
        if let Some(cache) = workers::asks_to_continue(line, session.read(cx).workers(), now) {
            // The window knows now what arko knew.
            self.forget_refusal(&cache.worker);
            return Some((cache.worker.clone(), cache.warning(now)));
        }
        let (to, _) = arko::worker_cache::message_parts(line)?;
        self.continue_refused
            .clone()
            .filter(|(worker, _)| worker.eq_ignore_ascii_case(to))
    }

    /// Forgets that arko didn't continue `worker`, if it didn't.
    fn forget_refusal(&mut self, worker: &str) {
        if self
            .continue_refused
            .as_ref()
            .is_some_and(|(refused, _)| refused.eq_ignore_ascii_case(worker))
        {
            self.continue_refused = None;
        }
    }

    /// Whether the shown session's old model writes its report for a switch.
    fn is_reporting(&self, cx: &App) -> bool {
        self.session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_reporting())
    }

    /// Puts the message arko didn't send back in the composer, to whom it
    /// went and with the questions it answered, unless something else was
    /// typed since or a message is being edited. The questions are open
    /// again either way. `is` says whether it is the message arko's notice
    /// is about. Gives back what returned.
    fn restore_unsent(
        &mut self,
        is: impl FnOnce(&Unsent) -> bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Outgoing> {
        let unsent = self.unsent.as_ref().filter(|unsent| is(unsent))?;
        if let Some(session) = &self.session {
            let answers = unsent.answers.clone();
            session.update(cx, |session, cx| session.reopen_questions(&answers, cx));
        }
        let empty = {
            let composer = self.composer.read(cx);
            composer.text(cx).is_empty() && composer.images().is_empty()
        };
        if !empty || self.editing.is_some() {
            return None;
        }
        let Unsent {
            text,
            images,
            target,
            answers,
            ..
        } = self.unsent.take()?;
        self.set_target(target, window, cx);
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.reattach_questions(&answers, cx));
        }
        self.composer_empty = false;
        self.composer.update(cx, |composer, cx| {
            composer.set_text(&text, window, cx);
            for image in images.iter().cloned() {
                composer.attach_image(image, cx);
            }
        });
        self.keep_draft(cx);
        cx.notify();
        Some(Outgoing { text, images })
    }

    /// Arko didn't continue `worker`, whose prompt cache expired, with the
    /// message given to it last. The message comes back to the composer,
    /// and the dialog asks with Arko's `warning` whether to continue the
    /// worker. Messages to the worker ask too, until one is confirmed.
    fn not_continued(
        &mut self,
        worker: &str,
        warning: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.continue_refused = Some((worker.to_string(), warning.to_string()));
        let goes_to_worker = |unsent: &Unsent| {
            unsent
                .worker
                .as_deref()
                .is_some_and(|to| to.eq_ignore_ascii_case(worker))
        };
        if let Some(outgoing) = self.restore_unsent(goes_to_worker, window, cx) {
            self.ask_continue(worker, warning, outgoing, window, cx);
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
            // by its name or its alias, once the list is known, or any of
            // OpenRouter's by its slug.
            (Some("/model"), Some(model), effort)
                if self.session.is_none() && model != "worker" =>
            {
                let listed = self.models.as_ref().map(|(_, choices)| {
                    choices
                        .groups
                        .iter()
                        .flat_map(|group| &group.models)
                        .find(|choice| choice.name == model || choice.alias == model)
                        .map(|choice| choice.name.clone())
                });
                let name = match listed {
                    Some(Some(name)) => name,
                    Some(None) if crate::openrouter::slug(model).is_none() => {
                        return Err(format!("Arko has no model called “{model}”."));
                    }
                    _ => model.to_string(),
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
            // What the model list shows is what runs.
            effort: self
                .draft_model(cx)
                .and_then(|model| self.effort_for(&model, cx))
                .map(|(effort, _)| effort),
        };
        let text = message.text.clone();
        remember_mode(new.mode, cx);
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
        // The session just started may change its model's last effort.
        self.last_effort_reads.clear();
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

    /// Moves the main agent's running call to the background, a long one
    /// that can move, such as a shell command or an MCP tool's call
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
            window.push_notification("Nothing is running that can move to the background.", cx);
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
                let (_, effort) = crate::live::conversation_model(&session.reading().models);
                (
                    Some(session.model().to_string()).filter(|model| !model.is_empty()),
                    effort.map(str::to_string),
                )
            }
            None => (
                self.draft_model(cx)
                    .map(|model| arko::models::display_name(&model)),
                self.draft_effort(cx),
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
        let opening = view.clone();
        Popover::new("mode-popover")
            .anchor(Anchor::BottomLeft)
            .on_open_change(move |open, _, cx| {
                if *open {
                    opening
                        .update(cx, |view, cx| {
                            // It opens on the main agent's models.
                            view.lists_worker_models = false;
                            view.forget_last_efforts(cx)
                        })
                        .ok();
                }
            })
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
        // status. Once its conversation has started, the list is the main
        // agent's, where another model starts a new conversation, or the
        // new workers'.
        let session_models = self
            .session
            .as_ref()
            .map(|session| session.read(cx).reading().models.clone());
        if let Some(models) = &session_models {
            self.see_session_models(models, cx);
        }
        let lists_both = started
            && session_models
                .as_ref()
                .is_some_and(|models| models.has_workers);
        let workers_only = lists_both && self.lists_worker_models;
        let (choices, current, current_effort) = match &session_models {
            None => (
                self.models
                    .as_ref()
                    .map(|(_, models)| models.clone())
                    .unwrap_or_default(),
                self.draft_model(cx),
                None,
            ),
            Some(models) => {
                let choices = self
                    .session_models
                    .as_ref()
                    .map(|(_, choices)| choices.clone())
                    .unwrap_or_default();
                let (current, effort) = if workers_only {
                    (models.workers.as_str(), None)
                } else if started {
                    crate::live::conversation_model(models)
                } else {
                    (models.main.as_str(), Some(models.main_effort.as_str()))
                };
                (
                    choices,
                    Some(current.to_string()).filter(|model| !model.is_empty()),
                    effort.map(str::to_string),
                )
            }
        };
        // A switch waits for the turn to end.
        let working = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_working());
        let waits = started && !workers_only && working;

        self.see_openrouter(cx);
        let rows = choices
            .groups
            .iter()
            .enumerate()
            .filter_map(|(group_ix, group)| {
                let openrouter = group.name.starts_with("OpenRouter");
                let models: Vec<_> = group
                    .models
                    .iter()
                    .filter(|model| {
                        // By the name its row shows too, OpenRouter's own
                        // for its models.
                        let shown = openrouter
                            .then(|| self.openrouter_model(&model.name))
                            .flatten()
                            .map(|known| known.label.to_lowercase());
                        filter.is_empty()
                            || model.name.to_lowercase().contains(&filter)
                            || model.alias.to_lowercase().contains(&filter)
                            || arko::models::display_name(&model.name)
                                .to_lowercase()
                                .contains(&filter)
                            || shown.is_some_and(|shown| shown.contains(&filter))
                    })
                    .cloned()
                    .collect();
                // A search also looks in OpenRouter's catalog: the first
                // few models found that aren't rows already join the group.
                let (found, unlisted, matching) = if openrouter && !filter.trim().is_empty() {
                    let catalog = &self.openrouter.catalog.models;
                    let matching = crate::openrouter::search(catalog, &filter);
                    let unlisted: Vec<_> = matching
                        .iter()
                        .map(|at| &catalog[*at])
                        .filter(|model| !models.iter().any(|row| row.name == model.name))
                        .collect();
                    let found: Vec<_> = unlisted
                        .iter()
                        .take(OPENROUTER_FOUND)
                        .map(|model| (*model).clone())
                        .collect();
                    (found, unlisted.len(), matching.len())
                } else {
                    (Vec::new(), 0, 0)
                };
                let loading = openrouter
                    && !filter.trim().is_empty()
                    && self.openrouter.loading
                    && self.openrouter.catalog.models.is_empty();
                if models.is_empty() && found.is_empty() && !loading {
                    return None;
                }
                let usable = group.needs.is_none();
                let needs = group.needs.clone();
                let muted = theme.muted_foreground;
                // Opens OpenRouter's catalog on `query`, choosing for this
                // list's agent.
                let browse = {
                    let (view, popover, needs) = (view.clone(), popover.clone(), needs.clone());
                    let current = current.clone();
                    move |query: String, window: &mut Window, cx: &mut App| {
                        popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                        let view = view.clone();
                        crate::openrouter::browse(
                            crate::openrouter::Browse {
                                query,
                                current: current.clone(),
                                needs: needs.clone(),
                                refuse_unusable: true,
                            },
                            Rc::new(move |model, window, cx| {
                                view.update(cx, |view, cx| {
                                    view.choose_model(model.name, workers_only, window, cx)
                                })
                                .ok();
                            }),
                            window,
                            cx,
                        );
                    }
                };
                let heading = h_flex()
                    .pt_1()
                    .justify_between()
                    .text_xs()
                    .text_color(muted)
                    .child(group.name.clone())
                    .when(openrouter, |this| {
                        let tooltip = match &self.openrouter.summary {
                            Some(summary) => format!("OpenRouter settings\n{summary}"),
                            None => "OpenRouter settings".to_string(),
                        };
                        let (view, popover) = (view.clone(), popover.clone());
                        this.child(
                            Button::new(("model-group-settings", group_ix))
                                .ghost()
                                .xsmall()
                                .compact()
                                .icon(IconName::Settings)
                                .tooltip(tooltip)
                                .accessibility_label("OpenRouter settings")
                                .on_click(move |_, window, cx| {
                                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
                                    view.update(cx, |_, cx| {
                                        cx.emit(SessionViewEvent::OpenAccounts(Some(
                                            crate::settings_page::OPENROUTER_WAY.into(),
                                        )))
                                    })
                                    .ok();
                                }),
                        )
                    });
                // A row that looks further, muted, and what a screen reader
                // says for it.
                let link = |id: SharedString, text: String, spoken: String| {
                    Button::new(id)
                        .ghost()
                        .small()
                        .w_full()
                        .justify_start()
                        .accessibility_label(spoken)
                        .child(div().text_xs().text_color(muted).child(text))
                };
                Some(
                    v_flex()
                        .gap_0p5()
                        .child(heading)
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
                            // An OpenRouter model's name as OpenRouter has
                            // it, and its context and prices in its tooltip.
                            let known = openrouter
                                .then(|| self.openrouter_model(&model.name))
                                .flatten();
                            let label = known
                                .map(|known| known.label.clone())
                                .unwrap_or_else(|| arko::models::display_name(&model.name));
                            let details = known
                                .map(|known| known.details())
                                .filter(|details| !details.is_empty());
                            // How a model billed per token is billed, at the
                            // row's end, as the terminal's picker and the
                            // Settings lists show it.
                            let billing = choices.billing(&model.name);
                            // What a screen reader hears: as in the Settings
                            // lists, then what its tooltip says.
                            let spoken = crate::model_select::spoken_row(
                                &label,
                                &group.name,
                                billing.as_deref(),
                                usable,
                            );
                            // What to know before choosing it, in the row's
                            // tooltip, marked by an (i) after its name.
                            let note = arko::models::note(&model.name);
                            let name = model.name.clone();
                            let view = view.clone();
                            let popover = popover.clone();
                            Button::new(SharedString::from(format!("model-{group_ix}-{ix}")))
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .disabled(!usable || waits)
                                .when(checked, |this| this.icon(IconName::Check))
                                .accessibility_label(match (note, &details) {
                                    (Some(note), _) => format!("{spoken}. {note}"),
                                    (None, Some(details)) => format!("{spoken}. {details}"),
                                    (None, None) => spoken,
                                })
                                .when_some(note, |this, note| {
                                    this.tooltip(note).child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "model-note-{group_ix}-{ix}"
                                            )))
                                            .test_support()
                                            .flex()
                                            .child(
                                                Icon::new(IconName::Info)
                                                    .xsmall()
                                                    .text_color(theme.muted_foreground),
                                            ),
                                    )
                                })
                                .when_some(details, |this, details| this.tooltip(details))
                                .when_some(billing, |this, billing| {
                                    this.child(
                                        div()
                                            .id(SharedString::from(format!(
                                                "model-billing-{group_ix}-{ix}"
                                            )))
                                            .test_support()
                                            .flex_none()
                                            .pl_1()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(billing),
                                    )
                                })
                                .label(label)
                                .on_click(move |_, window, cx| {
                                    let name = name.clone();
                                    // With its efforts to choose from, the
                                    // list stays open on them. After the
                                    // start, the switch asks for its effort.
                                    let stays = !started && reasons(&name);
                                    // Closed first, so a dialog that asks
                                    // gives the keyboard back to the chip.
                                    if !stays {
                                        popover
                                            .update(cx, |popover, cx| popover.dismiss(window, cx));
                                    }
                                    view.update(cx, |view, cx| {
                                        view.choose_model(name, workers_only, window, cx)
                                    })
                                    .ok();
                                })
                        }))
                        .children(found.into_iter().enumerate().map(|(ix, model)| {
                            let checked = current.as_deref() == Some(model.name.as_str());
                            let details = model.details();
                            // Billed and heard as the group's own rows are.
                            let billing = choices.billing(&model.name);
                            let spoken = crate::model_select::spoken_row(
                                &model.label,
                                &group.name,
                                billing.as_deref(),
                                usable,
                            );
                            let (view, popover) = (view.clone(), popover.clone());
                            Button::new(SharedString::from(format!("model-found-{ix}")))
                                .ghost()
                                .small()
                                .w_full()
                                .justify_start()
                                .disabled(!usable || waits)
                                .when(checked, |this| this.icon(IconName::Check))
                                .accessibility_label(if details.is_empty() {
                                    spoken
                                } else {
                                    format!("{spoken}. {details}")
                                })
                                .when(!details.is_empty(), |this| this.tooltip(details.clone()))
                                .when_some(billing, |this, billing| {
                                    this.child(
                                        div()
                                            .flex_none()
                                            .pl_1()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(billing),
                                    )
                                })
                                .label(model.label.clone())
                                .on_click(move |_, window, cx| {
                                    let name = model.name.clone();
                                    let stays = !started && reasons(&name);
                                    // Closed first, so a dialog that asks
                                    // gives the keyboard back to the chip.
                                    if !stays {
                                        popover
                                            .update(cx, |popover, cx| popover.dismiss(window, cx));
                                    }
                                    view.update(cx, |view, cx| {
                                        view.choose_model(name, workers_only, window, cx)
                                    })
                                    .ok();
                                })
                        }))
                        .when(loading, |this| {
                            this.child(
                                div()
                                    .id("model-found-loading")
                                    .test_support()
                                    .px_2()
                                    .text_xs()
                                    .text_color(muted)
                                    .child("Loading…"),
                            )
                        })
                        .when(openrouter && filter.trim().is_empty(), |this| {
                            let browse = browse.clone();
                            this.child(
                                link(
                                    "model-browse".into(),
                                    "All OpenRouter models…".into(),
                                    "All OpenRouter models".into(),
                                )
                                .on_click(move |_, window, cx| browse(String::new(), window, cx)),
                            )
                        })
                        .when(unlisted > OPENROUTER_FOUND, |this| {
                            let query = filter.trim().to_string();
                            this.child(
                                link(
                                    "model-see-all".into(),
                                    format!("See all {matching}"),
                                    format!("See all {matching} OpenRouter models found"),
                                )
                                .on_click(move |_, window, cx| browse(query.clone(), window, cx)),
                            )
                        })
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
                    None => self.effort_for(&model, cx),
                    Some(_) => current_effort
                        .clone()
                        .map(|effort| (effort, EffortFrom::Chosen)),
                };
                (model, efforts, chosen)
            })
            .filter(|(_, efforts, _)| !efforts.is_empty());

        v_flex()
            .id("mode-choice")
            .key_context(MODE_CHOICE_CONTEXT)
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
            .when(working, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(if waits {
                            "Switching modes or models waits for the turn to end."
                        } else {
                            "Switching between Chat and Agent waits for the turn to end."
                        }),
                )
            })
            .when(lists_both, |this| {
                this.child(
                    h_flex()
                        .justify_center()
                        .child(self.render_model_for(window, cx)),
                )
            })
            .child(Input::new(&self.model_filter).small().cleanable(true))
            .child(
                v_flex()
                    .id("model-list")
                    .test_support()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(rows),
            )
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
                            // The one chosen is filled. One the session would
                            // start with unless another is chosen is outlined,
                            // saying why.
                            let from = chosen
                                .as_ref()
                                .filter(|(shown, _)| *shown == effort)
                                .map(|(_, from)| *from);
                            let view = view.clone();
                            let popover = popover.clone();
                            let model = model.clone();
                            Button::new(SharedString::from(format!("effort-{effort}")))
                                .xsmall()
                                // A screen reader hears which one runs, and why.
                                .accessibility_label(match from {
                                    Some(EffortFrom::Chosen) => format!("{effort}, chosen"),
                                    Some(EffortFrom::LastUsed) => format!("{effort}, last used"),
                                    Some(EffortFrom::Settings | EffortFrom::Default) => {
                                        format!("{effort}, default")
                                    }
                                    None => effort.clone(),
                                })
                                .toggled(from.is_some())
                                .map(|this| match from {
                                    Some(EffortFrom::Chosen) => this.primary(),
                                    Some(EffortFrom::LastUsed) => {
                                        this.outline().tooltip(format!("{effort} - Last used"))
                                    }
                                    Some(EffortFrom::Settings | EffortFrom::Default) => {
                                        this.outline().tooltip(format!("{effort} - Default"))
                                    }
                                    None => this.ghost(),
                                })
                                .disabled(waits)
                                .label(effort.clone())
                                .on_click(move |_, window, cx| {
                                    let (model, effort) = (model.clone(), effort.clone());
                                    popover.update(cx, |popover, cx| popover.dismiss(window, cx));
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

    /// Main agent | New workers, once the conversation has started: whose
    /// models the list shows. It looks and works as Chat | Agent does, a
    /// group of radio buttons whose ←/→ choose while it has focus.
    fn render_model_for(&self, window: &Window, cx: &Context<Self>) -> impl IntoElement + use<> {
        let workers = self.lists_worker_models;
        let focused = self.model_for_focus.is_focused(window) && window.last_input_was_keyboard();
        let view = cx.entity().downgrade();
        let choose = Rc::new(move |workers: bool, cx: &mut App| {
            view.update(cx, |view, cx| {
                view.lists_worker_models = workers;
                cx.notify();
            })
            .ok();
        });
        let segment = |value: bool, id: &'static str, label: &'static str| {
            let checked = workers == value;
            let choose = choose.clone();
            h_flex()
                .id(id)
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
                .items_center()
                .justify_center()
                .map(|this| segmented::segment_style(this, checked, false, cx))
                .child(segmented::segment_label(label))
                .when(!checked, |this| {
                    this.cursor_pointer()
                        .on_click(move |_, _, cx| choose(value, cx))
                })
        };
        h_flex()
            .id("model-for")
            .test_support()
            .track_focus(&self.model_for_focus)
            .role(Role::RadioGroup)
            .aria_label("Agents")
            .map(|this| segmented::track_style(this, cx))
            .on_key_down({
                let choose = choose.clone();
                move |event: &KeyDownEvent, _, cx| {
                    let to = match event.keystroke.key.as_str() {
                        "left" => false,
                        "right" => true,
                        _ => return,
                    };
                    cx.stop_propagation();
                    if to != workers {
                        choose(to, cx);
                    }
                }
            })
            .when(focused, |this| this.focus_ring_style(window, cx))
            .child(segment(false, "model-for-main", "Main agent"))
            .child(segment(true, "model-for-workers", "New workers"))
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
        cx.spawn_in(window, async move |_, cx| match switching.await {
            Ok(_) => {
                cx.update(|_, cx| remember_mode(to, cx)).ok();
            }
            Err(why) => {
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
        let Some(session) = self.session.clone() else {
            self.set_draft_model(model, None, cx);
            return;
        };
        if workers {
            self.run_command(session, format!("/model worker {model}"), window, cx);
        } else if session.read(cx).has_started() {
            self.switch_model(session, model, None, window, cx);
        } else {
            self.run_command(session, format!("/model {model}"), window, cx);
        }
    }

    fn choose_effort(
        &mut self,
        model: String,
        effort: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.session.clone() {
            None => self.set_draft_model(model, Some(effort), cx),
            Some(session) if session.read(cx).has_started() => {
                self.switch_model(session, model, Some(effort), window, cx)
            }
            Some(session) => {
                self.run_command(session, format!("/model {model} {effort}"), window, cx)
            }
        }
    }

    /// Once the conversation has started, the main agent moves to `model`
    /// at `effort` (none: the one arko fits) in a new conversation that
    /// carries the current one over. Settings say whether to ask first.
    fn switch_model(
        &mut self,
        session: Entity<LiveSession>,
        model: String,
        effort: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let models = session.read(cx).reading().models.clone();
        let (current, current_effort) = crate::live::conversation_model(&models);
        let keeps_model = model == current;
        // (An effort not known to be the conversation's may differ.)
        if keeps_model && (effort.is_none() || effort.as_deref() == current_effort) {
            return;
        }
        if session.read(cx).is_working() {
            window.push_notification("Wait for the turn to end.", cx);
            return;
        }
        if !prefs::get(cx).sessions.ask_before_switch {
            self.run_command(
                session,
                model_command(&model, effort.as_deref()),
                window,
                cx,
            );
            return;
        }
        // Another model's effort is chosen in the dialog, from the one the
        // conversation runs at, as arko would fit it.
        let efforts = if keeps_model {
            Vec::new()
        } else {
            arko::commands::dynamic_choices(&arko::remote::Reading::default(), "/model", &[&model])
        };
        let from = current_effort.unwrap_or(&models.main_effort);
        let effort = effort.or_else(|| fit_effort(&efforts, from));
        let label = self
            .openrouter_model(&model)
            .map(|known| known.label.clone())
            .unwrap_or_else(|| arko::models::display_name(&model));
        let command = move |effort: Option<&str>| model_command(&model, effort);
        let view = cx.entity().downgrade();
        crate::prompts::confirm_model_switch(
            crate::prompts::ModelSwitch {
                label,
                effort,
                efforts,
                keeps_model,
            },
            window,
            cx,
            move |effort, window, cx| {
                let command = command(effort.as_deref());
                let session = session.clone();
                view.update(cx, |view, cx| {
                    view.run_command(session, command, window, cx)
                })
                .ok();
            },
        );
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
            ..Default::default()
        });
        if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
            window.push_notification(why, cx);
        }
    }

    /// Chat | Agent looks like the kit's segmented tabs, an outlined track
    /// with the chosen mode filled and outlined, each mode with its icon, and
    /// a line on what the chosen mode does. ←/→ switch while it has focus.
    /// `on_pick` hears the mode chosen. An open session switches between
    /// turns only.
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
                .gap_1p5()
                .items_center()
                .justify_center()
                .map(|this| segmented::segment_style(this, checked, !can_switch, cx))
                .child(Icon::new(icon).small())
                .child(segmented::segment_label(label))
                .when(can_switch && !checked, |this| {
                    this.cursor_pointer()
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
                    .when(can_switch, |this| this.track_focus(focus))
                    .role(Role::RadioGroup)
                    .aria_label("Mode")
                    .map(|this| segmented::track_style(this, cx))
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
            .min_w_0()
            .max_w_full()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .map(|this| match self.draft.mode {
                Mode::Chat => this.child("New chat"),
                Mode::Agent => this
                    .child(div().flex_none().child(match &folder {
                        Some(_) => "New agent session in",
                        None => "New agent session",
                    }))
                    .when_some(folder.clone(), |this, folder| {
                        let full_path: SharedString = folder.display().to_string().into();
                        this.child(
                            div()
                                .id("draft-folder")
                                .test_support()
                                .min_w_0()
                                .truncate()
                                .text_ellipsis_middle()
                                .text_color(theme.foreground)
                                .managed_tooltip(move |window, cx| {
                                    Tooltip::new(full_path.clone()).build(window, cx)
                                })
                                .child(shown_folder(&folder, home.as_deref())),
                        )
                    })
                    .child(
                        Button::new("change-folder")
                            .flex_none()
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
                .w_full()
                .min_w_0()
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
            Phase::Sample => "A sample session that shows, but doesn’t run.".into(),
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

    /// The chip above the composer for open questions neither attached nor
    /// in the panel: "Arko asks 2 questions" for new ones, which came while
    /// the user typed, else a quiet "2 open questions". It opens the panel,
    /// and "Not now" dismisses them.
    fn render_questions_chip(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (waiting, new) = self.waiting_questions(cx);
        if waiting.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let (label, color) = if new {
            (crate::questions::asks_text(&waiting), theme.foreground)
        } else {
            (
                crate::questions::open_text(waiting.len()),
                theme.muted_foreground,
            )
        };
        Some(
            h_flex()
                .id("questions-chip")
                .test_support()
                .mb_1()
                .gap_1()
                .child(
                    Button::new("questions-open")
                        .ghost()
                        .xsmall()
                        .icon(
                            Icon::new(gpui_kit::assets::IconName::MessageCircleQuestionMark)
                                .text_color(if new {
                                    theme.warning
                                } else {
                                    theme.muted_foreground
                                }),
                        )
                        .child(div().text_color(color).child(label.clone()))
                        .accessibility_label(label)
                        .tooltip_with_action("Answer", &crate::questions::AnswerQuestions, None)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.answer_questions(window, cx)),
                        ),
                )
                .child(
                    Button::new("questions-not-now")
                        .ghost()
                        .xsmall()
                        .label("Not now")
                        .tooltip("Arko goes on without your answers")
                        .on_click(
                            cx.listener(|this, _, window, cx| this.dismiss_questions(window, cx)),
                        ),
                )
                .into_any_element(),
        )
    }

    /// "Replying to 2 questions ×": the questions the next message answers,
    /// and the way to send it without answering them.
    fn render_replying(&self, count: usize, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .id("questions-replying")
            .test_support()
            .flex_none()
            .h_6()
            .pl_2()
            .gap_1()
            .rounded(theme.radius)
            .bg(theme.secondary)
            .text_xs()
            .text_color(theme.secondary_foreground)
            .child(Icon::new(gpui_kit::assets::IconName::MessageCircleQuestionMark).xsmall())
            .child(crate::questions::replying_text(count))
            .child(
                Button::new("questions-detach")
                    .ghost()
                    .xsmall()
                    .compact()
                    .icon(IconName::Close)
                    .tooltip("Remove")
                    .accessibility_label("Remove")
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(session) = &this.session {
                            session.update(cx, |session, cx| session.detach_questions(cx));
                        }
                        this.sync_composer(window, cx);
                    })),
            )
            .into_any_element()
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

    /// The strip above the composer while the old model writes its report
    /// for a switch: a message to the main agent waits meanwhile.
    fn render_reporting(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.is_reporting(cx) {
            return None;
        }
        let theme = cx.theme();
        Some(
            h_flex()
                .id("reporting-strip")
                .test_support()
                .mb_2()
                .px_3()
                .py_1()
                .gap_2()
                .text_sm()
                .rounded(theme.radius_lg)
                .bg(theme.secondary)
                .text_color(theme.secondary_foreground)
                .child(div().flex_1().min_w_0().child(crate::live::REPORTING))
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
                .child(div().flex_1().min_w_0().truncate().child(editing_words(
                    crate::prefs::get(cx).input.keymap == crate::prefs::Keymap::Vim,
                )))
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
                    .managed_tooltip(|window, cx| {
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
        let attached = self.attached(cx).len();
        let replying = (attached > 0).then(|| self.render_replying(attached, cx));
        let theme = cx.theme();
        let working = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_working());
        let tray = self.approval_tray().cloned();
        let panel = self.answer_panel().cloned().map(|panel| {
            // Above the composer and the approval tray, over the
            // transcript's end.
            div()
                .absolute()
                .left_0()
                .right_0()
                .bottom(relative(1.))
                .mb_2()
                .child(panel)
        });
        // With the tray, the panel floats above it.
        let (above_tray, panel) = if tray.is_some() {
            (panel, None)
        } else {
            (None, panel)
        };
        let expanded = prefs::get(cx).input.composer_expanded;
        let send_key = prefs::get(cx).input.send_key;
        let send_chord = Keystroke::parse(crate::composer::send_chord(send_key)).ok();
        let composer = v_flex()
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
            .shadow_sm()
            .children(panel)
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
                                "Shrink the message box"
                            } else {
                                "Expand the message box"
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
                    .children(replying)
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
                                .managed_tooltip(move |window, cx| {
                                    Tooltip::new(label)
                                        .key_binding(chord.clone().map(Kbd::new))
                                        .build(window, cx)
                                })
                                .child(button.accessibility_label(label)),
                        )
                    }),
            );
        // The approval tray right above the message box, whose words go
        // with its answer. It grows less while the answer panel floats
        // above it.
        let tallest = window.viewport_size().height
            * if above_tray.is_some() {
                crate::approvals::MAX_HEIGHT_UNDER_PANEL
            } else {
                crate::approvals::MAX_HEIGHT
            };
        let tray = tray.map(|tray| v_flex().w_full().max_h(tallest).mb_2().child(tray));
        v_flex()
            .relative()
            .w_full()
            .children(tray)
            .children(above_tray)
            .child(composer)
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

/// Keeps `mode` as the last one a session started or switched to, which
/// Last used starts the next new session in.
fn remember_mode(mode: Mode, cx: &mut App) {
    let mode = match mode {
        Mode::Chat => prefs::NewSessionMode::Chat,
        Mode::Agent => prefs::NewSessionMode::Agent,
    };
    if prefs::get(cx).new_sessions.last_mode != Some(mode) {
        prefs::update(cx, |prefs| prefs.new_sessions.last_mode = Some(mode));
    }
}

/// How a new session works until the user picks: as Settings › Models &
/// accounts says, or as the last session started for Last used.
fn default_mode(cx: &App) -> Mode {
    match prefs::get(cx).new_sessions.starting_mode() {
        prefs::NewSessionMode::Chat => Mode::Chat,
        _ => Mode::Agent,
    }
}

/// The command that sets the main agent's model, at `effort` if given.
fn model_command(model: &str, effort: Option<&str>) -> String {
    match effort {
        Some(effort) => format!("/model {model} {effort}"),
        None => format!("/model {model}"),
    }
}

/// Whether `model` takes an effort, so the model list offers its efforts.
fn reasons(model: &str) -> bool {
    !arko::commands::dynamic_choices(&arko::remote::Reading::default(), "/model", &[model])
        .is_empty()
}

/// The main agent's model Settings set for new sessions, as the backend
/// takes it.
fn settings_model(cx: &App) -> Option<String> {
    prefs::get(cx)
        .new_sessions
        .model
        .clone()
        .filter(|model| !model.trim().is_empty())
}

/// The effort Settings set for new sessions, when arko takes it.
fn settings_effort(cx: &App) -> Option<String> {
    prefs::get(cx)
        .new_sessions
        .effort
        .clone()
        .filter(|effort| effort.parse::<arko::Effort>().is_ok())
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
        Mode::Chat => "Chat has no tools.",
        Mode::Agent => "Agent works in a folder with tools and workers.",
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
        self.renders += 1;
        let narrow = self.width == WidthClass::Narrow;
        let questions = self.render_questions_chip(cx);
        let prompt = self.render_prompt(cx);
        let editing = self.render_editing(cx);
        let reporting = self.render_reporting(cx);
        let find = self.render_find(window, cx);
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
            .child(
                div()
                    .id("transcript")
                    .test_support()
                    .relative()
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
                    })
                    // The find bar floats over the transcript's top, so
                    // opening it moves nothing.
                    .children(find.map(|find| {
                        h_flex()
                            .absolute()
                            .top_0()
                            .left_0()
                            .right_0()
                            .justify_center()
                            .child(find)
                    })),
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
                    .children(questions)
                    .children(editing)
                    .children(reporting)
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

/// What the strip above the composer says while it edits a message, and how
/// Esc cancels (in Vim's normal mode, with its keys).
fn editing_words(vim: bool) -> &'static str {
    if vim {
        "Editing message. Esc in normal mode cancels."
    } else {
        "Editing message. Esc cancels."
    }
}

/// What the notification says of an export written to `path`.
fn exported_words(path: &Path, starts_partway: bool) -> String {
    let tail = if starts_partway {
        ", without the earliest messages."
    } else {
        "."
    };
    format!("Exported to {}{tail}", path.display())
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
                Ok(()) => window.push_notification(exported_words(&path, starts_partway), cx),
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
        FindStatus, editing_words, exported_words, find_status_text, images_not_kept,
        mode_description, recent_folders, shown_folder_from,
    };
    use crate::backend::Backend as _;
    use crate::backend::{FakeBackend, Mode, SessionEntry};
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    #[test]
    fn the_mode_line_says_what_the_mode_does_and_what_switching_costs() {
        assert_eq!(mode_description(Mode::Chat, false), "Chat has no tools.");
        assert_eq!(
            mode_description(Mode::Agent, true),
            "Agent works in a folder with tools and workers. Switching re-reads the conversation without its cache."
        );
    }

    #[test]
    fn the_editing_strip_says_how_esc_cancels() {
        assert_eq!(editing_words(false), "Editing message. Esc cancels.");
        assert_eq!(
            editing_words(true),
            "Editing message. Esc in normal mode cancels."
        );
    }

    #[test]
    fn an_export_that_starts_partway_says_so_in_one_sentence() {
        let path = Path::new("/tmp/Notes.md");
        assert_eq!(exported_words(path, false), "Exported to /tmp/Notes.md.");
        assert_eq!(
            exported_words(path, true),
            "Exported to /tmp/Notes.md, without the earliest messages."
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
