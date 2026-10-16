//! The main column: the transcript's reading column and the composer under
//! it. Until the engine is wired in, the transcript is empty and the
//! composer's buttons say so.

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Keymap, Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _, Toggle, ToggleGroup},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Mode, SessionEntry, SharedBackend};
use crate::layout::WidthClass;
use crate::prefs;

/// How many recent folders Change… offers.
const RECENT_FOLDERS: usize = 8;

/// A new session, before its first message: how it will work and where.
#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub mode: Mode,
    /// The folder chosen with Change…. `None` follows the most recent one.
    pub folder: Option<PathBuf>,
}

/// What the main column shows: a session, or a new one not started yet.
pub struct SessionView {
    session: Option<SessionEntry>,
    draft: Draft,
    /// The folders of recent agent sessions, the latest first.
    recent_folders: Vec<PathBuf>,
    /// The user's home folder, the last place a new session falls back on.
    home: Option<PathBuf>,
    /// The model new sessions start on.
    default_model: Option<String>,
    composer: Entity<TextareaState>,
    width: WidthClass,
    _model: Task<()>,
    _prefs: Subscription,
}

impl SessionView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = prefs::get(cx).input.clone();
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder("Message Arko")
                .smooth_caret(input.smooth_typing)
                .keymap(text_keymap(input.keymap))
        });
        // The composer follows Settings > Input as it changes. Send key, spell
        // check and word suggestions wait for GPUI Kit's writing tools
        // (PENDING(K1)): Settings shows them unavailable until then.
        let prefs = cx.observe_global::<prefs::PrefsStore>(|this, cx| {
            let input = prefs::get(cx).input.clone();
            this.composer.update(cx, |composer, cx| {
                if composer.has_smooth_caret() != input.smooth_typing {
                    composer.set_smooth_caret(input.smooth_typing, cx);
                }
                let keymap = text_keymap(input.keymap);
                if composer.current_keymap() != keymap {
                    composer.set_keymap(keymap, cx);
                }
            });
        });
        let backend = cx.global::<SharedBackend>().clone();
        let model = cx.background_spawn(async move { backend.default_model() });
        let model = cx.spawn(async move |this, cx| {
            let model = model.await;
            this.update(cx, |this, cx| {
                this.default_model = model;
                cx.notify();
            })
            .ok();
        });
        Self {
            session: None,
            draft: Draft {
                mode: Mode::Agent,
                folder: None,
            },
            recent_folders: Vec::new(),
            home: cx.global::<HomeFolder>().0.clone(),
            default_model: None,
            composer,
            width: WidthClass::Wide,
            _model: model,
            _prefs: prefs,
        }
    }

    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// Makes the new session a chat or an agent session.
    pub fn set_draft_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.draft.mode != mode {
            self.draft.mode = mode;
            cx.notify();
        }
    }

    /// Runs the new agent session in `folder`.
    pub fn set_draft_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.draft.folder = Some(folder);
        cx.notify();
    }

    /// The folders Change… offers, from the sessions in the sidebar.
    pub fn set_recent_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.recent_folders != folders {
            self.recent_folders = folders;
            cx.notify();
        }
    }

    /// Where the new agent session will run: the folder chosen, else the
    /// most recent one, else the home folder.
    pub fn draft_folder(&self) -> Option<PathBuf> {
        self.draft
            .folder
            .clone()
            .or_else(|| self.recent_folders.first().cloned())
            .or_else(|| self.home.clone())
    }

    /// The line the empty transcript of a new session shows: "New agent
    /// session in ~/src/arko", or "New chat". `None` once a session is open.
    pub fn draft_line(&self) -> Option<String> {
        if self.session.is_some() {
            return None;
        }
        Some(match self.draft.mode {
            Mode::Chat => "New chat".to_string(),
            Mode::Agent => match self.draft_folder() {
                Some(folder) => format!(
                    "New agent session in {}",
                    shown_folder(&folder, self.home.as_deref())
                ),
                None => "New agent session".to_string(),
            },
        })
    }

    pub fn session(&self) -> Option<&SessionEntry> {
        self.session.as_ref()
    }

    /// Shows `session`, or a new session when `None`.
    pub fn set_session(&mut self, session: Option<SessionEntry>, cx: &mut Context<Self>) {
        self.session = session;
        cx.notify();
    }

    pub fn set_width(&mut self, width: WidthClass, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }

    /// The composer's text field.
    pub fn composer(&self) -> &Entity<TextareaState> {
        &self.composer
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        self.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
    }

    /// The mode the composer's chip shows: the open session's, or the
    /// new session's.
    fn mode(&self) -> Mode {
        self.session
            .as_ref()
            .map_or(self.draft.mode, |session| session.mode)
    }

    /// The chip's text: the mode, and the model where there's room. The
    /// model is arko's id for it ("claude-opus-5-5") until the engine gives
    /// its display name and effort ("Opus 5.5 · high").
    pub fn chip_label(&self) -> String {
        let mode = match self.mode() {
            Mode::Chat => "Chat",
            Mode::Agent => "Agent",
        };
        let model = match &self.session {
            Some(session) => Some(session.model.as_str()),
            None => self.default_model.as_deref(),
        };
        match model {
            Some(model) if !model.is_empty() && self.width != WidthClass::Narrow => {
                format!("{mode} · {model}")
            }
            _ => mode.to_string(),
        }
    }

    /// The chip's popover: Chat or Agent, switched freely before the first
    /// message. The model list comes with the engine.
    fn render_mode_chip(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        Popover::new("mode-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(
                Button::new("mode-model")
                    .ghost()
                    .small()
                    .label(self.chip_label())
                    .dropdown_caret(true),
            )
            .content(move |_, _, cx| {
                let Some(view) = view.upgrade() else {
                    return div().into_any_element();
                };
                let (mode, started) = {
                    let view = view.read(cx);
                    (view.mode(), view.session.is_some())
                };
                let theme = cx.theme();
                v_flex()
                    .id("mode-choice")
                    .w(rems(16.))
                    .gap_2()
                    .child(
                        ToggleGroup::new("mode")
                            .segmented()
                            .small()
                            .disabled(started)
                            .child(
                                Toggle::new("mode-chat")
                                    .label("Chat")
                                    .checked(mode == Mode::Chat),
                            )
                            .child(
                                Toggle::new("mode-agent")
                                    .label("Agent")
                                    .checked(mode == Mode::Agent),
                            )
                            .on_click({
                                let view = view.clone();
                                move |checks, _, cx| {
                                    // The toggle whose state changed is the one clicked.
                                    let clicked = if checks[0] != (mode == Mode::Chat) {
                                        Mode::Chat
                                    } else {
                                        Mode::Agent
                                    };
                                    view.update(cx, |view, cx| view.set_draft_mode(clicked, cx));
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(if started {
                                "Switching an open session comes with the engine, as do the models."
                            } else {
                                "Choosing the model comes with the engine."
                            }),
                    )
                    .into_any_element()
            })
    }

    /// The one quiet line in a new session's empty transcript, with
    /// Change… for an agent session's folder.
    fn render_draft_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let folder = self.draft_folder();
        let recent = self.recent_folders.clone();
        let home = self.home.clone();
        let view = cx.entity().downgrade();
        h_flex()
            .id("draft-line")
            .test_support()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .map(|this| match self.draft.mode {
                Mode::Chat => this.child("New chat"),
                Mode::Agent => this
                    .child(match &folder {
                        Some(_) => "New agent session in",
                        None => "New agent session",
                    })
                    .when_some(folder.clone(), |this, folder| {
                        this.child(
                            div()
                                .text_color(theme.foreground)
                                .child(shown_folder(&folder, home.as_deref())),
                        )
                    })
                    .child(
                        Button::new("change-folder")
                            .link()
                            .small()
                            .label("Change…")
                            .dropdown_menu(move |menu, _, _| {
                                let menu = recent.iter().fold(menu, |menu, path| {
                                    let view = view.clone();
                                    let choice = path.clone();
                                    menu.item(
                                        PopupMenuItem::new(shown_folder(path, home.as_deref()))
                                            .checked(folder.as_ref() == Some(path))
                                            .on_click(move |_, _, cx| {
                                                let choice = choice.clone();
                                                view.update(cx, |view, cx| {
                                                    view.set_draft_folder(choice, cx)
                                                })
                                                .ok();
                                            }),
                                    )
                                });
                                let view = view.clone();
                                menu.when(!recent.is_empty(), |menu| menu.separator()).item(
                                    PopupMenuItem::new("Browse…").on_click(move |_, _, cx| {
                                        browse(view.clone(), cx);
                                    }),
                                )
                            }),
                    ),
            })
    }

    fn render_composer(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("composer")
            .w_full()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(theme.input)
            .rounded(theme.radius_lg)
            .bg(theme.background)
            .child(
                Textarea::new(&self.composer)
                    .appearance(false)
                    .aria_label("Message"),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        Button::new("attach")
                            .ghost()
                            .small()
                            .compact()
                            .icon(Icon::new(gpui_kit::assets::IconName::Paperclip))
                            .tooltip("Attach")
                            .on_click(|_, window, cx| not_yet("Attaching files", window, cx)),
                    )
                    .child(self.render_mode_chip(cx))
                    .child(div().flex_1())
                    .child(
                        Button::new("send")
                            .primary()
                            .small()
                            .compact()
                            .icon(IconName::ArrowUp)
                            .tooltip("Send")
                            .on_click(|_, window, cx| not_yet("Sending", window, cx)),
                    ),
            )
    }
}

