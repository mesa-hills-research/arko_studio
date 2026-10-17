//! The sessions running in this app: each on arko's engine, on threads of
//! its own, several at once, whichever window shows them.
//!
//! A [`LiveSession`] holds what the app keeps of a session's seam: where its
//! input goes, its feed and its figures. What the engine says (the feed's
//! activity and workers, the status, its questions before quitting) is
//! drained on background tasks and applied on the UI thread at most once a
//! frame, so the engine never waits for the interface and a burst of
//! updates is one change. Switching the window to another session leaves
//! this one running, and the sidebar shows what it does.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use arko::remote::{Activity, Feed, Question, Reading, Update, WorkerSummary};
use async_trait::async_trait;
use gpui_kit::*;
use tokio::sync::{Notify, mpsc, oneshot};

use crate::backend::{Launch, Mode, NewSession, SessionEntry, SessionId, SharedBackend, Start};

/// How often at most what a session's engine says reaches the interface:
/// once a frame.
pub const FRAME: Duration = Duration::from_millis(16);

/// What a session's indicators in the sidebar show: only what is unusual.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Indicator {
    /// A turn runs.
    pub working: bool,
    /// The agent asked a question that waits for an answer.
    pub question: bool,
    /// A turn ended while no window showed the session.
    pub unseen: bool,
}

/// The sessions open in this app, shared by its windows.
#[derive(Default)]
pub struct LiveSessions {
    sessions: Vec<Entity<LiveSession>>,
    indicators: HashMap<SessionId, Indicator>,
}

impl Global for LiveSessions {}

impl LiveSessions {
    /// The session `id`, if it is open in this app.
    pub fn find(id: &SessionId, cx: &App) -> Option<Entity<LiveSession>> {
        cx.global::<Self>()
            .sessions
            .iter()
            .find(|session| session.read(cx).id.as_ref() == Some(id))
            .cloned()
    }

    /// Every session open in this app.
    pub fn all(cx: &App) -> Vec<Entity<LiveSession>> {
        cx.global::<Self>().sessions.clone()
    }

    /// What the sidebar shows of the session `id`.
    pub fn indicator(id: &SessionId, cx: &App) -> Indicator {
        cx.global::<Self>()
            .indicators
            .get(id)
            .copied()
            .unwrap_or_default()
    }

    /// The ids of the sessions open in this app.
    pub fn ids(cx: &App) -> Vec<SessionId> {
        cx.global::<Self>()
            .sessions
            .iter()
            .filter_map(|session| session.read(cx).id.clone())
            .collect()
    }

    fn add(session: Entity<LiveSession>, cx: &mut App) {
        cx.update_global::<Self, _>(|this, _| this.sessions.push(session));
    }

    /// Forgets the session `entity` (with `id`), called while it updates.
    fn remove(entity: EntityId, id: Option<&SessionId>, cx: &mut App) {
        cx.update_global::<Self, _>(|this, _| {
            this.sessions
                .retain(|session| session.entity_id() != entity);
            if let Some(id) = id {
                this.indicators.remove(id);
            }
        });
    }

    fn set_indicator(id: &SessionId, indicator: Indicator, cx: &mut App) {
        if cx.global::<Self>().indicators.get(id) != Some(&indicator) {
            cx.update_global::<Self, _>(|this, _| {
                this.indicators.insert(id.clone(), indicator);
            });
        }
    }
}

/// Where a session is.
#[derive(Debug, Clone, PartialEq)]
pub enum Phase {
    /// The engine opens it: credentials, MCP servers, the workflow.
    Opening,
    Open,
    /// A sample session that shows but doesn't run (the fake backend).
    Sample,
    /// Asked to quit: the engine stops what it started.
    Closing,
    /// Its engine shut down.
    Ended,
    /// It didn't open: why, for the user.
    Failed(String),
}

/// What the app keeps of an open session's seam.
#[derive(Clone)]
pub struct Link {
    pub info: arko::SessionInfo,
    pub feed: Feed,
    input: mpsc::UnboundedSender<arko::Input>,
    requests: mpsc::UnboundedSender<arko::Request>,
    pub details: Arc<arko::details::Details>,
}

