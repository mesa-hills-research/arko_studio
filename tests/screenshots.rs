//! Golden screenshots of Arko Studio's window, rendered headlessly by the GPUI
//! fork's gpui-pre-screenshot on Mesa's software Vulkan driver, in Work Sans
//! and JetBrains Mono. The goldens live in `tests/screenshots`. On a mismatch
//! the actual image and a diff go to `tests/screenshots/failures`, and
//! `UPDATE_GOLDENS=1` rewrites the goldens.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::SystemTime;

use arko_studio::backend::FakeBackend;
use arko_studio::prefs::{self, Motion, Prefs, ThemeMode};
use arko_studio::{Startup, assets::Assets, fonts, workspace};
use gpui_kit::{px, size};
use gpui_screenshot::{Goldens, Screenshot, ScreenshotApp, Tolerance};

/// Mesa and LLVM releases round antialiased edges a level or two apart, so
/// each channel of each pixel may differ from its golden by up to 2.
fn goldens() -> Goldens {
    gpui_screenshot::goldens!().tolerance(Tolerance::channel(2))
}

/// A window as it opens, on a new session, `width` logical pixels wide, with
/// the sample sessions in the sidebar.
fn new_window(mode: ThemeMode, width: f32) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    // The harness draws animations at their end, and so does the app then.
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        for family in [prefs::DEFAULT_UI_FONT, prefs::DEFAULT_CODE_FONT] {
            assert!(fonts::is_available(family, cx), "{family} isn't loaded");
        }
    });
    let window = app
        .open_window_with(size(px(width), px(800.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn wide_light() {
    goldens().assert("wide-light", &new_window(ThemeMode::Light, 1300.));
}

#[test]
fn wide_dark() {
    goldens().assert("wide-dark", &new_window(ThemeMode::Dark, 1300.));
}

#[test]
fn half_light() {
    goldens().assert("half-light", &new_window(ThemeMode::Light, 900.));
}

#[test]
fn half_dark() {
    goldens().assert("half-dark", &new_window(ThemeMode::Dark, 900.));
}

#[test]
fn narrow_light() {
    goldens().assert("narrow-light", &new_window(ThemeMode::Light, 600.));
}

#[test]
fn narrow_dark() {
    goldens().assert("narrow-dark", &new_window(ThemeMode::Dark, 600.));
}
