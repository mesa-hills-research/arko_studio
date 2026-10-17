//! The boundary between Arko Studio's interface and arko's engine.
//!
//! The interface asks a [`Backend`] for the sessions in Arko's folder, acts
//! on them, and starts them. Its calls block (they read and write files), so
//! the interface makes them on a background thread and applies the answers
//! on the UI thread. [`Backend::start`] returns at once, the session opening
//! on threads of its own. [`ArkoBackend`] wraps arko's library
//! (`arko::sessions`, `arko::spawn`). [`FakeBackend`] answers from memory,
//! for tests whose sessions don't need to run.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

pub use arko::Holder;

mod accounts;
mod defaults;
pub mod extensions;

pub use accounts::{Accounts, key_named};
pub use defaults::{SessionDefaults, takes_cache_1h, web_page_bind};

/// Reading accounts in words, for Settings.
pub mod account_words {
    pub use super::accounts::{
        CACHE_1H_VARIABLE, ChatGptLogin, ClaudeCodeStatus, KeySource, KeyState, LoginSource,
        LoginState, Problem, Ready, source_words, switch_value, tilde,
    };
}

/// A session's id: the name of its folder in `<arko>/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a session works: a chat (no tools, no folder) or an agent in a folder.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    Chat,
    #[default]
    Agent,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::Chat => "Chat",
            Mode::Agent => "Agent",
        }
    }
}

impl From<arko::sessions::Mode> for Mode {
    fn from(mode: arko::sessions::Mode) -> Self {
        match mode {
            arko::sessions::Mode::Chat => Mode::Chat,
            _ => Mode::Agent,
        }
    }
}

impl From<Mode> for arko::sessions::Mode {
    fn from(mode: Mode) -> Self {
        match mode {
            Mode::Chat => arko::sessions::Mode::Chat,
            Mode::Agent => arko::sessions::Mode::Agent,
        }
    }
}

/// A conversation inside a session (the terminal's `/new` starts another,
/// `/resume` goes back to one).
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationEntry {
    /// 1-based, as `/resume N` takes it.
    pub number: u32,
    pub title: String,
    /// The one the session continues.
    pub current: bool,
}

/// A session as the list shows it, read without loading its conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub id: SessionId,
    /// The name the user gave it, else the start of its current
    /// conversation's first message, else "New session".
    pub title: String,
    pub mode: Mode,
    /// The folder an agent works in. Chats have none.
    pub workspace: Option<PathBuf>,
    /// The main agent's model, as arko describes it.
    pub model: String,
    pub created: SystemTime,
    /// When it was last worked in, `None` before its first message.
    pub last_active: Option<SystemTime>,
    pub conversations: Vec<ConversationEntry>,
    /// Who has it open, when someone does.
    pub holder: Option<Holder>,
    /// Whether it has started workers, running now or finished: its
    /// window then offers the workers panel. (The listing doesn't know it,
    /// an open session's status says.)
    pub had_workers: bool,
}

impl SessionEntry {
    /// When it was last worked in, or else made: what the list sorts by.
    pub fn activity(&self) -> SystemTime {
        self.last_active.unwrap_or(self.created)
    }

    fn from_arko(entry: arko::sessions::SessionEntry) -> Self {
        Self {
            id: SessionId(entry.id),
            title: entry.title,
            mode: entry.mode.into(),
            workspace: entry.workspace,
            model: entry.model,
            created: entry.created.into(),
            last_active: entry.last_active,
            conversations: entry
                .conversations
                .into_iter()
                .map(|conversation| ConversationEntry {
                    number: u32::try_from(conversation.number).unwrap_or(u32::MAX),
                    title: conversation.title,
                    current: conversation.current,
                })
                .collect(),
            holder: entry.holder,
            had_workers: false,
        }
    }
}

/// Why a backend call failed, in words for the user.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendError {
    /// The session is open elsewhere, `None` when an older arko holds it
    /// without saying who it is.
    Locked {
        holder: Option<Holder>,
    },
    NotFound(SessionId),
    /// Anything else, already worded for the user.
    Other(String),
}

impl BackendError {
    /// What went wrong, as a sentence or two for a notification.
    pub fn message(&self) -> String {
        match self {
            BackendError::Locked {
                holder: Some(holder),
            } => format!("It’s open in {}.", holder.describe()),
            BackendError::Locked { holder: None } => {
                "It’s open in another copy of Arko.".to_string()
            }
            BackendError::NotFound(id) => format!("There’s no session {id} any more."),
            BackendError::Other(message) => sentence(message),
        }
    }
}

/// `text` as a sentence: a capital first letter and a full stop.
pub fn sentence(text: &str) -> String {
    let text = text.trim();
    let mut chars = text.chars();
    let mut out: String = match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => return String::new(),
    };
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// What deleting a session leaves: its workers' worktrees with changes
/// that aren't committed stay, with their branches.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeletePlan {
    /// Each with its branch, when it has one.
    pub kept_worktrees: Vec<(PathBuf, Option<String>)>,
}

/// A new session, as the user set it up before its first message.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSession {
    pub mode: Mode,
    /// The folder an agent session works in. Chats have none.
    pub folder: Option<PathBuf>,
    /// The main agent's model as `/model` takes it. `None`: arko's default
    /// (the last session's, else one the credentials allow).
    pub model: Option<String>,
    /// `None`: the model's default.
    pub effort: Option<String>,
}

