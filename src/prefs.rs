//! Arko Studio's preferences, kept in `<arko folder>/studio.json`.
//!
//! Every field has a default and unknown fields are ignored, so an older
//! file loads in a newer app and a newer file in an older one. Changes are
//! written after a short pause, on a background thread, through a temporary
//! file renamed over the old one, so a crash never leaves half a file.

use std::path::{Path, PathBuf};
use std::time::Duration;

use gpui_kit::{App, BorrowAppContext as _, Global, Task};
use serde::{Deserialize, Serialize};

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
        }
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
}

impl Default for SessionPrefs {
    fn default() -> Self {
        Self {
            ask_before_quit: true,
            ask_before_switch: true,
        }
    }
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
    /// Reads `path`. A missing file gives the defaults, and so does one that
    /// can't be read, which is logged and left alone until the next save.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_else(|error| {
                tracing::warn!("ignored {}: {error}", path.display());
                Self::default()
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                tracing::warn!("couldn't read {}: {error}", path.display());
                Self::default()
            }
        }
    }
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
}

impl Global for PrefsStore {}

impl PrefsStore {
    pub fn new(prefs: Prefs, path: Option<PathBuf>) -> Self {
        Self { prefs, path }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
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
    match serde_json::to_string_pretty(&store.prefs) {
        Ok(text) => Some((path, text, saver.changes)),
        Err(error) => {
            tracing::error!("couldn't write the preferences: {error}");
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
                tracing::error!("couldn't save {}: {error}", path.display());
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
            Err(error) => tracing::error!("couldn't save {}: {error}", path.display()),
        }
    });
    cx.global_mut::<Saver>().pending = Some(task);
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_broken_file_gives_the_defaults() {
        let folder = tempfile::tempdir().unwrap();
        let path = folder.path().join("studio.json");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Prefs::load(&path), Prefs::default());
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
