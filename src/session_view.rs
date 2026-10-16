//! The main column: the transcript's reading column and the composer under
//! it. Until the engine is wired in, the transcript is empty and the
//! composer's buttons say so.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Mode, SessionEntry};
use crate::layout::WidthClass;
use crate::{menus, prefs};

/// What the main column shows: a session, or a new one not started yet.
pub struct SessionView {
    session: Option<SessionEntry>,
    composer: Entity<TextareaState>,
    width: WidthClass,
}

impl SessionView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let smooth = prefs::get(cx).input.smooth_typing;
        let composer = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 10)
                .placeholder("Message Arko")
                .smooth_caret(smooth)
        });
        Self {
            session: None,
            composer,
            width: WidthClass::Wide,
        }
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

    /// The chip's text: the mode, and the model where there's room.
    fn chip_label(&self) -> String {
        let mode = match self.session.as_ref().map(|session| session.mode) {
            Some(Mode::Chat) => "Chat",
            _ => "Agent",
        };
        match self.session.as_ref().map(|session| session.model.as_str()) {
            Some(model) if !model.is_empty() && self.width != WidthClass::Narrow => {
                format!("{mode} · {model}")
            }
            _ => mode.to_string(),
        }
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
                    .child(
                        Button::new("mode-model")
                            .ghost()
                            .small()
                            .label(self.chip_label())
                            .dropdown_caret(true)
                            .on_click(|_, window, cx| {
                                not_yet("Choosing the mode and model", window, cx)
                            }),
                    )
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
            .on_action(cx.listener(|this, _: &menus::ExpandComposer, window, cx| {
                this.focus_composer(window, cx);
                prefs::update(cx, |prefs| {
                    prefs.input.composer_expanded = !prefs.input.composer_expanded
                });
            }))
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
                            }),
                    )
                    .child(div().pb_3().child(self.render_composer(cx))),
            )
    }
}