/// What a session asks of the window that shows it.
#[derive(Debug, Clone, PartialEq)]
pub enum LiveEvent {
    Opened,
    /// It didn't open (`None` when the user cancelled).
    Failed(Option<String>),
    /// It asks for credentials or MCP approval: [`LiveSession::prompt`].
    Prompt,
    /// Arko said this as the session opened.
    Note(arko::Level, String),
    /// It asks whether to stop its jobs and workers to quit:
    /// [`LiveSession::quit_ask`].
    QuitAsked,
    Ended,
    TurnEnded,
    QuestionAsked,
    /// The question closed: answered in this app, or elsewhere (the web
    /// page), or with its turn stopped.
    QuestionClosed {
        answered_here: bool,
    },
    /// A request to the engine failed: the sentence to show.
    Refused(String),
}

/// What a session asks as it opens.
pub enum PromptKind {
    /// The credentials it can't open without.
    Credentials(arko::Need),
    /// Whether to start the project's MCP servers.
    Mcp(McpRequest),
}

/// A project's MCP servers waiting for the user's approval.
#[derive(Debug, Clone, PartialEq)]
pub struct McpRequest {
    /// The project's `.mcp.json`.
    pub file: PathBuf,
    /// Each server's name and command.
    pub servers: Vec<(String, String)>,
    /// Servers left out (approved or refused before, or invalid).
    pub skipped: Vec<String>,
}

/// The user's answer to a [`Prompt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptAnswer {
    /// Check the credentials again.
    Again,
    /// Don't open the session.
    Cancel,
    /// Start the MCP servers (`Some(true)`), not (`Some(false)`), both
    /// remembered by arko. `None`: not this time, asked again next time.
    Mcp(Option<bool>),
}

/// A question the engine waits on.
pub struct Prompt {
    pub kind: PromptKind,
    reply: oneshot::Sender<PromptAnswer>,
}

/// What the prompter sends the interface.
enum Asked {
    Prompt(Prompt),
    Note(arko::Level, String),
}

/// What an open session asks for and says, into the app: the engine's
/// side of the prompts, on its threads.
struct AppPrompter {
    asked: mpsc::UnboundedSender<Asked>,
}

/// What a cancelled prompt makes the open fail with, so the window says
/// nothing more.
pub const CANCELLED: &str = "You cancelled.";

#[async_trait]
impl arko::Prompter for AppPrompter {
    async fn need_credentials(&self, need: &arko::Need) -> Result<(), String> {
        let (reply, answer) = oneshot::channel();
        let prompt = Prompt {
            kind: PromptKind::Credentials(need.clone()),
            reply,
        };
        if self.asked.send(Asked::Prompt(prompt)).is_err() {
            return Err(need.message());
        }
        match answer.await {
            Ok(PromptAnswer::Again) => Ok(()),
            _ => Err(CANCELLED.to_string()),
        }
    }

    async fn approve_project_mcp(
        &self,
        approval: &arko::McpApproval,
    ) -> Result<Option<bool>, String> {
        let (reply, answer) = oneshot::channel();
        let prompt = Prompt {
            kind: PromptKind::Mcp(McpRequest {
                file: approval.file.clone(),
                servers: approval.servers.clone(),
                skipped: approval.skipped.clone(),
            }),
            reply,
        };
        if self.asked.send(Asked::Prompt(prompt)).is_err() {
            return Ok(None);
        }
        match answer.await {
            Ok(PromptAnswer::Mcp(answer)) => Ok(answer),
            Ok(PromptAnswer::Cancel) => Err(CANCELLED.to_string()),
            _ => Ok(None),
        }
    }

    fn note(&self, level: arko::Level, text: &str) {
        let _ = self.asked.send(Asked::Note(level, text.to_string()));
    }
}

/// What the background tasks drained since the interface last looked.
#[derive(Default)]
struct Drained {
    reading: Option<Arc<Reading>>,
    activity: Option<Activity>,
    workers: Vec<WorkerSummary>,
    quit_asks: Vec<arko::QuitAsk>,
}

