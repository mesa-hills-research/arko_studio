//! What Settings › Appearance controls: the color theme (as the system is, or
//! always light or dark, each with its own theme), previews of themes, the
//! interface and code fonts, zoom, and motion.
//!
//! The themes are the 41 Arko Studio bundles plus the ones in
//! `<arko folder>/themes`, which the terminal app reads too. A theme there
//! replaces a bundled one with the same name, as in the terminal app.

use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{Theme, ThemeConfig, ThemeMode, ThemeSet};
use gpui_kit::{App, Global, Pixels, SharedString, WindowAppearance, px};

use crate::fonts;
use crate::prefs::{self, Motion, PrefsStore};

/// How long the selection in a theme list rests before the theme shows.
pub const PREVIEW_REST: Duration = Duration::from_millis(350);

/// The bundled themes, parsed. Each file holds a family's light and dark
/// variants.
pub fn parse_bundled() -> Vec<ThemeConfig> {
    crate::bundled_themes::BUNDLED
        .iter()
        .flat_map(|(file, text)| parse(file, text))
        .collect()
}

/// The themes in `folder`'s JSON files. Files that don't parse are logged
/// and skipped. Blocks: call it off the UI thread.
pub fn read_user_themes(folder: &Path) -> Vec<ThemeConfig> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .collect();
    paths.sort();
    paths
        .into_iter()
        .flat_map(|path| match std::fs::read_to_string(&path) {
            Ok(text) => parse(&path.display().to_string(), &text),
            Err(error) => {
                tracing::warn!("couldn't read {}: {error}", path.display());
                Vec::new()
            }
        })
        .collect()
}

fn parse(file: &str, text: &str) -> Vec<ThemeConfig> {
    match serde_json::from_str::<ThemeSet>(text) {
        Ok(set) => set.themes,
        Err(error) => {
            tracing::warn!("ignored the theme file {file}: {error}");
            Vec::new()
        }
    }
}

/// The bundled themes with the user's: a user theme replaces a bundled one
/// with the same name, compared without case, and the rest are added.
///
/// Workaround until GPUI Kit's theme registry keeps bundled themes across
/// reloads of the user's folder: the app keeps this catalog itself, and
/// picks up changes to the folder at its next start.
pub fn merge(bundled: Vec<ThemeConfig>, user: Vec<ThemeConfig>) -> Vec<ThemeConfig> {
    let mut themes = bundled;
    for theme in user {
        match themes
            .iter_mut()
            .find(|existing| existing.name.eq_ignore_ascii_case(&theme.name))
        {
            Some(existing) => *existing = theme,
            None => themes.push(theme),
        }
    }
    themes
}

/// Every theme the user can choose.
pub struct Catalog {
    themes: Vec<Rc<ThemeConfig>>,
}

impl Global for Catalog {}

impl Catalog {
    pub fn new(themes: Vec<ThemeConfig>) -> Self {
        let mut themes: Vec<Rc<ThemeConfig>> = themes.into_iter().map(Rc::new).collect();
        themes.sort_by_key(|theme| theme.name.to_lowercase());
        Self { themes }
    }

    /// The theme named `name`, compared without case.
    pub fn find(&self, name: &str) -> Option<&Rc<ThemeConfig>> {
        self.themes
            .iter()
            .find(|theme| theme.name.eq_ignore_ascii_case(name))
    }

    /// The light or the dark themes, by name.
    pub fn of_mode(&self, dark: bool) -> impl Iterator<Item = &Rc<ThemeConfig>> {
        self.themes
            .iter()
            .filter(move |theme| theme.mode.is_dark() == dark)
    }
}

/// The system's light or dark appearance, as the windows last reported it.
pub struct SystemAppearance {
    dark: bool,
}

impl Global for SystemAppearance {}

/// The theme shown in place of the chosen one while the user rests on it
/// in a theme list.
#[derive(Default)]
pub struct Preview {
    theme: Option<Rc<ThemeConfig>>,
}

impl Global for Preview {}

