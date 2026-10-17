//! The main column: the transcript's reading column and the composer under
//! it, for the session the window shows, or a new one before its first
//! message.
//!
//! The composer is a view of its own (typing draws only it). This view
//! decides what a sent message is: a new session's first message, a
//! message, or a note while a turn runs.

use std::path::{Path, PathBuf};

use gpui_kit::component::kbd::Kbd;
use gpui_kit::component::tooltip::Tooltip;
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

use crate::backend::{Mode, ModelChoices, NewSession, SessionEntry, SharedBackend};
use crate::composer::{Composer, ComposerEvent};
use crate::layout::WidthClass;
use crate::live::{LiveSession, Phase};
use crate::prefs;

/// How many recent folders Change… offers.
const RECENT_FOLDERS: usize = 8;

/// A new session, before its first message: how it will work, where and on
/// which model.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draft {
    pub mode: Mode,
    /// The folder chosen with Change…. `None` follows the most recent one.
    pub folder: Option<PathBuf>,
    /// The model chosen in the chip's popover; `None`: the default.
    pub model: Option<String>,
    pub effort: Option<String>,
}

/// What the main column asks of the window.
#[derive(Debug, Clone)]
pub enum SessionViewEvent {
    /// A new session started with its first message: the window shows it.
    Created(Entity<LiveSession>),
}

impl EventEmitter<SessionViewEvent> for SessionView {}

/// What the main column shows: a session, or a new one not started yet.
pub struct SessionView {
    session: Option<Entity<LiveSession>>,
    draft: Draft,
    /// The folders of recent agent sessions, the latest first.
    recent_folders: Vec<PathBuf>,
    /// The user's home folder, the last place a new session falls back on.
    home: Option<PathBuf>,
    /// The models a new session can start on, for the folder they were
    /// listed for.
    models: Option<(Option<PathBuf>, ModelChoices)>,
    composer: Entity<Composer>,
    composer_empty: bool,
    width: WidthClass,
    _session: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    _models: Option<Task<()>>,
}

