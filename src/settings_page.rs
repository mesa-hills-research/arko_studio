//! Settings (Mod+,): a page in the window, in place of the transcript, with
//! the app's preferences in pages. Esc or the close button goes back.
//!
//! The session's own live settings are in its info popover; these are the
//! app's preferences and the defaults for new sessions.

use gpui_kit::component::font_picker::{
    FontPicker, FontPickerEvent, FontPickerState, FontSettings,
};
use gpui_kit::component::setting::{
    NumberFieldOptions, SelectIndex, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariant, ButtonVariants as _},
    group_box::GroupBoxVariant,
    h_flex, v_flex,
};
use gpui_kit::*;
use serde::Serialize;

use crate::prefs::{self, Keymap, Motion, SendKey, Suggestions, ThemeMode};
use crate::theme_picker::ThemePicker;
use crate::{fonts, menus, paths};

mod accounts;
mod extensions;
mod fields;
mod openrouter;
mod profiles;
mod sessions;
mod web;

pub use accounts::AccountsPage;
pub use extensions::ExtensionsPage;
pub use profiles::ProfilesPage;
pub use sessions::SessionsPage;
pub use web::WebPageSettings;

actions!(settings_page, [CloseSettings]);

/// The key context of the Settings page.
pub const CONTEXT: &str = "SettingsPage";

/// The pages, in their order in the page list.
pub const PAGES: [&str; 9] = [
    "Appearance",
    "Input",
    "Models & accounts",
    "Worker profiles",
    "Sessions",
    "Notifications",
    "Web page",
    "Extensions",
    "Advanced",
];

/// Where each page is in [`PAGES`].
pub const APPEARANCE_PAGE: usize = 0;
pub const INPUT_PAGE: usize = 1;
pub const ACCOUNTS_PAGE: usize = 2;
pub const PROFILES_PAGE: usize = 3;
pub const SESSIONS_PAGE: usize = 4;
pub const NOTIFICATIONS_PAGE: usize = 5;
pub const WEB_PAGE_PAGE: usize = 6;
pub const EXTENSIONS_PAGE: usize = 7;
pub const ADVANCED_PAGE: usize = 8;

/// Asks the window to go back from Settings.
pub struct CloseRequested;

impl EventEmitter<CloseRequested> for SettingsPage {}

pub struct SettingsPage {
    focus_handle: FocusHandle,
    light_theme: Entity<ThemePicker>,
    dark_theme: Entity<ThemePicker>,
    /// The font pickers, once the fonts they list are read.
    ui_font: Option<Entity<FontPickerState>>,
    code_font: Option<Entity<FontPickerState>>,
    /// The catalog the font pickers list ([`fonts::picker_catalog_version`]).
    fonts_version: u64,
    _font_pickers: Vec<Subscription>,
    accounts: Entity<AccountsPage>,
    profiles: Entity<ProfilesPage>,
    sessions: Entity<SessionsPage>,
    web_page: Entity<WebPageSettings>,
    extensions: Entity<ExtensionsPage>,
    /// How many times Models & accounts and Extensions changed: the keys of
    /// their groups, which draw again only then.
    accounts_revision: u64,
    extensions_revision: u64,
    /// The page to show, set from outside (the command palette), and how
    /// many times it was: the page list starts afresh on it each time.
    page: usize,
    shown: usize,
    /// The preferences the pages show, as they were drawn.
    shown_prefs: prefs::Prefs,
    /// How many times the pages were built (tests).
    #[cfg(test)]
    builds: usize,
    _subscriptions: Vec<Subscription>,
}

/// Whether the preferences Settings shows changed since `seen`, which then
/// takes them: a page draws again only then.
fn shown_changed(seen: &mut prefs::Prefs, cx: &App) -> bool {
    let shown = shown_prefs(prefs::get(cx));
    if shown == *seen {
        return false;
    }
    *seen = shown;
    true
}