/// What was applied last, so that a change elsewhere in the preferences
/// leaves the theme alone.
#[derive(Default, PartialEq, Clone)]
struct Applied {
    theme: Option<SharedString>,
    fonts: Option<FontsInForce>,
    motion: Option<Motion>,
}

impl Global for Applied {}

/// The fonts the theme carries, as the preferences and zoom make them.
#[derive(Debug, Clone, PartialEq)]
struct FontsInForce {
    /// `None` keeps the theme's own (the family isn't installed).
    ui_family: Option<SharedString>,
    ui_size: Pixels,
    code_family: Option<SharedString>,
    code_size: Pixels,
}

/// Installs the themes and applies the preferences, then follows their
/// changes.
pub fn init(themes: Vec<ThemeConfig>, cx: &mut App) {
    cx.set_global(Catalog::new(themes));
    let dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    cx.set_global(SystemAppearance { dark });
    cx.set_global(Preview::default());
    cx.set_global(Applied::default());
    apply(cx);
    cx.observe_global::<PrefsStore>(apply).detach();
}

/// Records the system's appearance, which windows report as it changes.
pub fn set_system_appearance(appearance: WindowAppearance, cx: &mut App) {
    let dark = matches!(
        appearance,
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    if cx.global::<SystemAppearance>().dark != dark {
        cx.global_mut::<SystemAppearance>().dark = dark;
        apply(cx);
    }
}

/// Shows `name` in place of the chosen theme, or the chosen one again.
pub fn preview(name: Option<&str>, cx: &mut App) {
    let theme = name.and_then(|name| cx.global::<Catalog>().find(name).cloned());
    cx.global_mut::<Preview>().theme = theme;
    apply(cx);
}

/// The theme being previewed, if any.
pub fn previewing(cx: &App) -> Option<SharedString> {
    cx.global::<Preview>()
        .theme
        .as_ref()
        .map(|theme| theme.name.clone())
}

/// Whether the interface shows a dark theme as the preferences stand
/// (previews aside).
pub fn is_dark(cx: &App) -> bool {
    match prefs::get(cx).appearance.theme_mode {
        prefs::ThemeMode::System => cx.global::<SystemAppearance>().dark,
        prefs::ThemeMode::Light => false,
        prefs::ThemeMode::Dark => true,
    }
}

/// The theme the preferences choose for the current appearance, the
/// default pair standing in for a theme that's gone.
pub fn chosen_theme(cx: &App) -> Rc<ThemeConfig> {
    let appearance = &prefs::get(cx).appearance;
    let dark = is_dark(cx);
    let (name, fallback) = if dark {
        (&appearance.dark_theme, prefs::DEFAULT_DARK_THEME)
    } else {
        (&appearance.light_theme, prefs::DEFAULT_LIGHT_THEME)
    };
    let catalog = cx.global::<Catalog>();
    catalog
        .find(name)
        .or_else(|| catalog.find(fallback))
        .cloned()
        .unwrap_or_else(|| {
            let mode = if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            };
            Rc::new(ThemeConfig {
                mode,
                ..ThemeConfig::default()
            })
        })
}

/// Applies the theme, fonts, zoom and motion the preferences and any
/// preview ask for, touching only what changed.
pub fn apply(cx: &mut App) {
    let theme = cx
        .global::<Preview>()
        .theme
        .clone()
        .unwrap_or_else(|| chosen_theme(cx));
    let fonts = fonts_in_force(cx);
    let motion = prefs::get(cx).appearance.motion;
    let last = cx.global::<Applied>().clone();

    if last.theme.as_ref() != Some(&theme.name) {
        Theme::update(cx, |t| t.apply_config(&theme));
    }
    // Applying a theme puts its own fonts back, so they go on again after
    // every theme change. Workaround until GPUI Kit's font overrides, which
    // survive theme changes: then they're set once, with the zoom.
    if last.theme.as_ref() != Some(&theme.name) || last.fonts.as_ref() != Some(&fonts) {
        Theme::update(cx, |t| {
            if let Some(family) = &fonts.ui_family {
                t.font_family = family.clone();
            }
            t.font_size = fonts.ui_size;
            if let Some(family) = &fonts.code_family {
                t.mono_font_family = family.clone();
            }
            t.mono_font_size = fonts.code_size;
        });
    }
    if last.motion != Some(motion) {
        match motion {
            Motion::System => gpui_kit::base::apply_system_reduce_motion(cx),
            Motion::On => cx.set_reduce_motion(false),
            Motion::Off => cx.set_reduce_motion(true),
        }
    }
    cx.set_global(Applied {
        theme: Some(theme.name.clone()),
        fonts: Some(fonts),
        motion: Some(motion),
    });
}

