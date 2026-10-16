//! The workers panel: the session's workers, opened on demand at the right
//! (docked on wide windows, a sheet on narrower ones). It never opens by
//! itself.

use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;

use crate::menus;

pub struct WorkersPanel;

impl WorkersPanel {
    pub fn new() -> Self {
        Self
    }
}

impl Default for WorkersPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for WorkersPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        v_flex()
            .id("workers-panel")
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .px_3()
                    .justify_between()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .child(div().text_sm().font_medium().child("Workers"))
                    .child(
                        Button::new("close-workers")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(IconName::Close)
                            .tooltip_with_action("Close", &menus::ToggleWorkersPanel, None)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx)
                            }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .p_3()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child("No workers in this session"),
            )
    }
}
