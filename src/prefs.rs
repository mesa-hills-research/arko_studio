//! Arko Studio's preferences, kept in `<arko folder>/studio.json`.
//!
//! Every field has a default, so an older file loads in a newer app and a
//! newer file in an older one. The file is read field by field: a value
//! this version can't read (a choice a newer version added, a number typed
//! by hand) keeps its own default, and the other settings load as they
//! are. What the app doesn't read, values and fields alike, is written back
//! as it was, unless its setting was changed here. Changes are written
//! after a short pause, on a background thread, through a temporary file
//! renamed over the old one, so a crash never leaves half a file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// How long a change waits for the next one before the file is written.
pub const SAVE_DELAY: Duration = Duration::from_millis(400);

/// Everything Arko Studio remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub appearance: Appearance,
    pub input: Input,
    pub notifications: Notifications,
    pub window: Option<WindowPlacement>,
    pub sidebar: SidebarPrefs,
    pub workers_panel: PanelPrefs,
    /// How much of a session the transcript shows, app-wide.
    pub view: TranscriptView,
    pub sessions: SessionPrefs,
    /// Sharing sessions on the web page (Settings › Web page).
    pub web_page: WebPagePrefs,
    /// What new sessions start with.
    pub new_sessions: NewSessions,
    pub advanced: AdvancedPrefs,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            appearance: Appearance::default(),
            input: Input::default(),
            notifications: Notifications::default(),
            window: None,
            sidebar: SidebarPrefs::default(),
            workers_panel: PanelPrefs::default(),
            view: TranscriptView::Full,
            sessions: SessionPrefs::default(),
            web_page: WebPagePrefs::default(),
            new_sessions: NewSessions::default(),
            advanced: AdvancedPrefs::default(),
        }
    }
}

/// The address the web page is served on when none is chosen: every
/// network this computer is on, so a phone or another computer on the same
/// network can open it.
pub const DEFAULT_WEB_PAGE_ADDRESS: &str = "0.0.0.0";

/// The address that keeps the web page to this computer.
pub const THIS_COMPUTER_ONLY: &str = "127.0.0.1";

/// Where sessions serve their web page, and whether they do from the start.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WebPagePrefs {
    /// Every session serves its web page as it opens.
    pub share_by_default: bool,
    /// The IP address to serve on, `None` for every network this computer
    /// is on ([`DEFAULT_WEB_PAGE_ADDRESS`]). [`THIS_COMPUTER_ONLY`] reaches
    /// this computer only. Kept as `ip`: earlier versions kept the address
    /// as `address`, with this computer only as the default (see
    /// [`migrate`]).
    #[serde(rename = "ip", skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    /// `None`: arko's port (8765), or any free one when that's taken.
    pub port: Option<u16>,
}

impl WebPagePrefs {
    /// The address to serve on: the one chosen, else the default.
    pub fn address(&self) -> &str {
        self.address
            .as_deref()
            .map(str::trim)
            .filter(|address| !address.is_empty())
            .unwrap_or(DEFAULT_WEB_PAGE_ADDRESS)
    }

    /// The address to serve on as an IP address, `None` when the one chosen
    /// isn't one (a hand-edited word or host name).
    pub fn ip(&self) -> Option<std::net::IpAddr> {
        self.address()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse()
            .ok()
    }

    /// The argument `/remote` takes to serve there, the address always
    /// given (arko's own default is this computer only, and without one a
    /// session serves where it served last): `0.0.0.0`, `0.0.0.0:9000`,
    /// `127.0.0.1` or `[::]:9000`. An address that isn't an IP address
    /// serves on this computer only.
    pub fn bind_argument(&self) -> String {
        let ip = self
            .ip()
            .unwrap_or(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        match self.port {
            Some(port) => std::net::SocketAddr::new(ip, port).to_string(),
            None => ip.to_string(),
        }
    }
}

/// Brings a preferences file of an earlier version up to this one's, in
/// place: the web page's `address`, where this computer only was the
/// default and saved as such (it couldn't be chosen apart from the
/// default), becomes `ip` when it names another address or automatic
/// sharing is on. An inactive local default follows the new default.
fn migrate(file: &mut Value) {
    let Some(page) = file.get_mut("web_page").and_then(Value::as_object_mut) else {
        return;
    };
    let Some(old) = page.remove("address") else {
        return;
    };
    let automatic = page.get("share_by_default").and_then(Value::as_bool) == Some(true);
    let chosen = old
        .as_str()
        .map(str::trim)
        .filter(|address| !address.is_empty() && (*address != THIS_COMPUTER_ONLY || automatic));
    if let Some(address) = chosen
        && !page.contains_key("ip")
    {
        page.insert("ip".into(), Value::String(address.to_string()));
    }
}

/// How sessions behave, app-wide (Settings › Sessions).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionPrefs {
    /// Quitting, or closing a session, asks first when background jobs or
    /// workers would stop.
    pub ask_before_quit: bool,
    /// Switching an open session between Chat and Agent asks first, with
    /// what it costs.
    pub ask_before_switch: bool,
    /// Editing or regenerating a message asks first, saying how many
    /// messages after it go.
    pub ask_before_edit: bool,
}