/// Which session to start.
#[derive(Debug, Clone, PartialEq)]
pub enum Start {
    Resume(SessionId),
    New(NewSession),
}

/// A session starting.
pub enum Launch {
    /// It opens on its own threads: arko's engine.
    Engine(arko::Running),
    /// A sample session that shows but doesn't run ([`FakeBackend`]).
    Sample(SessionEntry),
    /// It can't start.
    Refused(BackendError),
}

/// A way to reach some models: Claude Code, the Anthropic API, ...
#[derive(Debug, Clone, PartialEq)]
pub struct ModelGroup {
    /// As the picker heads it: "Claude Code, on your Claude plan".
    pub name: String,
    /// What it needs that isn't there. Its models can't be chosen.
    pub needs: Option<String>,
    pub models: Vec<ModelChoice>,
}

/// A model of a [`ModelGroup`].
#[derive(Debug, Clone, PartialEq)]
pub struct ModelChoice {
    /// As `/model` takes it: `claude-opus-5-5`, `cc:claude-opus-5-5`.
    pub name: String,
    /// Its short name: `opus`.
    pub alias: String,
}

/// The models a new session can start on.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelChoices {
    pub groups: Vec<ModelGroup>,
    /// What a new session starts on when none is chosen.
    pub default: Option<String>,
    /// Why the default isn't the last session's model (a credential it
    /// needs is gone).
    pub note: Option<String>,
}

impl ModelChoices {
    pub fn from_arko(groups: Vec<arko::models::ModelGroup>, default: Option<String>) -> Self {
        Self {
            groups: groups
                .into_iter()
                .map(|group| ModelGroup {
                    name: group.name,
                    needs: group.needs,
                    models: group
                        .models
                        .into_iter()
                        .map(|(name, alias)| ModelChoice {
                            name,
                            alias: alias.to_string(),
                        })
                        .collect(),
                })
                .collect(),
            default,
            note: None,
        }
    }

    /// Whether `name` can be chosen: its group needs nothing.
    pub fn usable(&self, name: &str) -> bool {
        self.groups.iter().any(|group| {
            group.needs.is_none() && group.models.iter().any(|model| model.name == name)
        })
    }
}

/// What Arko Studio needs from arko's engine to list, manage and start
/// sessions.
///
/// Every method but [`Backend::start`] blocks: call them from a background
/// task.
pub trait Backend: Send + Sync + 'static {
    /// Arko's folder, shared with the terminal app.
    fn data_root(&self) -> &Path;

    /// Every session in Arko's folder, in no particular order.
    fn sessions(&self) -> Result<Vec<SessionEntry>, BackendError>;

    /// Renames a session that isn't open in this app (an open one is renamed
    /// through its engine).
    fn rename(&self, id: &SessionId, title: &str) -> Result<(), BackendError>;

    /// What deleting a session would leave behind, for its confirmation.
    fn delete_plan(&self, id: &SessionId) -> Result<DeletePlan, BackendError>;

    /// Deletes a session's saved data (never the files in its folder).
    /// Refuses while the session is open anywhere, this app included.
    fn delete(&self, id: &SessionId) -> Result<(), BackendError>;

    /// The models a new session in `folder` (none: a chat) can start on,
    /// and the one arko chooses when neither the composer nor Settings
    /// chose one. May run `claude auth status`.
    fn models(&self, folder: Option<&Path>) -> ModelChoices;

    /// Starts a session, at once: it opens on threads of its own, asking
    /// `prompter` for what it lacks.
    fn start(&self, start: Start, prompter: Arc<dyn arko::Prompter>) -> Launch;

    /// A sample session ([`Launch::Sample`]) closed. Engine sessions close
    /// themselves.
    fn closed(&self, _id: &SessionId) {}

    /// Saves an API key (`provider` as `/keys add` names it: `anthropic`)
    /// in Arko's folder, where every open session finds it. `Ok` says what
    /// was done, `Err` why not.
    fn add_key(&self, provider: &str, value: &str) -> Result<String, String>;

    /// Uses the Codex auth file at `path` for the ChatGPT login. `Ok` says
    /// for how long, `Err` why not.
    fn set_chatgpt_login(&self, path: &Path) -> Result<String, String>;

    /// Starts the sessions started from now on with `defaults`, the
    /// preferences as they are now. It doesn't block.
    fn set_defaults(&self, defaults: SessionDefaults);

    /// The API keys, the ChatGPT login and Claude Code, as they are now.
    /// It runs `claude`.
    fn accounts(&self) -> Accounts;

    /// Removes the key of `provider` (as `/keys remove` names it) that
    /// Arko saved in its folder, from there and from every open session.
    /// `Ok` says what was done, `Err` why not.
    fn remove_key(&self, provider: &str) -> Result<String, String>;

    /// The MCP servers and skills sessions get, for the projects in
    /// `folders`, with the MCP config files `mcp_config` set in Settings.
    fn extensions(&self, folders: &[PathBuf], mcp_config: &[PathBuf]) -> extensions::Extensions;

    /// Forgets the answer about the MCP servers of the project in `folder`:
    /// its next session asks again.
    fn forget_mcp_approval(&self, folder: &Path) -> Result<(), String>;
}