/// A Settings group's key, which it draws again only when it changes: a
/// hash of what its rows show. `title` tells apart the groups a page shows
/// in turn at one place.
fn group_key(title: &str, shown: &impl Serialize) -> u64 {
    use std::hash::{DefaultHasher, Hash as _, Hasher};

    /// Hashes the text as it's written.
    struct Hashing(DefaultHasher);

    impl std::io::Write for Hashing {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            Hasher::write(&mut self.0, bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let mut hashing = Hashing(DefaultHasher::new());
    title.hash(&mut hashing.0);
    // Plain values, which serialize without failing.
    serde_json::to_writer(&mut hashing, shown).ok();
    hashing.0.finish()
}

/// The preferences Settings shows: all but where the window is and how
/// wide its panes are, which a resize or a drag changes many times a second.
fn shown_prefs(prefs: &prefs::Prefs) -> prefs::Prefs {
    prefs::Prefs {
        window: None,
        sidebar: prefs::SidebarPrefs::default(),
        workers_panel: prefs::PanelPrefs::default(),
        ..prefs.clone()
    }
}

impl SettingsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let accounts = cx.new(|cx| AccountsPage::new(window, cx));
        let profiles = cx.new(|cx| ProfilesPage::new(window, cx));
        let sessions = cx.new(|cx| SessionsPage::new(window, cx));
        let web_page = cx.new(|cx| WebPageSettings::new(window, cx));
        let extensions = cx.new(ExtensionsPage::new);
        // The fonts the pickers list are read in the background, the first
        // time: the pickers show once they're there.
        fonts::load_picker_catalog(cx);
        let subscriptions = vec![
            cx.observe(&accounts, |this, _, cx| {
                this.accounts_revision += 1;
                cx.notify()
            }),
            cx.observe(&profiles, |_, _, cx| cx.notify()),
            cx.observe(&sessions, |_, _, cx| cx.notify()),
            cx.observe(&web_page, |_, _, cx| cx.notify()),
            cx.observe(&extensions, |this, _, cx| {
                this.extensions_revision += 1;
                cx.notify()
            }),
            cx.observe_global_in::<fonts::PickerCatalog>(window, |this, window, cx| {
                this.make_font_pickers(window, cx)
            }),
            // Settings shows the preferences as they change, here or elsewhere
            // (a zoom shortcut, another window), but not the window's
            // placement and its panes' widths, which it doesn't show.
            cx.observe_global::<prefs::PrefsStore>(|this, cx| {
                if shown_changed(&mut this.shown_prefs, cx) {
                    cx.notify();
                }
            }),
        ];
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            light_theme: cx.new(|cx| ThemePicker::new(false, window, cx)),
            dark_theme: cx.new(|cx| ThemePicker::new(true, window, cx)),
            ui_font: None,
            code_font: None,
            fonts_version: 0,
            _font_pickers: Vec::new(),
            accounts,
            profiles,
            sessions,
            web_page,
            extensions,
            accounts_revision: 0,
            extensions_revision: 0,
            page: 0,
            shown: 0,
            shown_prefs: shown_prefs(prefs::get(cx)),
            #[cfg(test)]
            builds: 0,
            _subscriptions: subscriptions,
        };
        this.make_font_pickers(window, cx);
        this
    }

    /// Makes the font pickers, once the fonts they list are read, and again
    /// when they change (a font file added).
    fn make_font_pickers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let version = fonts::picker_catalog_version(cx);
        if self.ui_font.is_some() && self.fonts_version == version {
            return;
        }
        let Some(catalog) = fonts::picker_catalog(cx) else {
            return;
        };
        self.fonts_version = version;
        let appearance = prefs::get(cx).appearance.clone();
        let mut picker = |choice: &prefs::FontChoice, monospace: bool| {
            let settings = FontSettings::new(choice.family.clone()).with_size(px(choice.size()));
            let catalog = catalog.clone();
            cx.new(|cx| {
                FontPickerState::new(window, cx)
                    .catalog(catalog)
                    .default_settings(settings)
                    .monospace_only(monospace)
            })
        };
        let ui_font = picker(&appearance.ui_font, false);
        let code_font = picker(&appearance.code_font, true);
        // The pickers made before, if any, go with what they sent.
        self._font_pickers = vec![
            cx.subscribe(&ui_font, |_, _, event: &FontPickerEvent, cx| {
                let FontPickerEvent::Change(font) = event else {
                    return;
                };
                let choice = prefs::FontChoice::new(font.family(), f32::from(font.size()));
                prefs::update(cx, |prefs| prefs.appearance.ui_font = choice);
            }),
            cx.subscribe(&code_font, |_, _, event: &FontPickerEvent, cx| {
                let FontPickerEvent::Change(font) = event else {
                    return;
                };
                let choice = prefs::FontChoice::new(font.family(), f32::from(font.size()));
                prefs::update(cx, |prefs| prefs.appearance.code_font = choice);
            }),
        ];
        self.ui_font = Some(ui_font);
        self.code_font = Some(code_font);
        cx.notify();
    }

    /// The font pickers, `None` while the fonts they list are read.
    pub fn font_pickers(&self) -> Option<(&Entity<FontPickerState>, &Entity<FontPickerState>)> {
        Some((self.ui_font.as_ref()?, self.code_font.as_ref()?))
    }

    /// Shows the page at `page` in [`PAGES`].
    pub fn show_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.page = page.min(PAGES.len() - 1);
        self.shown += 1;
        cx.notify();
    }

    /// On Models & accounts, points at what the group of models `way`
    /// needs ("OpenAI, with your API key"): its row is marked and has the
    /// keyboard focus.
    pub fn point_at_account(
        &mut self,
        way: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let account = way.and_then(accounts::account_for);
        self.accounts
            .update(cx, |accounts, cx| accounts.point_at(account, window, cx));
    }

    /// The page last asked for with [`Self::show_page`].
    pub fn page(&self) -> usize {
        self.page
    }

    pub fn light_theme(&self) -> &Entity<ThemePicker> {
        &self.light_theme
    }

    pub fn dark_theme(&self) -> &Entity<ThemePicker> {
        &self.dark_theme
    }

    pub fn accounts(&self) -> &Entity<AccountsPage> {
        &self.accounts
    }

    pub fn profiles(&self) -> &Entity<ProfilesPage> {
        &self.profiles
    }

    pub fn sessions(&self) -> &Entity<SessionsPage> {
        &self.sessions
    }

    pub fn web_page(&self) -> &Entity<WebPageSettings> {
        &self.web_page
    }

    pub fn extensions(&self) -> &Entity<ExtensionsPage> {
        &self.extensions
    }

    /// Asks for a font file, copies it into Arko's folder and offers it in
    /// both font lists.
    fn add_font_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add Font".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            this.update_in(cx, |this, window, cx| this.add_font_from(path, window, cx))
                .ok();
        })
        .detach();
    }

    /// Copies the font file at `path` into Arko's folder and offers it in
    /// both font lists, saying how that went. The file is read, copied and
    /// registered with the text system once, off the UI thread, and the
    /// font lists hear of it as their catalog changes.
    pub fn add_font_from(
        &mut self,
        path: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let folder = paths::fonts_folder(cx.global::<crate::backend::SharedBackend>().data_root());
        let text_system = cx.text_system().clone();
        let importing = cx.background_spawn(async move {
            let font = fonts::import(&path, &folder)?;
            let data = fonts::keep(font.data);
            // One the folder had is registered already.
            if !font.already_imported {
                text_system.add_fonts(vec![data.clone()])?;
            }
            anyhow::Ok((font.families, font.already_imported, data))
        });
        cx.spawn_in(window, async move |_, cx| {
            let imported = importing.await;
            cx.update(|window, cx| match imported {
                Ok((families, already_imported, data)) => {
                    if !already_imported {
                        fonts::note_added(data, cx);
                    }
                    let names = families
                        .iter()
                        .map(|name| name.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    window.push_notification(
                        format!("Added {names}. Choose it under Interface font or Code font."),
                        cx,
                    );
                }
                Err(error) => window.push_notification(
                    gpui_kit::component::notification::Notification::error(error.to_string()),
                    cx,
                ),
            })
            .ok();
        })
        .detach();
    }

    /// Asks for an MCP config file that every session reads.
    fn add_mcp_config(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Add".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            this.update_in(cx, |this, window, cx| {
                this.add_checked_mcp_configs(paths, window, cx)
            })
            .ok();
        })
        .detach();
    }

    /// Adds the files of `files` that are MCP config files, read in the
    /// background, and says why each other one isn't added.
    pub fn add_checked_mcp_configs(
        &mut self,
        files: Vec<std::path::PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let checking = cx.background_spawn(async move {
            files
                .into_iter()
                .map(|file| {
                    let checked = crate::backend::extensions::check_mcp_file(&file);
                    (file, checked)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let checked = checking.await;
            this.update_in(cx, |this, window, cx| {
                let mut added = Vec::new();
                for (file, checked) in checked {
                    match checked {
                        Ok(()) => added.push(file),
                        Err(why) => window.push_notification(
                            gpui_kit::component::notification::Notification::error(format!(
                                "{} isn’t added ({why}).",
                                file.display()
                            )),
                            cx,
                        ),
                    }
                }
                if !added.is_empty() {
                    this.add_mcp_configs(added, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Adds MCP config files every session reads, each once. Extensions
    /// lists their servers as it hears of them.
    pub fn add_mcp_configs(&mut self, files: Vec<std::path::PathBuf>, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| {
            for file in files {
                if !prefs.advanced.mcp_config.contains(&file) {
                    prefs.advanced.mcp_config.push(file);
                }
            }
        });
    }

    fn confirm_reset(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.open_alert_dialog(cx, |alert, _, _| {
            alert
                .title("Reset all preferences?")
                .description(
                    "Every setting here goes back to its default. Sessions, keys and themes in Arko’s folder stay as they are.",
                )
                .confirm()
                .ok_text("Reset")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(|_, _, cx| {
                    prefs::update(cx, |prefs| {
                        let window = prefs.window;
                        *prefs = prefs::Prefs::default();
                        prefs.window = window;
                    });
                    true
                })
        });
    }

    fn pages(&self, cx: &mut Context<Self>) -> Vec<SettingPage> {
        let this = cx.entity();
        vec![
            self.appearance_page(cx),
            input_page(cx),
            self.accounts
                .read(cx)
                .page(&self.accounts, self.accounts_revision, cx),
            self.profiles.read(cx).page(&self.profiles),
            self.sessions.read(cx).page(cx),
            notifications_page(cx),
            self.web_page.read(cx).page(cx),
            self.extensions
                .read(cx)
                .page(&self.extensions, self.extensions_revision),
            advanced_page(
                &this,
                self.extensions.read(cx).extensions(),
                self.extensions_revision,
                cx,
            ),
        ]
    }

    fn appearance_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let defaults = prefs::Appearance::default();
        let shown = &prefs::get(cx).appearance;
        let picker_id = |picker: &Option<Entity<FontPickerState>>| {
            picker.as_ref().map(|picker| picker.entity_id().as_u64())
        };
        let keys = [
            group_key(
                "Theme",
                &(
                    shown.theme_mode,
                    &shown.light_theme,
                    &shown.dark_theme,
                    shown.theme_previews,
                ),
            ),
            group_key(
                "Interface font",
                &(picker_id(&self.ui_font), &shown.ui_font),
            ),
            group_key(
                "Code font",
                &(
                    picker_id(&self.code_font),
                    &shown.code_font,
                    shown.code_ligatures,
                ),
            ),
            group_key("Zoom", &shown.zoom),
            group_key(
                "Transcript",
                &(
                    shown.render_markdown,
                    shown.stream_fade,
                    shown.expand_thinking,
                ),
            ),
            group_key("Motion", &shown.motion),
        ];
        let light = self.light_theme.clone();
        let dark = self.dark_theme.clone();
        let ui_font = self.ui_font.clone();
        let code_font = self.code_font.clone();
        let view = cx.entity().downgrade();
        let zoom_options: Vec<(SharedString, SharedString)> = prefs::ZOOM_STEPS
            .iter()
            .map(|step| {
                let label = zoom_label(*step);
                (label.clone(), label)
            })
            .collect();
        SettingPage::new(PAGES[APPEARANCE_PAGE])
            .icon(IconName::Palette)
            .groups([
                SettingGroup::new().key(keys[0]).title("Theme").items([
                    SettingItem::new(
                        "Mode",
                        SettingField::dropdown(
                            vec![
                                ("system".into(), "System".into()),
                                ("light".into(), "Light".into()),
                                ("dark".into(), "Dark".into()),
                            ],
                            |cx: &App| match prefs::get(cx).appearance.theme_mode {
                                ThemeMode::System => "system".into(),
                                ThemeMode::Light => "light".into(),
                                ThemeMode::Dark => "dark".into(),
                            },
                            |value: SharedString, cx: &mut App| {
                                let mode = match value.as_ref() {
                                    "light" => ThemeMode::Light,
                                    "dark" => ThemeMode::Dark,
                                    _ => ThemeMode::System,
                                };
                                prefs::update(cx, |prefs| prefs.appearance.theme_mode = mode);
                            },
                        )
                        .default_value("system"),
                    )
                    .description("System follows the light or dark appearance of your computer."),
                    SettingItem::new(
                        "Light theme",
                        SettingField::render(move |_, _, _| light.clone()).on_reset(
                            |cx| prefs::get(cx).appearance.light_theme != prefs::DEFAULT_LIGHT_THEME,
                            |_, cx| {
                                prefs::update(cx, |prefs| {
                                    prefs.appearance.light_theme = prefs::DEFAULT_LIGHT_THEME.into()
                                })
                            },
                        ),
                    )
                    .keywords(["color", "colour"]),
                    SettingItem::new(
                        "Dark theme",
                        SettingField::render(move |_, _, _| dark.clone()).on_reset(
                            |cx| prefs::get(cx).appearance.dark_theme != prefs::DEFAULT_DARK_THEME,
                            |_, cx| {
                                prefs::update(cx, |prefs| {
                                    prefs.appearance.dark_theme = prefs::DEFAULT_DARK_THEME.into()
                                })
                            },
                        ),
                    )
                    .keywords(["color", "colour"]),
                    SettingItem::new(
                        "Previews",
                        SettingField::switch(
                            |cx: &App| prefs::get(cx).appearance.theme_previews,
                            |on: bool, cx: &mut App| {
                                prefs::update(cx, |prefs| {
                                    prefs.appearance.theme_previews = on;
                                    prefs.appearance.flash_notice = true;
                                })
                            },
                        )
                        .default_value(defaults.theme_previews),
                    )
                    .description("Resting on a theme in a theme list shows it. Moving between light and dark themes makes the window flash."),
                ]),
                SettingGroup::new()
                    .key(keys[1])
                    .title("Interface font")
                    .item(SettingItem::render(move |_, _, cx| font_picker(ui_font.as_ref(), &prefs::get(cx).appearance.ui_font, false, cx)).keywords(["font", "typeface", "size"])),
                SettingGroup::new()
                    .key(keys[2])
                    .title("Code font")
                    .item(SettingItem::render(move |_, _, cx| font_picker(code_font.as_ref(), &prefs::get(cx).appearance.code_font, true, cx)).keywords(["font", "monospace", "size"]))
                    .item(
                        switch(
                            "Code ligatures",
                            "Draws pairs such as -> and != as one symbol in code.",
                            |prefs| prefs.appearance.code_ligatures,
                            |prefs, on| prefs.appearance.code_ligatures = on,
                            defaults.code_ligatures,
                        )
                        .keywords(["ligatures", "font", "monospace"]),
                    ),
                SettingGroup::new().key(group_key("Font files", &())).title("Font files").item(
                    SettingItem::new(
                        "Add font file…",
                        SettingField::render(move |_, _, _| {
                            let view = view.clone();
                            Button::new("add-font")
                                .outline()
                                .small()
                                .label("Add…")
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |view, cx| view.add_font_file(window, cx)).ok();
                                })
                        }),
                    )
                    .description("Copies a .ttf or .otf file into Arko’s folder and lists it under both fonts."),
                ),
                SettingGroup::new().key(keys[3]).item(
                    SettingItem::new(
                        "Zoom",
                        SettingField::dropdown(
                            zoom_options,
                            |cx: &App| zoom_label(prefs::get(cx).appearance.zoom()),
                            |value: SharedString, cx: &mut App| {
                                if let Some(zoom) = parse_zoom(&value) {
                                    prefs::update(cx, |prefs| prefs.appearance.zoom = zoom);
                                }
                            },
                        )
                        .default_value(zoom_label(1.0)),
                    )
                    .description("Scales the whole window, as View › Zoom In and Zoom Out do."),
                ),
                SettingGroup::new().key(keys[4]).title("Transcript").items([
                    switch(
                        "Render Markdown",
                        "Show replies formatted. View › Show Markdown Source, or a reply’s menu, shows the source instead.",
                        |prefs| prefs.appearance.render_markdown,
                        |prefs, on| prefs.appearance.render_markdown = on,
                        defaults.render_markdown,
                    ),
                    switch(
                        "Stream fade",
                        "New text fades in as it streams.",
                        |prefs| prefs.appearance.stream_fade,
                        |prefs, on| prefs.appearance.stream_fade = on,
                        defaults.stream_fade,
                    ),
                    switch(
                        "Expand thinking",
                        "Show the model’s reasoning open instead of folded.",
                        |prefs| prefs.appearance.expand_thinking,
                        |prefs, on| prefs.appearance.expand_thinking = on,
                        defaults.expand_thinking,
                    ),
                ]),
                SettingGroup::new().key(keys[5]).item(
                    SettingItem::new(
                        "Motion",
                        SettingField::dropdown(
                            vec![
                                ("system".into(), "System".into()),
                                ("on".into(), "On".into()),
                                ("off".into(), "Off".into()),
                            ],
                            |cx: &App| match prefs::get(cx).appearance.motion {
                                Motion::System => "system".into(),
                                Motion::On => "on".into(),
                                Motion::Off => "off".into(),
                            },
                            |value: SharedString, cx: &mut App| {
                                let motion = match value.as_ref() {
                                    "on" => Motion::On,
                                    "off" => Motion::Off,
                                    _ => Motion::System,
                                };
                                prefs::update(cx, |prefs| prefs.appearance.motion = motion);
                            },
                        )
                        .default_value("system"),
                    )
                    .description("System follows your computer’s reduce-motion setting. Off stops animations, smooth typing included."),
                ),
            ])
    }
}

/// A font picker showing a family and its size, or, while the installed
/// fonts are read, the chosen family and size. `monospace_filter` offers
/// Monospace only, which the code font starts with, and which the interface
/// font has no use for.
fn font_picker(
    picker: Option<&Entity<FontPickerState>>,
    choice: &prefs::FontChoice,
    monospace_filter: bool,
    cx: &App,
) -> AnyElement {
    match picker {
        Some(picker) => FontPicker::new(picker)
            .compact()
            .show_monospace_filter(monospace_filter)
            .show_weight(false)
            .show_line_height(false)
            .show_features(false)
            .into_any_element(),
        None => h_flex()
            .id("font-picker-loading")
            .test_support()
            .gap_2()
            .text_sm()
            .child(choice.family.clone())
            .child(format!("{} px", choice.size()))
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Reading the installed fonts…"),
            )
            .into_any_element(),
    }
}

/// A switch bound to a preference.
fn switch(
    title: &'static str,
    description: &'static str,
    get: fn(&prefs::Prefs) -> bool,
    set: fn(&mut prefs::Prefs, bool),
    default: bool,
) -> SettingItem {
    let item = SettingItem::new(
        title,
        SettingField::switch(
            move |cx: &App| get(prefs::get(cx)),
            move |on: bool, cx: &mut App| prefs::update(cx, |prefs| set(prefs, on)),
        )
        .default_value(default),
    );
    // A title that says it all needs no description.
    if description.is_empty() {
        item
    } else {
        item.description(description)
    }
}

/// A dropdown bound to a preference with a few named values.
fn choice<T: Copy + PartialEq + 'static>(
    title: &'static str,
    options: &'static [(T, &'static str)],
    get: fn(&prefs::Prefs) -> T,
    set: fn(&mut prefs::Prefs, T),
    default: T,
) -> SettingItem {
    choice_of(
        title,
        options
            .iter()
            .map(|(option, label)| (*option, SharedString::from(*label)))
            .collect(),
        get,
        set,
        default,
    )
}