fn fonts_in_force(cx: &mut App) -> FontsInForce {
    let appearance = prefs::get(cx).appearance.clone();
    let zoom = appearance.zoom();
    let family = |name: &str, cx: &mut App| {
        fonts::is_available(name, cx).then(|| SharedString::from(name.to_string()))
    };
    FontsInForce {
        ui_family: family(&appearance.ui_font.family, cx),
        ui_size: px((appearance.ui_font.size() * zoom).round()),
        code_family: family(&appearance.code_font.family, cx),
        code_size: px((appearance.code_font.size() * zoom).round()),
    }
}

/// Zooms in (`direction` > 0), out (< 0) or back to 100 % (0).
pub fn zoom(direction: i32, cx: &mut App) {
    prefs::update(cx, |prefs| {
        prefs.appearance.zoom = if direction == 0 {
            1.0
        } else {
            prefs::step_zoom(prefs.appearance.zoom(), direction)
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(name: &str, dark: bool) -> ThemeConfig {
        ThemeConfig {
            name: name.to_string().into(),
            mode: if dark {
                ThemeMode::Dark
            } else {
                ThemeMode::Light
            },
            ..ThemeConfig::default()
        }
    }

    #[test]
    fn every_bundled_theme_parses_and_the_default_pair_is_there() {
        let themes = parse_bundled();
        assert!(themes.len() >= 41, "{} themes", themes.len());
        let catalog = Catalog::new(themes);
        assert!(
            !catalog
                .find(prefs::DEFAULT_LIGHT_THEME)
                .unwrap()
                .mode
                .is_dark()
        );
        assert!(
            catalog
                .find(prefs::DEFAULT_DARK_THEME)
                .unwrap()
                .mode
                .is_dark()
        );
    }

    #[test]
    fn a_user_theme_replaces_a_bundled_one_of_the_same_name() {
        let mut replacement = theme("ALDUIN", true);
        replacement.radius = Some(2);
        let merged = merge(
            vec![theme("Alduin", true), theme("Fern", false)],
            vec![replacement, theme("Mine", false)],
        );
        let names: Vec<_> = merged.iter().map(|theme| theme.name.to_string()).collect();
        assert_eq!(names, ["ALDUIN", "Fern", "Mine"]);
        assert_eq!(merged[0].radius, Some(2));
    }

    #[test]
    fn user_theme_files_that_dont_parse_are_skipped() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("broken.json"), "{").unwrap();
        std::fs::write(
            folder.path().join("mine.json"),
            r#"{ "name": "Mine", "themes": [ { "name": "Mine Dark", "mode": "dark" } ] }"#,
        )
        .unwrap();
        std::fs::write(folder.path().join("notes.txt"), "x").unwrap();
        let themes = read_user_themes(folder.path());
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].name, "Mine Dark");
        assert!(read_user_themes(&folder.path().join("missing")).is_empty());
    }

    #[test]
    fn the_catalog_lists_by_mode_and_name() {
        let catalog = Catalog::new(vec![
            theme("b dark", true),
            theme("A light", false),
            theme("a dark", true),
        ]);
        let dark: Vec<_> = catalog.of_mode(true).map(|t| t.name.to_string()).collect();
        assert_eq!(dark, ["a dark", "b dark"]);
        assert_eq!(catalog.find("A LIGHT").unwrap().name, "A light");
    }
}