impl Default for SessionPrefs {
    fn default() -> Self {
        Self {
            ask_before_quit: true,
            ask_before_switch: true,
            ask_before_edit: true,
        }
    }
}

/// How a new session works, before the user picks in its composer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NewSessionMode {
    Chat,
    #[default]
    Agent,
}

/// Which agents get the extra rules for small models in their
/// instructions (arko's `--extra-rules`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExtraRules {
    /// The agents on small models (Haiku, luna).
    #[default]
    Small,
    All,
    Off,
}

/// Which of a project's AGENTS.md and CLAUDE.md the agents get (arko's
/// `--project-instructions`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectInstructions {
    /// AGENTS.md, else CLAUDE.md.
    #[default]
    One,
    Both,
    Off,
}

/// What new sessions start with (Settings › Models & accounts and
/// Sessions). A session keeps what it started with: changing these leaves
/// the sessions there are alone. `None` leaves the choice to arko, as the
/// terminal app makes it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NewSessions {
    pub mode: NewSessionMode,
    /// The main agent's model, as `/model` takes it. `None`: the last
    /// session's, else one the credentials allow.
    pub model: Option<String>,
    /// As `/model` takes it (`high`). `None`: arko's default, xhigh.
    pub effort: Option<String>,
    /// The workers' model. `None`: the last session's workers' when the
    /// main agent starts on the last session's model, else the main
    /// agent's.
    pub worker_model: Option<String>,
    /// `None`: the main agent's effort.
    pub worker_effort: Option<String>,
    /// Anthropic's 1-hour prompt cache, for Claude by API key. `None`: as
    /// `ARKO_CACHE_1H` in the environment or a `.env` file says, else off.
    pub cache_1h: Option<bool>,
    /// The main agent's handover size, in tokens. `None`: its model's own.
    pub handover_at: Option<usize>,
    /// The workers' handover size, in tokens. `None`: the main agent's.
    pub workers_handover_at: Option<usize>,
    /// After how many seconds a shell command still running moves to the
    /// background, 0 for never. `None`: arko's 10 minutes.
    pub background_after: Option<u64>,
    /// How long after the user's last message a monitor's warning before
    /// it ends still wakes the agent, as `/monitor-wakes` takes it (`2h`,
    /// `always`, `off`). `None`: arko's 2 hours.
    pub monitor_wakes: Option<String>,
    /// Who chooses the workers' models, as `/worker-models` takes it.
    pub worker_models: arko::ModelChoice,
    /// How many levels of workers the session allows, 1 to 4, as
    /// `/worker-depth` takes it.
    pub worker_depth: u32,
    /// The agents may change files outside the session's folder.
    pub outside_access: bool,
    /// The main agent ends its turns with a reply in text (else with
    /// `complete_task`).
    pub text_replies: bool,
    /// Agents on `cc:` models let Claude Code compact their conversation at
    /// the handover size (else they hand over).
    pub cc_compact: bool,
    pub extra_rules: ExtraRules,
    pub project_instructions: ProjectInstructions,
    /// Workers get the project's instructions too.
    pub worker_project_instructions: bool,
    /// What a new chat's model is told, on top of Arko's own instructions.
    pub chat_instructions: String,
}

impl Default for NewSessions {
    fn default() -> Self {
        Self {
            mode: NewSessionMode::Agent,
            model: None,
            effort: None,
            worker_model: None,
            worker_effort: None,
            cache_1h: None,
            handover_at: None,
            workers_handover_at: None,
            background_after: None,
            monitor_wakes: None,
            worker_models: arko::ModelChoice::default(),
            worker_depth: arko::workers::DEFAULT_DEPTH,
            outside_access: false,
            text_replies: true,
            cc_compact: true,
            extra_rules: ExtraRules::Small,
            project_instructions: ProjectInstructions::One,
            worker_project_instructions: true,
            chat_instructions: String::new(),
        }
    }
}

/// The shell new sessions run commands in on Windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowsShell {
    Cmd,
    Powershell,
}

/// Settings › Advanced.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AdvancedPrefs {
    /// `None`: as `ARKO_SHELL` says, else cmd.exe.
    pub windows_shell: Option<WindowsShell>,
    /// MCP config files every session starts the servers of, besides the
    /// project's `.mcp.json` and Arko's `mcp.json` (arko's `--mcp-config`).
    pub mcp_config: Vec<PathBuf>,
}

/// The transcript's views (the terminal app's Ctrl+O).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptView {
    /// Everything.
    #[default]
    Full,
    /// Calls grouped, thinking folded.
    Quiet,
    /// Only the messages, questions and answers.
    Messages,
}

