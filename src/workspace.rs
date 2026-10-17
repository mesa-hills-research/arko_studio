//! A window: the title bar, the sessions sidebar, the main column (a
//! session, or Settings), the workers panel and the status bar, laid out
//! for the window's width.

use gpui_kit::component::{
    ActiveTheme as _, Placement, WindowExt as _, h_flex, notification::Notification, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::appearance;
use crate::backend::{SessionEntry, SessionId};
use crate::layout::WidthClass;
use crate::live::{LiveEvent, LiveSession, LiveSessions, Phase};
use crate::prefs::{self, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};
use crate::session_view::{Branched, SessionView, SessionViewEvent, recent_folders};
use crate::settings_page::{CloseRequested, SettingsPage};
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::status_bar::{STATUS_BAR_HEIGHT, StatusBar};
use crate::title_bar::AppTitleBar;
use crate::workers_panel::{WorkersPanel, WorkersPanelEvent};
use crate::{about, chrome, menus, palette, prompts, quit, shortcuts};

/// The Settings page of models and accounts.
const ACCOUNTS_PAGE: usize = 2;
/// The Settings page of the web page.
const WEB_PAGE_PAGE: usize = 5;

/// The narrowest and widest the workers panel docks, in rem.
const MIN_PANEL_WIDTH: f32 = 16.;
const MAX_PANEL_WIDTH: f32 = 36.;
/// The sidebar's and the panel's width as sheets, in rem.
const SHEET_WIDTH: f32 = 18.;

/// Opens a window showing a new session.
pub fn open(cx: &mut App) -> Option<AnyWindowHandle> {
    open_with(None, cx)
}

/// Opens a window with `options`, and gives back its workspace.
pub fn open_window(
    options: WindowOptions,
    cx: &mut App,
) -> Result<(AnyWindowHandle, Entity<Workspace>)> {
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| Workspace::new(window, cx))
    })
}

/// Opens a window, showing `session` when given.
pub fn open_with(session: Option<SessionId>, cx: &mut App) -> Option<AnyWindowHandle> {
    match open_window(chrome::window_options(cx), cx) {
        Ok((handle, workspace)) => {
            if let Some(id) = session {
                handle
                    .update(cx, |_, window, cx| {
                        workspace.update(cx, |workspace, cx| workspace.open_session(id, window, cx))
                    })
                    .ok();
            }
            Some(handle)
        }
        Err(error) => {
            tracing::error!("couldn't open a window: {error}");
            None
        }
    }
}

/// What's being dragged to resize.
#[derive(Clone, Copy, PartialEq)]
enum Edge {
    Sidebar,
    Panel,
}

#[derive(Clone)]
struct ResizeDrag(Edge);

impl Render for ResizeDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        Empty
    }
}

pub struct Workspace {
    focus_handle: FocusHandle,
    title_bar: Entity<AppTitleBar>,
    status_bar: Entity<StatusBar>,
    sidebar: Entity<Sidebar>,
    session_view: Entity<SessionView>,
    workers: Entity<WorkersPanel>,
    settings: Option<Entity<SettingsPage>>,
    /// The session this window shows. It runs on whether shown or not.
    session: Option<Entity<LiveSession>>,
    _session: Vec<Subscription>,
    /// The title the window last showed.
    shown_title: Option<SharedString>,
    width: WidthClass,
    workers_open: bool,
    /// The pane showing as a sheet over a narrow window.
    sheet: Option<Edge>,
    /// The sidebar's state and the panes' widths in the preferences, as the
    /// window last followed them.
    panes: (bool, f32, f32),
    /// Widths while a drag resizes them, in rem; saved when it ends.
    sidebar_width: f32,
    panel_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.on_window_should_close(cx, chrome::can_close);
        appearance::set_system_appearance(window.appearance(), cx);

        let title_bar = cx.new(|cx| AppTitleBar::new(window, cx));
        let status_bar = cx.new(|_| StatusBar::new());
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let session_view = cx.new(|cx| SessionView::new(window, cx));
        let workers = cx.new(|cx| WorkersPanel::new(window, cx));
        let sidebar_prefs = prefs::get(cx).sidebar.clone();
        let panel_width = prefs::get(cx).workers_panel.width;