/// A dropdown bound to a preference, with labels made when the page is.
fn choice_of<T: Copy + PartialEq + 'static>(
    title: &'static str,
    options: Vec<(T, SharedString)>,
    get: fn(&prefs::Prefs) -> T,
    set: fn(&mut prefs::Prefs, T),
    default: T,
) -> SettingItem {
    let options = std::rc::Rc::new(options);
    let label = {
        let options = options.clone();
        move |value: T| {
            options
                .iter()
                .find(|(option, _)| *option == value)
                .map(|(_, label)| label.clone())
                .unwrap_or_default()
        }
    };
    let choices = options
        .iter()
        .map(|(_, label)| (label.clone(), label.clone()))
        .collect();
    SettingItem::new(
        title,
        SettingField::dropdown(
            choices,
            {
                let label = label.clone();
                move |cx: &App| label(get(prefs::get(cx)))
            },
            move |value: SharedString, cx: &mut App| {
                if let Some((option, _)) = options.iter().find(|(_, label)| value == *label) {
                    let option = *option;
                    prefs::update(cx, |prefs| set(prefs, option));
                }
            },
        )
        .default_value(label(default)),
    )
}

fn zoom_label(zoom: f32) -> SharedString {
    format!("{}%", (zoom * 100.).round()).into()
}