/// GPUI Kit's keybinding scheme for the composer.
fn text_keymap(keymap: prefs::Keymap) -> Keymap {
    match keymap {
        prefs::Keymap::Standard => Keymap::Cua,
        prefs::Keymap::Emacs => Keymap::Emacs,
        prefs::Keymap::Vim => Keymap::Vim,
    }
}

/// Asks for a folder for the new agent session.
fn browse(view: WeakEntity<SessionView>, cx: &mut App) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose".into()),
    });
    cx.spawn(async move |cx| {
        if let Ok(Ok(Some(paths))) = paths.await
            && let Some(folder) = paths.into_iter().next()
        {
            view.update(cx, |view, cx| view.set_draft_folder(folder, cx))
                .ok();
        }
    })
    .detach();
}

/// The folders of `sessions` that ran agents, the most recently active
/// first, each once.
pub fn recent_folders(sessions: &[SessionEntry]) -> Vec<PathBuf> {
    let mut agents: Vec<&SessionEntry> = sessions
        .iter()
        .filter(|session| session.mode == Mode::Agent && session.workspace.is_some())
        .collect();
    agents.sort_by_key(|session| std::cmp::Reverse(session.activity()));
    let mut folders: Vec<PathBuf> = Vec::new();
    for folder in agents
        .into_iter()
        .filter_map(|session| session.workspace.clone())
    {
        if !folders.contains(&folder) {
            folders.push(folder);
        }
    }
    folders.truncate(RECENT_FOLDERS);
    folders
}

