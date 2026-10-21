//! What opening Settings costs on the UI thread, the first time and the
//! times after, measured on GPUI's headless platform with the text system
//! the app has on Linux, the system's fonts included. A frame's time here
//! is `Window::draw`: layout, prepaint and paint, glyphs rasterized as they
//! first show, without the GPU's work.
//!
//! The headless platform runs background tasks on the UI thread when a
//! test waits for them, so the reading done in the background (the font
//! catalog, the accounts) is timed apart.
//!
//! The tests are ignored by default. Run them in release:
//!
//! ```sh
//! cargo test --release --test settings_performance -- --ignored --nocapture --test-threads 1
//! ```
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use arko_studio::backend::FakeBackend;
use arko_studio::prefs::{Motion, Prefs};
use arko_studio::settings_page::{CloseSettings, PAGES};
use arko_studio::{Startup, assets::Assets, fonts, menus, workspace};
use gpui_kit::{
    AnyWindowHandle, Bounds, Entity, HeadlessAppContext, PlatformHeadlessRenderer, WindowBounds,
    WindowOptions, point, px, size,
};
use gpui_wgpu::{CosmicTextSystem, WgpuHeadlessRenderer};

/// An app as on Linux, with the system's fonts and the bundled ones, and
/// its window.
fn app() -> (
    HeadlessAppContext,
    AnyWindowHandle,
    Entity<workspace::Workspace>,
) {
    let text_system = Arc::new(CosmicTextSystem::new("IBM Plex Sans"));
    let mut cx = HeadlessAppContext::with_platform(text_system, Arc::new(Assets), || {
        Ok(Some(
            Box::new(WgpuHeadlessRenderer::new_software()?) as Box<dyn PlatformHeadlessRenderer>
        ))
    });
    let mut prefs = Prefs::default();
    prefs.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(prefs)
        .with_fonts(fonts::bundled());
    cx.update(|cx| arko_studio::init(startup, cx));
    let opened = std::cell::RefCell::new(None);
    let window: AnyWindowHandle = cx
        .update(|cx| {
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1300.), px(900.)),
                })),
                focus: false,
                show: false,
                ..Default::default()
            };
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened.replace(Some(workspace));
                window
            })
        })
        .unwrap();
    let workspace = opened.take().unwrap();
    // The app starts: its first frames, and what it reads in the background.
    frame(&mut cx, window);
    cx.run_until_parked();
    frame(&mut cx, window);
    (cx, window, workspace)
}

fn timed<T>(f: impl FnOnce() -> T) -> (T, Duration) {
    let start = Instant::now();
    let value = f();
    (value, start.elapsed())
}

fn ms(duration: Duration) -> String {
    format!("{:.1} ms", duration.as_secs_f64() * 1000.)
}

fn median(mut times: Vec<Duration>) -> Duration {
    times.sort();
    times[times.len() / 2]
}

/// Draws a frame of `window` and returns how long it took.
fn frame(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Duration {
    cx.update_window(window, |_, window, cx| {
        let start = Instant::now();
        window.draw(cx).clear(cx);
        start.elapsed()
    })
    .unwrap()
}

/// Opens Settings, and returns how long the UI thread took to its first
/// frame: the action, then the window drawn in the same update, and the
/// frame after.
fn open_settings(cx: &mut HeadlessAppContext, window: AnyWindowHandle) -> Duration {
    let (_, opened) = timed(|| {
        cx.update_window(window, |_, window, cx| {
            window.dispatch_action(Box::new(menus::OpenSettings), cx)
        })
        .unwrap()
    });
    opened + frame(cx, window)
}

fn close_settings(cx: &mut HeadlessAppContext, window: AnyWindowHandle) {
    cx.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(CloseSettings), cx)
    })
    .unwrap();
    frame(cx, window);
    cx.run_until_parked();
}

/// Shows each page after the first, and returns how long each took.
fn show_pages(
    cx: &mut HeadlessAppContext,
    window: AnyWindowHandle,
    workspace: &Entity<workspace::Workspace>,
) -> Vec<Duration> {
    (1..PAGES.len())
        .map(|page| {
            let (_, shown) = timed(|| {
                cx.update(|cx| {
                    let settings = workspace.read(cx).settings().unwrap().clone();
                    settings.update(cx, |settings, cx| settings.show_page(page, cx));
                })
            });
            shown + frame(cx, window)
        })
        .collect()
}

#[test]
#[ignore = "a measurement: run it in release"]
fn opening_settings_the_first_time_and_after() {
    let (mut cx, window, workspace) = app();
    for round in 1..=3 {
        let opening = open_settings(&mut cx, window);
        let (_, background) = timed(|| cx.run_until_parked());
        let after = frame(&mut cx, window);
        let pages = show_pages(&mut cx, window, &workspace);
        println!(
            "open {round}: {} to the first frame, {} in the background, {} the frame after, \
             the other pages {} in all (slowest {})",
            ms(opening),
            ms(background),
            ms(after),
            ms(pages.iter().sum()),
            ms(pages.iter().max().copied().unwrap_or_default()),
        );
        close_settings(&mut cx, window);
    }
}

/// Settings' first opening with and without the app's warm-up, each run in
/// an app of its own, and a second opening for comparison: medians.
#[test]
#[ignore = "a measurement: run it in release"]
fn warming_up_before_settings_opens() {
    const RUNS: usize = 9;
    // (The first app reads the font files from the disk.)
    let _ = app();
    for (name, warm_up) in [("no warm-up", false), ("fonts::warm_up", true)] {
        let (mut warms, mut firsts, mut reads, mut seconds) =
            (Vec::new(), Vec::new(), Vec::new(), Vec::new());
        for _ in 0..RUNS {
            let (mut cx, window, _) = app();
            let (_, warm) = timed(|| {
                if warm_up {
                    cx.update(fonts::warm_up);
                    cx.advance_clock(fonts::WARM_UP_AFTER);
                    cx.run_until_parked();
                }
            });
            let first = open_settings(&mut cx, window);
            let (_, read) = timed(|| cx.run_until_parked());
            frame(&mut cx, window);
            close_settings(&mut cx, window);
            let second = open_settings(&mut cx, window);
            cx.run_until_parked();
            warms.push(warm);
            firsts.push(first);
            reads.push(read);
            seconds.push(second);
        }
        println!(
            "{name}: warm-up {} (background), first opening {}, then {} until all is read, \
             second opening {}",
            ms(median(warms)),
            ms(median(firsts)),
            ms(median(reads)),
            ms(median(seconds)),
        );
    }
}