fn parse_zoom(label: &str) -> Option<f32> {
    let percent: f32 = label.trim_end_matches('%').parse().ok()?;
    prefs::ZOOM_STEPS
        .iter()
        .copied()
        .find(|step| ((step * 100.).round() - percent).abs() < 0.5)
}

fn send_key_options() -> &'static [(SendKey, &'static str)] {
    if cfg!(target_os = "macos") {
        &[
            (SendKey::Enter, "Enter sends, ⇧Enter adds a line"),
            (SendKey::ModEnter, "⌘Enter sends, Enter adds a line"),
            (SendKey::ShiftEnter, "⇧Enter sends, Enter adds a line"),
        ]
    } else {
        &[
            (SendKey::Enter, "Enter sends, Shift+Enter adds a line"),
            (SendKey::ModEnter, "Ctrl+Enter sends, Enter adds a line"),
            (SendKey::ShiftEnter, "Shift+Enter sends, Enter adds a line"),
        ]
    }
}

fn input_page(cx: &App) -> SettingPage {
    let defaults = prefs::Input::default();
    let shown = &prefs::get(cx).input;
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    };
    SettingPage::new(PAGES[INPUT_PAGE])
        .icon(IconName::ALargeSmall)
        .groups([
            SettingGroup::new()
                .key(group_key("Sending", &shown.send_key))
                .title("Sending")
                .item(
                choice(
                    "Send key",
                    send_key_options(),
                    |prefs| prefs.input.send_key,
                    |prefs, key| prefs.input.send_key = key,
                    defaults.send_key,
                )
                .description(format!(
                    "The key that sends a message, while the other one adds a line. {modifier}Enter always sends too."
                ))
                .keywords(["enter", "shift", "newline", "submit"]),
            ),
            SettingGroup::new()
                .key(group_key(
                    "Typing",
                    &(
                        shown.smooth_typing,
                        shown.spell_check,
                        shown.suggestions,
                        shown.keymap,
                    ),
                ))
                .title("Typing")
                .items([
                switch(
                    "Smooth typing",
                    "The caret glides to each new letter. Follows Motion.",
                    |prefs| prefs.input.smooth_typing,
                    |prefs, on| prefs.input.smooth_typing = on,
                    defaults.smooth_typing,
                )
                .keywords(["caret", "cursor", "animation"]),
                switch(
                    "Spell check",
                    "Underlines misspelled words (English).",
                    |prefs| prefs.input.spell_check,
                    |prefs, on| prefs.input.spell_check = on,
                    defaults.spell_check,
                )
                .keywords(["spelling", "dictionary", "typos"]),
                choice(
                    "Word suggestions",
                    &[
                        (Suggestions::Off, "Off"),
                        (Suggestions::Quiet, "Quiet"),
                        (Suggestions::Eager, "Eager"),
                    ],
                    |prefs| prefs.input.suggestions,
                    |prefs, value| prefs.input.suggestions = value,
                    defaults.suggestions,
                )
                .description(
                    "Completes words from what you’ve typed in Arko Studio, never from pasted text or replies. Tab accepts.",
                )
                .keywords(["autocomplete", "completion", "predictions"]),
                choice(
                    "Keybindings",
                    &[
                        (Keymap::Standard, "Standard"),
                        (Keymap::Emacs, "Emacs"),
                        (Keymap::Vim, "Vim"),
                    ],
                    |prefs| prefs.input.keymap,
                    |prefs, value| prefs.input.keymap = value,
                    defaults.keymap,
                )
                .description(
                    "Move and edit in the composer with your platform’s keys, Emacs’s or Vim’s.",
                )
                .keywords(["keymap", "vim", "emacs", "shortcuts"]),
            ]),
            SettingGroup::new()
                .key(group_key("Pasting", &shown.paste_token_lines))
                .title("Pasting")
                .item(
                SettingItem::new(
                    "Paste as a token",
                    SettingField::number_input(
                        NumberFieldOptions {
                            min: 2.0,
                            max: 1000.0,
                            step: 1.0,
                        },
                        |cx: &App| prefs::get(cx).input.paste_token_lines as f64,
                        |lines: f64, cx: &mut App| {
                            prefs::update(cx, |prefs| {
                                prefs.input.paste_token_lines =
                                    lines.round().clamp(2., 1000.) as u32
                            })
                        },
                    )
                    .default_value(defaults.paste_token_lines as f64),
                )
                .description(
                    "Pastes longer than this many lines (or 2 KB) become one token. Click it to open it into the text.",
                )
                .keywords(["paste", "clipboard", "long text"]),
            ),
        ])
}