impl TranscriptView {
    /// The next view, as View › Next Transcript View steps.
    pub fn next(self) -> Self {
        match self {
            TranscriptView::Full => TranscriptView::Quiet,
            TranscriptView::Quiet => TranscriptView::Messages,
            TranscriptView::Messages => TranscriptView::Full,
        }
    }
}

/// Light, dark, or as the system is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    System,
    Light,
    Dark,
}

/// Whether animations play: as the system's reduced-motion setting says, or
/// always, or never.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Motion {
    #[default]
    System,
    On,
    Off,
}

pub const DEFAULT_LIGHT_THEME: &str = "macOS Classic Light";
pub const DEFAULT_DARK_THEME: &str = "macOS Classic Dark";
pub const DEFAULT_UI_FONT: &str = gpui_kit::assets::fonts::WORK_SANS_FAMILY;
pub const DEFAULT_CODE_FONT: &str = gpui_kit::assets::fonts::JETBRAINS_MONO_FAMILY;
/// The interface's base size at 100 % zoom: the rem every size derives from.
pub const DEFAULT_UI_SIZE: f32 = 16.;
pub const DEFAULT_CODE_SIZE: f32 = 13.;

/// The zoom steps Mod+= and Mod+- move through, as browsers have them.
pub const ZOOM_STEPS: [f32; 11] = [0.5, 0.67, 0.75, 0.8, 0.9, 1.0, 1.1, 1.25, 1.5, 1.75, 2.0];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Appearance {
    pub theme_mode: ThemeMode,
    pub light_theme: String,
    pub dark_theme: String,
    /// Resting on a theme in a theme list shows it.
    pub theme_previews: bool,
    /// The notice that previews can flash was answered.
    pub flash_notice: bool,
    pub ui_font: FontChoice,
    pub code_font: FontChoice,
    /// 1.0 is 100 %.
    pub zoom: f32,
    pub motion: Motion,
    pub render_markdown: bool,
    pub stream_fade: bool,
    pub expand_thinking: bool,
    /// The code font joins characters such as `->` into ligatures.
    pub code_ligatures: bool,
}

impl Default for Appearance {
    fn default() -> Self {
        Self {
            theme_mode: ThemeMode::System,
            light_theme: DEFAULT_LIGHT_THEME.into(),
            dark_theme: DEFAULT_DARK_THEME.into(),
            theme_previews: true,
            flash_notice: false,
            ui_font: FontChoice::new(DEFAULT_UI_FONT, DEFAULT_UI_SIZE),
            code_font: FontChoice::new(DEFAULT_CODE_FONT, DEFAULT_CODE_SIZE),
            zoom: 1.0,
            motion: Motion::System,
            render_markdown: true,
            stream_fade: false,
            expand_thinking: false,
            code_ligatures: true,
        }
    }
}

impl Appearance {
    /// The zoom, held to the range the steps cover.
    pub fn zoom(&self) -> f32 {
        if self.zoom.is_finite() {
            self.zoom
                .clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1])
        } else {
            1.0
        }
    }
}

/// The next zoom step up (`direction` > 0) or down from `zoom`.
pub fn step_zoom(zoom: f32, direction: i32) -> f32 {
    if direction > 0 {
        ZOOM_STEPS
            .iter()
            .copied()
            .find(|step| *step > zoom + 0.001)
            .unwrap_or(ZOOM_STEPS[ZOOM_STEPS.len() - 1])
    } else {
        ZOOM_STEPS
            .iter()
            .rev()
            .copied()
            .find(|step| *step < zoom - 0.001)
            .unwrap_or(ZOOM_STEPS[0])
    }
}

/// A font family and its size in pixels at 100 % zoom.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontChoice {
    pub family: String,
    pub size: f32,
}

impl FontChoice {
    pub fn new(family: &str, size: f32) -> Self {
        Self {
            family: family.into(),
            size,
        }
    }

    /// The size, held to what the font picker offers.
    pub fn size(&self) -> f32 {
        if self.size.is_finite() {
            self.size.clamp(6., 96.)
        } else {
            DEFAULT_UI_SIZE
        }
    }
}

impl Default for FontChoice {
    fn default() -> Self {
        Self::new(DEFAULT_UI_FONT, DEFAULT_UI_SIZE)
    }
}

/// Which chord sends a message from the composer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SendKey {
    #[default]
    Enter,
    ModEnter,
    ShiftEnter,
}

