//! Arko Studio: the desktop app of the Arko agent, on GPUI and GPUI Kit.
//!
//! The interface talks to Arko's engine through [`backend::Backend`], and
//! runs each open session on the engine's threads ([`live`]). The binary
//! (`src/main.rs`) sets up logging, reads a [`Startup`] and opens the first
//! window after [`init`].

pub mod about;
pub mod appearance;
pub mod assets;
pub mod backend;
mod bundled_themes;
pub mod cache_reminders;
pub mod cache_timer;
pub mod chrome;
pub mod commands;
pub mod composer;
pub mod context_ring;
pub mod drafts;
pub mod editing;
pub mod export;
pub mod figures;
pub mod fonts;
pub mod history;
pub mod layout;
pub mod live;
pub mod mentions;
pub mod menus;
pub mod notify;
pub mod palette;
pub mod paths;
pub mod prefs;
pub mod prompts;
pub mod quit;
pub mod session_info;
pub mod session_view;
pub mod settings_page;
pub mod shortcuts;
pub mod sidebar;
pub mod status_bar;
pub mod theme_picker;
pub mod title_bar;
pub mod transcript;
pub mod web_page;
pub mod workers;
pub mod workers_panel;
pub mod workspace;
pub mod writing;

#[cfg(test)]
mod composer_tests;
#[cfg(test)]
mod engine_tests;
#[cfg(test)]
mod status_tests;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use gpui_kit::component::ThemeConfig;
use gpui_kit::{App, QuitMode};

use crate::appearance::ThemesFolder;
use crate::backend::{ArkoBackend, Backend, SharedBackend};
use crate::prefs::{Prefs, PrefsStore};

/// The name people see in menus, window titles and the About dialog.
pub const APP_NAME: &str = "Arko Studio";

/// The reverse-DNS app id: the Linux `app_id` and desktop file name, the
/// Windows `AppUserModelID` and, once bundled, the macOS bundle id.
pub const APP_ID: &str = "com.mesahillsresearch.ArkoStudio";

pub const COPYRIGHT: &str = "Copyright © 2026 Luke Erickson";

/// What the app reads before its first window: the preferences, the drafts,
/// the themes and the fonts, and the backend it runs on.
pub struct Startup {
    prefs: PrefsStore,
    themes: Themes,
    bundled_fonts: Vec<Cow<'static, [u8]>>,
    added_fonts: Vec<Cow<'static, [u8]>>,
    backend: Arc<dyn Backend>,
    /// The user's home folder.
    home: Option<PathBuf>,
    /// Where release builds write their log.
    logs: Option<PathBuf>,
    /// The user's themes, watched while the app runs.
    themes_folder: Option<ThemesFolder>,
    drafts: BTreeMap<String, String>,
    /// Arko's folder, where the drafts, the input history and the writing
    /// aids' vocabulary are kept. `None` keeps them in memory (tests).
    arko: Option<PathBuf>,
}

enum Themes {
    Ready(Vec<ThemeConfig>),
    /// Parsed on a thread of their own while GPUI starts.
    Loading(JoinHandle<Vec<ThemeConfig>>),
}

impl Startup {
    /// Reads Arko's folder. The bundled themes are parsed on another
    /// thread, which [`init`] waits for.
    pub fn load() -> Self {
        let arko = paths::arko_folder().unwrap_or_else(|| PathBuf::from("arko"));
        let prefs = PrefsStore::load(&paths::prefs_file(&arko));
        let themes = std::thread::Builder::new()
            .name("themes".into())
            .spawn(appearance::parse_bundled)
            .map(Themes::Loading)
            .unwrap_or_else(|_| Themes::Ready(appearance::parse_bundled()));
        let added_fonts = fonts::added(&paths::fonts_folder(&arko));
        Self {
            prefs,
            themes,
            bundled_fonts: fonts::bundled(),
            added_fonts,
            backend: Arc::new(ArkoBackend::new(&arko)),
            home: session_view::home_from_env(),
            logs: paths::logs_folder(),
            themes_folder: Some(ThemesFolder::Watch(paths::themes_folder(&arko))),
            drafts: drafts::load(&drafts::file(&arko)),
            arko: Some(arko),
        }
    }

    /// An app on `backend` with the default preferences but motion off,
    /// kept in memory, the bundled themes and no fonts of its own, for
    /// `/home/ada`: for tests, which then read the same on every machine.
    pub fn for_tests(backend: Arc<dyn Backend>) -> Self {
        // Animations play in real time. With motion off they go straight to
        // their end, so a test clicks where the last frame drew a dialog.
        let mut prefs = Prefs::default();
        prefs.appearance.motion = prefs::Motion::Off;
        Self {
            prefs: PrefsStore::new(prefs, None),
            themes: Themes::Ready(appearance::parse_bundled()),
            bundled_fonts: Vec::new(),
            added_fonts: Vec::new(),
            backend,
            home: Some(PathBuf::from("/home/ada")),
            logs: Some(PathBuf::from("/home/ada/.local/state").join(APP_ID)),
            themes_folder: None,
            drafts: BTreeMap::new(),
            arko: None,
        }
    }

    /// Fonts to register beside the bundled ones (screenshot tests load
    /// the interface fonts this way).
    pub fn with_fonts(mut self, fonts: Vec<Cow<'static, [u8]>>) -> Self {
        self.bundled_fonts.extend(fonts);
        self
    }

    /// Reads the user's themes in `folder`, once.
    pub fn with_themes_folder(mut self, folder: PathBuf) -> Self {
        self.themes_folder = Some(ThemesFolder::ReadOnce(folder));
        self
    }

    /// Starts from `prefs` instead.
    pub fn with_prefs(mut self, prefs: Prefs) -> Self {
        self.prefs = PrefsStore::new(prefs, None);
        self
    }

    /// Starts with these drafts (tests).
    pub fn with_drafts(mut self, drafts: BTreeMap<String, String>) -> Self {
        self.drafts = drafts;
        self
    }
}

/// Sets the app up, before its first window.
pub fn init(startup: Startup, cx: &mut App) {
    gpui_kit::init(cx);
    cx.set_app_identity(APP_ID, APP_NAME);
    // Windows and Linux quit with the last window. macOS keeps the app
    // running, as Mac apps do, until Quit.
    cx.set_quit_mode(QuitMode::Default);
    fonts::init(startup.bundled_fonts, startup.added_fonts, cx);
    prefs::init(startup.prefs, cx);
    cx.set_global(SharedBackend(startup.backend));
    backend::follow_prefs(cx);
    cx.set_global(session_view::HomeFolder(startup.home));
    cx.set_global(paths::LogsFolder(startup.logs));
    live::init(cx);
    writing::init(startup.arko.as_deref(), cx);
    history::init(startup.arko.clone(), cx);
    drafts::init(
        startup.drafts,
        startup.arko.as_deref().map(drafts::file),
        cx,
    );
    let themes = match startup.themes {
        Themes::Ready(themes) => themes,
        Themes::Loading(thread) => thread
            .join()
            .unwrap_or_else(|_| appearance::parse_bundled()),
    };
    appearance::init(themes, startup.themes_folder, cx);
    // Before any title bar builds its menu bar.
    menus::init(cx);
}