/// The accounts as the app last read them. Observers hear when a reading
/// differs from the one before (a key added or removed, a login chosen,
/// Claude Code signed in): the models new sessions can use changed.
#[derive(Default)]
pub struct AccountsSeen {
    last: Option<Accounts>,
}

impl gpui_kit::Global for AccountsSeen {}

/// Records `accounts` as just read, telling the observers of
/// [`AccountsSeen`] when they changed.
pub fn accounts_read(accounts: &Accounts, cx: &mut gpui_kit::App) {
    use gpui_kit::BorrowAppContext as _;
    let seen = cx.default_global::<AccountsSeen>();
    if seen.last.as_ref() == Some(accounts) {
        return;
    }
    let accounts = accounts.clone();
    cx.update_global::<AccountsSeen, _>(|seen, _| seen.last = Some(accounts));
}

/// Gives the backend the preferences sessions start with, now and whenever
/// they change.
pub fn follow_prefs(cx: &mut gpui_kit::App) {
    fn give(cx: &mut gpui_kit::App) {
        let defaults = SessionDefaults::from_prefs(crate::prefs::get(cx));
        cx.global::<SharedBackend>().set_defaults(defaults);
    }
    give(cx);
    cx.observe_global::<crate::prefs::PrefsStore>(give).detach();
}

/// The main model in Arko's `last-model.json`, which every new session
/// writes: `{ "main": "claude-opus-5-5", ... }`.
pub fn last_model(arko_folder: &Path) -> Option<String> {
    #[derive(serde::Deserialize)]
    struct LastModels {
        main: String,
    }
    let text = std::fs::read_to_string(arko_folder.join("last-model.json")).ok()?;
    let last: LastModels = serde_json::from_str(&text).ok()?;
    Some(last.main).filter(|model| !model.is_empty())
}

/// The backend the app runs on, shared by its windows.
#[derive(Clone)]
pub struct SharedBackend(pub Arc<dyn Backend>);

impl gpui_kit::Global for SharedBackend {}

impl std::ops::Deref for SharedBackend {
    type Target = dyn Backend;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// Changes the options of every session a backend starts.
pub type Configure = Arc<dyn Fn(&mut arko::Options) + Send + Sync>;

/// Arko's sessions in Arko's folder, run by its engine.
pub struct ArkoBackend {
    root: PathBuf,
    configure: Option<Configure>,
    defaults: Mutex<SessionDefaults>,
}

impl ArkoBackend {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            configure: None,
            defaults: Mutex::new(SessionDefaults::default()),
        }
    }

    fn defaults(&self) -> SessionDefaults {
        lock(&self.defaults).clone()
    }

    /// Lets `configure` change the options of every session started (tests
    /// give the sessions the mock model server's client).
    pub fn with_configure(mut self, configure: Configure) -> Self {
        self.configure = Some(configure);
        self
    }

    /// The options a session starts with. It reads files: call it off the
    /// UI thread.
    pub fn options(&self, start: Start) -> arko::Options {
        prepare(&self.root, self.configure.as_ref(), &self.defaults(), start).0
    }
}

/// The options a session in Arko's folder `root` starts with, and notes
/// for the user about what was left out of them. It reads files (the last
/// session's model, the MCP config files): it runs on the thread that
/// starts the session.
fn prepare(
    root: &Path,
    configure: Option<&Configure>,
    defaults: &SessionDefaults,
    start: Start,
) -> (arko::Options, Vec<String>) {
    let mut options = arko::Options {
        root: Some(root.to_path_buf()),
        ..arko::Options::default()
    };
    match start {
        Start::Resume(id) => {
            options.session = arko::SessionChoice::Resume(id.0);
            defaults.apply(&mut options, None, || None);
        }
        Start::New(new) => {
            options.session = arko::SessionChoice::New;
            match new.mode {
                Mode::Agent => {
                    options.mode = arko::sessions::Mode::Agent;
                    options.workspace = new.folder;
                }
                // A chat works in no folder of the user's: Arko keeps it in
                // the session's own.
                Mode::Chat => options.mode = arko::sessions::Mode::Chat,
            }
            options.model = new.model;
            let effort = new.effort.and_then(|effort| effort.parse().ok());
            if let Some(effort) = effort.or(defaults.effort) {
                options.effort = effort;
            }
            defaults.apply(&mut options, Some(new.mode), || last_model(root));
        }
    }
    let notes = defaults::keep_readable_mcp_config(&mut options.mcp_config);
    if let Some(configure) = configure {
        configure(&mut options);
    }
    (options, notes)
}

impl Backend for ArkoBackend {
    fn data_root(&self) -> &Path {
        &self.root
    }

    fn sessions(&self) -> Result<Vec<SessionEntry>, BackendError> {
        Ok(arko::sessions::list(&self.root)
            .into_iter()
            .map(SessionEntry::from_arko)
            .collect())
    }