fn notifications_page(cx: &App) -> SettingPage {
    let defaults = prefs::Notifications::default();
    let shown = &prefs::get(cx).notifications;
    SettingPage::new(PAGES[NOTIFICATIONS_PAGE])
        .icon(IconName::Bell)
        .group(SettingGroup::new().key(group_key("", shown)).items([
            switch(
                "When a turn ends",
                "A notification from your system while Arko Studio is in the background.",
                |prefs| prefs.notifications.turn_ended,
                |prefs, on| prefs.notifications.turn_ended = on,
                defaults.turn_ended,
            )
            .keywords(["finished", "done", "notify", "unfocused"]),
            switch(
                "When a question arrives",
                "A notification from your system while Arko Studio is in the background, or in the app while it shows another session.",
                |prefs| prefs.notifications.question,
                |prefs, on| prefs.notifications.question = on,
                defaults.question,
            )
            .keywords(["ask_user", "answer", "notify", "unfocused"]),
            switch(
                "When a worker reports or fails",
                "A notification from your system while Arko Studio is in the background. A worker that another worker started notifies only when it fails.",
                |prefs| prefs.notifications.workers,
                |prefs, on| prefs.notifications.workers = on,
                defaults.workers,
            )
            .keywords(["subagent", "report", "failed", "error", "notify", "unfocused"]),
            switch(
                "Cache reminders",
                "5 minutes before a session’s prompt cache expires (for an hour-long cache), and at 1 minute with a countdown and Refresh.",
                |prefs| prefs.notifications.cache_reminders,
                |prefs, on| prefs.notifications.cache_reminders = on,
                defaults.cache_reminders,
            )
            .keywords(["prompt cache", "expiry", "refresh", "timer"]),
            switch(
                "Sound",
                "",
                |prefs| prefs.notifications.sound,
                |prefs, on| prefs.notifications.sound = on,
                defaults.sound,
            )
            .keywords(["audio", "chime", "alert"]),
        ]))
}

