//! The title bar: GPUI Kit's `TitleBar` with the app's menus and buttons.

use gpui_kit::component::{
    IconName, Sizable as _, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::AppMenuBar,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::{chrome, menus};

pub struct AppTitleBar {
    menu_bar: Entity<AppMenuBar>,
}

impl AppTitleBar {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            menu_bar: AppMenuBar::new(cx),
        }
    }
}

impl Render for AppTitleBar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let mac = cfg!(target_os = "macos");
        TitleBar::new()
            // GPUI Kit's Linux close button calls `window.remove_window()`, which
            // skips the close check. This sends it through the check instead. Only
            // Linux uses it: macOS and Windows close through the system.
            .on_close_window(|_, window, cx| chrome::close_window(window, cx))
            // Left: the window title on macOS, where the menus are in the menu
            // bar, and the menus elsewhere.
            .child(
                h_flex()
                    .when(mac, |this| this.text_sm().child(crate::APP_NAME))
                    .when(!mac, |this| {
                        this.child(title_bar_item(self.menu_bar.clone()))
                    }),
            )
            // Right: the app's own buttons.
            .child(
                title_bar_item(
                    Button::new("settings")
                        .small()
                        .ghost()
                        .compact()
                        .icon(IconName::Settings)
                        .tooltip_with_action("Settings", &menus::OpenSettings, None)
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(menus::OpenSettings), cx);
                        }),
                )
                .gap_1()
                .pr_2(),
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