    fn rename(&self, id: &SessionId, title: &str) -> Result<(), BackendError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(BackendError::Other(
                "A session’s name can’t be empty.".into(),
            ));
        }
        arko::sessions::rename(&self.root, id.as_str(), title).map_err(BackendError::Other)
    }

    fn delete_plan(&self, id: &SessionId) -> Result<DeletePlan, BackendError> {
        arko::sessions::delete_plan(&self.root, id.as_str())
            .map(|plan| DeletePlan {
                kept_worktrees: plan.worktrees_dirty,
            })
            .map_err(delete_error)
    }

    fn delete(&self, id: &SessionId) -> Result<(), BackendError> {
        arko::sessions::delete(&self.root, id.as_str())
            .map(|_| ())
            .map_err(delete_error)
    }

    fn models(&self, folder: Option<&Path>) -> ModelChoices {
        let models = arko::models::for_new_session(&self.root, folder);
        let mut choices = ModelChoices::from_arko(models.groups, models.main);
        choices.note = models.note;
        choices
    }

    fn start(&self, start: Start, prompter: Arc<dyn arko::Prompter>) -> Launch {
        // The options are made on a thread of their own, since making them
        // reads files, and that thread starts the session and hands it
        // over as it opens, then as it ends.
        let (opened_sender, opened) = tokio::sync::oneshot::channel();
        let (ended_sender, ended) = tokio::sync::oneshot::channel();
        let root = self.root.clone();
        let configure = self.configure.clone();
        let defaults = self.defaults();
        let starting = std::thread::Builder::new()
            .name("arko-studio-start".into())
            .spawn(move || {
                let (options, notes) = prepare(&root, configure.as_ref(), &defaults, start);
                for note in &notes {
                    prompter.note(arko::Level::Warn, note);
                }
                let running = arko::spawn(options, prompter);
                // Without an answer, the session stopped as it opened, and
                // so does the app's.
                let Ok(open) = running.opened.blocking_recv() else {
                    return;
                };
                let opened = open.is_ok();
                if opened_sender.send(open).is_err() || !opened {
                    return;
                }
                if let Ok(end) = running.ended.blocking_recv() {
                    let _ = ended_sender.send(end);
                }
            });
        match starting {
            Ok(_) => Launch::Engine(arko::Running { opened, ended }),
            Err(error) => Launch::Refused(BackendError::Other(format!(
                "could not start the session: {error}"
            ))),
        }
    }

    fn add_key(&self, provider: &str, value: &str) -> Result<String, String> {
        let key = key_named(provider).ok_or_else(|| format!("Arko takes no {provider} key."))?;
        outcome(arko::accounts::add(
            &self.root,
            key,
            arko::accounts::Secret::new(value),
        ))
    }

    fn set_chatgpt_login(&self, path: &Path) -> Result<String, String> {
        let first = accounts::login_set_first();
        arko::accounts::set_chatgpt_login(&self.root, path)?;
        Ok(accounts::login_chosen_words(first.as_deref()))
    }

    fn set_defaults(&self, defaults: SessionDefaults) {
        *lock(&self.defaults) = defaults;
    }

    fn accounts(&self) -> Accounts {
        Accounts::read(&self.root)
    }

    fn remove_key(&self, provider: &str) -> Result<String, String> {
        let key = key_named(provider).ok_or_else(|| format!("Arko takes no {provider} key."))?;
        outcome(arko::accounts::remove(&self.root, key))
    }

    fn extensions(&self, folders: &[PathBuf], mcp_config: &[PathBuf]) -> extensions::Extensions {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
            .filter(|home| !home.is_empty())
            .map(PathBuf::from);
        extensions::read(&self.root, folders, mcp_config, home.as_deref())
    }

    fn forget_mcp_approval(&self, folder: &Path) -> Result<(), String> {
        extensions::forget_approval(&self.root, folder)
    }
}

