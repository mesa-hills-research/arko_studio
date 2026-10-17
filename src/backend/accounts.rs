//! The API keys, the ChatGPT login and Claude Code, as Settings › Models &
//! accounts shows them, read without a session.

use std::path::{Path, PathBuf};

pub use arko::accounts::{
    ChatGptLogin, ClaudeCodeStatus, KeySource, KeyState, LoginSource, LoginState, Problem, Ready,
};

/// The variable that turns Anthropic's 1-hour prompt cache on for the
/// sessions arko starts.
pub const CACHE_1H_VARIABLE: &str = "ARKO_CACHE_1H";

/// What can reach the models, as it is now.
#[derive(Debug, Clone, PartialEq)]
pub struct Accounts {
    /// Every API key arko takes, in its keys menu's order: Anthropic,
    /// OpenAI, Gemini, OpenRouter, Voyage.
    pub keys: Vec<KeyState>,
    pub chatgpt: ChatGptLogin,
    pub claude_code: ClaudeCodeStatus,
    /// The 1-hour cache as `ARKO_CACHE_1H` sets it, in the environment or
    /// a `.env` file. `None` where it isn't set.
    pub cache_1h: Option<bool>,
}

impl Accounts {
    /// As arko finds them now: the keys in the app's environment and the
    /// `.env` files it reads (Arko's folder `root`'s last), the ChatGPT
    /// login, Claude Code. It runs `claude`: call it off the UI thread.
    pub fn read(root: &Path) -> Self {
        let keys = key_states_in(root);
        Self {
            keys,
            chatgpt: arko::accounts::chatgpt_login(None),
            claude_code: arko::accounts::claude_code(),
            cache_1h: arko::Env::for_folder_in(None, true, Some(root))
                .var(CACHE_1H_VARIABLE)
                .and_then(|value| switch_value(&value)),
        }
    }

    /// The key of `provider` (as `/keys add` names it: `anthropic`).
    pub fn key(&self, provider: &str) -> Option<&KeyState> {
        self.keys.iter().find(|state| state.key.id == provider)
    }

    /// A set-up for tests and screenshots, in `home` with Arko's folder
    /// `arko`: an Anthropic key saved in Arko's folder, an OpenAI key from
    /// the environment, a Gemini key in a project's `.env`, no ChatGPT
    /// login, and Claude Code signed in to a Claude plan.
    pub fn sample(home: &Path, arko: &Path) -> Self {
        let state = |id: &str, masked: Option<&str>, source: Option<KeySource>| {
            let key = key_named(id).expect("arko takes this key");
            KeyState {
                key,
                masked: masked.map(str::to_string),
                variable: key.name,
                source,
            }
        };
        let claude = home.join(".local/bin/claude");
        Self {
            keys: vec![
                state(
                    "anthropic",
                    Some("sk-ant-…3f9a"),
                    Some(KeySource::Arko(arko.join(".env"))),
                ),
                state("openai", Some("sk-…b20c"), Some(KeySource::Environment)),
                state(
                    "gemini",
                    Some("AIza…k1Qe"),
                    Some(KeySource::DotEnv(home.join("src/arko/.env"))),
                ),
                state("openrouter", None, None),
                state("voyage", None, None),
            ],
            chatgpt: ChatGptLogin {
                path: Some(home.join(".codex/auth.json")),
                source: LoginSource::Default,
                state: LoginState::Missing,
            },
            claude_code: ClaudeCodeStatus {
                path: Some(claude.clone()),
                on_path: true,
                version: Some("2.1.42 (Claude Code)".into()),
                ready: Ok(Ready {
                    path: claude,
                    method: Some("claude.ai".into()),
                    plan: Some("max".into()),
                }),
            },
            cache_1h: None,
        }
    }
}

/// The API keys as arko finds them for Arko's folder `root`, reading the
/// `.env` files as they are now: arko in the terminal, or the user, may
/// have changed them since the app started. What sessions read before one
/// is open (the ChatGPT login too) then comes from these files.
pub fn key_states_in(root: &Path) -> Vec<KeyState> {
    arko::accounts::use_env(arko::Env::for_folder_in(None, true, Some(root)));
    arko::accounts::key_states()
}

/// The key arko takes for `provider`, as `/keys add` names it.
pub fn key_named(provider: &str) -> Option<&'static arko::accounts::Key> {
    arko::accounts::KEYS
        .into_iter()
        .find(|key| key.id == provider)
}

/// A yes-or-no setting as arko reads it: `1`, `true`, `yes`, `on`, or `0`,
/// `false`, `no`, `off`.
pub fn switch_value(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// `value` as arko shows a key: its provider's prefix and its last 4
/// characters.
pub fn masked(value: &str, prefix: &str) -> String {
    let start = if value.starts_with(prefix) {
        prefix
    } else {
        ""
    };
    let count = value.chars().count();
    if count >= start.chars().count() + 12 {
        let end: String = value.chars().skip(count - 4).collect();
        format!("{start}…{end}")
    } else {
        format!("{start}…")
    }
}

/// Where a key comes from, in a few words: "the environment", "Arko's
/// folder", or the `.env` file's path (with `~` for `home`).
pub fn source_words(source: &KeySource, home: Option<&Path>) -> String {
    match source {
        KeySource::Arko(_) => "saved in Arko’s folder".to_string(),
        KeySource::Environment => "from the environment".to_string(),
        KeySource::DotEnv(path) => format!("from {}", tilde(path, home)),
    }
}

/// `path` with `~` for `home`, as macOS and Linux write it (Windows shows
/// it whole).
pub fn tilde(path: &Path, home: Option<&Path>) -> String {
    let home = home.filter(|_| !cfg!(windows));
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~{}{}", std::path::MAIN_SEPARATOR, rest.display()),
        None => path.display().to_string(),
    }
}

