//! Settings (Mod+,): a page in the window, in place of the transcript, with
//! the app's preferences in pages. Esc or the close button goes back.
//!
//! The session's own live settings are in its info popover; these are the
//! app's preferences and the defaults for new sessions.

use std::borrow::Cow;

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

use crate::prefs::{self, Keymap, Motion, SendKey, Suggestions, ThemeMode};
use crate::theme_picker::ThemePicker;
use crate::{fonts, menus, paths};

actions!(settings_page, [CloseSettings]);

/// The key context of the Settings page.
pub const CONTEXT: &str = "SettingsPage";

/// The pages, in their order in the page list.
pub const PAGES: [&str; 8] = [
    "Appearance",
    "Input",
    "Models & accounts",
    "Sessions",
    "Notifications",
    "Web page",
    "Extensions",
    "Advanced",
];

/// Asks the window to go back from Settings.
pub struct CloseRequested;

impl EventEmitter<CloseRequested> for SettingsPage {}

pub struct SettingsPage {
    focus_handle: FocusHandle,
    light_theme: Entity<ThemePicker>,
    dark_theme: Entity<ThemePicker>,
    ui_font: Entity<FontPickerState>,
    code_font: Entity<FontPickerState>,
    /// The page to show, set from outside (the command palette), and how
    /// many times it was: the page list starts afresh on it each time.
    page: usize,
    shown: usize,
    _subscriptions: Vec<Subscription>,
}

