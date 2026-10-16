// Release builds on Windows start without a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod chrome;
mod menus;
mod title_bar;
mod workspace;

use std::path::PathBuf;

/// The name people see in menus, window titles and the About dialog.
pub const APP_NAME: &str = "Arko Studio";

/// The reverse-DNS app id: the Linux `app_id` and desktop file name, the
/// Windows `AppUserModelID` and, once bundled, the macOS bundle id.
pub const APP_ID: &str = "com.mesahillsresearch.ArkoStudio";

pub const COPYRIGHT: &str = "Copyright © 2026 The Arko Studio Authors";

fn main() {
    init_logging();

    let app = gpui_kit::application().with_assets(gpui_kit::assets::Assets);
    // macOS keeps the app running when its last window closes. Clicking the
    // Dock icon then opens a new one.
    app.on_reopen(|cx| {
        if cx.windows().is_empty() {
            workspace::open(cx);
        }
    });
    app.run(|cx| {
        gpui_kit::init(cx);
        cx.set_app_identity(APP_ID, APP_NAME);
        menus::init(cx); // before any title bar builds its menu bar
        workspace::open(cx);
        cx.activate(true);
    });
}

/// Debug builds log to the terminal. Release builds log to a file, since a
/// Windows app has no console: `<log folder>/<APP_ID>/<crate>.log`, with the
/// previous run's log kept beside it. `RUST_LOG` sets the level, `info` by
/// default.
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
    let dir = log_folder()?.join(APP_ID);
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(concat!(env!("CARGO_PKG_NAME"), ".log"));
    let _ = std::fs::rename(&path, path.with_extension("old.log"));
    std::fs::File::create(path).ok()
}

/// Where the platform keeps app logs.
fn log_folder() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    if cfg!(target_os = "windows") {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Logs"))
    } else {
        var("XDG_STATE_HOME").or_else(|| var("HOME").map(|home| home.join(".local/state")))
    }
}
