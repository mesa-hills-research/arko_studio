//! The title bar: GPUI Kit's `TitleBar` with the menus (Windows and Linux),
//! the sidebar button, and the open session's title and folder.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::AppMenuBar,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{chrome, menus};

/// The title bar of one window. It re-renders only when its own state
/// changes, never with the transcript or the composer.
pub struct AppTitleBar {
    menu_bar: Entity<AppMenuBar>,
    title: Option<SharedString>,
    folder: Option<SharedString>,
    sidebar_open: bool,
    _menus: Subscription,
}

impl AppTitleBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
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
            _menus: menus,
        }
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
}

impl Render for AppTitleBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mac = cfg!(target_os = "macos");
        let theme = cx.theme();
        let sidebar_label = if self.sidebar_open {
            "Hide Sidebar"
        } else {
            "Show Sidebar"
        };
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
                    // The menus (Windows and Linux; macOS has them in the menu bar),
                    // then the sidebar button.
                    .when(!mac, |this| {
                        this.child(title_bar_item(self.menu_bar.clone()))
                    })
                    .child(title_bar_item(
                        Button::new("toggle-sidebar")
                            .small()
                            .ghost()
                            .compact()
                            .icon(if self.sidebar_open {
                                IconName::PanelLeftClose
                            } else {
                                IconName::PanelLeftOpen
                            })
                            .tooltip_with_action(sidebar_label, &menus::ToggleSidebar, None)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(menus::ToggleSidebar), cx);
                            }),
                    ))
                    // The session's title, centred in the space left, and cut
                    // short when it doesn't fit. Plain text: the bar stays a drag
                    // area around it.
                    .child(
                        h_flex()
                            .id("session-title")
                            .flex_1()
                            .min_w_0()
                            .justify_center()
                            .gap_1p5()
                            .text_sm()
                            .when_some(self.title.clone(), |this, title| {
                                this.child(div().min_w_0().truncate().child(title))
                            })
                            .when_some(self.folder.clone(), |this, folder| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_color(theme.muted_foreground)
                                        .child(format!("· {folder}")),
                                )
                            }),
                    )
                    // Balances the buttons on the left of a macOS bar, so the title
                    // sits in the middle of the window there.
                    .when(mac, |this| this.child(div().w_20().flex_none())),
            )
    }
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