        let subscriptions = vec![
            cx.observe_window_appearance(window, |_, window, cx| {
                appearance::set_system_appearance(window.appearance(), cx);
            }),
            cx.observe_window_bounds(window, |this, window, cx| {
                chrome::remember_placement(window, cx);
                this.update_width(window, cx);
            }),
            // Coming back to the window lists the sessions again: the
            // terminal may have made or changed some.
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
                }
            }),
            cx.subscribe_in(&sidebar, window, Self::on_sidebar_event),
            cx.subscribe_in(&session_view, window, Self::on_session_view_event),
            cx.subscribe_in(&workers, window, Self::on_workers_event),
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                this.prefs_changed(window, cx)
            }),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            title_bar,
            status_bar,
            sidebar,
            session_view,
            workers,
            settings: None,
            session: None,
            _session: Vec::new(),
            shown_title: None,
            width: WidthClass::of(window.viewport_size().width),
            workers_open: false,
            sheet: None,
            panes: (sidebar_prefs.open, sidebar_prefs.width(), panel_width),
            sidebar_width: sidebar_prefs.width(),
            panel_width,
            _subscriptions: subscriptions,
        };
        this.sync_chrome(window, cx);
        this.session_view
            .update(cx, |view, cx| view.set_width(this.width, cx));
        this.status_bar
            .update(cx, |bar, cx| bar.set_width(this.width, cx));
        this.focus_composer(window, cx);
        this
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        let textarea = self
            .session_view
            .read(cx)
            .composer()
            .read(cx)
            .textarea()
            .clone();
        textarea.update(cx, |textarea, cx| textarea.focus(window, cx));
    }

    /// The session this window shows.
    pub fn live_session(&self) -> Option<&Entity<LiveSession>> {
        self.session.as_ref()
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn session_view(&self) -> &Entity<SessionView> {
        &self.session_view
    }

    pub fn status_bar(&self) -> &Entity<StatusBar> {
        &self.status_bar
    }

    pub fn title_bar(&self) -> &Entity<AppTitleBar> {
        &self.title_bar
    }

    pub fn settings(&self) -> Option<&Entity<SettingsPage>> {
        self.settings.as_ref()
    }

    pub fn width_class(&self) -> WidthClass {
        self.width
    }

    pub fn is_workers_open(&self) -> bool {
        self.workers_open
    }

    /// Whether the sidebar shows docked beside the main column.
    pub fn is_sidebar_docked(&self, cx: &App) -> bool {
        self.width.docks_sidebar() && prefs::get(cx).sidebar.open
    }

    fn update_width(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let width = WidthClass::of(window.viewport_size().width);
        if width == self.width {
            return;
        }
        let was = self.width;
        self.width = width;
        // A sheet opened for a narrower window gives way to the docked view.
        let docks = match self.sheet {
            Some(Edge::Sidebar) => !was.docks_sidebar() && width.docks_sidebar(),
            Some(Edge::Panel) => !was.docks_panel() && width.docks_panel(),
            None => false,
        };
        if docks {
            self.sheet = None;
            window.defer(cx, |window, cx| window.close_sheet(cx));
        }
        self.sync_chrome(window, cx);
        self.session_view
            .update(cx, |view, cx| view.set_width(width, cx));
        self.status_bar
            .update(cx, |bar, cx| bar.set_width(width, cx));
        cx.notify();
    }

    /// Follows the preferences the window's layout takes: whether the
    /// sidebar shows, and the panes' widths. Others (the window's own
    /// placement, saved as it moves) leave it as it is.
    fn prefs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prefs = prefs::get(cx);
        let panes = (
            prefs.sidebar.open,
            prefs.sidebar.width(),
            prefs.workers_panel.width,
        );
        if panes == self.panes {
            return;
        }
        self.panes = panes;
        self.sidebar_width = panes.1;
        self.panel_width = panes.2;
        self.sync_chrome(window, cx);
        cx.notify();
    }

    /// Shows the open session's title in the title bar and window switchers.
    fn sync_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let session = self.session.as_ref().map(|session| session.read(cx));
        let title = session
            .map(|session| session.title().clone())
            .unwrap_or_else(|| "New session".into());
        let folder = session
            .and_then(|session| session.workspace())
            .and_then(|folder| folder.file_name())
            .map(|name| SharedString::from(name.to_string_lossy().into_owned()));
        let had_workers = session.is_some_and(|session| !session.workers().is_empty())
            || self.session.as_ref().is_some_and(|session| {
                let id = session.read(cx).id();
                self.sidebar
                    .read(cx)
                    .sessions()
                    .iter()
                    .any(|entry| Some(&entry.id) == id && entry.had_workers)
            });
        self.shown_title = Some(title.clone());
        let open = match self.width.docks_sidebar() {
            true => prefs::get(cx).sidebar.open,
            false => self.sheet == Some(Edge::Sidebar),
        };
        // The workers panel's button: for a session that has or had
        // workers, or while the panel shows anyway (from the View menu).
        let workers_open = match self.width.docks_panel() {
            true => self.workers_open,
            false => self.sheet == Some(Edge::Panel),
        };
        let workers = (had_workers || workers_open).then_some(workers_open);
        self.title_bar.update(cx, |bar, cx| {
            bar.set_session(Some(title.clone()), folder, cx);
            bar.set_sidebar_open(open, cx);
            bar.set_workers(workers, cx);
        });
        window.set_window_title(&format!("{title} – {}", crate::APP_NAME));
    }

    /// Shows the session `id` in this window: the one running in this app,
    /// or it opens. The session shown before keeps running.
    pub fn open_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).id() == Some(&id))
        {
            self.after_choosing(window, cx);
            return;
        }
        let session = match LiveSessions::find(&id, cx) {
            Some(session) => session,
            None => {
                let entry = self
                    .sidebar
                    .read(cx)
                    .sessions()
                    .iter()
                    .find(|entry| entry.id == id)
                    .cloned()
                    .unwrap_or_else(|| SessionEntry {
                        id: id.clone(),
                        title: "Session".into(),
                        mode: crate::backend::Mode::Agent,
                        workspace: None,
                        model: String::new(),
                        created: std::time::SystemTime::now(),
                        last_active: None,
                        conversations: Vec::new(),
                        holder: None,
                        had_workers: false,
                    });
                LiveSession::resume(&entry, cx)
            }
        };
        // Refused at once: open elsewhere, or gone.
        if let Phase::Failed(why) = session.read(cx).phase() {
            window.push_notification(
                Notification::error(format!("Couldn’t open the session. {why}")),
                cx,
            );
            return;
        }
        self.show(Some(session), window, cx);
        self.after_choosing(window, cx);
    }

    /// Shows the conversation `number` of the session `id`, as `/resume N`.
    fn open_conversation(
        &mut self,
        id: SessionId,
        number: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_session(id.clone(), window, cx);
        let Some(session) = self.session.clone() else {
            return;
        };
        let input = arko::Input::Message(arko::Message {
            text: format!("/resume {number}"),
            images: Vec::new(),
        });
        if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
            window.push_notification(why, cx);
        }
    }

    /// After a session is picked: the sheet closes and the composer has focus.
    fn after_choosing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sheet(window, cx);
        self.close_settings(window, cx);
        self.focus_composer(window, cx);
    }

    /// Closes the sidebar or the panel showing as a sheet.
    fn close_sheet(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.sheet.take().is_some() {
            window.close_sheet(cx);
            self.sync_chrome(window, cx);
        }
    }

    /// Shows a pane as a sheet: the sidebar from the left, the panel from
    /// the right.
    fn open_sheet(&mut self, edge: Edge, window: &mut Window, cx: &mut Context<Self>) {
        let view = match edge {
            Edge::Sidebar => AnyView::from(self.sidebar.clone()),
            Edge::Panel => AnyView::from(self.workers.clone()),
        };
        let placement = match edge {
            Edge::Sidebar => Placement::Left,
            Edge::Panel => Placement::Right,
        };
        let this = cx.entity().downgrade();
        window.open_sheet_at(placement, cx, move |sheet, _, _| {
            let this = this.clone();
            sheet
                .size(rems(SHEET_WIDTH))
                .p_0()
                // Escape and a click outside close it without asking us.
                .on_close(move |_, window, cx| {
                    this.update(cx, |this, cx| {
                        if this.sheet == Some(edge) {
                            this.sheet = None;
                            this.sync_chrome(window, cx);
                        }
                        if edge == Edge::Panel {
                            this.workers.update(cx, |panel, cx| panel.hidden(cx));
                        }
                    })
                    .ok();
                })
                .child(view.clone())
        });
        self.sheet = Some(edge);
        self.sync_chrome(window, cx);
    }

    /// Shows `session` here, or a new session when `None`.
    fn show(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self._session.clear();
        if let Some(session) = &session {
            self._session
                .push(cx.subscribe_in(session, window, Self::on_live_event));
            // The title follows the session's name as the engine gives it.
            self._session
                .push(cx.observe_in(session, window, |this, session, window, cx| {
                    if this.shown_title.as_ref() != Some(session.read(cx).title()) {
                        this.sync_chrome(window, cx);
                    }
                }));
        }
        let id = session
            .as_ref()
            .and_then(|session| session.read(cx).id().cloned());
        self.session = session.clone();
        self.status_bar
            .update(cx, |bar, cx| bar.set_session(session.clone(), cx));
        self.title_bar.update(cx, |bar, cx| {
            bar.set_live_session(session.clone(), window, cx)
        });
        self.session_view
            .update(cx, |view, cx| view.set_session(session.clone(), window, cx));
        self.workers
            .update(cx, |panel, cx| panel.set_session(session.clone(), cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_active(id, cx));
        self.sync_chrome(window, cx);
        // A question it asked while elsewhere is asked now.
        if let Some(session) = &session
            && session.read(cx).prompt().is_some()
        {
            prompts::show(session, window, cx);
        }
    }

    fn new_session(&mut self, _: &menus::NewSession, window: &mut Window, cx: &mut Context<Self>) {
        self.show(None, window, cx);
        self.after_choosing(window, cx);
    }

    /// What the session this window shows says.
    fn on_live_event(
        &mut self,
        session: &Entity<LiveSession>,
        event: &LiveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            LiveEvent::Opened => {
                // A new session has its id now, and a row in the list.
                let id = session.read(cx).id().cloned();
                self.sidebar.update(cx, |sidebar, cx| {
                    sidebar.set_active(id, cx);
                    sidebar.refresh(cx);
                });
                self.sync_chrome(window, cx);
            }
            LiveEvent::Failed(why) => {
                if let Some(why) = why {
                    window.push_notification(
                        Notification::error(format!("Couldn’t open the session. {why}")),
                        cx,
                    );
                }
                self.show(None, window, cx);
            }
            LiveEvent::Prompt => prompts::show(session, window, cx),
            LiveEvent::Note(level, text) => {
                let text = crate::backend::sentence(text.trim_start_matches("arko: "));
                window.push_notification(
                    match level {
                        arko::Level::Error => Notification::error(text),
                        arko::Level::Warn => Notification::warning(text),
                        _ => Notification::info(text),
                    },
                    cx,
                );
            }
            LiveEvent::Ended => {
                self.show(None, window, cx);
                self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
            }
            LiveEvent::TurnEnded => {
                self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
                self.sync_chrome(window, cx);
            }
            LiveEvent::QuestionClosed {
                answered_here: false,
            } if session.read(cx).is_working() => {
                window.push_notification(
                    Notification::info("The question was answered elsewhere."),
                    cx,
                );
            }
            // `/exit` typed with jobs or workers running: closing asks first.
            // (A close the app started asks through its own dialog.)
            LiveEvent::QuitAsked if *session.read(cx).phase() != Phase::Closing => {
                let Some(ask) = session.read(cx).quit_ask().cloned() else {
                    return;
                };
                if !prefs::get(cx).sessions.ask_before_quit {
                    session.update(cx, |session, cx| session.agree_to_quit(cx));
                    return;
                }
                let work = vec![(session.read(cx).title().clone(), ask)];
                let session = session.clone();
                prompts::confirm_stop(work, false, window, cx, move |agreed, _, cx| {
                    session.update(cx, |session, cx| {
                        if agreed {
                            session.agree_to_quit(cx)
                        } else {
                            session.cancel_close(cx)
                        }
                    });
                });
            }
            LiveEvent::Refused(why) => window.push_notification(why.clone(), cx),
            _ => {}
        }
    }

    fn on_session_view_event(
        &mut self,
        _: &Entity<SessionView>,
        event: &SessionViewEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SessionViewEvent::Created(session) => {
                if let Phase::Failed(why) = session.read(cx).phase() {
                    window.push_notification(
                        Notification::error(format!("Couldn’t start the session. {why}")),
                        cx,
                    );
                    return;
                }
                self.show(Some(session.clone()), window, cx);
                self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
            }
            SessionViewEvent::OpenAccounts(way) => self.open_accounts(way.as_deref(), window, cx),
            SessionViewEvent::ShowPrompt => {
                if let Some(session) = &self.session {
                    prompts::show(session, window, cx);
                }
            }
            SessionViewEvent::OpenBranch(branched) => {
                let Branched {
                    entry,
                    first,
                    draft,
                } = (**branched).clone();
                self.open_branch(entry, first, draft, window, cx)
            }
            SessionViewEvent::OpenWorker(id) => {
                self.show_workers(window, cx);
                self.workers
                    .update(cx, |panel, cx| panel.show_worker(id.clone(), window, cx));
            }
        }
    }

    /// Makes sure the workers panel shows: docked, or as a sheet.
    fn show_workers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.width.docks_panel() {
            if !self.workers_open {
                self.workers_open = true;
                self.sync_chrome(window, cx);
                cx.notify();
            }
        } else if self.sheet != Some(Edge::Panel) {
            self.open_sheet(Edge::Panel, window, cx);
        }
    }

    /// Opens a branch of the shown session (it keeps running): it sends
    /// `first` as its first message, or shows `draft` in its composer.
    fn open_branch(
        &mut self,
        entry: SessionEntry,
        first: Option<arko::Message>,
        draft: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(draft) = &draft {
            crate::drafts::set(&entry.id.0, draft, cx);
        }
        let session =
            LiveSessions::find(&entry.id, cx).unwrap_or_else(|| LiveSession::resume(&entry, cx));
        if let Phase::Failed(why) = session.read(cx).phase() {
            window.push_notification(
                Notification::error(format!("Couldn’t open the branch. {why}")),
                cx,
            );
            return;
        }
        if let Some(first) = first {
            let input = arko::Input::Message(first);
            if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
                window.push_notification(why, cx);
            }
        }
        self.show(Some(session), window, cx);
        self.sidebar.update(cx, |sidebar, cx| sidebar.refresh(cx));
        self.after_choosing(window, cx);
    }

    /// What the workers panel asks.
    fn on_workers_event(
        &mut self,
        _: &Entity<WorkersPanel>,
        event: &WorkersPanelEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            WorkersPanelEvent::Command(command) => {
                if let Some(session) = self.session.clone() {
                    let input = arko::Input::Message(arko::Message {
                        text: command.clone(),
                        images: Vec::new(),
                    });
                    if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
                        window.push_notification(why, cx);
                    }
                }
            }
            WorkersPanelEvent::Target(target) => {
                let target = target.clone();
                self.session_view
                    .update(cx, |view, cx| view.set_target(target, window, cx));
            }
            WorkersPanelEvent::Message(id) => {
                let target = Some(id.clone());
                self.session_view
                    .update(cx, |view, cx| view.set_target(target, window, cx));
                // A sheet gives way to the composer it covers.
                if self.sheet == Some(Edge::Panel) {
                    self.close_sheet(window, cx);
                }
                self.focus_composer(window, cx);
            }
            WorkersPanelEvent::Leave => {
                if self.sheet == Some(Edge::Panel) {
                    self.close_sheet(window, cx);
                }
                self.focus_composer(window, cx);
            }
        }
    }

    /// Settings › Models & accounts, on what the group of models `way`
    /// needs, if one is.
    fn open_accounts(&mut self, way: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.show_settings(window, cx);
        settings.update(cx, |settings, cx| {
            settings.show_page(ACCOUNTS_PAGE, cx);
            settings.point_at_account(way, window, cx);
        });
    }

    /// Serves the shown session's web page where Settings › Web page says,
    /// and shows its links and QR code once it's up.
    fn share_web_page(
        &mut self,
        _: &menus::ShareWebPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if session.read(cx).reading().page.is_none() {
            let page = prefs::get(cx).web_page.clone();
            if page.ip().is_none() {
                window.push_notification(
                    Notification::warning(format!(
                        "“{}” in Settings › Web page isn’t an IP address: the page is served on this computer only.",
                        page.address.trim()
                    )),
                    cx,
                );
            }
            if let Err(why) = crate::web_page::share(&session, &page.bind_argument(), cx) {
                window.push_notification(why, cx);
                return;
            }
        }
        self.status_bar
            .update(cx, |bar, cx| bar.show_web_page(window, cx));
    }

    /// Stops serving the shown session's web page.
    fn stop_sharing_web_page(
        &mut self,
        _: &menus::StopSharingWebPage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session.clone() else {
            return;
        };
        if session.read(cx).reading().page.is_none() {
            window.push_notification("The web page isn’t shared.", cx);
            return;
        }
        if let Err(why) = crate::web_page::stop(&session, cx) {
            window.push_notification(why, cx);
        }
    }

    /// Shows `session` here (tests).
    #[cfg(test)]
    pub fn show_session(
        &mut self,
        session: Entity<LiveSession>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show(Some(session), window, cx);
    }

    /// Closes the session shown here, asking first about what runs.
    fn close_shown_session(
        &mut self,
        _: &menus::CloseSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(session) = self.session.clone() {
            quit::close(vec![session], false, window, cx);
        }
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let shows = |this: &Self, id: &SessionId, cx: &App| {
            this.session
                .as_ref()
                .is_some_and(|session| session.read(cx).id() == Some(id))
        };
        match event {
            SidebarEvent::Open(id) => self.open_session(id.clone(), window, cx),
            SidebarEvent::OpenConversation(id, number) => {
                self.open_conversation(id.clone(), *number, window, cx)
            }
            SidebarEvent::OpenInNewWindow(id) => {
                if shows(self, id, cx) {
                    // It moves: the new window shows it, this one a new session.
                    self.show(None, window, cx);
                }
                let id = id.clone();
                cx.defer(move |cx| {
                    open_with(Some(id), cx);
                });
            }
            SidebarEvent::Renamed(id, _) => {
                if shows(self, id, cx) {
                    self.sync_chrome(window, cx);
                }
            }
            SidebarEvent::Listed => {
                let folders = recent_folders(self.sidebar.read(cx).sessions());
                self.session_view
                    .update(cx, |view, cx| view.set_recent_folders(folders, cx));
            }
            SidebarEvent::Deleted(id) => {
                if shows(self, id, cx) {
                    self.show(None, window, cx);
                }
            }
        }
    }

    fn toggle_sidebar(
        &mut self,
        _: &menus::ToggleSidebar,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.width.docks_sidebar() {
            prefs::update(cx, |prefs| prefs.sidebar.open = !prefs.sidebar.open);
        } else if self.sheet == Some(Edge::Sidebar) {
            self.close_sheet(window, cx);
        } else {
            self.open_sheet(Edge::Sidebar, window, cx);
        }
    }

    /// Makes sure the sidebar shows: docked, or as a sheet on narrow windows.
    fn reveal_sidebar(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.width.docks_sidebar() {
            if !prefs::get(cx).sidebar.open {
                prefs::update(cx, |prefs| prefs.sidebar.open = true);
            }
        } else if self.sheet != Some(Edge::Sidebar) {
            self.open_sheet(Edge::Sidebar, window, cx);
        }
    }

    fn focus_filter(
        &mut self,
        _: &menus::FocusSessionFilter,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reveal_sidebar(window, cx);
        let sidebar = self.sidebar.clone();
        window.defer(cx, move |window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.focus_filter(window, cx));
        });
    }

    fn rename_session(
        &mut self,
        _: &menus::RenameSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).id().cloned())
        else {
            return;
        };
        self.reveal_sidebar(window, cx);
        let sidebar = self.sidebar.clone();
        window.defer(cx, move |window, cx| {
            sidebar.update(cx, |sidebar, cx| sidebar.start_rename(id, window, cx));
        });
    }

    fn delete_session(
        &mut self,
        _: &menus::DeleteSession,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(id) = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).id().cloned())
        {
            self.sidebar
                .update(cx, |sidebar, cx| sidebar.confirm_delete(id, window, cx));
        }
    }

    fn toggle_workers(
        &mut self,
        _: &menus::ToggleWorkersPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let had_focus = self.workers.read(cx).contains_focus(window, cx);
        let open = if self.width.docks_panel() {
            self.workers_open = !self.workers_open;
            self.sync_chrome(window, cx);
            cx.notify();
            self.workers_open
        } else if self.sheet == Some(Edge::Panel) {
            self.close_sheet(window, cx);
            false
        } else {
            self.open_sheet(Edge::Panel, window, cx);
            true
        };
        // The keyboard goes with the panel: into it as it opens, back to the
        // composer when it closes with the focus in it.
        if open {
            let workers = self.workers.clone();
            window.defer(cx, move |window, cx| {
                workers.update(cx, |panel, cx| panel.focus(window, cx));
            });
        } else {
            self.workers.update(cx, |panel, cx| panel.hidden(cx));
            if had_focus {
                self.focus_composer(window, cx);
            }
        }
    }

    fn open_settings(
        &mut self,
        _: &menus::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.show_settings(window, cx);
    }

    /// Shows Settings in the main column, and gives back its page.
    fn show_settings(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<SettingsPage> {
        let settings = match &self.settings {
            Some(settings) => settings.clone(),
            None => {
                let settings = cx.new(|cx| SettingsPage::new(window, cx));
                self._subscriptions.push(cx.subscribe_in(
                    &settings,
                    window,
                    |this, _, _: &CloseRequested, window, cx| this.close_settings(window, cx),
                ));
                self.settings = Some(settings.clone());
                settings
            }
        };
        settings.read(cx).focus_handle(cx).focus(window, cx);
        cx.notify();
        settings
    }

    /// Opens the command palette over the window.
    fn command_palette(
        &mut self,
        _: &menus::CommandPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let sections = palette::sections(self.sidebar.read(cx).sessions());
        // The keyboard goes back where it was, and the command runs from there.
        let before = window.focused(cx);
        let this = cx.entity().downgrade();
        palette::open(
            sections,
            move |target, window, cx| {
                let before = before.clone();
                this.update(cx, |this, cx| this.run(target, before, window, cx))
                    .ok();
            },
            window,
            cx,
        );
    }

    /// Runs what the palette chose, as if from `focus`.
    pub fn run(
        &mut self,
        target: palette::Target,
        focus: Option<FocusHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match target {
            palette::Target::Action(action) => {
                let focus = focus.unwrap_or_else(|| self.focus_handle.clone());
                // After this update: the command may be one of this window's.
                window.defer(cx, move |window, cx| {
                    focus.focus(window, cx);
                    focus.dispatch_action(action.as_ref(), window, cx);
                });
            }
            palette::Target::SettingsPage(page) => {
                let settings = self.show_settings(window, cx);
                settings.update(cx, |settings, cx| settings.show_page(page, cx));
            }
            palette::Target::Session(id) => self.open_session(id, window, cx),
            palette::Target::Slash(name) => {
                self.close_settings(window, cx);
                self.session_view
                    .update(cx, |view, cx| view.run_slash(name, window, cx));
            }
        }
    }

    fn expand_composer(
        &mut self,
        _: &menus::ExpandComposer,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.focus_composer(window, cx);
        prefs::update(cx, |prefs| {
            prefs.input.composer_expanded = !prefs.input.composer_expanded
        });
    }

    fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings.take().is_some() {
            self.focus_composer(window, cx);
            cx.notify();
        }
    }

    fn about(&mut self, _: &menus::About, window: &mut Window, cx: &mut Context<Self>) {
        about::open(window, cx);
    }

    fn set_view(&mut self, view: prefs::TranscriptView, cx: &mut Context<Self>) {
        prefs::update(cx, |prefs| prefs.view = view);
    }

    fn resize(
        &mut self,
        edge: Edge,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rem = f32::from(window.rem_size());
        match edge {
            Edge::Sidebar => {
                let width = f32::from(position.x) / rem;
                self.sidebar_width = width.clamp(MIN_SIDEBAR_WIDTH, MAX_SIDEBAR_WIDTH);
            }
            Edge::Panel => {
                let width = f32::from(window.viewport_size().width - position.x) / rem;
                self.panel_width = width.clamp(MIN_PANEL_WIDTH, MAX_PANEL_WIDTH);
            }
        }
        cx.notify();
    }

    fn finish_resize(&mut self, cx: &mut Context<Self>) {
        let (sidebar, panel) = (self.sidebar_width, self.panel_width);
        prefs::update(cx, |prefs| {
            prefs.sidebar.width = sidebar;
            prefs.workers_panel.width = panel;
        });
    }

    fn render_main(&self) -> AnyElement {
        match &self.settings {
            Some(settings) => pane(settings),
            // Not kept itself: GPUI draws every kept view inside a kept view
            // that draws again, and the session view draws with each
            // keystroke and blink of the caret. Its transcript is kept.
            None => self.session_view.clone().into_any_element(),
        }
    }
}

