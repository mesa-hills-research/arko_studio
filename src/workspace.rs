//! A window: the title bar, the sessions sidebar, the main column, the
//! workers panel and the status bar, laid out for the window's width.

use gpui_kit::component::{
    ActiveTheme as _, Placement, WindowExt as _, h_flex, notification::Notification, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::appearance;
use crate::backend::{SessionHandle, SessionId, SharedBackend};
use crate::layout::WidthClass;
use crate::prefs::{self, MAX_SIDEBAR_WIDTH, MIN_SIDEBAR_WIDTH};
use crate::session_view::{SessionView, not_yet};
use crate::sidebar::{Sidebar, SidebarEvent};
use crate::title_bar::AppTitleBar;
use crate::workers_panel::WorkersPanel;
use crate::{chrome, menus, status_bar};

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
    sidebar: Entity<Sidebar>,
    session_view: Entity<SessionView>,
    workers: Entity<WorkersPanel>,
    /// The session open here, held so it stays open.
    handle: Option<SessionHandle>,
    width: WidthClass,
    workers_open: bool,
    /// The pane showing as a sheet over a narrow window.
    sheet: Option<Edge>,
    /// Widths while a drag resizes them, in rem; saved when it ends.
    sidebar_width: f32,
    panel_width: f32,
    _subscriptions: Vec<Subscription>,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.on_window_should_close(cx, chrome::can_close);
        appearance::set_system_appearance(window.appearance(), cx);

        let title_bar = cx.new(AppTitleBar::new);
        let sidebar = cx.new(|cx| Sidebar::new(window, cx));
        let session_view = cx.new(|cx| SessionView::new(window, cx));
        let workers = cx.new(|_| WorkersPanel::new());
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
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                this.prefs_changed(window, cx)
            }),
        ];

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            title_bar,
            sidebar,
            session_view,
            workers,
            handle: None,
            width: WidthClass::of(window.viewport_size().width),
            workers_open: false,
            sheet: None,
            sidebar_width: sidebar_prefs.width(),
            panel_width,
            _subscriptions: subscriptions,
        };
        this.sync_chrome(window, cx);
        this.session_view
            .update(cx, |view, cx| view.set_width(this.width, cx));
        this.focus_composer(window, cx);
        this
    }

    fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        let composer = self.session_view.read(cx).composer().clone();
        composer.update(cx, |composer, cx| composer.focus(window, cx));
    }

    pub fn sidebar(&self) -> &Entity<Sidebar> {
        &self.sidebar
    }

    pub fn session_view(&self) -> &Entity<SessionView> {
        &self.session_view
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
        cx.notify();
    }

    fn prefs_changed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let sidebar = &prefs::get(cx).sidebar;
        self.sidebar_width = sidebar.width();
        self.panel_width = prefs::get(cx).workers_panel.width;
        self.sync_chrome(window, cx);
        cx.notify();
    }

    /// Shows the open session's title in the title bar and window switchers.
    fn sync_chrome(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let entry = self.handle.as_ref().map(|handle| handle.entry.clone());
        let title = entry
            .as_ref()
            .map(|entry| SharedString::from(entry.title.clone()))
            .unwrap_or_else(|| "New session".into());
        let folder = entry
            .as_ref()
            .and_then(|entry| entry.workspace.as_ref())
            .and_then(|folder| folder.file_name())
            .map(|name| SharedString::from(name.to_string_lossy().into_owned()));
        let open = match self.width.docks_sidebar() {
            true => prefs::get(cx).sidebar.open,
            false => self.sheet == Some(Edge::Sidebar),
        };
        self.title_bar.update(cx, |bar, cx| {
            bar.set_session(Some(title.clone()), folder, cx);
            bar.set_sidebar_open(open, cx);
        });
        window.set_window_title(&format!("{title} – {}", crate::APP_NAME));
    }

    /// Opens a session in this window, closing the one open here.
    pub fn open_session(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        if self
            .handle
            .as_ref()
            .is_some_and(|handle| handle.entry.id == id)
        {
            self.after_choosing(window, cx);
            return;
        }
        let backend = cx.global::<SharedBackend>().clone();
        let opening = cx.background_spawn(async move { backend.open(&id) });
        cx.spawn_in(window, async move |this, cx| {
            let opened = opening.await;
            this.update_in(cx, |this, window, cx| match opened {
                Ok(handle) => {
                    this.close_session(cx);
                    this.show(Some(handle), window, cx);
                    this.after_choosing(window, cx);
                }
                Err(error) => {
                    let host = std::env::var("HOSTNAME").ok();
                    window.push_notification(
                        Notification::error(format!(
                            "Couldn’t open the session. {}",
                            error.message(host.as_deref())
                        )),
                        cx,
                    );
                }
            })
            .ok();
        })
        .detach();
    }

    /// After a session is picked: the sheet closes and the composer has focus.
    fn after_choosing(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_sheet(window, cx);
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
                    })
                    .ok();
                })
                .child(view.clone())
        });
        self.sheet = Some(edge);
        self.sync_chrome(window, cx);
    }

    fn close_session(&mut self, cx: &mut Context<Self>) {
        if let Some(handle) = self.handle.take() {
            let backend = cx.global::<SharedBackend>().clone();
            cx.background_spawn(async move { backend.close(handle) })
                .detach();
        }
    }

    fn show(&mut self, handle: Option<SessionHandle>, window: &mut Window, cx: &mut Context<Self>) {
        let entry = handle.as_ref().map(|handle| handle.entry.clone());
        let id = entry.as_ref().map(|entry| entry.id.clone());
        self.handle = handle;
        self.session_view
            .update(cx, |view, cx| view.set_session(entry, cx));
        self.sidebar
            .update(cx, |sidebar, cx| sidebar.set_active(id, cx));
        self.sync_chrome(window, cx);
    }

    fn new_session(&mut self, _: &menus::NewSession, window: &mut Window, cx: &mut Context<Self>) {
        self.close_session(cx);
        self.show(None, window, cx);
        self.after_choosing(window, cx);
    }

    fn on_sidebar_event(
        &mut self,
        _: &Entity<Sidebar>,
        event: &SidebarEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            SidebarEvent::Open(id) | SidebarEvent::OpenConversation(id, _) => {
                self.open_session(id.clone(), window, cx)
            }
            SidebarEvent::OpenInNewWindow(id) => {
                let here = self
                    .handle
                    .as_ref()
                    .is_some_and(|handle| &handle.entry.id == id);
                if here {
                    // It moves: a session is open in one window at a time.
                    self.close_session(cx);
                    self.show(None, window, cx);
                }
                let id = id.clone();
                cx.defer(move |cx| {
                    open_with(Some(id), cx);
                });
            }
            SidebarEvent::Renamed(id, title) => {
                if let Some(handle) = self.handle.as_mut().filter(|handle| &handle.entry.id == id) {
                    handle.entry.title = title.clone();
                    let entry = handle.entry.clone();
                    self.session_view
                        .update(cx, |view, cx| view.set_session(Some(entry), cx));
                    self.sync_chrome(window, cx);
                }
            }
            SidebarEvent::Deleted(id) => {
                if self
                    .handle
                    .as_ref()
                    .is_some_and(|handle| &handle.entry.id == id)
                {
                    self.handle = None;
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
        let Some(id) = self.handle.as_ref().map(|handle| handle.entry.id.clone()) else {
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
        if let Some(id) = self.handle.as_ref().map(|handle| handle.entry.id.clone()) {
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
        if self.width.docks_panel() {
            self.workers_open = !self.workers_open;
            cx.notify();
        } else if self.sheet == Some(Edge::Panel) {
            self.close_sheet(window, cx);
        } else {
            self.open_sheet(Edge::Panel, window, cx);
        }
    }

    fn about(&mut self, _: &menus::About, window: &mut Window, cx: &mut Context<Self>) {
        window.open_alert_dialog(cx, |alert, _, _| {
            alert.title(crate::APP_NAME).description(format!(
                "Version {}\n{}",
                env!("CARGO_PKG_VERSION"),
                crate::COPYRIGHT
            ))
        });
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
            .on_action(cx.listener(|_, _: &menus::SessionInfo, window, cx| {
                not_yet("Session info", window, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::BranchSession, window, cx| {
                not_yet("Branching", window, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::StopTurn, window, cx| {
                not_yet("Stopping a turn", window, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::MoveToBackground, window, cx| {
                not_yet("Moving a command to the background", window, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::HandOverNow, window, cx| {
                not_yet("Handing over", window, cx)
            }))
            .on_action(
                cx.listener(|_, _: &menus::ExportMarkdown, window, cx| {
                    not_yet("Export", window, cx)
                }),
            )
            .on_action(cx.listener(|_, _: &menus::Find, window, cx| not_yet("Find", window, cx)))
            .on_action(cx.listener(|_, _: &menus::CommandPalette, window, cx| {
                not_yet("The command palette", window, cx)
            }))
            .on_action(cx.listener(|_, _: &menus::KeyboardShortcuts, window, cx| {
                not_yet("The shortcuts sheet", window, cx)
            }))
            .on_drag_move(
                cx.listener(|this, event: &DragMoveEvent<ResizeDrag>, window, cx| {
                    let edge = event.drag(cx).0;
                    this.resize(edge, event.event.position, window, cx);
                }),
            )
            .on_drop(cx.listener(|this, _: &ResizeDrag, _, cx| this.finish_resize(cx)))
            .child(self.title_bar.clone())
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
                                .child(self.sidebar.clone())
                                .child(resize_handle(Edge::Sidebar, cx)),
                        )
                    })
                    .child(
                        div()
                            .id("main")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(self.session_view.clone()),
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
                                .child(self.workers.clone())
                                .child(resize_handle(Edge::Panel, cx)),
                        )
                    }),
            )
            .child(status_bar::status_bar())
    }
}
