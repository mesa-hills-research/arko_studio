//! The main window's options, where it opens, and the one close check.

use gpui_kit::component::TitleBar;
use gpui_kit::*;

use crate::prefs::{self, WindowPlacement};

/// The icon the window manager shows for a window on X11, the 128 px size
/// of the installed icon. GPUI takes one image and the window manager
/// scales it. Wayland has no window icon in GPUI, so a compositor takes it
/// from the desktop entry that `app_id` names. Windows reads it from the
/// executable (`build.rs`) and macOS from the app bundle.
#[cfg(target_os = "linux")]
fn window_icon() -> Option<std::sync::Arc<image::RgbaImage>> {
    use std::sync::{Arc, OnceLock};

    static ICON: OnceLock<Option<Arc<image::RgbaImage>>> = OnceLock::new();
    ICON.get_or_init(|| {
        let png = include_bytes!(
            "../packaging/linux/icons/hicolor/128x128/apps/com.mesahillsresearch.ArkoStudio.png"
        );
        image::load_from_memory_with_format(png, image::ImageFormat::Png)
            .ok()
            .map(|icon| Arc::new(icon.to_rgba8()))
    })
    .clone()
}

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
        // A binary run from the build folder has no desktop entry to take the
        // icon from, so X11 gets it with the window.
        #[cfg(target_os = "linux")]
        icon: window_icon(),
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
        // The sessions close first, asking about their work in one window:
        // with none left open (macOS runs on without one), a new one.
        if crate::quit::any_running(cx)
            && let Some(window) = cx
                .active_window()
                .or_else(|| cx.windows().first().copied())
                .or_else(|| crate::workspace::open_with(None, cx))
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::SystemTime;

    use gpui_kit::TestAppContext;

    use crate::Startup;
    use crate::backend::FakeBackend;

    /// A binary run from the build folder has no desktop entry, so on Linux
    /// the window brings the icon itself. The other systems take it from
    /// the executable or the app bundle.
    #[gpui_kit::test]
    fn a_window_brings_the_apps_icon_on_linux(cx: &mut TestAppContext) {
        let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
        let icon = cx.read(|cx| super::window_options(cx).icon);
        if !cfg!(target_os = "linux") {
            assert!(icon.is_none());
            return;
        }
        let icon = icon.expect("the window has an icon");
        assert_eq!((icon.width(), icon.height()), (128, 128));
        assert!(
            icon.pixels().any(|pixel| pixel.0[3] == 255),
            "the icon has opaque pixels"
        );
        let again = cx.read(|cx| super::window_options(cx).icon).unwrap();
        assert!(Arc::ptr_eq(&icon, &again), "decoded once, not per window");
    }
}