impl SessionView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let composer = cx.new(|cx| Composer::new(window, cx));
        let subscriptions = vec![cx.subscribe_in(&composer, window, Self::on_composer_event)];
        let mut this = Self {
            session: None,
            draft: Draft::default(),
            recent_folders: Vec::new(),
            home: cx.global::<HomeFolder>().0.clone(),
            models: None,
            composer,
            composer_empty: true,
            width: WidthClass::Wide,
            _session: Vec::new(),
            _subscriptions: subscriptions,
            _models: None,
        };
        this.sync_composer(window, cx);
        this.list_models(cx);
        this
    }

    pub fn draft(&self) -> &Draft {
        &self.draft
    }

    /// Makes the new session a chat or an agent session.
    pub fn set_draft_mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.draft.mode != mode {
            self.draft.mode = mode;
            self.list_models(cx);
            cx.notify();
        }
    }

    /// Runs the new agent session in `folder`.
    pub fn set_draft_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        self.draft.folder = Some(folder);
        self.list_models(cx);
        cx.notify();
    }

    /// The folders Change… offers, from the sessions in the sidebar.
    pub fn set_recent_folders(&mut self, folders: Vec<PathBuf>, cx: &mut Context<Self>) {
        if self.recent_folders != folders {
            let before = self.draft_folder();
            self.recent_folders = folders;
            if self.draft_folder() != before {
                self.list_models(cx);
            }
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

    /// The folder whose `.env` files the new session reads: an agent's.
    fn models_folder(&self) -> Option<PathBuf> {
        match self.draft.mode {
            Mode::Agent => self.draft_folder(),
            Mode::Chat => None,
        }
    }

    /// Lists the models a new session can start on, in the background.
    fn list_models(&mut self, cx: &mut Context<Self>) {
        let folder = self.models_folder();
        if self
            .models
            .as_ref()
            .is_some_and(|(listed, _)| *listed == folder)
        {
            return;
        }
        let backend = cx.global::<SharedBackend>().clone();
        let listing = cx.background_spawn({
            let folder = folder.clone();
            async move { backend.models(folder.as_deref()) }
        });
        self._models = Some(cx.spawn(async move |this, cx| {
            let models = listing.await;
            this.update(cx, |this, cx| {
                this.models = Some((folder, models));
                cx.notify();
            })
            .ok();
        }));
    }

    /// The model a new session would start on.
    pub fn draft_model(&self) -> Option<String> {
        self.draft.model.clone().or_else(|| {
            self.models
                .as_ref()
                .and_then(|(_, models)| models.default.clone())
        })
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

    pub fn live_session(&self) -> Option<&Entity<LiveSession>> {
        self.session.as_ref()
    }

    /// The open session's entry, for the window's title and tests.
    pub fn session(&self, cx: &App) -> Option<SessionEntry> {
        let session = self.session.as_ref()?.read(cx);
        Some(SessionEntry {
            id: session
                .id()
                .cloned()
                .unwrap_or_else(|| crate::backend::SessionId(String::new())),
            title: session.title().to_string(),
            mode: session.mode(),
            workspace: session.workspace().cloned(),
            model: session.model().to_string(),
            created: std::time::SystemTime::now(),
            last_active: None,
            conversations: Vec::new(),
            holder: None,
            had_workers: !session.workers().is_empty(),
        })
    }

    /// Shows `session`, or a new session when `None`.
    pub fn set_session(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.as_ref().map(Entity::entity_id) == session.as_ref().map(Entity::entity_id) {
            return;
        }
        if let Some(old) = self.session.take() {
            old.update(cx, |old, cx| old.set_shown(false, cx));
        }
        self._session.clear();
        self.session = session;
        if let Some(session) = &self.session {
            session.update(cx, |session, cx| session.set_shown(true, cx));
            self._session
                .push(cx.observe_in(session, window, |this, _, window, cx| {
                    this.sync_composer(window, cx);
                    cx.notify();
                }));
        }
        self.sync_composer(window, cx);
        cx.notify();
    }

    pub fn set_width(&mut self, width: WidthClass, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }

    /// The composer.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        let textarea = self.composer.read(cx).textarea().clone();
        textarea.update(cx, |textarea, cx| textarea.focus(window, cx));
    }

    /// The mode the composer's chip shows: the open session's, or the
    /// new session's.
    fn mode(&self, cx: &App) -> Mode {
        self.session
            .as_ref()
            .map_or(self.draft.mode, |session| session.read(cx).mode())
    }

    /// What the composer says while empty.
    pub fn placeholder(&self, cx: &App) -> &'static str {
        let Some(session) = &self.session else {
            return "Message Arko";
        };
        let session = session.read(cx);
        if session.is_working() {
            "Add to the task — Arko is working"
        } else {
            "Message Arko"
        }
    }

    /// Gives the composer the session's state: its placeholder, whether Esc
    /// stops a turn.
    fn sync_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let placeholder = self.placeholder(cx);
        let working = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_working());
        self.composer.update(cx, |composer, cx| {
            composer.set_placeholder(placeholder, window, cx);
            composer.set_working(working);
        });
    }

    fn on_composer_event(
        &mut self,
        _: &Entity<Composer>,
        event: &ComposerEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            ComposerEvent::Submit(text) => self.send(text.clone(), window, cx),
            ComposerEvent::Stop => self.stop(window, cx),
            ComposerEvent::Edited => {
                let empty = self.composer.read(cx).textarea().read(cx).text().len() == 0;
                if empty != self.composer_empty {
                    self.composer_empty = empty;
                    cx.notify();
                }
            }
        }
    }

    /// Sends what the composer holds (the Send button).
    pub fn send_composer(&mut self, cx: &mut Context<Self>) {
        self.composer.update(cx, |composer, cx| composer.send(cx));
    }

    /// Sends `text`: starts a new session with it, or gives it to the open
    /// session as a message (a note while a turn runs). The composer
    /// empties once it is taken.
    pub fn send(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let result = match &self.session {
            None => self.start_session(text.clone(), cx),
            Some(session) => {
                let input = arko::Input::Message(arko::Message {
                    text: text.clone(),
                    images: Vec::new(),
                });
                session.update(cx, |session, cx| session.send(input, cx))
            }
        };
        match result {
            Ok(()) => {
                self.composer
                    .update(cx, |composer, cx| composer.sent(window, cx));
                self.composer_empty = true;
                cx.notify();
            }
            Err(why) => window.push_notification(why, cx),
        }
    }

    /// Starts the new session with `text` as its first message.
    fn start_session(&mut self, text: String, cx: &mut Context<Self>) -> Result<(), String> {
        let folder = match self.draft.mode {
            Mode::Agent => Some(
                self.draft_folder()
                    .ok_or("Choose a folder for the agent to work in first.")?,
            ),
            Mode::Chat => None,
        };
        let new = NewSession {
            mode: self.draft.mode,
            folder,
            model: self.draft.model.clone(),
            effort: self.draft.effort.clone(),
        };
        let first = arko::Input::Message(arko::Message {
            text,
            images: Vec::new(),
        });
        let session = LiveSession::create(new, first, cx);
        self.draft.model = None;
        self.draft.effort = None;
        cx.emit(SessionViewEvent::Created(session));
        Ok(())
    }

    /// Stops the turn that runs (Esc, Stop, Mod+.).
    pub fn stop(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        if !session.read(cx).is_working() {
            return;
        }
        if let Err(why) = session.update(cx, |session, cx| session.send(arko::Input::Interrupt, cx))
        {
            window.push_notification(why, cx);
        }
    }

    /// Moves the shell command the main agent runs to the background
    /// (Ctrl+B).
    pub fn move_to_background(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = &self.session else {
            return;
        };
        let running = session
            .read(cx)
            .reading()
            .commands
            .iter()
            .any(|agent| agent == arko::remote::MAIN_AGENT);
        if !running {
            window.push_notification("No command is running in the foreground.", cx);
            return;
        }
        let input = arko::Input::Background(arko::remote::MAIN_AGENT.to_string());
        if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
            window.push_notification(why, cx);
        }
    }

    /// Attach comes with the composer's tools.
    fn attach(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        not_yet("Attaching files", window, cx);
    }

    /// The chip's text: the mode, and the model where there's room ("Agent
    /// · Opus 5.5 · high"); on narrow windows the model alone.
    pub fn chip_label(&self, cx: &App) -> String {
        let mode = self.mode(cx).label();
        let (model, effort) = match &self.session {
            Some(session) => {
                let session = session.read(cx);
                let models = &session.reading().models;
                (
                    Some(session.model().to_string()).filter(|model| !model.is_empty()),
                    Some(models.main_effort.clone()).filter(|effort| !effort.is_empty()),
                )
            }
            None => (
                self.draft_model()
                    .map(|model| arko::models::display_name(&model)),
                self.draft.effort.clone(),
            ),
        };
        match (model, self.width) {
            (Some(model), WidthClass::Narrow) => model,
            (Some(model), _) => {
                let mut label = format!("{mode} · {model}");
                if let Some(effort) = effort {
                    label.push_str(&format!(" · {effort}"));
                }
                label
            }
            (None, _) => mode.to_string(),
        }
    }

    /// The chip's popover: Chat or Agent, then the models by the way they
    /// are reached, then the effort of the chosen one.
    fn render_mode_chip(&self, cx: &mut Context<Self>) -> AnyElement {
        let view = cx.entity().downgrade();
        Popover::new("mode-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(
                Button::new("mode-model")
                    .ghost()
                    .small()
                    .label(self.chip_label(cx))
                    .dropdown_caret(true),
            )
            .content(move |_, window, cx| {
                let popover = cx.entity();
                let Some(view) = view.upgrade() else {
                    return div().into_any_element();
                };
                view.update(cx, |view, cx| view.render_mode_popover(popover, window, cx))
                    .into_any_element()
            })
            .into_any_element()
    }

    fn render_mode_popover(
        &mut self,
        _: Entity<gpui_kit::component::popover::PopoverState>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let mode = self.mode(cx);
        let view = cx.entity().downgrade();
        v_flex()
            .id("mode-choice")
            .w(rems(18.))
            .gap_2()
            .child(
                ToggleGroup::new("mode")
                    .segmented()
                    .small()
                    // HOOK(engine): an open session switches with D4.
                    .disabled(self.session.is_some())
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
                            view.update(cx, |view, cx| view.set_draft_mode(clicked, cx))
                                .ok();
                        }
                    }),
            )
            .into_any_element()
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

    /// What the transcript's place shows until the transcript view comes:
    /// the session's state.
    fn render_transcript(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let Some(session) = &self.session else {
            return self.render_draft_line(cx).into_any_element();
        };
        let session = session.read(cx);
        let text: SharedString = match session.phase() {
            Phase::Opening if session.id().is_none() => "Starting the session…".into(),
            Phase::Opening => "Opening the session…".into(),
            Phase::Open | Phase::Closing => {
                // HOOK(transcript): the transcript view (A2-transcript).
                format!("{} · {}", session.title(), session.model()).into()
            }
            Phase::Sample => "A sample session: it shows, but doesn’t run.".into(),
            Phase::Ended => "The session has closed.".into(),
            Phase::Failed(why) => why.clone().into(),
        };
        div()
            .id("transcript-state")
            .test_support()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(text)
            .into_any_element()
    }

    fn render_composer(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let chip = self.render_mode_chip(cx);
        let theme = cx.theme();
        let working = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).is_working());
        let send_key = prefs::get(cx).input.send_key;
        let send_chord = Keystroke::parse(crate::composer::send_chord(send_key)).ok();
        let _ = window;
        v_flex()
            .id("composer")
            .test_support()
            .relative()
            .w_full()
            .gap_1()
            .p_2()
            .border_1()
            .border_color(theme.input)
            .rounded(theme.radius_lg)
            .bg(theme.background)
            .child(self.composer.clone())
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
                            .on_click(cx.listener(|this, _, window, cx| this.attach(window, cx))),
                    )
                    .child(chip)
                    .child(div().flex_1())
                    .map(|this| {
                        // The tooltips name the keys: Esc stops, the send key sends.
                        let (label, chord) = if working {
                            ("Stop", Keystroke::parse("escape").ok())
                        } else {
                            ("Send", send_chord.clone())
                        };
                        let button = if working {
                            Button::new("stop")
                                .primary()
                                .small()
                                .compact()
                                .icon(IconName::Square)
                                .on_click(cx.listener(|this, _, window, cx| this.stop(window, cx)))
                        } else {
                            Button::new("send")
                                .primary()
                                .small()
                                .compact()
                                .icon(IconName::ArrowUp)
                                .disabled(self.composer_empty)
                                .on_click(cx.listener(|this, _, _, cx| this.send_composer(cx)))
                        };
                        this.child(
                            div()
                                .id("send-or-stop")
                                .tooltip(move |window, cx| {
                                    Tooltip::new(label)
                                        .key_binding(chord.clone().map(Kbd::new))
                                        .build(window, cx)
                                })
                                .child(button.accessibility_label(label)),
                        )
                    }),
            )
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
pub fn shown_folder(folder: &Path, home: Option<&Path>) -> String {
    shown_folder_from(folder, home.filter(|_| !cfg!(windows)))
}

fn shown_folder_from(folder: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| folder.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => folder.display().to_string(),
    }
}

/// Says that a part of the app comes later.
pub fn not_yet(what: &str, window: &mut Window, cx: &mut App) {
    window.push_notification(format!("{what} comes with a later version."), cx);
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let narrow = self.width == WidthClass::Narrow;
        let theme = cx.theme();
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
                            .child(self.render_transcript(cx)),
                    )
                    .child(v_flex().pb_3().child(self.render_composer(window, cx))),
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