/// The user's home folder, as the app found it at start-up.
pub struct HomeFolder(pub Option<PathBuf>);

impl Global for HomeFolder {}

/// The user's home folder, from the environment.
pub fn home_from_env() -> Option<PathBuf> {
    let name = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(name)
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
}

/// `folder` as people read it: under the home folder as `~/…` on macOS and
/// Linux, in full on Windows.
fn shown_folder(folder: &Path, home: Option<&Path>) -> String {
    shown_folder_from(folder, home.filter(|_| !cfg!(windows)))
}

fn shown_folder_from(folder: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// Says that a part of the app waits for arko's engine.
pub fn not_yet(what: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(format!("{what} comes with the engine."), cx);
}

impl Render for SessionView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let narrow = self.width == WidthClass::Narrow;
        let opened = self.session.is_some();
        v_flex()
            .id("session-view")
            .test_support()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .items_center()
            .child(
                // The reading column: centred and at most ~46 rem wide, full
                // width on narrow windows.
                v_flex()
                    .size_full()
                    .when(!narrow, |this| this.max_w(rems(46.)))
                    .px_4()
                    .child(
                        div()
                            .id("transcript")
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .when(opened, |this| {
                                this.child("The transcript comes with the engine.")
                            })
                            .when(!opened, |this| this.child(self.render_draft_line(cx))),
                    )
                    .child(div().pb_3().child(self.render_composer(cx))),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{recent_folders, shown_folder_from};
    use crate::backend::Backend as _;
    use crate::backend::{FakeBackend, Mode, SessionEntry};
    use std::path::{Path, PathBuf};
    use std::time::SystemTime;

    #[test]
    fn recent_folders_are_agent_folders_latest_first_each_once() {
        let backend = FakeBackend::sample("/arko", SystemTime::now());
        let sessions: Vec<SessionEntry> = backend.sessions().unwrap();
        assert!(sessions.iter().any(|session| session.mode == Mode::Chat));
        assert_eq!(
            recent_folders(&sessions),
            [
                PathBuf::from("/home/ada/src/arko"),
                PathBuf::from("/home/ada/src/gpui_kit")
            ]
        );
    }

    #[test]
    fn folders_under_home_read_from_a_tilde() {
        let home = Some(Path::new("/home/ada"));
        assert_eq!(
            shown_folder_from(Path::new("/home/ada/src/arko"), home),
            "~/src/arko"
        );
        assert_eq!(shown_folder_from(Path::new("/home/ada"), home), "~");
        assert_eq!(shown_folder_from(Path::new("/srv/data"), home), "/srv/data");
        assert_eq!(
            shown_folder_from(Path::new("/home/adam"), home),
            "/home/adam"
        );
        assert_eq!(shown_folder_from(Path::new("/srv"), None), "/srv");
    }
}
