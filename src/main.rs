// Release builds on Windows start without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use arko_studio::{Startup, assets::Assets, paths, workspace};

fn main() {
    init_logging();

    let startup = Startup::load();
    let app = gpui_kit::application().with_assets(Assets);
    // macOS keeps the app running when its last window closes. Clicking the
    // Dock icon then opens a new one.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            workspace::open(cx);
        }
    });
    app.run(move |cx| {
        arko_studio::init(startup, cx);
        if workspace::open(cx).is_none() {
            panic!("Arko Studio couldn’t open a window");
        }
        cx.activate(true);
    });
}

/// Debug builds log to the terminal. Release builds log to a file, since a
/// Windows app has no console: `<log folder>/<APP_ID>/arko-studio.log`, with
/// the previous run's log kept beside it. `RUST_LOG` sets the level, `info`
/// by default.
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let logger = tracing_subscriber::fmt().with_env_filter(filter);

    let file = if cfg!(debug_assertions) {
        None
    } else {
        open_log_file()
    };
    let Some(file) = file else {
        let _ = logger.try_init();
        return;
    };
    let _ = logger
        .with_ansi(false)
        .with_writer(std::sync::Mutex::new(file))
        .try_init();
    // Panics would otherwise go to the missing console.
    std::panic::set_hook(Box::new(|info| {
        let backtrace = std::backtrace::Backtrace::force_capture();
        tracing::error!("{info}\n{backtrace}");
    }));
}

fn open_log_file() -> Option<std::fs::File> {
    let dir = paths::logs_folder()?;
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("arko-studio.log");
    let _ = std::fs::rename(&path, path.with_extension("old.log"));
    std::fs::File::create(path).ok()
}
