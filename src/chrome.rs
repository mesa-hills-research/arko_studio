//! The main window's options, where it opens, and the one close check.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::prefs::{self, WindowPlacement};

/// The size a first window opens at, centred.
const FIRST_SIZE: (f32, f32) = (1100., 720.);
/// How far a new window sits from the last one, so both stay visible.
const CASCADE: f32 = 28.;

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
        window_bounds: Some(initial_bounds(cx)),
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

/// Where a window opens: where the last one was, a step down and right when
/// another window is open, or centred the first time.
fn initial_bounds(cx: &App) -> WindowBounds {
    let Some(placement) = prefs::get(cx).window else {
        return WindowBounds::centered(size(px(FIRST_SIZE.0), px(FIRST_SIZE.1)), cx);
    };
    let offset = if cx.windows().is_empty() { 0. } else { CASCADE };
    let bounds = Bounds::new(
        point(px(placement.x + offset), px(placement.y + offset)),
        size(px(placement.width), px(placement.height)),
    );
    // A window whose spot is on no display any more (a monitor was
    // unplugged) opens centred instead.
    let visible = cx.displays().iter().any(|display| {
        display.bounds().intersects(&Bounds::new(
            bounds.origin,
            size(
                px(placement.width.min(100.)),
                px(placement.height.min(100.)),
            ),
        ))
    });
    if !visible {
        return WindowBounds::centered(bounds.size, cx);
    }
    if placement.maximized && offset == 0. {
        WindowBounds::Maximized(bounds)
    } else {
        WindowBounds::Windowed(bounds)
    }
}

/// Remembers where `window` is, for the next window to open there.
pub fn remember_placement(window: &Window, cx: &mut App) {
    let (bounds, maximized) = match window.window_bounds() {
        WindowBounds::Windowed(bounds) => (bounds, false),
        WindowBounds::Maximized(bounds) => (bounds, true),
        // Full screen keeps the placement it had before.
        WindowBounds::Fullscreen(_) => return,
    };
    let placement = WindowPlacement {
        x: f32::from(bounds.origin.x),
        y: f32::from(bounds.origin.y),
        width: f32::from(bounds.size.width),
        height: f32::from(bounds.size.height),
        maximized,
    };
    prefs::update(cx, |prefs| prefs.window = Some(placement));
}

/// The one close check. The macOS and Windows close buttons, Alt+F4, the
/// Linux compositor and title bar, Close Window and Quit all end here.
///
/// Closing the last window quits the app on Windows and Linux, so its
/// sessions close first (asking about the work still running), and the app
/// quits once they did. On macOS the app and its sessions run on.
pub fn can_close(window: &mut Window, cx: &mut App) -> bool {
    let last = cx.windows().len() <= 1;
    if last && !cfg!(target_os = "macos") && crate::quit::any_running(cx) {
        crate::quit::quit(window, cx);
        return false;
    }
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
/// Quitting from the macOS Dock or at logout skips the check. Anything that
/// has to survive that is saved in `cx.on_app_quit`.
pub fn quit(cx: &mut App) {
    // Deferred, so the window that sent Quit is free to answer too.
    cx.defer(|cx| {
        // The sessions close first, asking about their work in one window.
        if crate::quit::any_running(cx)
            && let Some(window) = cx.active_window().or_else(|| cx.windows().first().copied())
        {
            window
                .update(cx, |_, window, cx| crate::quit::quit(window, cx))
                .ok();
            return;
        }
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