impl SettingsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let appearance = prefs::get(cx).appearance.clone();
        let (bundled, added) = {
            let fonts = cx.global::<fonts::Fonts>();
            (fonts.bundled(), fonts.added())
        };
        let picker = |choice: &prefs::FontChoice,
                      monospace: bool,
                      bundled: Vec<Cow<'static, [u8]>>,
                      added: Vec<Cow<'static, [u8]>>,
                      window: &mut Window,
                      cx: &mut Context<Self>| {
            let settings = FontSettings::new(choice.family.clone()).with_size(px(choice.size()));
            cx.new(|cx| {
                FontPickerState::new(window, cx)
                    .default_settings(settings)
                    .monospace_only(monospace)
                    .app_fonts(bundled)
                    .added_fonts(added)
            })
        };
        let ui_font = picker(
            &appearance.ui_font,
            false,
            bundled.clone(),
            added.clone(),
            window,
            cx,
        );
        let code_font = picker(&appearance.code_font, true, bundled, added, window, cx);
        let subscriptions = vec![
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
            // Settings shows the preferences as they change, here or elsewhere
            // (a zoom shortcut, another window).
            cx.observe_global::<prefs::PrefsStore>(|_, cx| cx.notify()),
        ];
        Self {
            focus_handle: cx.focus_handle(),
            light_theme: cx.new(|cx| ThemePicker::new(false, window, cx)),
            dark_theme: cx.new(|cx| ThemePicker::new(true, window, cx)),
            ui_font,
            code_font,
            page: 0,
            shown: 0,
            _subscriptions: subscriptions,
        }
    }

    /// Shows the page at `page` in [`PAGES`].
    pub fn show_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.page = page.min(PAGES.len() - 1);
        self.shown += 1;
        cx.notify();
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

    /// Asks for a font file, copies it into Arko's folder and offers it in
    /// both font lists.
    fn add_font_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Add Font".into()),
        });
        let folder = paths::fonts_folder(cx.global::<crate::backend::SharedBackend>().data_root());
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let imported = cx
                .background_spawn(async move { fonts::import(&path, &folder) })
                .await;
            this.update_in(cx, |this, window, cx| match imported {
                Ok(font) => {
                    let data: Cow<'static, [u8]> = Cow::Owned(font.data);
                    if !font.already_imported {
                        fonts::note_added(data.clone(), cx);
                    }
                    for picker in [this.ui_font.clone(), this.code_font.clone()] {
                        picker
                            .update(cx, |picker, cx| {
                                picker.add_fonts(vec![data.clone()], window, cx)
                            })
                            .ok();
                    }
                    let names = font
                        .families
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
        vec![
            self.appearance_page(cx),
            input_page(),
            planned_page(
                PAGES[2],
                IconName::Bot,
                &[
                    ("Default mode", "Chat or Agent, for new sessions."),
                    (
                        "Default model and effort",
                        "For the main agent and for workers.",
                    ),
                    ("1-hour cache", "For Claude with an API key."),
                    (
                        "API keys",
                        "Anthropic, OpenAI, Gemini, OpenRouter and Voyage: where each comes from, and adding, replacing or removing it.",
                    ),
                    (
                        "ChatGPT login",
                        "The discovered auth.json, or a file you choose.",
                    ),
                    (
                        "Claude Code",
                        "Where it's installed and whether you're signed in.",
                    ),
                ],
            ),
            sessions_page(),
            notifications_page(),
            planned_page(
                PAGES[5],
                IconName::Globe,
                &[
                    (
                        "Share the web page",
                        "Follow and steer a session from a browser: address and port.",
                    ),
                    ("Links and QR code", "Where the page is, and a new key."),
                ],
            ),
            planned_page(
                PAGES[6],
                IconName::Frame,
                &[
                    (
                        "MCP servers",
                        "From .mcp.json and --mcp-config, and their approvals.",
                    ),
                    ("Skills", "Where skills are read from."),
                ],
            ),
            advanced_page(cx),
        ]
    }

    fn appearance_page(&self, cx: &mut Context<Self>) -> SettingPage {
        let defaults = prefs::Appearance::default();
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
        SettingPage::new(PAGES[0])
            .icon(IconName::Palette)
            .default_open(true)
            .groups([
                SettingGroup::new().title("Theme").items([
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
                    .title("Interface font")
                    .item(SettingItem::render(move |_, _, _| FontPicker::new(&ui_font).compact().show_weight(false).show_line_height(false).show_features(false)).keywords(["font", "typeface", "size"])),
                SettingGroup::new()
                    .title("Code font")
                    .item(SettingItem::render(move |_, _, _| FontPicker::new(&code_font).compact().show_weight(false).show_line_height(false).show_features(false)).keywords(["font", "monospace", "size"])),
                SettingGroup::new().title("Size").items([
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
                    .description(zoom_description()),
                ]),
                SettingGroup::new().title("Transcript").items([
                    switch(
                        "Render Markdown",
                        "Show replies formatted. Each message can still show its source.",
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
                SettingGroup::new().title("Motion").item(
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

/// A switch bound to a preference.
fn switch(
    title: &'static str,
    description: &'static str,
    get: fn(&prefs::Prefs) -> bool,
    set: fn(&mut prefs::Prefs, bool),
    default: bool,
) -> SettingItem {
    SettingItem::new(
        title,
        SettingField::switch(
            move |cx: &App| get(prefs::get(cx)),
            move |on: bool, cx: &mut App| prefs::update(cx, |prefs| set(prefs, on)),
        )
        .default_value(default),
    )
    .description(description)
}

/// A dropdown bound to a preference with a few named values.
fn choice<T: Copy + PartialEq + 'static>(
    title: &'static str,
    options: &'static [(T, &'static str)],
    get: fn(&prefs::Prefs) -> T,
    set: fn(&mut prefs::Prefs, T),
    default: T,
) -> SettingItem {
    let label = move |value: T| {
        options
            .iter()
            .find(|(option, _)| *option == value)
            .map(|(_, label)| SharedString::from(*label))
            .unwrap_or_default()
    };
    SettingItem::new(
        title,
        SettingField::dropdown(
            options
                .iter()
                .map(|(_, label)| (SharedString::from(*label), SharedString::from(*label)))
                .collect(),
            move |cx: &App| label(get(prefs::get(cx))),
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

fn zoom_description() -> String {
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    };
    format!(
        "Scales the whole window. {modifier}= and {modifier}- step it, {modifier}0 goes back to 100%."
    )
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

/// Says why a setting can't be changed yet, after what it does.
fn pending(item: SettingItem, description: &str) -> SettingItem {
    item.description(format!("{description} Not available yet."))
        .disabled(true)
}

fn input_page() -> SettingPage {
    let defaults = prefs::Input::default();
    let modifier = if cfg!(target_os = "macos") {
        "⌘"
    } else {
        "Ctrl+"
    };
    // Spell check, word suggestions and paste tokens come with the
    // composer's tools: until then they show what they'll do, unavailable.
    SettingPage::new(PAGES[1])
        .icon(IconName::ALargeSmall)
        .groups([
            SettingGroup::new().title("Sending").item(
                choice(
                    "Send key",
                    send_key_options(),
                    |prefs| prefs.input.send_key,
                    |prefs, key| prefs.input.send_key = key,
                    defaults.send_key,
                )
                .description(format!(
                    "The key that sends a message; the other one adds a line. {modifier}Enter always sends too."
                )),
            ),
            SettingGroup::new().title("Typing").items([
                switch(
                    "Smooth typing",
                    "The caret glides to each new letter. Follows Motion.",
                    |prefs| prefs.input.smooth_typing,
                    |prefs, on| prefs.input.smooth_typing = on,
                    defaults.smooth_typing,
                ),
                pending(
                    switch(
                        "Spell check",
                        "",
                        |prefs| prefs.input.spell_check,
                        |prefs, on| prefs.input.spell_check = on,
                        defaults.spell_check,
                    ),
                    "Underlines misspelled words (English).",
                ),
                pending(
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
                    ),
                    "Completes words you’ve typed before. Tab accepts.",
                ),
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
                    "How the composer’s keys move and edit: your platform’s, Emacs’s or Vim’s.",
                ),
            ]),
            SettingGroup::new().title("Pasting").item(pending(
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
                ),
                "Pastes longer than this many lines become one token you can open.",
            )),
        ])
}

fn notifications_page() -> SettingPage {
    let defaults = prefs::Notifications::default();
    SettingPage::new(PAGES[4])
        .icon(IconName::Bell)
        .group(SettingGroup::new().items([
            switch(
                "When a turn ends",
                "While Arko Studio is in the background.",
                |prefs| prefs.notifications.turn_ended,
                |prefs, on| prefs.notifications.turn_ended = on,
                defaults.turn_ended,
            ),
            switch(
                "When a question arrives",
                "The agent waits for your answer.",
                |prefs| prefs.notifications.question,
                |prefs, on| prefs.notifications.question = on,
                defaults.question,
            ),
            switch(
                "Cache reminders",
                "Before a session’s prompt cache expires, with a refresh button.",
                |prefs| prefs.notifications.cache_reminders,
                |prefs, on| prefs.notifications.cache_reminders = on,
                defaults.cache_reminders,
            ),
            switch(
                "Sound",
                "Notifications make a sound.",
                |prefs| prefs.notifications.sound,
                |prefs, on| prefs.notifications.sound = on,
                defaults.sound,
            ),
        ]))
}

/// Sessions: asking before quitting works; the defaults for new sessions
/// come later.
fn sessions_page() -> SettingPage {
    let defaults = prefs::SessionPrefs::default();
    SettingPage::new(PAGES[3])
        .icon(IconName::FolderOpen)
        .groups([
            SettingGroup::new().title("Quitting").item(switch(
                "Ask about running work",
                "Before quitting or closing a session stops background jobs and workers.",
                |prefs| prefs.sessions.ask_before_quit,
                |prefs, on| prefs.sessions.ask_before_quit = on,
                defaults.ask_before_quit,
            )),
            SettingGroup::new()
                .title("New sessions")
                .description("These settings come with a later version.")
                .items(
                    [
                        (
                            "Handover size",
                            "When the main agent and workers hand over to a fresh conversation.",
                        ),
                        (
                            "Background after",
                            "When a running command moves to the background.",
                        ),
                        (
                            "Outside access",
                            "Whether new sessions may touch files outside their folder.",
                        ),
                        (
                            "Text replies",
                            "Whether the agent ends its turns with a reply in text.",
                        ),
                        (
                            "Claude Code compaction",
                            "Whether Claude Code compacts or hands over.",
                        ),
                        (
                            "Extra rules and project instructions",
                            "What new agents are told.",
                        ),
                        (
                            "Chat instructions",
                            "Your custom instructions for new chats, and what happens at the context limit.",
                        ),
                    ]
                    .into_iter()
                    .map(|(title, description)| {
                        SettingItem::new(title, SettingField::render(|_, _, _| div()))
                            .description(description)
                            .disabled(true)
                    }),
                ),
        ])
}

/// A page whose settings come later: their names and what they'll do,
/// without controls yet.
fn planned_page(
    title: &'static str,
    icon: IconName,
    items: &'static [(&'static str, &'static str)],
) -> SettingPage {
    SettingPage::new(title).icon(icon).resettable(false).group(
        SettingGroup::new()
            .description("These settings come with the engine.")
            .items(items.iter().map(|(title, description)| {
                SettingItem::new(*title, SettingField::render(|_, _, _| div()))
                    .description(*description)
                    .disabled(true)
            })),
    )
}

fn advanced_page(cx: &App) -> SettingPage {
    let data = cx
        .global::<crate::backend::SharedBackend>()
        .data_root()
        .display()
        .to_string();
    let logs = paths::logs_folder()
        .map(|folder| folder.display().to_string())
        .unwrap_or_default();
    SettingPage::new(PAGES[7])
        .icon(IconName::Settings2)
        .resettable(false)
        .groups([
            SettingGroup::new().title("Folders").items([
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
                    "{data}. Shared with arko in the terminal: sessions, keys, history and themes."
                )),
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
                .description(logs),
            ]),
            SettingGroup::new().title("Reset").item(
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
                .description("Every setting in Arko Studio goes back to its default."),
            ),
        ])
}

actions!(settings_page, [ResetPreferences]);

impl Focusable for SettingsPage {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SettingsPage {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let pages = self.pages(cx);
        let nav_width = window.rem_size() * 13.;
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
mod tests {
    use super::{parse_zoom, zoom_label};
    use crate::prefs;

    #[test]
    fn zoom_labels_round_trip() {
        for step in prefs::ZOOM_STEPS {
            assert_eq!(parse_zoom(&zoom_label(step)), Some(step));
        }
        assert_eq!(zoom_label(0.67), "67%");
        assert_eq!(parse_zoom("300%"), None);
    }
}
