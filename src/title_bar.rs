//! The title bar: GPUI Kit's `TitleBar` with the menus (Windows and Linux),
//! the open session's title and folder, which open its info, and the
//! layout buttons at the right end.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Selectable as _, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::AppMenuBar,
    popover::Popover,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::LiveSession;
use crate::session_info::SessionInfo;
use crate::{chrome, menus};

/// The title bar of one window. It re-renders only when its own state
/// changes, never with the transcript or the composer.
pub struct AppTitleBar {
    menu_bar: Entity<AppMenuBar>,
    title: Option<SharedString>,
    folder: Option<SharedString>,
    sidebar_open: bool,
    /// The workers panel's button: shown for a session that has or had
    /// workers, and whether the panel shows.
    workers: Option<bool>,
    /// The session's info, which the title opens.
    info: Entity<SessionInfo>,
    info_open: bool,
    _menus: Subscription,
}

impl AppTitleBar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let menu_bar = AppMenuBar::new(cx);
        // The menus change with the app's state (Show or Hide Sidebar).
        let menus = cx.observe_global::<menus::MenuState>(|this, cx| {
            this.menu_bar.update(cx, |menu_bar, cx| menu_bar.reload(cx));
        });
        Self {
            menu_bar,
            title: None,
            folder: None,
            sidebar_open: true,
            workers: None,
            info: cx.new(|cx| SessionInfo::new(window, cx)),
            info_open: false,
            _menus: menus,
        }
    }

    /// The session whose title shows: its info opens from it.
    pub fn set_live_session(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if session.is_none() {
            self.set_info_open(false, cx);
        }
        self.info
            .update(cx, |info, cx| info.set_session(session, window, cx));
        cx.notify();
    }

    /// Opens the session's info (Session › Info), or closes it.
    pub fn toggle_info(&mut self, cx: &mut Context<Self>) {
        let has_session = self.info.read(cx).session().is_some();
        self.set_info_open(!self.info_open && has_session, cx);
    }

    fn set_info_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.info_open != open {
            self.info.update(cx, |info, cx| info.set_open(open, cx));
            self.info_open = open;
            cx.notify();
        }
    }

    pub fn is_info_open(&self) -> bool {
        self.info_open
    }

    pub fn info(&self) -> &Entity<SessionInfo> {
        &self.info
    }

    /// Shows the open session's title and the name of its folder.
    pub fn set_session(
        &mut self,
        title: Option<SharedString>,
        folder: Option<SharedString>,
        cx: &mut Context<Self>,
    ) {
        if (&self.title, &self.folder) != (&title, &folder) {
            self.title = title;
            self.folder = folder;
            cx.notify();
        }
    }

    pub fn set_sidebar_open(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.sidebar_open != open {
            self.sidebar_open = open;
            cx.notify();
        }
    }

    /// Offers the workers panel's button (`Some`, with whether the panel
    /// shows), or leaves it out.
    pub fn set_workers(&mut self, workers: Option<bool>, cx: &mut Context<Self>) {
        if self.workers != workers {
            self.workers = workers;
            cx.notify();
        }
    }
}

impl AppTitleBar {
    /// The title and the folder: plain text, or, for an open session, a
    /// button that opens its info.
    fn render_title(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let words = h_flex()
            .min_w_0()
            .gap_1p5()
            .text_sm()
            .when_some(self.title.clone(), |this, title| {
                this.child(div().min_w_0().truncate().child(title))
            })
            .when_some(self.folder.clone(), |this, folder| {
                this.child(
                    div()
                        .flex_none()
                        .text_color(muted)
                        .child(format!("· {folder}")),
                )
            });
        let has_session = self.info.read(cx).session().is_some();
        if !has_session {
            return words.into_any_element();
        }
        let info = self.info.clone();
        let view = cx.entity().downgrade();
        // Clicks on it don't drag the window. The space around it does.
        title_bar_item(
            Popover::new("session-info-popover")
                .anchor(Anchor::TopCenter)
                .open(self.info_open)
                .on_open_change(move |open, _, cx| {
                    let open = *open;
                    view.update(cx, |this, cx| this.set_info_open(open, cx))
                        .ok();
                })
                .trigger(
                    Button::new("session-title-button")
                        .ghost()
                        .small()
                        .max_w_full()
                        .tooltip_with_action("Session info", &menus::SessionInfo, None)
                        .child(words),
                )
                .content(move |_, _, _| info.clone()),
        )
        .min_w_0()
        .into_any_element()
    }
}

impl Render for AppTitleBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("AppTitleBar");
        let mac = cfg!(target_os = "macos");
        let title = self.render_title(cx);
        TitleBar::new()
            // GPUI Kit's Linux close button calls `window.remove_window()`, which
            // skips the close check. This sends it through the check instead. Only
            // Linux uses it: macOS and Windows close through the system.
            .on_close_window(|_, window, cx| chrome::close_window(window, cx))
            .child(
                h_flex()
                    .size_full()
                    .min_w_0()
                    .gap_1()
                    // The menus (Windows and Linux; macOS has them in the menu bar).
                    .when(!mac, |this| {
                        this.child(title_bar_item(self.menu_bar.clone()))
                    })
                    // The session's title, centred in the space left, and cut
                    // short when it doesn't fit. The bar stays a drag area
                    // around it.
                    .child(
                        h_flex()
                            .id("session-title")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .child(title),
                    )
                    // The layout buttons, together at the right end, before
                    // the window buttons on Windows and Linux.
                    .child(
                        title_bar_item(
                            h_flex()
                                .id("layout-buttons")
                                .gap_0p5()
                                .child(layout_button(
                                    "toggle-sidebar",
                                    IconName::PanelLeft,
                                    self.sidebar_open,
                                    "Toggle sidebar",
                                    Box::new(menus::ToggleSidebar),
                                ))
                                .when_some(self.workers, |this, open| {
                                    this.child(layout_button(
                                        "toggle-workers",
                                        IconName::PanelRight,
                                        open,
                                        "Toggle workers panel",
                                        Box::new(menus::ToggleWorkersPanel),
                                    ))
                                }),
                        )
                        .mr_1(),
                    ),
            )
    }
}

/// A button that shows or hides a pane, with the pane's shortcut in its
/// tooltip.
fn layout_button(
    id: &'static str,
    icon: IconName,
    selected: bool,
    label: &'static str,
    action: Box<dyn Action>,
) -> Button {
    let tooltip_action = action.boxed_clone();
    Button::new(id)
        .small()
        .ghost()
        .compact()
        .icon(icon)
        .selected(selected)
        .tooltip_with_action(label, tooltip_action.as_ref(), None)
        .accessibility_label(label)
        .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
}

/// Holds clickable title bar content: buttons, menus, tabs, search fields.
///
/// GPUI Kit's `TitleBar` makes the whole bar a drag area, and its buttons let
/// presses through to it. Without `occlude()` a click turns into a window
/// drag on Windows, and on macOS and Linux the window moves if the pointer
/// moves during the click. `occlude()` keeps the drag area from seeing
/// presses on this element. Text and empty space stay draggable.
pub fn title_bar_item(child: impl IntoElement) -> Div {
    h_flex().occlude().child(child)
}