/// The variable that names the Codex auth file of the ChatGPT login.
pub const CODEX_AUTH_VARIABLE: &str = "ARKO_CODEX_AUTH";

/// Where [`CODEX_AUTH_VARIABLE`] is set ahead of Arko's folder's `.env`,
/// in words ("from the environment"), if it is: a login chosen in the app
/// is saved in Arko's folder, so that one comes first again in later runs.
pub fn login_set_first() -> Option<String> {
    // Without Arko's folder: the environment and the `.env` beside the
    // program, what arko reads before it.
    arko::Env::for_folder_in(None, true, None).var(CODEX_AUTH_VARIABLE)?;
    let from_environment =
        std::env::var(CODEX_AUTH_VARIABLE).is_ok_and(|value| !value.trim().is_empty());
    Some(if from_environment {
        "from the environment".to_string()
    } else {
        "from the .env beside Arko Studio".to_string()
    })
}

/// What choosing a ChatGPT login did, when `first` (from
/// [`login_set_first`]) sets one ahead of Arko's folder in later runs.
pub fn login_chosen_words(first: Option<&str>) -> String {
    match first {
        None => {
            "Arko uses this ChatGPT login from now on, in new sessions and in arko in the terminal."
                .to_string()
        }
        Some(source) => format!(
            "Arko uses this ChatGPT login until Arko Studio quits. After that, {CODEX_AUTH_VARIABLE} {source} comes first again: change it there to replace it for good."
        ),
    }
}

/// The ChatGPT login after `path` was chosen, as arko keeps it.
pub fn chosen_login(path: PathBuf) -> ChatGptLogin {
    ChatGptLogin {
        path: Some(path),
        source: LoginSource::Entered,
        state: LoginState::Ready,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_read_as_arko_reads_them() {
        assert_eq!(switch_value(" On "), Some(true));
        assert_eq!(switch_value("0"), Some(false));
        assert_eq!(switch_value("maybe"), None);
    }

    #[test]
    fn keys_are_masked_as_arko_masks_them() {
        assert_eq!(
            masked("sk-ant-api03-abcdefghijkl1234", "sk-ant-"),
            "sk-ant-…1234"
        );
        assert_eq!(masked("pa-abcdefghijklmnop", "sk-"), "…mnop");
        assert_eq!(masked("short", "sk-"), "…");
    }

    #[test]
    fn sources_read_in_words() {
        let home = Path::new("/home/ada");
        assert_eq!(
            source_words(&KeySource::DotEnv(home.join("src/arko/.env")), Some(home)),
            "from ~/src/arko/.env"
        );
        assert_eq!(
            source_words(&KeySource::Environment, Some(home)),
            "from the environment"
        );
        assert_eq!(tilde(Path::new("/srv/x"), Some(home)), "/srv/x");
    }

    /// A key written to Arko's `.env` after the app started (by arko in the
    /// terminal, or by hand) shows on its row.
    #[test]
    fn key_rows_read_the_env_files_as_they_are_now() {
        if std::env::var_os("OPENROUTER_API_KEY").is_some() {
            return;
        }
        let root = tempfile::tempdir().unwrap();
        let set = |root: &Path| {
            key_states_in(root)
                .into_iter()
                .any(|state| state.key.id == "openrouter" && state.masked.is_some())
        };
        assert!(!set(root.path()));
        std::fs::write(
            root.path().join(".env"),
            "OPENROUTER_API_KEY=sk-or-v1-notarealkey0000000000\n",
        )
        .unwrap();
        assert!(set(root.path()));
    }

    /// A ChatGPT login chosen in the app is saved in Arko's folder, so one
    /// set ahead of it wins again in later runs, and the app says so.
    #[test]
    fn a_chosen_login_says_for_how_long() {
        assert!(login_chosen_words(None).contains("from now on"));
        let words = login_chosen_words(Some("from the environment"));
        assert!(words.contains("until Arko Studio quits"), "{words}");
        assert!(
            words.contains("ARKO_CODEX_AUTH from the environment comes first again"),
            "{words}"
        );
    }

    #[test]
    fn the_sample_has_every_key_arko_takes() {
        let sample = Accounts::sample(Path::new("/home/ada"), Path::new("/arko"));
        let ids: Vec<&str> = sample.keys.iter().map(|state| state.key.id).collect();
        let arko: Vec<&str> = arko::accounts::KEYS.iter().map(|key| key.id).collect();
        assert_eq!(ids, arko);
        assert!(sample.key("voyage").is_some_and(|key| key.masked.is_none()));
    }
}