/// How eagerly word suggestions come up.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Suggestions {
    Off,
    #[default]
    Quiet,
    Eager,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Keymap {
    #[default]
    Standard,
    Emacs,
    Vim,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Input {
    pub send_key: SendKey,
    pub smooth_typing: bool,
    pub spell_check: bool,
    pub suggestions: Suggestions,
    pub keymap: Keymap,
    /// Pastes longer than this many lines become a token.
    pub paste_token_lines: u32,
    /// The composer's tall mode, kept app-wide.
    pub composer_expanded: bool,
}

impl Default for Input {
    fn default() -> Self {
        Self {
            send_key: SendKey::Enter,
            smooth_typing: true,
            spell_check: true,
            suggestions: Suggestions::Quiet,
            keymap: Keymap::Standard,
            paste_token_lines: 20,
            composer_expanded: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Notifications {
    pub turn_ended: bool,
    pub question: bool,
    pub cache_reminders: bool,
    pub sound: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            turn_ended: true,
            question: true,
            cache_reminders: true,
            sound: false,
        }
    }
}

/// Where the main window was, in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowPlacement {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    #[serde(default)]
    pub maximized: bool,
}

pub const DEFAULT_SIDEBAR_WIDTH: f32 = 16.;
pub const MIN_SIDEBAR_WIDTH: f32 = 13.;
pub const MAX_SIDEBAR_WIDTH: f32 = 26.;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidebarPrefs {
    /// In rem, so it follows the zoom.
    pub width: f32,
    pub open: bool,
    /// The groups the user closed: `chats`, or a folder's path.
    pub collapsed: Vec<String>,
}

impl Default for SidebarPrefs {
    fn default() -> Self {
        Self {
            width: DEFAULT_SIDEBAR_WIDTH,
            open: true,
            collapsed: Vec::new(),
        }
    }
}

impl SidebarPrefs {
    pub fn width(&self) -> f32 {
        if self.width.is_finite() {
            self.width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH)
        } else {
            DEFAULT_SIDEBAR_WIDTH
        }
    }
}

pub const DEFAULT_PANEL_WIDTH: f32 = 22.;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PanelPrefs {
    /// In rem.
    pub width: f32,
}

impl Default for PanelPrefs {
    fn default() -> Self {
        Self {
            width: DEFAULT_PANEL_WIDTH,
        }
    }
}

impl Prefs {
    /// Reads `path`, field by field ([`PrefsStore::load`]).
    pub fn load(path: &Path) -> Self {
        PrefsStore::load(path).prefs
    }
}

/// What a file held that this version doesn't read: fields it doesn't know
/// and values it can't read, each where it was. They are written back on
/// save where the setting is still as it was read.
#[derive(Debug, Clone, Default, PartialEq)]
struct Unread {
    /// Each value, by its path of keys in the file.
    values: Vec<(Vec<String>, Value)>,
    /// The preferences as read, which tell whether a setting changed since.
    read: Value,
}

impl Unread {
    /// `value` (the preferences to save) with what wasn't read put back,
    /// where its setting is as it was read.
    fn restore(&self, mut value: Value) -> Value {
        for (path, kept) in &self.values {
            let pointer = pointer(path);
            if value.pointer(&pointer) == self.read.pointer(&pointer) {
                put(&mut value, path, kept.clone());
            }
        }
        value
    }
}

/// Reads the preferences in `file`, field by field: a value that can't be
/// read keeps its default, and goes in what isn't read, as do fields this
/// version doesn't know.
fn read_fields(file: &Value) -> (Prefs, Unread) {
    let mut values = Vec::new();
    let mut prefs = serde_json::from_value::<Prefs>(file.clone()).unwrap_or_else(|_| {
        let mut accepted = serde_json::to_value(Prefs::default()).unwrap_or_default();
        take_fields(&mut accepted, file, &mut Vec::new(), &mut values);
        serde_json::from_value(accepted).unwrap_or_default()
    });
    if prefs.web_page.address.is_none()
        && file.pointer("/web_page/ip").is_some_and(|ip| !ip.is_null())
    {
        // An unreadable restriction must not broaden automatic sharing.
        prefs.web_page.address = Some(THIS_COMPUTER_ONLY.to_string());
    }
    let read = serde_json::to_value(&prefs).unwrap_or_default();
    unknown_fields(file, &read, &mut Vec::new(), &mut values);
    (prefs, Unread { values, read })
}

/// Puts each field of `file` (at `path` in it) into `accepted`, the
/// preferences read so far, where the preferences still read with it.
/// A value that doesn't goes in `unread` (a section is tried field by
/// field). Fields no preference has are left to [`unknown_fields`].
fn take_fields(
    accepted: &mut Value,
    file: &Value,
    path: &mut Vec<String>,
    unread: &mut Vec<(Vec<String>, Value)>,
) {
    let Value::Object(fields) = file else {
        return;
    };
    for (key, value) in fields {
        path.push(key.clone());
        // A default serialization may omit a valid optional field. Let
        // deserialization decide whether each supplied value is usable.
        let before = accepted
            .pointer(&pointer(path))
            .cloned()
            .unwrap_or(Value::Null);
        put(accepted, path, value.clone());
        if serde_json::from_value::<Prefs>(accepted.clone()).is_err() {
            put(accepted, path, before.clone());
            if value.is_object() && before.is_object() {
                take_fields(accepted, value, path, unread);
            } else {
                unread.push((path.clone(), value.clone()));
            }
        }
        path.pop();
    }
}

