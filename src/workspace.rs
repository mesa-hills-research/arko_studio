//! The main window's root view.

use gpui_kit::component::{Theme, WindowExt as _, v_flex};
use gpui_kit::*;

use crate::{chrome, menus, title_bar::AppTitleBar};

/// Opens a main window.
pub fn open(cx: &mut App) {
    gpui_kit::open_window(chrome::window_options(cx), cx, |window, cx| {
        cx.new(|cx| Workspace::new(window, cx))
    })
    .expect("failed to open the main window");
}

pub struct Workspace {
    focus_handle: FocusHandle,
    /// Phones and tablets have no title bar.
    title_bar: Option<Entity<AppTitleBar>>,
    _appearance: Subscription,
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        window.on_window_should_close(cx, chrome::can_close);

        // Follow the system's light or dark appearance.
        Theme::sync_system_appearance(Some(window), cx);
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            Theme::sync_system_appearance(Some(window), cx);
        });

        // Menu items and key bindings reach the focused element and its
        // parents, so the window's actions below need focus in the workspace.
        let focus_handle = cx.focus_handle();
        focus_handle.focus(window, cx);

        Self {
            focus_handle,
            title_bar: (!gpui_kit::is_mobile()).then(|| cx.new(AppTitleBar::new)),
            _appearance: appearance,
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

    fn open_settings(
        &mut self,
        _: &menus::OpenSettings,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.push_notification("Settings open here.", cx);
    }
}

impl Render for Workspace {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::about))
            .on_action(cx.listener(Self::open_settings))
            .on_action(cx.listener(|_, _: &menus::CloseWindow, window, cx| {
                chrome::close_window(window, cx);
            }))
            .on_action(cx.listener(|_, _: &menus::Minimize, window, _| window.minimize_window()))
            .on_action(cx.listener(|_, _: &menus::Zoom, window, _| window.zoom_window()))
            .children(self.title_bar.clone())
            .child(
                // The app's content.
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(crate::APP_NAME),
            )
    }
}