/// Settings › Advanced, with `read`, what Extensions last read of the MCP
/// config files, and how many times it read them.
fn advanced_page(
    this: &Entity<SettingsPage>,
    read: Option<&crate::backend::extensions::Extensions>,
    read_revision: u64,
    cx: &App,
) -> SettingPage {
    let home = cx.global::<crate::session_view::HomeFolder>().0.clone();
    // The folders as the user's home folder holds them, with `~`.
    let in_home =
        |folder: &std::path::Path| crate::backend::account_words::tilde(folder, home.as_deref());
    let data = in_home(cx.global::<crate::backend::SharedBackend>().data_root());
    let logs = cx
        .global::<paths::LogsFolder>()
        .0
        .as_deref()
        .map(in_home)
        .unwrap_or_default();
    let mut groups = Vec::new();
    // Only Windows runs commands in a shell the user picks.
    if cfg!(windows) {
        groups.push(
            SettingGroup::new()
                .key(group_key("Shell", &prefs::get(cx).advanced.windows_shell))
                .title("Shell")
                .item(
                choice(
                    "Shell on Windows",
                    &[
                        (None, "As ARKO_SHELL says"),
                        (Some(prefs::WindowsShell::Cmd), "cmd.exe"),
                        (Some(prefs::WindowsShell::Powershell), "PowerShell"),
                    ],
                    |prefs| prefs.advanced.windows_shell,
                    |prefs, shell| prefs.advanced.windows_shell = shell,
                    None,
                )
                .description(
                    "What new sessions run commands in. PowerShell is PowerShell 7 where it’s installed, else Windows PowerShell. Without ARKO_SHELL, PowerShell.",
                )
                .keywords(["cmd", "powershell", "pwsh", "ARKO_SHELL"]),
            ),
        );
    }
    // What Extensions read of each file: one that isn't there, or can't be
    // read, is left out of the sessions.
    let mut files: Vec<SettingItem> = prefs::get(cx)
        .advanced
        .mcp_config
        .iter()
        .enumerate()
        .map(|(ix, file)| {
            let state = read
                .and_then(|read| {
                    read.files
                        .iter()
                        .find(|read| !read.arko && read.path == *file)
                })
                .map(|read| &read.servers);
            let description = match state {
                Some(None) => "It isn’t there, so sessions leave it out.".to_string(),
                Some(Some(Err(why))) => {
                    format!("It can’t be read ({why}), so sessions leave it out.")
                }
                _ => "Sessions opened from now on start the servers in it. Extensions lists them."
                    .to_string(),
            };
            let file = file.clone();
            SettingItem::new(
                crate::backend::account_words::tilde(&file, home.as_deref()),
                SettingField::render(move |_, _, _| {
                    let file = file.clone();
                    Button::new(("remove-mcp-config", ix))
                        .ghost()
                        .small()
                        .label("Remove")
                        .on_click(move |_, _, cx| {
                            let file = file.clone();
                            prefs::update(cx, |prefs| {
                                prefs.advanced.mcp_config.retain(|kept| *kept != file)
                            })
                        })
                }),
            )
            .description(description)
            .keywords(["mcp", "--mcp-config", "servers"])
        })
        .collect();
    let add = this.downgrade();
    files.push(
        SettingItem::new(
            "Add an MCP config file…",
            SettingField::render(move |_, _, _| {
                let add = add.clone();
                Button::new("add-mcp-config")
                    .outline()
                    .small()
                    .label("Add…")
                    .on_click(move |_, window, cx| {
                        add.update(cx, |page, cx| page.add_mcp_config(window, cx))
                            .ok();
                    })
            }),
        )
        .description(
            "Servers agent sessions start ahead of a project’s and Arko’s, as arko --mcp-config does. In Claude Code’s format or arko’s.",
        )
        .keywords(["mcp", "--mcp-config", "servers", "json"]),
    );
    let mcp_config = &prefs::get(cx).advanced.mcp_config;
    groups.push(
        SettingGroup::new()
            .key(group_key("MCP config files", &(mcp_config, read_revision)))
            .title("MCP config files")
            .items(files),
    );
    let folders = group_key("Folders", &(&data, &logs));
    groups.push(
        SettingGroup::new().key(folders).title("Folders").items([
            SettingItem::new(
                "Arko’s folder",
                SettingField::render(|_, _, _| {
                    Button::new("open-data")
                        .outline()
                        .small()
                        .label("Open")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(menus::OpenDataFolder), cx)
                        })
                }),
            )
            .description(format!(
                "{data}. Sessions, keys, history and themes are shared with Arko in the terminal."
            ))
            .keywords(["data", "ARKO_HOME", "sessions"]),
            SettingItem::new(
                "Logs",
                SettingField::render(|_, _, _| {
                    Button::new("open-logs")
                        .outline()
                        .small()
                        .label("Open")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(menus::OpenLogsFolder), cx)
                        })
                }),
            )
            .description(logs)
            .keywords(["log", "debug"]),
        ]),
    );
    groups.push(
        SettingGroup::new().key(group_key("Reset", &())).item(
            SettingItem::new(
                "Reset preferences…",
                SettingField::render(|_, _, _| {
                    Button::new("reset-preferences")
                        .outline()
                        .small()
                        .label("Reset…")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ResetPreferences), cx)
                        })
                }),
            )
            .keywords(["defaults", "studio.json"]),
        ),
    );
    SettingPage::new(PAGES[ADVANCED_PAGE])
        .icon(IconName::Settings2)
        .groups(groups)
}

