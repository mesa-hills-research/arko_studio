//! The main window's options and the one close check.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

/// The main window: a transparent title bar that the app draws itself, with
/// the macOS traffic lights at (9, 9) and the drag left to the app, as GPUI
/// Kit's `TitleBar` expects.
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            // Hidden in the window, shown by the taskbar, the Window menu and
            // window switchers.
            title: Some(crate::APP_NAME.into()),
            ..TitleBar::title_bar_options()
        }),
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(720.)), cx)),
        // 500 px or less keeps Windows 11 snap layouts working.
        window_min_size: Some(size(px(400.), px(300.))),
        // Linux: the app draws its own frame on every compositor, as on macOS
        // and Windows. `TitleBar::window_options()` leaves this unset, which
        // gives KDE, Sway and X11 a system title bar above the app's own.
        window_decorations: Some(WindowDecorations::Client),
        // Matches packaging/linux/<APP_ID>.desktop, for the icon and window grouping.
        app_id: Some(crate::APP_ID.into()),
        ..TitleBar::window_options()
    }
}

/// The one close check. The macOS and Windows close buttons, Alt+F4, the
/// Linux compositor and title bar, Close Window and Quit all end here.
///
/// To ask about unsaved work, show a dialog, return `false`, and call
/// `window.remove_window()` (or `quit` again) once the user decides.
pub fn can_close(_window: &mut Window, _cx: &mut App) -> bool {
    true
}

/// Closes `window` if `can_close` agrees. `window.remove_window()` on its own
/// skips the check.
pub fn close_window(window: &mut Window, cx: &mut App) {
    if can_close(window, cx) {
        window.remove_window();
    }
}

/// Quits once every window agrees to close.
///
/// Quitting from the macOS Dock or at logout skips the check. Save anything
/// that has to survive that in `cx.on_app_quit`.
pub fn quit(cx: &mut App) {
    // Deferred, so the window that sent Quit is free to answer too.
    cx.defer(|cx| {
        let all_agree = cx.windows().into_iter().all(|window| {
            window
                .update(cx, |_, window, cx| can_close(window, cx))
                .unwrap_or(true)
        });
        if all_agree {
            cx.quit();
        }
    });
}