/// What adding or removing a key did, in sentences: what was done and
/// what to look out for, or why nothing was.
fn outcome(outcome: arko::accounts::Outcome) -> Result<String, String> {
    match outcome {
        arko::accounts::Outcome::Done { said, warnings } => Ok(std::iter::once(said)
            .chain(warnings.iter().map(|warning| sentence(warning)))
            .collect::<Vec<_>>()
            .join(" ")),
        arko::accounts::Outcome::Failed(why) => Err(sentence(&why)),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

fn delete_error(error: arko::sessions::DeleteError) -> BackendError {
    match error {
        arko::sessions::DeleteError::Locked { holder } => BackendError::Locked { holder },
        arko::sessions::DeleteError::Io(error) => BackendError::Other(error),
    }
}

/// Sessions in memory, for tests whose sessions don't run.
pub struct FakeBackend {
    root: PathBuf,
    models: ModelChoices,
    /// A group of models needs nothing once the key of the provider it's
    /// named after is set, as with arko.
    groups_follow_keys: bool,
    state: Mutex<FakeState>,
    defaults: Mutex<SessionDefaults>,
}

struct FakeState {
    sessions: Vec<SessionEntry>,
    /// Sessions started, which the delete refuses.
    open: Vec<SessionId>,
    created: u32,
    /// The keys added, by provider.
    keys: Vec<(String, String)>,
    accounts: Accounts,
    /// How many times the accounts were read.
    accounts_read: usize,
    extensions: extensions::Extensions,
}

impl FakeBackend {
    pub fn new(root: impl Into<PathBuf>, sessions: Vec<SessionEntry>) -> Self {
        let root = root.into();
        Self {
            root: root.clone(),
            models: ModelChoices::default(),
            groups_follow_keys: false,
            state: Mutex::new(FakeState {
                sessions,
                open: Vec::new(),
                created: 0,
                keys: Vec::new(),
                accounts: Accounts::sample(Path::new("/home/ada"), &root),
                accounts_read: 0,
                extensions: extensions::Extensions::sample(Path::new("/home/ada"), &root),
            }),
            defaults: Mutex::new(SessionDefaults::default()),
        }
    }

    /// No sessions at all.
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self::new(root, Vec::new())
    }

    /// A handful of sessions in two folders and two chats, one of them open
    /// in the terminal, all active in the week before `now`, so the sidebar
    /// shows the same times on any day.
    pub fn sample(root: impl Into<PathBuf>, now: SystemTime) -> Self {
        let ago = |minutes: u64| now - Duration::from_secs(minutes * 60);
        let agent = |id: &str, title: &str, folder: &str, minutes: u64| SessionEntry {
            id: SessionId(id.into()),
            title: title.into(),
            mode: Mode::Agent,
            workspace: Some(PathBuf::from(folder)),
            model: "claude-opus-5-5".into(),
            created: ago(minutes + 30),
            last_active: Some(ago(minutes)),
            conversations: Vec::new(),
            holder: None,
            had_workers: false,
        };
        let chat = |id: &str, title: &str, minutes: u64| SessionEntry {
            mode: Mode::Chat,
            workspace: None,
            ..agent(id, title, "", minutes)
        };
        let mut sessions = vec![
            agent(
                "20261016-091200-a1b2",
                "Split the engine out of main.rs",
                "/home/ada/src/arko",
                4,
            ),
            agent(
                "20261015-171000-c3d4",
                "Fix the handover size for workers",
                "/home/ada/src/arko",
                60 * 20,
            ),
            agent(
                "20261014-110500-e5f6",
                "Golden screenshots on Linux",
                "/home/ada/src/gpui_kit",
                60 * 47,
            ),
            chat(
                "20261016-080000-a7b8",
                "Variable fonts and the wght axis",
                90,
            ),
            chat("20261011-120000-c9d0", "Naming a desktop app", 60 * 24 * 5),
        ];
        sessions[1].conversations = vec![
            ConversationEntry {
                number: 1,
                title: "Fix the handover size for workers".into(),
                current: false,
            },
            ConversationEntry {
                number: 2,
                title: "Check /handover-at 950k".into(),
                current: true,
            },
        ];
        sessions[1].had_workers = true;
        sessions[2].holder = Some(Holder {
            app: "arko".into(),
            pid: 4121,
            host: String::new(),
            since: chrono::DateTime::<chrono::Utc>::from(ago(60)),
        });
        Self::new(root, sessions).with_default_model(Some("claude-opus-5-5".into()))
    }

    /// Starts new sessions on `model`, from one group that needs nothing.
    pub fn with_default_model(mut self, model: Option<String>) -> Self {
        self.models = ModelChoices {
            groups: vec![ModelGroup {
                name: "Anthropic API, billed per token".into(),
                needs: None,
                models: [("claude-opus-5-5", "opus"), ("claude-sonnet-5-5", "sonnet")]
                    .into_iter()
                    .map(|(name, alias)| ModelChoice {
                        name: name.into(),
                        alias: alias.into(),
                    })
                    .collect(),
            }],
            default: model,
            note: None,
        };
        self
    }

    /// Offers `models` to new sessions.
    pub fn with_models(mut self, models: ModelChoices) -> Self {
        self.models = models;
        self
    }

    /// Lets a group of models need nothing once the key of the provider
    /// it's named after is set ("OpenRouter, billed per token" and the
    /// OpenRouter key), as arko's list does.
    pub fn with_groups_following_keys(mut self) -> Self {
        self.groups_follow_keys = true;
        self
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut lock(&self.state))
    }

    /// The preferences sessions start with, as the app gave them last.
    pub fn defaults(&self) -> SessionDefaults {
        lock(&self.defaults).clone()
    }
}

impl Backend for FakeBackend {
    fn data_root(&self) -> &Path {
        &self.root
    }

    fn sessions(&self) -> Result<Vec<SessionEntry>, BackendError> {
        Ok(self.with_state(|state| state.sessions.clone()))
    }

