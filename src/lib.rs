//! Arko Studio: the desktop app of the Arko agent, on GPUI and GPUI Kit.

pub mod appearance;
pub mod backend;
mod bundled_themes;
pub mod fonts;
pub mod paths;
pub mod prefs;

/// The name people see in menus, window titles and the About dialog.
pub const APP_NAME: &str = "Arko Studio";

/// The reverse-DNS app id: the Linux `app_id` and desktop file name, the
/// Windows `AppUserModelID` and, once bundled, the macOS bundle id.
pub const APP_ID: &str = "com.mesahillsresearch.ArkoStudio";
