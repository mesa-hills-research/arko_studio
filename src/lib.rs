//! Arko Studio: the desktop app of the Arko agent, on GPUI and GPUI Kit.
//!
//! The interface talks to Arko's engine through [`backend::Backend`]. The
//! binary (`src/main.rs`) sets up logging, reads a [`Startup`] and opens the
//! first window after [`init`].

pub mod appearance;
pub mod assets;
pub mod backend;
mod bundled_themes;
pub mod chrome;
pub mod fonts;
pub mod layout;
pub mod menus;
pub mod paths;
pub mod prefs;
pub mod session_view;
pub mod settings_page;
pub mod sidebar;
pub mod status_bar;
pub mod theme_picker;
pub mod title_bar;
pub mod workers_panel;
pub mod workspace;

#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;
use std::thread::JoinHandle;

use gpui_kit::component::ThemeConfig;
use gpui_kit::{App, QuitMode};

use crate::backend::{Backend, FakeBackend, SharedBackend};
use crate::prefs::{Prefs, PrefsStore};

/// The name people see in menus, window titles and the About dialog.
pub const APP_NAME: &str = "Arko Studio";

/// The reverse-DNS app id: the Linux `app_id` and desktop file name, the
/// Windows `AppUserModelID` and, once bundled, the macOS bundle id.
pub const APP_ID: &str = "com.mesahillsresearch.ArkoStudio";

pub const COPYRIGHT: &str = "Copyright © 2026 Luke Erickson";

/// What the app reads before its first window: the preferences, the themes
/// and the fonts, and the backend it runs on.
pub struct Startup {
    prefs: PrefsStore,
    themes: Themes,
    bundled_fonts: Vec<Cow<'static, [u8]>>,
    added_fonts: Vec<Cow<'static, [u8]>>,
    backend: Arc<dyn Backend>,
    /// The user's home folder.
    home: Option<PathBuf>,
}

enum Themes {
    Ready(Vec<ThemeConfig>),
    /// Parsed on a thread of their own while GPUI starts.
    Loading(JoinHandle<Vec<ThemeConfig>>),
}

impl Startup {
    /// Reads Arko's folder. The themes are parsed on another thread, which
    /// [`init`] waits for.
    pub fn load() -> Self {
        let arko = paths::arko_folder().unwrap_or_else(|| PathBuf::from("arko"));
        let prefs = Prefs::load(&paths::prefs_file(&arko));
        let themes_folder = paths::themes_folder(&arko);
        let themes = std::thread::Builder::new()
            .name("themes".into())
            .spawn(move || {
                appearance::merge(
                    appearance::parse_bundled(),
                    appearance::read_user_themes(&themes_folder),
                )
            })
            .map(Themes::Loading)
            .unwrap_or_else(|_| Themes::Ready(appearance::parse_bundled()));
        let added_fonts = fonts::added(&paths::fonts_folder(&arko));
        // The engine's backend replaces this one once arko is a library:
        // until then the sidebar lists sample sessions, kept in memory.
        let backend = Arc::new(
            FakeBackend::sample(&arko, std::time::SystemTime::now())
                .with_default_model(backend::last_model(&arko)),
        );
        Self {
            prefs: PrefsStore::new(prefs, Some(paths::prefs_file(&arko))),
            themes,
            bundled_fonts: fonts::bundled(),
            added_fonts,
            backend,
            home: session_view::home_from_env(),
        }
    }

    /// An app on `backend` with the default preferences, kept in memory,
    /// the bundled themes and no fonts of its own, for `/home/ada`: for
    /// tests, which then read the same on every machine.
    pub fn for_tests(backend: Arc<dyn Backend>) -> Self {
        Self {
            prefs: PrefsStore::new(Prefs::default(), None),
            themes: Themes::Ready(appearance::parse_bundled()),
            bundled_fonts: Vec::new(),
            added_fonts: Vec::new(),
            backend,
            home: Some(PathBuf::from("/home/ada")),
        }
    }

    /// Fonts to register beside the bundled ones (screenshot tests load
    /// the interface fonts this way).
    pub fn with_fonts(mut self, fonts: Vec<Cow<'static, [u8]>>) -> Self {
        self.bundled_fonts.extend(fonts);
        self
    }

    /// Starts from `prefs` instead.
    pub fn with_prefs(mut self, prefs: Prefs) -> Self {
        self.prefs = PrefsStore::new(prefs, None);
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
    cx.set_global(session_view::HomeFolder(startup.home));
    let themes = match startup.themes {
        Themes::Ready(themes) => themes,
        Themes::Loading(thread) => thread
            .join()
            .unwrap_or_else(|_| appearance::parse_bundled()),
    };
    appearance::init(themes, cx);
    sidebar::init(cx);
    settings_page::init(cx);
    // Before any title bar builds its menu bar.
    menus::init(cx);
}