/// The fields of `file` (at `path` in it) that `known`, the preferences as
/// read from it, doesn't have.
fn unknown_fields(
    file: &Value,
    known: &Value,
    path: &mut Vec<String>,
    unread: &mut Vec<(Vec<String>, Value)>,
) {
    let (Value::Object(fields), Value::Object(known)) = (file, known) else {
        return;
    };
    for (key, value) in fields {
        path.push(key.clone());
        match known.get(key) {
            None => unread.push((path.clone(), value.clone())),
            Some(known) => unknown_fields(value, known, path, unread),
        }
        path.pop();
    }
}

/// The JSON pointer of `path`.
fn pointer(path: &[String]) -> String {
    path.iter()
        .map(|key| format!("/{}", key.replace('~', "~0").replace('/', "~1")))
        .collect()
}

/// Sets the value at `path` in `value`, making the objects on the way.
fn put(value: &mut Value, path: &[String], new: Value) {
    let Some((last, parents)) = path.split_last() else {
        *value = new;
        return;
    };
    let mut at = value;
    for key in parents {
        if !at.is_object() {
            *at = Value::Object(Map::new());
        }
        at = at
            .as_object_mut()
            .expect("an object")
            .entry(key.clone())
            .or_insert(Value::Null);
    }
    if !at.is_object() {
        *at = Value::Object(Map::new());
    }
    at.as_object_mut()
        .expect("an object")
        .insert(last.clone(), new);
}

/// Writes `text` to `path` through a temporary file in the same folder, so
/// readers see the old file or the new one, never part of one.
pub fn write_atomically(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(folder) = path.parent() {
        std::fs::create_dir_all(folder)?;
    }
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    std::fs::write(&temporary, text)?;
    std::fs::rename(&temporary, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&temporary);
    })
}

/// The preferences in force, and where they are saved. Observing this global
/// hears every change to the preferences.
pub struct PrefsStore {
    prefs: Prefs,
    /// `None` keeps them in memory only (tests, or no Arko folder).
    path: Option<PathBuf>,
    /// What the file held that this version doesn't read.
    unread: Unread,
}

impl Global for PrefsStore {}

impl PrefsStore {
    pub fn new(prefs: Prefs, path: Option<PathBuf>) -> Self {
        Self {
            prefs,
            path,
            unread: Unread::default(),
        }
    }

    /// The preferences in the file at `path`, saved there. Each value that
    /// can't be read keeps its default alone ([`read_fields`]). A missing
    /// file gives the defaults. A file that isn't JSON is copied beside it
    /// as `studio.json.bad` before the first save replaces it, and one that
    /// can't be read at all is left alone: this run's changes aren't saved.
    pub fn load(path: &Path) -> Self {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Self::new(Prefs::default(), Some(path.to_path_buf()));
            }
            Err(error) => {
                tracing::warn!("couldn't read {} ({error})", path.display());
                return Self::new(Prefs::default(), None);
            }
        };
        match serde_json::from_str::<Value>(&text) {
            Ok(mut file) if file.is_object() => {
                migrate(&mut file);
                let (prefs, unread) = read_fields(&file);
                for (keys, _) in &unread.values {
                    tracing::warn!("in {}, left {} as it is", path.display(), keys.join("."));
                }
                Self {
                    prefs,
                    path: Some(path.to_path_buf()),
                    unread,
                }
            }
            _ => {
                let kept = path.with_extension("json.bad");
                tracing::warn!(
                    "{} isn't a preferences file, so it was kept as {}",
                    path.display(),
                    kept.display()
                );
                match std::fs::write(&kept, &text) {
                    Ok(()) => Self::new(Prefs::default(), Some(path.to_path_buf())),
                    Err(error) => {
                        tracing::warn!("couldn't keep it ({error})");
                        Self::new(Prefs::default(), None)
                    }
                }
            }
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The file's text for the preferences now.
    fn text(&self) -> serde_json::Result<String> {
        let value = self.unread.restore(serde_json::to_value(&self.prefs)?);
        serde_json::to_string_pretty(&value)
    }
}

/// The saves' bookkeeping, apart from [`PrefsStore`] so that writing the
/// file doesn't look like a change to its observers.
#[derive(Default)]
struct Saver {
    /// Counts the changes, and the change the file holds.
    changes: u64,
    saved: u64,
    pending: Option<Task<()>>,
}

impl Global for Saver {}

/// The file, its text and the change it holds, while the file is behind.
fn unsaved(cx: &App) -> Option<(PathBuf, String, u64)> {
    let store = cx.global::<PrefsStore>();
    let saver = cx.global::<Saver>();
    let path = store.path.clone()?;
    if saver.saved == saver.changes {
        return None;
    }
    match store.text() {
        Ok(text) => Some((path, text, saver.changes)),
        Err(error) => {
            tracing::error!("couldn't write the preferences ({error})");
            None
        }
    }
}

