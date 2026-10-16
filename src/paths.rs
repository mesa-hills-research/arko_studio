//! Where Arko Studio keeps its files. Arko's folder is shared with the
//! terminal app: sessions, keys, history and user themes live there, and
//! Arko Studio adds its own preferences beside them.

use std::path::{Path, PathBuf};

/// Arko's folder: `ARKO_HOME`, or `arko` in the user's data directory, as
/// the terminal app finds it (`arko::session::data_root`).
pub fn arko_folder() -> Option<PathBuf> {
    arko_folder_from(&|name| std::env::var_os(name))
}

/// [`arko_folder`] with the environment given, for tests.
pub fn arko_folder_from(var: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if let Some(home) = var("ARKO_HOME").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(home));
    }
    data_directory(var).map(|base| base.join("arko"))
}

/// The user's data directory, which holds Arko's folder.
fn data_directory(var: &dyn Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    if cfg!(windows) {
        var("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else {
        var("XDG_DATA_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| var("HOME").map(|home| PathBuf::from(home).join(".local/share")))
    }
}

/// Arko Studio's preferences, in Arko's folder.
pub fn prefs_file(arko: &Path) -> PathBuf {
    arko.join("studio.json")
}

/// The themes the user added, shared with the terminal app.
pub fn themes_folder(arko: &Path) -> PathBuf {
    arko.join("themes")
}

/// The font files the user added with Add Font File….
pub fn fonts_folder(arko: &Path) -> PathBuf {
    arko.join("fonts")
}

/// Where release builds write their log: the platform's log folder, under
/// the app id.
pub fn logs_folder() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).map(PathBuf::from);
    let base = if cfg!(target_os = "windows") {
        var("LOCALAPPDATA")
    } else if cfg!(target_os = "macos") {
        var("HOME").map(|home| home.join("Library/Logs"))
    } else {
        var("XDG_STATE_HOME").or_else(|| var("HOME").map(|home| home.join(".local/state")))
    }?;
    Some(base.join(crate::APP_ID))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let pairs: Vec<(String, String)> = pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| OsString::from(value))
        }
    }

    #[test]
    fn arko_home_names_the_folder() {
        let var = env(&[("ARKO_HOME", "/data/arko"), ("HOME", "/home/ada")]);
        assert_eq!(arko_folder_from(&var), Some(PathBuf::from("/data/arko")));
    }

    #[test]
    fn an_empty_arko_home_falls_back_to_the_data_directory() {
        let var = env(&[("ARKO_HOME", ""), ("HOME", "/home/ada")]);
        let folder = arko_folder_from(&var).unwrap();
        assert!(folder.ends_with("arko"));
        if cfg!(target_os = "linux") {
            assert_eq!(folder, PathBuf::from("/home/ada/.local/share/arko"));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn xdg_data_home_wins_over_home_on_linux() {
        let var = env(&[("XDG_DATA_HOME", "/xdg"), ("HOME", "/home/ada")]);
        assert_eq!(arko_folder_from(&var), Some(PathBuf::from("/xdg/arko")));
    }

    #[test]
    fn the_files_sit_in_arkos_folder() {
        let arko = Path::new("/a");
        assert_eq!(prefs_file(arko), Path::new("/a/studio.json"));
        assert_eq!(themes_folder(arko), Path::new("/a/themes"));
        assert_eq!(fonts_folder(arko), Path::new("/a/fonts"));
    }
}