impl Drained {
    fn is_empty(&self) -> bool {
        self.reading.is_none()
            && self.activity.is_none()
            && self.workers.is_empty()
            && self.quit_asks.is_empty()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A session open in this app.
pub struct LiveSession {
    /// Tells this one from the others, before it has an id.
    key: u64,
    /// `None` for a new session until the engine created it.
    id: Option<SessionId>,
    title: SharedString,
    mode: Mode,
    /// The folder an agent session works in.
    workspace: Option<PathBuf>,
    /// The main agent's model, as people say it.
    model: SharedString,
    phase: Phase,
    link: Option<Link>,
    reading: Arc<Reading>,
    activity: Activity,
    workers: Vec<WorkerSummary>,
    unseen: bool,
    /// How many windows show it.
    shown: usize,
    /// Input sent before it opened, sent once it does.
    queued: Vec<arko::Input>,
    prompt: Option<Prompt>,
    quit_ask: Option<arko::QuitAsk>,
    /// What the user agreed to stop to close it.
    quit_agreed: Option<arko::QuitAsk>,
    /// The question this app answered last.
    answered: Option<u64>,
    /// How many times what was drained was applied (tests).
    #[cfg(test)]
    applied: usize,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<LiveEvent> for LiveSession {}

impl LiveSession {
    fn blank(cx: &mut App) -> Self {
        static KEYS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let _ = cx;
        Self {
            key: KEYS.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            id: None,
            title: "New session".into(),
            mode: Mode::Agent,
            workspace: None,
            model: SharedString::default(),
            phase: Phase::Opening,
            link: None,
            reading: Arc::default(),
            activity: Activity::default(),
            workers: Vec::new(),
            unseen: false,
            shown: 0,
            queued: Vec::new(),
            prompt: None,
            quit_ask: None,
            quit_agreed: None,
            answered: None,
            #[cfg(test)]
            applied: 0,
            _tasks: Vec::new(),
        }
    }

    /// Opens the saved session `entry` in this app.
    pub fn resume(entry: &SessionEntry, cx: &mut App) -> Entity<Self> {
        let mut this = Self::blank(cx);
        this.set_entry(entry);
        Self::start(this, Start::Resume(entry.id.clone()), cx)
    }

    /// Creates a new session, which sends `first` as soon as it opens.
    pub fn create(new: NewSession, first: arko::Input, cx: &mut App) -> Entity<Self> {
        let mut this = Self::blank(cx);
        this.mode = new.mode;
        this.workspace = new.folder.clone().filter(|_| new.mode == Mode::Agent);
        this.model = new
            .model
            .as_deref()
            .map(arko::models::display_name)
            .unwrap_or_default()
            .into();
        this.title = match &first {
            arko::Input::Message(message) => first_line(&message.text),
            _ => "New session".into(),
        };
        this.queued.push(first);
        Self::start(this, Start::New(new), cx)
    }

    fn start(this: Self, start: Start, cx: &mut App) -> Entity<Self> {
        let session = cx.new(|cx| {
            let mut this = this;
            this.launch(start, cx);
            this
        });
        LiveSessions::add(session.clone(), cx);
        session.update(cx, |session, cx| session.update_indicator(cx));
        session
    }

    fn set_entry(&mut self, entry: &SessionEntry) {
        self.id = Some(entry.id.clone());
        self.title = entry.title.clone().into();
        self.mode = entry.mode;
        self.workspace = entry.workspace.clone();
        self.model = arko::models::display_name(&entry.model).into();
    }

    fn launch(&mut self, start: Start, cx: &mut Context<Self>) {
        let (asked, mut asks) = mpsc::unbounded_channel();
        let prompter = Arc::new(AppPrompter { asked });
        let backend = cx.global::<SharedBackend>().clone();
        match backend.start(start, prompter) {
            Launch::Engine(running) => {
                self.phase = Phase::Opening;
                let arko::Running { opened, ended } = running;
                self._tasks.push(cx.spawn(async move |this, cx| {
                    let opened = opened.await;
                    this.update(cx, |this, cx| match opened {
                        Ok(Ok(seam)) => this.opened(seam, cx),
                        Ok(Err(error)) => this.failed(open_error(&error), cx),
                        Err(_) => this.failed("The session stopped as it opened.".into(), cx),
                    })
                    .ok();
                }));
                self._tasks.push(cx.spawn(async move |this, cx| {
                    // It fails when the session didn't open: then `failed` said why.
                    if let Ok(end) = ended.await {
                        this.update(cx, |this, cx| match end {
                            // It switched between Chat and Agent: it opens again.
                            arko::RunEnd::Reopen => this.reopen(cx),
                            _ => this.ended(cx),
                        })
                        .ok();
                    }
                }));
                self._tasks.push(cx.spawn(async move |this, cx| {
                    while let Some(asked) = asks.recv().await {
                        let kept = this.update(cx, |this, cx| match asked {
                            Asked::Prompt(prompt) => {
                                this.prompt = Some(prompt);
                                cx.emit(LiveEvent::Prompt);
                                cx.notify();
                            }
                            Asked::Note(level, text) => cx.emit(LiveEvent::Note(level, text)),
                        });
                        if kept.is_err() {
                            break;
                        }
                    }
                }));
            }
            Launch::Sample(entry) => {
                self.set_entry(&entry);
                self.phase = Phase::Sample;
                self.queued.clear();
            }
            Launch::Refused(error) => self.phase = Phase::Failed(error.message()),
        }
    }

    fn failed(&mut self, message: String, cx: &mut Context<Self>) {
        self.prompt = None;
        let cancelled = message == CANCELLED;
        self.phase = Phase::Failed(message.clone());
        cx.emit(LiveEvent::Failed((!cancelled).then_some(message)));
        LiveSessions::remove(cx.entity_id(), self.id.as_ref(), cx);
        cx.notify();
    }

    fn opened(&mut self, seam: arko::Seam, cx: &mut Context<Self>) {
        let arko::Seam {
            session,
            feed,
            status,
            details,
            input,
            requests,
            quit_asks,
            ..
        } = seam;
        self.prompt = None;
        self.id = Some(SessionId(session.id.clone()));
        if self.mode == Mode::Agent {
            self.workspace = Some(session.workspace.clone());
        }
        let (snapshot, batches) = feed.subscribe_batched();
        self.activity = snapshot.activity;
        self.workers = snapshot.workers;
        self.reading = status.borrow().clone();
        self.apply_reading();
        for queued in self.queued.drain(..) {
            let _ = input.send(queued);
        }
        self.link = Some(Link {
            info: session,
            feed: feed.clone(),
            input,
            requests,
            details,
        });
        self.phase = Phase::Open;
        self.drain(feed, batches, status, quit_asks, cx);
        self.update_indicator(cx);
        cx.emit(LiveEvent::Opened);
        cx.notify();
    }

    /// Drains what the engine says on background tasks, and applies it on
    /// the UI thread at most once a frame.
    fn drain(
        &mut self,
        feed: Feed,
        mut batches: arko::remote::Batches,
        mut status: tokio::sync::watch::Receiver<Arc<Reading>>,
        mut quit_asks: mpsc::UnboundedReceiver<arko::QuitAsk>,
        cx: &mut Context<Self>,
    ) {
        let drained = Arc::new(Mutex::new(Drained::default()));
        let wake = Arc::new(Notify::new());
        self._tasks.push(cx.background_spawn({
            let (drained, wake) = (drained.clone(), wake.clone());
            async move {
                while let Some(batch) = batches.next().await {
                    let mut activity = None;
                    let mut workers = Vec::new();
                    for update in batch {
                        match update {
                            Update::Activity => activity = Some(feed.activity()),
                            Update::Worker(worker) => workers.push(worker),
                            _ => {}
                        }
                    }
                    if activity.is_none() && workers.is_empty() {
                        continue;
                    }
                    {
                        let mut drained = lock(&drained);
                        if activity.is_some() {
                            drained.activity = activity;
                        }
                        drained.workers.extend(workers);
                    }
                    wake.notify_one();
                }
            }
        }));
        self._tasks.push(cx.background_spawn({
            let (drained, wake) = (drained.clone(), wake.clone());
            async move {
                while status.changed().await.is_ok() {
                    let reading = status.borrow_and_update().clone();
                    lock(&drained).reading = Some(reading);
                    wake.notify_one();
                }
            }
        }));
        self._tasks.push(cx.background_spawn({
            let (drained, wake) = (drained.clone(), wake.clone());
            async move {
                while let Some(ask) = quit_asks.recv().await {
                    lock(&drained).quit_asks.push(ask);
                    wake.notify_one();
                }
            }
        }));
        self._tasks.push(cx.spawn(async move |this, cx| {
            loop {
                wake.notified().await;
                let batch = std::mem::take(&mut *lock(&drained));
                // A wake-up whose news an earlier frame took already.
                if batch.is_empty() {
                    continue;
                }
                if this.update(cx, |this, cx| this.apply(batch, cx)).is_err() {
                    break;
                }
                cx.background_executor().timer(FRAME).await;
            }
        }));
    }

    fn apply(&mut self, drained: Drained, cx: &mut Context<Self>) {
        #[cfg(test)]
        {
            self.applied += 1;
        }
        if let Some(reading) = drained.reading {
            self.reading = reading;
            self.apply_reading();
        }
        if let Some(activity) = drained.activity {
            let was = std::mem::replace(&mut self.activity, activity);
            let now = &self.activity;
            let question = |activity: &Activity| activity.question.as_ref().map(|q| q.id);
            if was.working && !now.working {
                if self.shown == 0 {
                    self.unseen = true;
                }
                cx.emit(LiveEvent::TurnEnded);
            }
            if question(&was) != question(now) {
                if let Some(closed) = question(&was) {
                    cx.emit(LiveEvent::QuestionClosed {
                        answered_here: self.answered == Some(closed),
                    });
                }
                if now.question.is_some() {
                    cx.emit(LiveEvent::QuestionAsked);
                }
            }
        }
        for worker in drained.workers {
            match self.workers.iter_mut().find(|known| known.id == worker.id) {
                Some(known) => *known = worker,
                None => self.workers.push(worker),
            }
        }
        for ask in drained.quit_asks {
            self.quit_asked(ask, cx);
        }
        self.update_indicator(cx);
        cx.notify();
    }

    fn apply_reading(&mut self) {
        if !self.reading.title.is_empty() {
            self.title = self.reading.title.clone().into();
        }
        if !self.reading.models.main.is_empty() {
            self.model = arko::models::display_name(&self.reading.models.main).into();
        }
    }

    fn update_indicator(&self, cx: &mut App) {
        if let Some(id) = &self.id {
            let indicator = Indicator {
                working: self.activity.working,
                question: self.activity.question.is_some(),
                unseen: self.unseen,
            };
            LiveSessions::set_indicator(id, indicator, cx);
        }
    }

    /// Opens the session again after its engine closed it to switch modes:
    /// a new seam, the same entity, so every window keeps showing it.
    fn reopen(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.id.clone() else {
            return self.ended(cx);
        };
        self.link = None;
        self.activity = Activity::default();
        self.launch(Start::Resume(id), cx);
        cx.notify();
    }

    /// Switches the session between Chat and Agent (between turns): the
    /// engine closes it and it opens again in `to`, the conversation going
    /// on in place. An agent works in `folder` (none: where it worked
    /// before). The task says why not, if not.
    pub fn switch_mode(
        &mut self,
        to: Mode,
        folder: Option<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), String>> {
        let Some(link) = &self.link else {
            return Task::ready(Err("The session isn’t open.".to_string()));
        };
        let (reply, answer) = oneshot::channel();
        let request = arko::Request::SwitchMode {
            to: to.into(),
            workspace: folder.clone(),
            reply,
        };
        if link.requests.send(request).is_err() {
            return Task::ready(Err("The session has closed.".to_string()));
        }
        cx.spawn(async move |this, cx| {
            let answer = answer
                .await
                .unwrap_or_else(|_| Err("The session has closed.".to_string()));
            if answer.is_ok() {
                this.update(cx, |this, cx| {
                    this.mode = to;
                    if to == Mode::Agent && folder.is_some() {
                        this.workspace = folder;
                    }
                    cx.notify();
                })
                .ok();
            }
            answer
        })
    }

    /// What switching to `to` costs (the conversation re-read without its
    /// cache), worked out off the UI thread, `None` when not known.
    pub fn switch_cost(&self, to: Mode, cx: &App) -> Task<Option<arko::details::SwitchCost>> {
        let Some(details) = self.link.as_ref().map(|link| link.details.clone()) else {
            return Task::ready(None);
        };
        cx.background_spawn(async move { details.switch_cost(to.into()) })
    }

    fn ended(&mut self, cx: &mut Context<Self>) {
        self.phase = Phase::Ended;
        self.activity = Activity::default();
        self.quit_ask = None;
        LiveSessions::remove(cx.entity_id(), self.id.as_ref(), cx);
        cx.emit(LiveEvent::Ended);
        cx.notify();
    }

    pub fn key(&self) -> u64 {
        self.key
    }

    pub fn id(&self) -> Option<&SessionId> {
        self.id.as_ref()
    }

    pub fn title(&self) -> &SharedString {
        &self.title
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    pub fn workspace(&self) -> Option<&PathBuf> {
        self.workspace.as_ref()
    }

    /// The main agent's model, as people say it.
    pub fn model(&self) -> &SharedString {
        &self.model
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    /// Whether it runs, or is about to: input reaches it.
    pub fn is_running(&self) -> bool {
        matches!(self.phase, Phase::Opening | Phase::Open | Phase::Closing)
    }

    pub fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    pub fn feed(&self) -> Option<&Feed> {
        self.link.as_ref().map(|link| &link.feed)
    }

    /// Its latest status.
    pub fn reading(&self) -> &Arc<Reading> {
        &self.reading
    }

    pub fn activity(&self) -> &Activity {
        &self.activity
    }

    pub fn is_working(&self) -> bool {
        self.activity.working
    }

    /// The question the agent waits on.
    pub fn question(&self) -> Option<&Question> {
        self.activity.question.as_ref()
    }

    pub fn workers(&self) -> &[WorkerSummary] {
        &self.workers
    }

    /// Whether the conversation has started: the main agent keeps its
    /// model from then on.
    pub fn has_started(&self) -> bool {
        // A new session is created by its first message, and a sample
        // session is an old one.
        self.reading.models.started || self.id.is_none() || self.phase == Phase::Sample
    }

    /// A window shows it (`true`) or stopped showing it.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if shown {
            self.shown += 1;
            self.unseen = false;
        } else {
            self.shown = self.shown.saturating_sub(1);
        }
        self.update_indicator(cx);
        cx.notify();
    }

    /// Sends `input` to the engine, or keeps it until the session opens.
    pub fn send(&mut self, input: arko::Input, cx: &mut Context<Self>) -> Result<(), String> {
        if let arko::Input::Answer { question, .. } = &input {
            self.answered = Some(*question);
        }
        match (&self.phase, &self.link) {
            (Phase::Open | Phase::Closing, Some(link)) => link
                .input
                .send(input)
                .map_err(|_| "The session has closed.".to_string()),
            (Phase::Opening, _) => {
                self.queued.push(input);
                cx.notify();
                Ok(())
            }
            (Phase::Sample, _) => Err("This sample session doesn’t run.".to_string()),
            (Phase::Failed(why), _) => Err(why.clone()),
            _ => Err("The session has closed.".to_string()),
        }
    }

    /// Asks the engine to rename the session (empty: back to its
    /// conversation's name).
    pub fn rename(&mut self, title: String, cx: &mut Context<Self>) {
        let Some(link) = &self.link else {
            return;
        };
        let (reply, answer) = oneshot::channel();
        if link
            .requests
            .send(arko::Request::Rename {
                title: title.clone(),
                reply,
            })
            .is_err()
        {
            return;
        }
        if !title.trim().is_empty() {
            self.title = title.trim().to_string().into();
            cx.notify();
        }
        self._tasks.push(cx.spawn(async move |this, cx| {
            if let Ok(Err(why)) = answer.await {
                this.update(cx, |_, cx| cx.emit(LiveEvent::Refused(why)))
                    .ok();
            }
        }));
    }

    /// Asks the user as the engine's prompter does (tests).
    #[cfg(test)]
    pub fn ask(
        &mut self,
        kind: PromptKind,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<PromptAnswer> {
        let (reply, answer) = oneshot::channel();
        self.prompt = Some(Prompt { kind, reply });
        cx.emit(LiveEvent::Prompt);
        cx.notify();
        answer
    }

    /// The prompt waiting for the user, if any.
    pub fn prompt(&self) -> Option<&PromptKind> {
        self.prompt.as_ref().map(|prompt| &prompt.kind)
    }

    /// Answers the waiting prompt.
    pub fn answer_prompt(&mut self, answer: PromptAnswer, cx: &mut Context<Self>) {
        if let Some(prompt) = self.prompt.take() {
            let _ = prompt.reply.send(answer);
            cx.notify();
        }
    }

    /// What closing it would stop, as its status says now.
    pub fn running_work(&self) -> arko::QuitAsk {
        arko::QuitAsk {
            jobs: self.reading.jobs.clone(),
            workers: self.reading.workers_running.clone(),
        }
    }

    /// The engine asks before quitting: the jobs and workers it would stop.
    pub fn quit_ask(&self) -> Option<&arko::QuitAsk> {
        self.quit_ask.as_ref()
    }

    /// Closes it, stopping the jobs and workers in `agreed` (what the user
    /// agreed to stop). Should the engine find more running, it asks
    /// ([`LiveEvent::QuitAsked`]).
    pub fn close(&mut self, agreed: arko::QuitAsk, cx: &mut Context<Self>) {
        match &self.phase {
            Phase::Opening => {
                // It hasn't opened: it closes as soon as it does.
                self.queued.push(arko::Input::Quit(Some(Vec::new())));
                self.quit_agreed = Some(agreed);
                self.phase = Phase::Closing;
            }
            Phase::Open | Phase::Closing => {
                let jobs = agreed.jobs.iter().map(|job| job.job_id).collect();
                self.quit_agreed = Some(agreed);
                self.phase = Phase::Closing;
                if let Some(link) = &self.link {
                    let _ = link.input.send(arko::Input::Quit(Some(jobs)));
                }
            }
            Phase::Sample | Phase::Failed(_) => {
                if let Some(id) = &self.id {
                    cx.global::<SharedBackend>().closed(id);
                }
                self.ended(cx);
            }
            Phase::Ended => {}
        }
        cx.notify();
    }

    /// The user doesn't want to close it after all.
    pub fn cancel_close(&mut self, cx: &mut Context<Self>) {
        if self.phase == Phase::Closing {
            self.phase = Phase::Open;
        }
        self.quit_ask = None;
        self.quit_agreed = None;
        cx.notify();
    }

    /// The user agrees to stop what the engine asked about.
    pub fn agree_to_quit(&mut self, cx: &mut Context<Self>) {
        if let Some(ask) = self.quit_ask.take() {
            self.close(ask, cx);
        }
    }

    fn quit_asked(&mut self, ask: arko::QuitAsk, cx: &mut Context<Self>) {
        let covered = self.quit_agreed.as_ref().is_some_and(|agreed| {
            ask.jobs
                .iter()
                .all(|job| agreed.jobs.iter().any(|seen| seen.job_id == job.job_id))
                && ask
                    .workers
                    .iter()
                    .all(|worker| agreed.workers.contains(worker))
        });
        if covered {
            // What it found running is what the user agreed to stop.
            self.close(ask, cx);
        } else {
            self.quit_ask = Some(ask);
            cx.emit(LiveEvent::QuitAsked);
        }
    }
}

/// The first line of `text`, shortened, as a new session's title until the
/// engine names it.
fn first_line(text: &str) -> SharedString {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let line = line.trim();
    let short: String = line.chars().take(80).collect();
    if short.is_empty() {
        "New session".into()
    } else {
        short.into()
    }
}

/// Why a session didn't open, for the user.
fn open_error(error: &arko::OpenError) -> String {
    match error {
        arko::OpenError::Locked {
            holder: Some(holder),
        } => format!("It’s open in {}.", holder.describe()),
        arko::OpenError::Locked { holder: None } => {
            "It’s open in another copy of Arko.".to_string()
        }
        arko::OpenError::Failed(message) if message == CANCELLED => CANCELLED.to_string(),
        other => crate::backend::sentence(&other.to_string()),
    }
}

/// Starts the app's list of open sessions.
pub fn init(cx: &mut App) {
    cx.set_global(LiveSessions::default());
}

#[cfg(test)]
mod tests {
    use super::{Drained, LiveEvent, LiveSession, first_line};
    use arko::remote::{Activity, Question};
    use gpui_kit::{AppContext as _, TestAppContext};

    /// A turn that ends while no window shows the session leaves it
    /// unseen. A question that closes without this app's answer was
    /// answered elsewhere.
    #[gpui_kit::test]
    fn turns_and_questions_are_told_as_they_change(cx: &mut TestAppContext) {
        cx.update(super::init);
        let session = cx.new(|cx| {
            let mut session = LiveSession::blank(cx);
            session.id = Some(crate::backend::SessionId("s1".into()));
            session
        });
        let heard = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        cx.update(|cx| {
            let heard = heard.clone();
            cx.subscribe(&session, move |_, event: &LiveEvent, _| {
                heard.borrow_mut().push(event.clone())
            })
            .detach();
        });
        let apply = |cx: &mut TestAppContext, working: bool, question: Option<u64>| {
            session.update(cx, |session, cx| {
                session.apply(
                    Drained {
                        activity: Some(Activity {
                            working,
                            question: question.map(|id| Question {
                                id,
                                text: "Tabs or spaces?".into(),
                            }),
                            since: None,
                        }),
                        ..Drained::default()
                    },
                    cx,
                )
            })
        };
        apply(cx, true, Some(1));
        let indicator = |cx: &mut TestAppContext| {
            cx.read(|cx| {
                super::LiveSessions::indicator(&crate::backend::SessionId("s1".into()), cx)
            })
        };
        assert!(indicator(cx).question && indicator(cx).working);
        apply(cx, true, None);
        apply(cx, true, Some(2));
        session.update(cx, |session, _| session.answered = Some(2));
        apply(cx, false, None);
        assert_eq!(
            *heard.borrow(),
            [
                LiveEvent::QuestionAsked,
                LiveEvent::QuestionClosed {
                    answered_here: false
                },
                LiveEvent::QuestionAsked,
                LiveEvent::TurnEnded,
                LiveEvent::QuestionClosed {
                    answered_here: true
                },
            ]
        );
        assert!(indicator(cx).unseen, "no window showed it");
        session.update(cx, |session, cx| session.set_shown(true, cx));
        assert!(!indicator(cx).unseen);
    }

    /// What the engine says in a burst (here 4,000 changes of the turn's
    /// state and 50 statuses, sent from another thread) reaches the
    /// interface as one change a frame, whatever its size.
    #[gpui_kit::test]
    fn a_burst_from_the_engine_is_applied_once_a_frame(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(super::init);
        let feed = arko::remote::Feed::new("main");
        let (status, statuses) =
            tokio::sync::watch::channel(std::sync::Arc::new(arko::remote::Reading::default()));
        let (_quit, quit_asks) = tokio::sync::mpsc::unbounded_channel();
        let session = cx.new(|cx| {
            let mut session = LiveSession::blank(cx);
            let (_, batches) = feed.subscribe_batched();
            session.drain(feed.clone(), batches, statuses, quit_asks, cx);
            session
        });
        std::thread::spawn({
            let feed = feed.clone();
            move || {
                for turn in 0..2000 {
                    feed.turn_started();
                    feed.turn_ended("", None);
                    if turn % 40 == 0 {
                        status.send_modify(|reading| {
                            std::sync::Arc::make_mut(reading).title = format!("Turn {turn}")
                        });
                    }
                }
            }
        })
        .join()
        .unwrap();
        cx.run_until_parked();
        let applied = cx.read(|cx| session.read(cx).applied);
        assert_eq!(applied, 1, "one change for the whole burst");
        assert_eq!(
            cx.read(|cx| session.read(cx).title.to_string()),
            "Turn 1960"
        );
        assert!(!cx.read(|cx| session.read(cx).is_working()));
        // Nothing more comes: the next frame applies nothing.
        cx.executor().advance_clock(super::FRAME);
        cx.run_until_parked();
        assert_eq!(cx.read(|cx| session.read(cx).applied), 1);
    }

    #[test]
    fn a_new_sessions_title_is_its_first_line() {
        assert_eq!(
            first_line("\n  Count the pears.\nThen the apples."),
            "Count the pears."
        );
        assert_eq!(first_line("   "), "New session");
        assert_eq!(first_line(&"a".repeat(200)).len(), 80);
    }
}
