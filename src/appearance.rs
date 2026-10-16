//! What Settings › Appearance controls: the color theme (as the system is, or
//! always light or dark, each with its own theme), previews of themes, the
//! interface and code fonts, zoom, and motion.
//!
//! The themes are the 41 Arko Studio bundles plus the ones in
//! `<arko folder>/themes`, which the terminal app reads too. GPUI Kit's
//! theme registry holds them: a theme in the folder replaces a bundled one
//! with the same name, as in the terminal app, and a change to the folder
//! shows at once.

use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gpui_kit::component::{FontOverrides, Theme, ThemeConfig, ThemeRegistry, ThemeSet};
use gpui_kit::{App, Global, SharedString, WindowAppearance, px};

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

fn parse(file: &str, text: &str) -> Vec<ThemeConfig> {
    match serde_json::from_str::<ThemeSet>(text) {
        Ok(set) => set.themes,
        Err(error) => {
            tracing::warn!("ignored the theme file {file}: {error}");
            Vec::new()
        }
    }
}

/// The light or the dark themes the user can choose, by name: the bundled
/// ones and the user's, sorted without case.
pub fn theme_names(dark: bool, cx: &App) -> Vec<SharedString> {
    let kit_defaults = &cx.global::<KitDefaults>().0;
    let mut names: Vec<SharedString> = ThemeRegistry::global(cx)
        .themes()
        .values()
        .filter(|theme| theme.mode.is_dark() == dark)
        .filter(|theme| !kit_defaults.iter().any(|kit| Rc::ptr_eq(kit, theme)))
        .map(|theme| theme.name.clone())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// The theme named `name`, compared without case when no name matches
/// exactly.
pub fn find(name: &str, cx: &App) -> Option<Rc<ThemeConfig>> {
    ThemeRegistry::global(cx).theme(name).cloned()
}

/// GPUI Kit's own light and dark themes, which stand in when a theme is
/// missing and stay out of the theme lists.
struct KitDefaults(Vec<Rc<ThemeConfig>>);

impl Global for KitDefaults {}

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
#[derive(Default, Clone)]
struct Applied {
    theme: Option<Rc<ThemeConfig>>,
    fonts: Option<FontOverrides>,
    motion: Option<Motion>,
}

impl Global for Applied {}

/// Registers the bundled themes, watches the user's `themes_folder` when
/// given, and applies the preferences, then follows their changes.
pub fn init(themes: Vec<ThemeConfig>, themes_folder: Option<PathBuf>, cx: &mut App) {
    let registry = ThemeRegistry::global_mut(cx);
    let kit_defaults = vec![
        registry.default_light_theme().clone(),
        registry.default_dark_theme().clone(),
    ];
    registry.add_bundled_themes(themes);
    cx.set_global(KitDefaults(kit_defaults));
    let dark = matches!(
        cx.window_appearance(),
        WindowAppearance::Dark | WindowAppearance::VibrantDark
    );
    cx.set_global(SystemAppearance { dark });
    cx.set_global(Preview::default());
    cx.set_global(Applied::default());
    apply(cx);
    cx.observe_global::<PrefsStore>(apply).detach();
    // A theme file that changes in the folder may be the one in force.
    cx.observe_global::<ThemeRegistry>(apply).detach();
    if let Some(folder) = themes_folder
        && let Err(error) = ThemeRegistry::watch_dir(folder, cx, apply)
    {
        tracing::error!("couldn't read the themes folder: {error}");
    }
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
    let theme = name.and_then(|name| find(name, cx));
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
    find(name, cx)
        .or_else(|| find(fallback, cx))
        .unwrap_or_else(|| {
            let kit_defaults = &cx.global::<KitDefaults>().0;
            kit_defaults[usize::from(dark)].clone()
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
    let fonts = font_overrides(cx);
    let motion = prefs::get(cx).appearance.motion;
    let last = cx.global::<Applied>().clone();

    // A theme file reloaded from the folder is a new config under the same
    // name, so the configs are compared, not their names.
    let theme_changed = !last
        .theme
        .as_ref()
        .is_some_and(|last| Rc::ptr_eq(last, &theme));
    let fonts_changed = last.fonts.as_ref() != Some(&fonts);
    if theme_changed || fonts_changed {
        // The fonts outlast every theme applied after them.
        Theme::update(cx, |t| {
            if fonts_changed {
                t.set_font_overrides(fonts.clone());
            }
            if theme_changed {
                t.apply_config(&theme);
            }
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
        theme: Some(theme),
        fonts: Some(fonts),
        motion: Some(motion),
    });
}

/// The fonts and sizes the preferences and zoom ask for. A family the text
/// system can't draw is left to the theme.
fn font_overrides(cx: &mut App) -> FontOverrides {
    let appearance = prefs::get(cx).appearance.clone();
    let zoom = appearance.zoom();
    let mut fonts = FontOverrides::new()
        .with_font_size(px((appearance.ui_font.size() * zoom).round()))
        .with_mono_font_size(px((appearance.code_font.size() * zoom).round()));
    if fonts::is_available(&appearance.ui_font.family, cx) {
        fonts = fonts.with_font_family(appearance.ui_font.family.clone());
    }
    if fonts::is_available(&appearance.code_font.family, cx) {
        fonts = fonts.with_mono_font_family(appearance.code_font.family.clone());
    }
    fonts
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

    #[test]
    fn every_bundled_theme_parses_and_the_default_pair_is_there() {
        let themes = parse_bundled();
        assert!(themes.len() >= 41, "{} themes", themes.len());
        let mode = |name: &str| {
            themes
                .iter()
                .find(|theme| theme.name == name)
                .map(|theme| theme.mode.is_dark())
        };
        assert_eq!(mode(prefs::DEFAULT_LIGHT_THEME), Some(false));
        assert_eq!(mode(prefs::DEFAULT_DARK_THEME), Some(true));
    }
}