actions!(settings_page, [ResetPreferences]);

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        {
            self.builds += 1;
        }
        let pages = self.pages(cx);
        // Wide enough for "Models & accounts" beside its icon and chevron.
        let nav_width = window.rem_size() * 14.;
        let theme = cx.theme();
        v_flex()
            .id("settings-page")
            .test_support()
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(|_, _: &CloseSettings, _, cx| cx.emit(CloseRequested)))
            .on_action(
                cx.listener(|this, _: &ResetPreferences, window, cx| {
                    this.confirm_reset(window, cx)
                }),
            )
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().text_sm().font_medium().child("Settings"))
                    .child(
                        Button::new("close-settings")
                            .ghost()
                            .small()
                            .compact()
                            .icon(IconName::Close)
                            .tooltip_with_action("Close", &CloseSettings, Some(CONTEXT))
                            .accessibility_label("Close settings")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseRequested))),
                    ),
            )
            .child(
                div().flex_1().min_h_0().child(
                    Settings::new(("settings", self.shown))
                        .default_selected_index(SelectIndex {
                            page_ix: self.page,
                            group_ix: None,
                        })
                        .with_group_variant(GroupBoxVariant::Outline)
                        .sidebar_width(nav_width)
                        .pages(pages),
                ),
            )
    }
}

