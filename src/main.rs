// Release builds on Windows start without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use arko_studio::{Startup, assets::Assets, fonts, paths, workspace};

fn main() {
    // Before any other thread starts: `claude` where Claude Code installs
    // it, when it isn't on the PATH, so every session finds it.
    if let Some(folder) = arko::accounts::add_to_path() {
        eprintln!("Claude Code found in {}", folder.display());
    }
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
        fonts::warm_up(cx);
    });
}

/// Debug builds log to the terminal. Release builds log to a file, since a
/// Windows app has no console: `<log folder>/<APP_ID>/arko-studio.log`, with
/// the previous run's log kept beside it. `RUST_LOG` sets the level, `info`
/// by default.
fn init_logging() {
    // The engine's lines (it logs through `log`) go to `engine.log` beside
    // the app's, each tagged with its session. It owns the `log` logger, so
    // the app's subscriber below doesn't take `log` records.
    if let Some(dir) = paths::logs_folder()
        && let Err(error) = arko::logging::ProcessLog::install(&dir.join("engine.log"))
    {
        eprintln!("couldn't start the engine's log ({error})");
    }
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let logger = tracing_subscriber::fmt().with_env_filter(filter);

    let file = if cfg!(debug_assertions) {
        None
    } else {
        open_log_file()
    };
    let Some(file) = file else {
        let _ = tracing::subscriber::set_global_default(logger.finish());
        return;
    };
    let _ = tracing::subscriber::set_global_default(
        logger
            .with_ansi(false)
            .with_writer(std::sync::Mutex::new(file))
            .finish(),
    );
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