    fn rename(&self, id: &SessionId, title: &str) -> Result<(), BackendError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(BackendError::Other(
                "A session’s name can’t be empty.".into(),
            ));
        }
        self.with_state(|state| {
            let entry = state
                .sessions
                .iter_mut()
                .find(|entry| &entry.id == id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            entry.title = title.to_string();
            Ok(())
        })
    }

    fn delete_plan(&self, id: &SessionId) -> Result<DeletePlan, BackendError> {
        self.with_state(|state| {
            state
                .sessions
                .iter()
                .any(|entry| &entry.id == id)
                .then(DeletePlan::default)
                .ok_or_else(|| BackendError::NotFound(id.clone()))
        })
    }

    fn delete(&self, id: &SessionId) -> Result<(), BackendError> {
        self.with_state(|state| {
            let index = state
                .sessions
                .iter()
                .position(|entry| &entry.id == id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            if let Some(holder) = &state.sessions[index].holder {
                return Err(BackendError::Locked {
                    holder: Some(holder.clone()),
                });
            }
            if state.open.contains(id) {
                return Err(BackendError::Locked {
                    holder: Some(Holder {
                        app: "arko-studio".into(),
                        pid: std::process::id(),
                        host: String::new(),
                        since: chrono::Utc::now(),
                    }),
                });
            }
            state.sessions.remove(index);
            Ok(())
        })
    }

    fn models(&self, _: Option<&Path>) -> ModelChoices {
        let mut models = self.models.clone();
        if self.groups_follow_keys {
            self.with_state(|state| {
                for group in &mut models.groups {
                    let key_set = state.accounts.keys.iter().any(|row| {
                        row.masked.is_some() && group.name.starts_with(row.key.provider)
                    });
                    if key_set {
                        group.needs = None;
                    }
                }
            });
        }
        models
    }

    fn start(&self, start: Start, _: Arc<dyn arko::Prompter>) -> Launch {
        self.with_state(|state| match start {
            Start::Resume(id) => {
                let Some(entry) = state.sessions.iter().find(|entry| entry.id == id).cloned()
                else {
                    return Launch::Refused(BackendError::NotFound(id));
                };
                if let Some(holder) = &entry.holder {
                    return Launch::Refused(BackendError::Locked {
                        holder: Some(holder.clone()),
                    });
                }
                if !state.open.contains(&id) {
                    state.open.push(id);
                }
                Launch::Sample(entry)
            }
            Start::New(new) => {
                state.created += 1;
                let entry = SessionEntry {
                    id: SessionId(format!("20261016-120000-new{}", state.created)),
                    title: "New session".into(),
                    mode: new.mode,
                    workspace: new.folder.filter(|_| new.mode == Mode::Agent),
                    // As arko starts one: the composer's model, else the
                    // one Settings set, else its own choice.
                    model: new
                        .model
                        .or_else(|| self.defaults().model)
                        .or_else(|| self.models.default.clone())
                        .unwrap_or_default(),
                    created: SystemTime::now(),
                    last_active: None,
                    conversations: Vec::new(),
                    holder: None,
                    had_workers: false,
                };
                state.open.push(entry.id.clone());
                state.sessions.push(entry.clone());
                Launch::Sample(entry)
            }
        })
    }

    fn closed(&self, id: &SessionId) {
        self.with_state(|state| state.open.retain(|open| open != id));
    }

    fn add_key(&self, provider: &str, value: &str) -> Result<String, String> {
        let value = value.trim();
        if value.len() < 8 {
            return Err("That doesn’t look like a key.".into());
        }
        let key = key_named(provider).ok_or_else(|| format!("Arko takes no {provider} key."))?;
        let dotenv = self.root.join(".env");
        self.with_state(|state| {
            state.keys.push((provider.into(), value.into()));
            if let Some(row) = state.accounts.keys.iter_mut().find(|row| row.key == key) {
                row.masked = Some(accounts::masked(value, key.prefix));
                row.variable = key.name;
                row.source = Some(account_words::KeySource::Arko(dotenv));
            }
        });
        Ok(format!("Arko saved the {provider} key."))
    }

    fn set_chatgpt_login(&self, path: &Path) -> Result<String, String> {
        if path.file_name().is_none_or(|name| name != "auth.json") {
            return Err(format!(
                "Could not use {}: it holds no ChatGPT login.",
                path.display()
            ));
        }
        self.with_state(|state| state.accounts.chatgpt = accounts::chosen_login(path.into()));
        Ok(accounts::login_chosen_words(None))
    }

    fn set_defaults(&self, defaults: SessionDefaults) {
        *lock(&self.defaults) = defaults;
    }

    fn accounts(&self) -> Accounts {
        self.with_state(|state| {
            state.accounts_read += 1;
            state.accounts.clone()
        })
    }

    fn remove_key(&self, provider: &str) -> Result<String, String> {
        self.with_state(|state| {
            let row = state
                .accounts
                .keys
                .iter_mut()
                .find(|row| row.key.id == provider)
                .ok_or_else(|| format!("Arko takes no {provider} key."))?;
            if !matches!(row.source, Some(account_words::KeySource::Arko(_))) {
                return Err("Arko didn’t save this key, so it can’t remove it.".into());
            }
            row.masked = None;
            row.source = None;
            state.keys.retain(|(id, _)| id != provider);
            Ok(format!("Arko removed the {provider} key from its .env."))
        })
    }

    fn extensions(&self, _: &[PathBuf], mcp_config: &[PathBuf]) -> extensions::Extensions {
        let mut found = self.with_state(|state| state.extensions.clone());
        // The files set in Settings, none of them there.
        found
            .files
            .extend(mcp_config.iter().map(|path| extensions::McpFile {
                path: path.clone(),
                arko: false,
                servers: None,
            }));
        found
    }

    fn forget_mcp_approval(&self, folder: &Path) -> Result<(), String> {
        self.with_state(|state| {
            for project in &mut state.extensions.projects {
                if project.folder == folder {
                    project.approval = extensions::Approval::NotAsked;
                }
            }
        });
        Ok(())
    }
}