#[cfg(test)]
mod interaction;
#[cfg(test)]
mod profiles_tests;

#[cfg(test)]
mod tests {
    use super::{
        ACCOUNTS_PAGE, ADVANCED_PAGE, APPEARANCE_PAGE, EXTENSIONS_PAGE, INPUT_PAGE,
        NOTIFICATIONS_PAGE, PAGES, PROFILES_PAGE, SESSIONS_PAGE, WEB_PAGE_PAGE, group_key,
        parse_zoom, zoom_label,
    };
    use crate::prefs;

    /// Each page is where its constant says, and the list has no other.
    #[test]
    fn the_page_constants_name_their_pages() {
        let pages = [
            (APPEARANCE_PAGE, "Appearance"),
            (INPUT_PAGE, "Input"),
            (ACCOUNTS_PAGE, "Models & accounts"),
            (PROFILES_PAGE, "Worker profiles"),
            (SESSIONS_PAGE, "Sessions"),
            (NOTIFICATIONS_PAGE, "Notifications"),
            (WEB_PAGE_PAGE, "Web page"),
            (EXTENSIONS_PAGE, "Extensions"),
            (ADVANCED_PAGE, "Advanced"),
        ];
        assert_eq!(pages.len(), PAGES.len());
        for (ix, name) in pages {
            assert_eq!(PAGES[ix], name);
        }
    }

    #[test]
    fn a_groups_key_changes_with_what_it_shows_and_its_title() {
        let shown = prefs::Prefs::default().notifications;
        let mut changed = shown.clone();
        changed.sound = !changed.sound;
        assert_eq!(group_key("", &shown), group_key("", &shown.clone()));
        assert_ne!(group_key("", &shown), group_key("", &changed));
        assert_ne!(group_key("Zoom", &()), group_key("Reset", &()));
    }

    #[test]
    fn zoom_labels_round_trip() {
        for step in prefs::ZOOM_STEPS {
            assert_eq!(parse_zoom(&zoom_label(step)), Some(step));
        }
        assert_eq!(zoom_label(0.67), "67%");
        assert_eq!(parse_zoom("300%"), None);
    }
}