/// Installs the preferences for the app.
pub fn init(store: PrefsStore, cx: &mut App) {
    cx.set_global(store);
    cx.set_global(Saver::default());
    // A change still waiting for its save is written before the app quits.
    cx.on_app_quit(|cx| {
        cx.global_mut::<Saver>().pending = None;
        let save = unsaved(cx);
        async move {
            if let Some((path, text, _)) = save
                && let Err(error) = write_atomically(&path, &text)
            {
                tracing::error!("couldn't save {} ({error})", path.display());
            }
        }
    })
    .detach();
}

/// The preferences in force.
pub fn get(cx: &App) -> &Prefs {
    &cx.global::<PrefsStore>().prefs
}

/// Changes the preferences and saves them after [`SAVE_DELAY`]. Observers of
/// [`PrefsStore`] hear of the change only when something changed.
pub fn update(cx: &mut App, change: impl FnOnce(&mut Prefs)) {
    let mut prefs = get(cx).clone();
    change(&mut prefs);
    if prefs == *get(cx) {
        return;
    }
    cx.update_global::<PrefsStore, _>(|store, _| store.prefs = prefs);
    cx.global_mut::<Saver>().changes += 1;
    schedule_save(cx);
}

fn schedule_save(cx: &mut App) {
    if cx.global::<PrefsStore>().path.is_none() {
        return;
    }
    // Replacing the task drops the one waiting, so only the last change of a
    // burst is written.
    let task = cx.spawn(async move |cx| {
        cx.background_executor().timer(SAVE_DELAY).await;
        let Some((path, text, change)) = cx.update(|cx| unsaved(cx)) else {
            return;
        };
        let written = cx
            .background_executor()
            .spawn({
                let path = path.clone();
                async move { write_atomically(&path, &text) }
            })
            .await;
        match written {
            Ok(()) => cx.update(|cx| {
                let saver = cx.global_mut::<Saver>();
                saver.saved = saver.saved.max(change);
            }),
            Err(error) => tracing::error!("couldn't save {} ({error})", path.display()),
        }
    });
    cx.global_mut::<Saver>().pending = Some(task);
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_unreadable_sharing_restriction_falls_back_to_this_computer() {
        let (prefs, _) = read_fields(&serde_json::json!({
            "web_page": { "ip": 127, "share_by_default": true }
        }));
        assert_eq!(prefs.web_page.address(), THIS_COMPUTER_ONLY);
        assert!(prefs.web_page.share_by_default);
    }

    #[test]
    fn old_automatic_local_sharing_stays_local() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        for automatic in [false, true] {
            let file = serde_json::json!({"web_page": {
                "address": "127.0.0.1", "port": 8765, "share_by_default": automatic
            }});
            std::fs::write(&path, file.to_string()).unwrap();
            let prefs = Prefs::load(&path);
            assert_eq!(
                prefs.web_page.address(),
                if automatic { "127.0.0.1" } else { "0.0.0.0" }
            );
            assert_eq!(prefs.web_page.share_by_default, automatic);
        }
    }

    #[test]
    fn an_invalid_sibling_preserves_the_chosen_sharing_address() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        for page in [
            serde_json::json!({"ip": "127.0.0.1", "port": -1, "share_by_default": true}),
            serde_json::json!({"ip": "127.0.0.1", "port": 9000, "share_by_default": "yes"}),
            serde_json::json!({"ip": "127.0.0.1", "port": -1, "share_by_default": true, "future": {"x": 1}}),
        ] {
            let file = serde_json::json!({"web_page": page, "appearance": {"theme_mode": "dark"}});
            std::fs::write(&path, file.to_string()).unwrap();
            let prefs = Prefs::load(&path);
            assert_eq!(prefs.web_page.address(), "127.0.0.1");
            assert_eq!(
                prefs.web_page.share_by_default,
                page["share_by_default"] == true
            );
            assert_eq!(
                prefs.web_page.port,
                page["port"].as_u64().map(|port| port as u16)
            );
            assert_eq!(prefs.appearance.theme_mode, ThemeMode::Dark);
        }
    }

    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let folder = tempfile::tempdir().unwrap();
        assert_eq!(
            Prefs::load(&folder.path().join("studio.json")),
            Prefs::default()
        );
    }

    #[test]
    fn missing_and_unknown_fields_keep_their_defaults() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        std::fs::write(
            &path,
            r#"{ "appearance": { "dark_theme": "Alduin", "future": 1 }, "later": [] }"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.appearance.dark_theme, "Alduin");
        assert_eq!(prefs.appearance.light_theme, DEFAULT_LIGHT_THEME);
        assert_eq!(prefs.sidebar, SidebarPrefs::default());
    }

    /// A file that isn't JSON gives the defaults, and is kept beside, since
    /// the next save replaces it.
    #[test]
    fn a_broken_file_gives_the_defaults_and_is_kept() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        std::fs::write(&path, "{ not json").unwrap();
        let store = PrefsStore::load(&path);
        assert_eq!(store.prefs, Prefs::default());
        assert_eq!(store.path(), Some(path.as_path()));
        assert_eq!(
            std::fs::read_to_string(folder.path().join("studio.json.bad")).unwrap(),
            "{ not json"
        );
    }

    /// One value that doesn't read (a choice a newer version added, a
    /// negative number typed by hand) keeps its own default: every other
    /// setting loads as it is.
    #[test]
    fn a_value_that_doesnt_read_keeps_only_its_own_default() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        std::fs::write(
            &path,
            r#"{ "appearance": { "theme_mode": "dark", "light_theme": "Solarized Light" },
                 "new_sessions": { "handover_at": 950000, "extra_rules": "medium" } }"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.appearance.theme_mode, ThemeMode::Dark);
        assert_eq!(prefs.appearance.light_theme, "Solarized Light");
        assert_eq!(prefs.new_sessions.handover_at, Some(950_000));
        assert_eq!(prefs.new_sessions.extra_rules, ExtraRules::Small);
        std::fs::write(
            &path,
            r#"{ "appearance": { "theme_mode": "dark" },
                 "new_sessions": { "handover_at": -5, "text_replies": false },
                 "window": { "x": "left" } }"#,
        )
        .unwrap();
        let prefs = Prefs::load(&path);
        assert_eq!(prefs.appearance.theme_mode, ThemeMode::Dark);
        assert_eq!(prefs.new_sessions.handover_at, None);
        assert!(!prefs.new_sessions.text_replies);
        assert_eq!(prefs.window, None);
    }

    /// What the app doesn't read, values and fields alike, is saved as it
    /// was, until its setting is changed here.
    #[gpui_kit::test]
    async fn what_isnt_read_is_saved_as_it_was(cx: &mut gpui_kit::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        std::fs::write(
            &path,
            r#"{ "appearance": { "zoom": 1.25, "glow": true },
                 "new_sessions": { "extra_rules": "medium", "project_instructions": "both" },
                 "later": [1] }"#,
        )
        .unwrap();
        cx.update(|cx| init(PrefsStore::load(&path), cx));
        let saved = |cx: &mut gpui_kit::TestAppContext| {
            cx.executor().advance_clock(SAVE_DELAY);
            cx.run_until_parked();
            serde_json::from_str::<Value>(&std::fs::read_to_string(&path).unwrap()).unwrap()
        };
        cx.update(|cx| update(cx, |prefs| prefs.appearance.zoom = 1.5));
        let file = saved(cx);
        assert_eq!(file["appearance"]["zoom"], 1.5);
        assert_eq!(file["appearance"]["glow"], true);
        assert_eq!(file["new_sessions"]["extra_rules"], "medium");
        assert_eq!(file["new_sessions"]["project_instructions"], "both");
        assert_eq!(file["later"], serde_json::json!([1]));
        cx.update(|cx| update(cx, |prefs| prefs.new_sessions.extra_rules = ExtraRules::All));
        assert_eq!(saved(cx)["new_sessions"]["extra_rules"], "all");
    }

    #[test]
    fn writes_replace_the_file_whole() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("nested").join("studio.json");
        let mut prefs = Prefs::default();
        prefs.appearance.zoom = 1.25;
        write_atomically(&path, &serde_json::to_string_pretty(&prefs).unwrap()).unwrap();
        assert_eq!(Prefs::load(&path), prefs);
        let leftovers: Vec<_> = std::fs::read_dir(path.parent().unwrap())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(leftovers, ["studio.json"]);
    }

    #[test]
    fn the_web_page_address_becomes_the_remote_argument() {
        let page = |address: &str, port: Option<u16>| WebPagePrefs {
            share_by_default: false,
            address: Some(address.into()),
            port,
        };
        // The default is every network, which arko's own default isn't:
        // the address is always given.
        assert_eq!(WebPagePrefs::default().bind_argument(), "0.0.0.0");
        assert_eq!(page("", None).bind_argument(), "0.0.0.0");
        assert_eq!(page("127.0.0.1", None).bind_argument(), "127.0.0.1");
        assert_eq!(
            page("127.0.0.1", Some(9000)).bind_argument(),
            "127.0.0.1:9000"
        );
        assert_eq!(page(" 0.0.0.0 ", None).bind_argument(), "0.0.0.0");
        assert_eq!(page("0.0.0.0", Some(9000)).bind_argument(), "0.0.0.0:9000");
        assert_eq!(page("::", Some(9000)).bind_argument(), "[::]:9000");
        assert_eq!(page("[::1]", None).bind_argument(), "::1");
        // A word or a host name isn't taken for a command or looked up: the
        // page stays on this computer.
        for address in ["off", "new-key", "my-laptop"] {
            assert_eq!(page(address, None).ip(), None);
            assert_eq!(page(address, None).bind_argument(), "127.0.0.1");
        }
        // Each reads back as the same address and port.
        for (address, port) in [
            ("0.0.0.0", Some(9000)),
            ("127.0.0.1", None),
            ("::", Some(9000)),
            ("::1", None),
        ] {
            let bind = arko::remote::parse_bind(&page(address, port).bind_argument()).unwrap();
            assert_eq!(bind.ip, address.parse::<std::net::IpAddr>().unwrap());
            assert_eq!(bind.port, port);
        }
    }

    /// Earlier versions saved this computer only as the web page's
    /// address whether or not it was chosen. It follows the new default
    /// unless automatic sharing is on. Other explicit addresses stay.
    #[test]
    fn an_earlier_web_page_address_moves_to_the_new_default() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        let read = |text: &str| {
            std::fs::write(&path, text).unwrap();
            Prefs::load(&path).web_page
        };
        let page = read(
            r#"{"web_page": {"share_by_default": true, "address": "127.0.0.1", "port": 9000}}"#,
        );
        assert_eq!(page.address.as_deref(), Some("127.0.0.1"));
        assert_eq!(page.address(), "127.0.0.1");
        assert!(page.share_by_default);
        assert_eq!(page.port, Some(9000));
        assert_eq!(
            read(r#"{"web_page": {"address": "192.168.1.5"}}"#)
                .address
                .as_deref(),
            Some("192.168.1.5")
        );
        assert_eq!(read(r#"{"web_page": {"address": ""}}"#).address, None);
        // This computer only, chosen in this version, is kept as chosen.
        assert_eq!(
            read(r#"{"web_page": {"ip": "127.0.0.1"}}"#)
                .address
                .as_deref(),
            Some("127.0.0.1")
        );
    }

    /// The default address isn't written, a chosen one is, and either
    /// reads back as it was.
    #[test]
    fn web_page_addresses_round_trip() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        for address in [None, Some("127.0.0.1"), Some("0.0.0.0")] {
            let mut prefs = Prefs::default();
            prefs.web_page.address = address.map(str::to_string);
            let text = serde_json::to_string_pretty(&prefs).unwrap();
            assert_eq!(text.contains("\"ip\""), address.is_some(), "{text}");
            assert!(!text.contains("\"address\""), "{text}");
            write_atomically(&path, &text).unwrap();
            assert_eq!(Prefs::load(&path).web_page.address.as_deref(), address);
        }
    }

    #[test]
    fn zoom_steps_move_and_stop_at_the_ends() {
        assert_eq!(step_zoom(1.0, 1), 1.1);
        assert_eq!(step_zoom(1.0, -1), 0.9);
        assert_eq!(step_zoom(2.0, 1), 2.0);
        assert_eq!(step_zoom(0.5, -1), 0.5);
        // Between steps, the next one in that direction.
        assert_eq!(step_zoom(1.17, 1), 1.25);
        assert_eq!(step_zoom(1.17, -1), 1.1);
    }

    #[test]
    fn out_of_range_values_are_held_in_range() {
        let mut prefs = Prefs::default();
        prefs.appearance.zoom = 9.;
        prefs.sidebar.width = 2.;
        prefs.appearance.ui_font.size = f32::NAN;
        assert_eq!(prefs.appearance.zoom(), 2.0);
        assert_eq!(prefs.sidebar.width(), MIN_SIDEBAR_WIDTH);
        assert_eq!(prefs.appearance.ui_font.size(), DEFAULT_UI_SIZE);
    }

    #[gpui_kit::test]
    async fn changes_are_saved_once_after_a_pause(cx: &mut gpui_kit::TestAppContext) {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        cx.update(|cx| init(PrefsStore::new(Prefs::default(), Some(path.clone())), cx));
        cx.update(|cx| update(cx, |prefs| prefs.sidebar.width = 20.));
        cx.update(|cx| update(cx, |prefs| prefs.sidebar.width = 21.));
        cx.run_until_parked();
        assert!(!path.exists(), "saved before the pause");
        cx.executor().advance_clock(SAVE_DELAY);
        cx.run_until_parked();
        assert_eq!(Prefs::load(&path).sidebar.width, 21.);
    }

    #[gpui_kit::test]
    fn an_update_that_changes_nothing_is_not_announced(cx: &mut gpui_kit::TestAppContext) {
        cx.update(|cx| init(PrefsStore::new(Prefs::default(), None), cx));
        let heard = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            let heard = heard.clone();
            cx.observe_global::<PrefsStore>(move |_| heard.set(heard.get() + 1))
                .detach();
        });
        cx.update(|cx| update(cx, |prefs| prefs.sidebar.open = true));
        assert_eq!(heard.get(), 0);
        cx.update(|cx| update(cx, |prefs| prefs.sidebar.open = false));
        assert_eq!(heard.get(), 1);
    }
}
