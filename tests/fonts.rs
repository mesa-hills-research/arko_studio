//! Font files added in Settings › Appearance (Add font file…), on the real
//! text system of the screenshot harness, which the test platform's
//! doesn't have.
#![cfg(target_os = "linux")]

use std::cell::RefCell;
use std::sync::Arc;
use std::time::SystemTime;

use arko_studio::backend::FakeBackend;
use arko_studio::prefs::{self, FontChoice, Motion, Prefs};
use arko_studio::{Startup, assets::Assets, fonts, menus, workspace};
use gpui_kit::component::font_picker::FontCatalog;
use gpui_kit::{px, size};
use gpui_screenshot::ScreenshotApp;

/// A font file added while Settings still reads the installed fonts is
/// registered at once, and both font lists have it once they're read.
#[test]
fn a_font_added_while_the_fonts_are_read_is_drawn_and_listed() {
    let temp = tempfile::tempdir().unwrap();
    // The app draws in Work Sans alone. The code font it bundles is the
    // user's to add, from a file.
    let family_of = |data: &[u8]| {
        FontCatalog::from_fonts(&[data]).families()[0]
            .name()
            .to_string()
    };
    let (registered, others): (Vec<_>, Vec<_>) = fonts::bundled()
        .into_iter()
        .partition(|data| family_of(data) == prefs::DEFAULT_UI_FONT);
    let data = others.into_iter().next().unwrap();
    let family = family_of(&data);
    let file = temp.path().join("added.ttf");
    std::fs::write(&file, data.as_ref()).unwrap();

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.motion = Motion::Off;
    settings.appearance.code_font = FontChoice::new(prefs::DEFAULT_UI_FONT, 13.);
    let backend = Arc::new(FakeBackend::sample(
        temp.path().join("arko"),
        SystemTime::now(),
    ));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(registered);
    app.update(|cx| arko_studio::init(startup, cx));
    let opened = RefCell::new(None);
    let window = app
        .open_window_with(size(px(1300.), px(900.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened.replace(Some(workspace));
                window
            })
        })
        .unwrap();
    let workspace = opened.take().unwrap();
    app.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(menus::OpenSettings), cx)
    })
    .unwrap();
    app.update_window(window, |_, window, cx| {
        let settings = workspace.read(cx).settings().unwrap().clone();
        assert!(
            settings.read(cx).font_pickers().is_none(),
            "the fonts are still being read"
        );
        settings.update(cx, |settings, cx| {
            settings.add_font_from(file.clone(), window, cx)
        });
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        let names = cx.text_system().all_font_names();
        assert!(names.contains(&family), "{family} isn't registered");
        let catalog = fonts::picker_catalog(cx).unwrap();
        assert!(
            catalog
                .families()
                .iter()
                .any(|listed| listed.name().as_ref() == family),
            "{family} isn't listed"
        );
        let settings = workspace.read(cx).settings().unwrap().clone();
        assert!(settings.read(cx).font_pickers().is_some());
    });
}