/// A pane of the window filling its place, kept from frame to frame and
/// drawn again only when something in it changed: a frame drawn for one
/// view (a tick of the status bar's clock, the composer's caret) takes the
/// panes around it from the frame before.
fn pane<V: Render>(view: &Entity<V>) -> AnyElement {
    view.clone()
        .cached(StyleRefinement::default().size_full())
        .into_any_element()
}

/// The strip along a docked pane's inner edge that resizes it.
fn resize_handle(edge: Edge, cx: &mut Context<Workspace>) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id(match edge {
            Edge::Sidebar => "sidebar-resize",
            Edge::Panel => "panel-resize",
        })
        .test_support()
        .absolute()
        .top_0()
        .bottom_0()
        .map(|this| match edge {
            Edge::Sidebar => this.right(px(-3.)),
            Edge::Panel => this.left(px(-3.)),
        })
        // A physical hit area: 6 px at any zoom, around a 1 px hairline.
        .w(px(6.))
        .flex()
        .justify_center()
        .cursor_col_resize()
        .hover(|this| this.bg(theme.drag_border.opacity(0.5)))
        .on_drag(ResizeDrag(edge), |drag, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| drag.clone())
        })
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("Workspace");
        let theme = cx.theme();
        let (background, foreground, font_family, border) = (
            theme.background,
            theme.foreground,
            theme.font_family.clone(),
            theme.sidebar_border,
        );
        let sidebar_docked = self.is_sidebar_docked(cx);
        let panel_docked = self.width.docks_panel() && self.workers_open;
        // A pane never takes more than its share of a small window.
        let room = f32::from(window.viewport_size().width) / f32::from(window.rem_size());
        let sidebar_width = self.sidebar_width.min(room * 0.4);
        let panel_width = self.panel_width.min(room * 0.35);

        v_flex()
            .id("workspace")
            .size_full()
            .bg(background)
            .text_color(foreground)
            .font_family(font_family)
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(Self::command_palette))
            .on_action(cx.listener(Self::expand_composer))
            .on_action(cx.listener(Self::new_session))
            .on_action(cx.listener(Self::toggle_sidebar))
            .on_action(cx.listener(Self::toggle_workers))
            .on_action(cx.listener(Self::focus_filter))
            .on_action(cx.listener(Self::rename_session))
            .on_action(cx.listener(Self::delete_session))
            .on_action(cx.listener(|this, _: &menus::ViewFull, _, cx| {
                this.set_view(prefs::TranscriptView::Full, cx)
            }))
            .on_action(cx.listener(|this, _: &menus::ViewQuiet, _, cx| {
                this.set_view(prefs::TranscriptView::Quiet, cx)
            }))
            .on_action(cx.listener(|this, _: &menus::ViewMessages, _, cx| {
                this.set_view(prefs::TranscriptView::Messages, cx)
            }))
            .on_action(cx.listener(|this, _: &menus::CycleView, _, cx| {
                let next = prefs::get(cx).view.next();
                this.set_view(next, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::CloseWindow, window, cx| {
                chrome::close_window(window, cx);
            }))
            .on_action(cx.listener(|_, _: &menus::Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &menus::Zoom, window, _| window.zoom_window()))
            .on_action(cx.listener(|this, _: &menus::SessionInfo, _, cx| {
                this.title_bar.update(cx, |bar, cx| bar.toggle_info(cx));
            }))
            .on_action(cx.listener(|this, _: &menus::BranchSession, window, cx| {
                this.session_view
                    .update(cx, |view, cx| view.branch_from(None, window, cx));
            }))
            .on_action(cx.listener(|this, _: &menus::StopTurn, window, cx| {
                this.session_view
                    .update(cx, |view, cx| view.stop(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &menus::MoveToBackground, window, cx| {
                    this.session_view
                        .update(cx, |view, cx| view.move_to_background(window, cx));
                }),
            )
            .on_action(cx.listener(|this, _: &menus::HandOverNow, window, cx| {
                if let Some(session) = this.session.clone() {
                    let input = arko::Input::Message(arko::Message {
                        text: "/handover".into(),
                        images: Vec::new(),
                    });
                    if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
                        window.push_notification(why, cx);
                    }
                }
            }))
            .on_action(cx.listener(Self::share_web_page))
            .on_action(cx.listener(Self::stop_sharing_web_page))
            .on_action(cx.listener(|this, _: &menus::RefreshCache, window, cx| {
                if let Some(session) = this.session.clone() {
                    crate::cache_reminders::refresh(&session, window, cx);
                }
            }))
            .on_action(cx.listener(Self::close_shown_session))
            .on_action(cx.listener(|this, _: &menus::OpenAccounts, window, cx| {
                this.open_accounts(None, window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &menus::OpenWebPageSettings, window, cx| {
                    let settings = this.show_settings(window, cx);
                    settings.update(cx, |settings, cx| settings.show_page(WEB_PAGE_PAGE, cx));
                }),
            )
            .on_action(cx.listener(|this, _: &menus::ExportMarkdown, window, cx| {
                this.session_view
                    .update(cx, |view, cx| view.export(window, cx));
            }))
            .on_action(
                cx.listener(|this, _: &crate::transcript::FocusTranscript, window, cx| {
                    if this.settings.is_none() {
                        this.session_view
                            .update(cx, |view, cx| view.focus_transcript(window, cx));
                    }
                }),
            )
            .on_action(cx.listener(|this, _: &menus::Find, window, cx| {
                if this.settings.is_none() {
                    this.session_view
                        .update(cx, |view, cx| view.open_find(window, cx));
                }
            }))
            .on_action(cx.listener(|_, _: &menus::KeyboardShortcuts, window, cx| {
                shortcuts::open(window, cx)
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ResizeDrag>, window, cx| {
                    let edge = event.drag(cx).0;
                    this.resize(edge, event.event.position, window, cx);
                }),
            )
            .on_drop(cx.listener(|this, _: &ResizeDrag, _, cx| this.finish_resize(cx)))
            .child(
                self.title_bar.clone().cached(
                    StyleRefinement::default()
                        .w_full()
                        .flex_none()
                        .h(gpui_kit::component::TITLE_BAR_HEIGHT),
                ),
            )
            .child(
                h_flex()
                    .id("panes")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .when(sidebar_docked, |this| {
                        this.child(
                            div()
                                .id("sidebar-pane")
                                .test_support()
                                .relative()
                                .flex_none()
                                .h_full()
                                .w(rems(sidebar_width))
                                .border_r_1()
                                .border_color(border)
                                .child(pane(&self.sidebar))
                                .child(resize_handle(Edge::Sidebar, cx)),
                        )
                    })
                    .child(
                        div()
                            .id("main")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.render_main()),
                    )
                    .when(panel_docked, |this| {
                        this.child(
                            div()
                                .id("workers-pane")
                                .relative()
                                .flex_none()
                                .h_full()
                                .w(rems(panel_width))
                                .border_l_1()
                                .border_color(border)
                                .child(pane(&self.workers))
                                .child(resize_handle(Edge::Panel, cx)),
                        )
                    }),
            )
            .child(
                div()
                    .id("status-bar")
                    .test_support()
                    .flex_none()
                    .w_full()
                    .child(
                        self.status_bar
                            .clone()
                            .cached(StyleRefinement::default().w_full().h(STATUS_BAR_HEIGHT)),
                    ),
            )
    }
}