impl FakeBackend {
    /// The keys added, by provider.
    pub fn added_keys(&self) -> Vec<(String, String)> {
        self.with_state(|state| state.keys.clone())
    }

    /// How many times the accounts were read (Check again reads them).
    pub fn accounts_read(&self) -> usize {
        self.with_state(|state| state.accounts_read)
    }

    /// Sets what the accounts read as.
    pub fn set_accounts(&self, accounts: Accounts) {
        self.with_state(|state| state.accounts = accounts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_model_comes_from_last_model_json() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(last_model(folder.path()), None);
        std::fs::write(
            folder.path().join("last-model.json"),
            r#"{ "main": "cc:claude-opus-5-5", "worker": "gemini-3.8-flash" }"#,
        )
        .unwrap();
        assert_eq!(
            last_model(folder.path()).as_deref(),
            Some("cc:claude-opus-5-5")
        );
        std::fs::write(folder.path().join("last-model.json"), "not json").unwrap();
        assert_eq!(last_model(folder.path()), None);
    }

    fn backend() -> FakeBackend {
        FakeBackend::sample("/arko", SystemTime::now())
    }

    fn prompter() -> Arc<dyn arko::Prompter> {
        Arc::new(arko::NoPrompter)
    }

    #[test]
    fn a_session_open_elsewhere_says_who_has_it() {
        let backend = backend();
        let Launch::Refused(error) = backend.start(
            Start::Resume(SessionId("20261014-110500-e5f6".into())),
            prompter(),
        ) else {
            panic!("started");
        };
        let message = error.message();
        assert!(
            message.starts_with("It’s open in Arko in a terminal (process 4121), since "),
            "{message}"
        );
    }

    #[test]
    fn errors_read_as_sentences() {
        assert_eq!(
            BackendError::Other("could not create a session: disk full".into()).message(),
            "Could not create a session: disk full."
        );
        assert_eq!(sentence("Done!"), "Done!");
        assert_eq!(sentence(""), "");
    }

    #[test]
    fn rename_trims_and_refuses_an_empty_name() {
        let backend = backend();
        let id = SessionId("20261016-080000-a7b8".into());
        backend.rename(&id, "  Fonts  ").unwrap();
        let sessions = backend.sessions().unwrap();
        assert_eq!(
            sessions.iter().find(|entry| entry.id == id).unwrap().title,
            "Fonts"
        );
        assert!(backend.rename(&id, "   ").is_err());
    }

    #[test]
    fn delete_refuses_a_started_session_and_removes_a_closed_one() {
        let backend = backend();
        let id = SessionId("20261016-091200-a1b2".into());
        assert!(matches!(
            backend.start(Start::Resume(id.clone()), prompter()),
            Launch::Sample(_)
        ));
        assert!(matches!(
            backend.delete(&id),
            Err(BackendError::Locked { .. })
        ));
        backend.closed(&id);
        backend.delete(&id).unwrap();
        assert!(
            backend
                .sessions()
                .unwrap()
                .iter()
                .all(|entry| entry.id != id)
        );
        assert_eq!(backend.delete(&id), Err(BackendError::NotFound(id.clone())));
    }

    /// The real backend lists, renames and deletes the sessions in Arko's
    /// folder, through arko's own functions.
    #[test]
    fn arko_sessions_are_listed_renamed_and_deleted() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let backend = ArkoBackend::new(&root);
        assert!(backend.sessions().unwrap().is_empty());
        let id = crate::testing::create_session(&root, temp.path(), "Count the pears.");
        let sessions = backend.sessions().unwrap();
        assert_eq!(sessions.len(), 1);
        let entry = &sessions[0];
        assert_eq!(entry.id, id);
        assert_eq!(entry.mode, Mode::Agent);
        assert_eq!(entry.workspace.as_deref(), Some(temp.path()));
        assert!(entry.holder.is_none());
        backend.rename(&id, "Pears").unwrap();
        assert_eq!(backend.sessions().unwrap()[0].title, "Pears");
        assert_eq!(backend.delete_plan(&id).unwrap(), DeletePlan::default());
        backend.delete(&id).unwrap();
        assert!(backend.sessions().unwrap().is_empty());
        assert!(temp.path().is_dir(), "the workspace stays");
    }

    #[test]
    fn a_new_session_starts_in_its_folder_on_its_model() {
        let backend = ArkoBackend::new("/arko");
        let options = backend.options(Start::New(NewSession {
            mode: Mode::Agent,
            folder: Some(PathBuf::from("/home/ada/src/arko")),
            model: Some("sonnet".into()),
            effort: Some("high".into()),
        }));
        assert_eq!(options.root.as_deref(), Some(Path::new("/arko")));
        assert_eq!(options.session, arko::SessionChoice::New);
        assert_eq!(
            options.workspace.as_deref(),
            Some(Path::new("/home/ada/src/arko"))
        );
        assert_eq!(options.model.as_deref(), Some("sonnet"));
        assert_eq!(options.effort, arko::Effort::High);
        assert_eq!(options.app, "arko-studio");
        assert_eq!(options.mode, arko::sessions::Mode::Agent);
        let options = backend.options(Start::Resume(SessionId("s1".into())));
        assert_eq!(options.session, arko::SessionChoice::Resume("s1".into()));
    }

    /// Settings choose a new session's model and effort when its composer
    /// didn't, and a resumed session keeps its own.
    #[test]
    fn a_new_session_takes_the_models_set_in_settings() {
        let backend = ArkoBackend::new("/arko");
        let mut prefs = crate::prefs::Prefs::default();
        prefs.new_sessions.model = Some("claude-sonnet-5-5".into());
        prefs.new_sessions.effort = Some("low".into());
        prefs.new_sessions.handover_at = Some(300_000);
        backend.set_defaults(SessionDefaults::from_prefs(&prefs));
        let new = |model: Option<&str>, effort: Option<&str>| {
            backend.options(Start::New(NewSession {
                mode: Mode::Agent,
                folder: Some(PathBuf::from("/home/ada/src/arko")),
                model: model.map(str::to_string),
                effort: effort.map(str::to_string),
            }))
        };
        let options = new(None, None);
        assert_eq!(options.model.as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(options.effort, arko::Effort::Low);
        assert_eq!(options.handover_at, Some(300_000));
        let options = new(Some("opus"), Some("high"));
        assert_eq!(options.model.as_deref(), Some("opus"));
        assert_eq!(options.effort, arko::Effort::High);
        let options = backend.options(Start::Resume(SessionId("s1".into())));
        assert_eq!(options.model, None);
        assert_eq!(options.handover_at, None);
    }

    /// Starting a session returns at once, the files its options come from
    /// read on another thread: here the last session's model, in a file
    /// whose reading waits until something writes to it.
    #[cfg(target_os = "linux")]
    #[test]
    fn starting_a_session_reads_no_files_on_the_callers_thread() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        std::fs::create_dir_all(&root).unwrap();
        let fifo = root.join("last-model.json");
        let made = std::process::Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap();
        assert!(made.success());
        // A session that isn't there: it stops as it opens, quickly.
        let backend = ArkoBackend::new(&root).with_configure(Arc::new(|options| {
            options.session = arko::SessionChoice::Resume("none".into())
        }));
        // The 1-hour cache makes the options ask for the last model.
        let mut prefs = crate::prefs::Prefs::default();
        prefs.new_sessions.cache_1h = Some(true);
        backend.set_defaults(SessionDefaults::from_prefs(&prefs));
        let (started, returned) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let launch = backend.start(
                Start::New(NewSession {
                    mode: Mode::Chat,
                    folder: None,
                    model: None,
                    effort: None,
                }),
                Arc::new(arko::NoPrompter),
            );
            started.send(matches!(launch, Launch::Engine(_))).ok();
        });
        let launched = returned.recv_timeout(Duration::from_secs(5));
        // Lets the reading end, whatever happened, once it's waiting.
        let waited = std::time::Instant::now();
        loop {
            use std::io::Write as _;
            use std::os::unix::fs::OpenOptionsExt as _;
            // O_NONBLOCK: no reader yet is an error, not a wait.
            let opened = std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(0o4000)
                .open(&fifo);
            match opened {
                Ok(mut file) => {
                    file.write_all(b"{}").ok();
                    break;
                }
                Err(_) if waited.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(10))
                }
                Err(_) => break,
            }
        }
        assert_eq!(launched, Ok(true), "start waited for the file");
    }

    /// MCP config files set in Settings that are gone, or aren't MCP config
    /// files, are left out of a session's options with a note each, since
    /// arko would refuse to open it.
    #[test]
    fn mcp_config_files_that_cant_be_read_are_left_out_with_a_note() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("servers.json");
        let bad = temp.path().join("notes.json");
        let gone = temp.path().join("gone.json");
        std::fs::write(&good, r#"{"mcpServers": {}}"#).unwrap();
        std::fs::write(&bad, "{}").unwrap();
        let mut prefs = crate::prefs::Prefs::default();
        prefs.advanced.mcp_config = vec![good.clone(), bad.clone(), gone.clone()];
        let defaults = SessionDefaults::from_prefs(&prefs);
        let resume = Start::Resume(SessionId("s1".into()));
        let (options, notes) = prepare(Path::new("/arko"), None, &defaults, resume);
        assert_eq!(options.mcp_config, [good]);
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes[0].contains(&bad.display().to_string()), "{notes:?}");
        assert!(notes[0].contains("it has no mcpServers"), "{notes:?}");
        assert!(notes[1].contains(&gone.display().to_string()), "{notes:?}");
        assert!(notes[1].contains("it isn’t there"), "{notes:?}");
    }

    /// A chat is created in no folder of the user's, even when one was
    /// chosen for an agent session before.
    #[test]
    fn a_new_chat_has_no_folder() {
        let backend = ArkoBackend::new("/arko");
        let options = backend.options(Start::New(NewSession {
            mode: Mode::Chat,
            folder: Some(PathBuf::from("/home/ada/src/arko")),
            model: None,
            effort: None,
        }));
        assert_eq!(options.mode, arko::sessions::Mode::Chat);
        assert_eq!(options.workspace, None);
        assert_eq!(options.model, None);
    }
}
