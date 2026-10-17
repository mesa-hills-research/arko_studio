//! What the app's preferences give the sessions it starts: the models and
//! settings of new sessions (Settings › Models & accounts and Sessions),
//! and what every session gets as it opens (the MCP config files, sharing
//! the web page).

use std::net::IpAddr;
use std::path::PathBuf;

use crate::prefs::{self, Prefs};

use super::Mode;

/// The preferences a session's options come from, as the app holds them
/// now. The backend keeps a copy ([`super::Backend::set_defaults`]), since
/// sessions start on threads that can't read the app's state.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionDefaults {
    pub mode: Mode,
    /// The main agent's model, as `/model` takes it. `None`: arko's choice.
    pub model: Option<String>,
    pub effort: Option<arko::Effort>,
    pub worker_model: Option<String>,
    pub worker_effort: Option<arko::Effort>,
    /// `None`: as `ARKO_CACHE_1H` says.
    pub cache_1h: Option<bool>,
    pub handover_at: Option<usize>,
    pub workers_handover_at: Option<usize>,
    /// In seconds, 0 for never. `None`: arko's own.
    pub background_after: Option<u64>,
    pub outside_access: bool,
    pub text_replies: bool,
    pub cc_compact: bool,
    pub extra_rules: arko::ExtraRules,
    pub project_instructions: arko::ProjectFiles,
    pub worker_project_instructions: bool,
    /// A new chat's instructions, `None` when there are none.
    pub chat_instructions: Option<String>,
    /// `None`: as `ARKO_SHELL` says.
    pub shell: Option<arko::WindowsShell>,
    /// MCP config files every session reads (arko's `--mcp-config`).
    pub mcp_config: Vec<PathBuf>,
    /// Where every session serves its web page as it opens. `None` when it
    /// doesn't.
    pub web_page: Option<arko::remote::Bind>,
}

impl Default for SessionDefaults {
    fn default() -> Self {
        Self::from_prefs(&Prefs::default())
    }
}

impl SessionDefaults {
    pub fn from_prefs(prefs: &Prefs) -> Self {
        let new = &prefs.new_sessions;
        let effort = |effort: &Option<String>| effort.as_deref().and_then(|e| e.parse().ok());
        let instructions = new.chat_instructions.trim();
        Self {
            mode: match new.mode {
                prefs::NewSessionMode::Chat => Mode::Chat,
                prefs::NewSessionMode::Agent => Mode::Agent,
            },
            model: new.model.clone().filter(|model| !model.trim().is_empty()),
            effort: effort(&new.effort),
            worker_model: new
                .worker_model
                .clone()
                .filter(|model| !model.trim().is_empty()),
            worker_effort: effort(&new.worker_effort),
            cache_1h: new.cache_1h,
            handover_at: new.handover_at,
            workers_handover_at: new.workers_handover_at,
            background_after: new.background_after,
            outside_access: new.outside_access,
            text_replies: new.text_replies,
            cc_compact: new.cc_compact,
            extra_rules: match new.extra_rules {
                prefs::ExtraRules::Small => arko::ExtraRules::Small,
                prefs::ExtraRules::All => arko::ExtraRules::All,
                prefs::ExtraRules::Off => arko::ExtraRules::Off,
            },
            project_instructions: match new.project_instructions {
                prefs::ProjectInstructions::One => arko::ProjectFiles::One,
                prefs::ProjectInstructions::Both => arko::ProjectFiles::Both,
                prefs::ProjectInstructions::Off => arko::ProjectFiles::Off,
            },
            worker_project_instructions: new.worker_project_instructions,
            chat_instructions: (!instructions.is_empty()).then(|| instructions.to_string()),
            shell: prefs.advanced.windows_shell.map(|shell| match shell {
                prefs::WindowsShell::Cmd => arko::WindowsShell::Cmd,
                prefs::WindowsShell::Powershell => arko::WindowsShell::Powershell,
            }),
            mcp_config: prefs.advanced.mcp_config.clone(),
            web_page: prefs
                .web_page
                .share_by_default
                .then(|| web_page_bind(&prefs.web_page))
                .flatten(),
        }
    }

    /// Fills in `options` for a session starting: one resumed (`new` is
    /// `None`) gets only what every session gets, since it keeps the
    /// models and settings it was started with. A new one gets what the
    /// user didn't choose in its composer. `last_model` is the last
    /// session's main model, which a new session takes when no model is
    /// chosen (read only when it matters).
    pub fn apply(
        &self,
        options: &mut arko::Options,
        new: Option<Mode>,
        last_model: impl FnOnce() -> Option<String>,
    ) {
        options.mcp_config.extend(self.mcp_config.iter().cloned());
        if options.remote.is_none() {
            options.remote = self.web_page;
        }
        let Some(mode) = new else {
            return;
        };
        if options.model.is_none() {
            options.model = self.model.clone();
        }
        if options.worker_model.is_none() {
            options.worker_model = self.worker_model.clone();
        }
        if options.worker_effort.is_none() {
            options.worker_effort = self.worker_effort;
        }
        // The 1-hour cache is Anthropic's, by API key: it's asked for only
        // when one of the session's models takes it, as arko notes it
        // otherwise.
        options.cache_1h = match self.cache_1h {
            Some(true) => {
                let main = options.model.clone().or_else(last_model);
                let takes = main
                    .iter()
                    .chain(options.worker_model.as_ref())
                    .any(|model| takes_cache_1h(model));
                takes.then_some(true)
            }
            other => other,
        };
        options.handover_at = self.handover_at;
        options.workers_handover_at = self.workers_handover_at;
        options.background_after = self.background_after;
        if cfg!(windows) {
            options.shell = self.shell;
        }
        match mode {
            Mode::Agent => {
                options.allow_outside_workspace = self.outside_access;
                options.text_replies = Some(self.text_replies);
                options.cc_compact = Some(self.cc_compact);
                options.extra_rules = Some(self.extra_rules);
                options.project_instructions = Some(self.project_instructions);
                options.worker_project_instructions = Some(self.worker_project_instructions);
            }
            Mode::Chat => options.chat_instructions = self.chat_instructions.clone(),
        }
    }
}

/// Leaves out of `files`, the MCP config files set in Settings, those that
/// can't be read, and says why for each. arko stops a session that names
/// a file it can't read, as `--mcp-config` does for one run, while these
/// are kept for every session. Arko's `mcp.json` and a project's are read
/// by arko, which notes their problems itself. It reads the files: call it
/// off the UI thread.
pub fn keep_readable_mcp_config(files: &mut Vec<PathBuf>) -> Vec<String> {
    let mut notes = Vec::new();
    files.retain(|file| match super::extensions::check_mcp_file(file) {
        Ok(()) => true,
        Err(why) => {
            notes.push(format!(
                "The MCP config file {} (Settings › Advanced) can’t be read: {why}. Its servers aren’t started.",
                file.display()
            ));
            false
        }
    });
    notes
}

/// Whether the model `name` (as `/model` takes it) keeps Anthropic's 1-hour
/// prompt cache: Claude by API key, directly or on OpenRouter. Claude Code
/// chooses its own.
pub fn takes_cache_1h(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    if name.starts_with("cc:") || name.starts_with("claude-code:") {
        return false;
    }
    // A `~` asks OpenRouter for the latest of a family.
    if let Some(slug) = name.strip_prefix("openrouter:") {
        return slug.trim_start_matches('~').starts_with("anthropic/");
    }
    name.starts_with("claude-") || matches!(name.as_str(), "opus" | "sonnet" | "haiku" | "fable")
}

/// Where the web page is served, from the preferences: `None` when the
/// address isn't an IP address.
pub fn web_page_bind(page: &prefs::WebPagePrefs) -> Option<arko::remote::Bind> {
    let address = page.address.trim();
    let address = address.trim_start_matches('[').trim_end_matches(']');
    let ip = if address.is_empty() {
        prefs::DEFAULT_WEB_PAGE_ADDRESS.parse().ok()?
    } else {
        address.parse::<IpAddr>().ok()?
    };
    Some(arko::remote::Bind {
        ip,
        port: page.port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_options() -> arko::Options {
        arko::Options::default()
    }

    #[test]
    fn the_defaults_leave_every_choice_to_arko() {
        let mut options = new_options();
        SessionDefaults::default().apply(&mut options, Some(Mode::Agent), || None);
        let plain = new_options();
        assert_eq!(options.model, plain.model);
        assert_eq!(options.worker_model, plain.worker_model);
        assert_eq!(options.cache_1h, None);
        assert_eq!(options.handover_at, None);
        assert_eq!(options.background_after, None);
        assert!(!options.allow_outside_workspace);
        assert_eq!(options.text_replies, Some(true));
        assert_eq!(options.cc_compact, Some(true));
        assert_eq!(options.extra_rules, Some(arko::ExtraRules::Small));
        assert_eq!(options.project_instructions, Some(arko::ProjectFiles::One));
        assert_eq!(options.remote, None);
        assert!(options.mcp_config.is_empty());
    }

    #[test]
    fn a_new_agent_session_gets_the_preferences() {
        let mut prefs = Prefs::default();
        let new = &mut prefs.new_sessions;
        new.model = Some("claude-opus-5-5".into());
        new.worker_model = Some("gpt-6.1-sol".into());
        new.worker_effort = Some("medium".into());
        new.cache_1h = Some(true);
        new.handover_at = Some(950_000);
        new.workers_handover_at = Some(300_000);
        new.background_after = Some(0);
        new.outside_access = true;
        new.text_replies = false;
        new.cc_compact = false;
        new.extra_rules = prefs::ExtraRules::All;
        new.project_instructions = prefs::ProjectInstructions::Both;
        new.worker_project_instructions = false;
        new.chat_instructions = "Answer briefly.".into();
        prefs.advanced.mcp_config = vec![PathBuf::from("/home/ada/mcp.json")];
        let defaults = SessionDefaults::from_prefs(&prefs);
        let mut options = new_options();
        defaults.apply(&mut options, Some(Mode::Agent), || None);
        assert_eq!(options.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(options.worker_model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(options.worker_effort, Some(arko::Effort::Medium));
        assert_eq!(options.cache_1h, Some(true));
        assert_eq!(options.handover_at, Some(950_000));
        assert_eq!(options.workers_handover_at, Some(300_000));
        assert_eq!(options.background_after, Some(0));
        assert!(options.allow_outside_workspace);
        assert_eq!(options.text_replies, Some(false));
        assert_eq!(options.cc_compact, Some(false));
        assert_eq!(options.extra_rules, Some(arko::ExtraRules::All));
        assert_eq!(options.project_instructions, Some(arko::ProjectFiles::Both));
        assert_eq!(options.worker_project_instructions, Some(false));
        assert_eq!(options.chat_instructions, None, "an agent has no chat's");
        assert_eq!(options.mcp_config, [PathBuf::from("/home/ada/mcp.json")]);
    }

    /// The model chosen in the composer wins over the default.
    #[test]
    fn what_the_composer_chose_stays() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.model = Some("claude-opus-5-5".into());
        let mut options = new_options();
        options.model = Some("sonnet".into());
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent), || None);
        assert_eq!(options.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn a_new_chat_gets_its_instructions_and_no_agent_settings() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.chat_instructions = "  Answer briefly.\n".into();
        prefs.new_sessions.outside_access = true;
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Chat), || None);
        assert_eq!(
            options.chat_instructions.as_deref(),
            Some("Answer briefly.")
        );
        assert!(!options.allow_outside_workspace);
        assert_eq!(options.text_replies, None);
        assert_eq!(options.project_instructions, None);
    }

    /// A resumed session keeps its own models and settings: asking for
    /// others would only make arko say so.
    #[test]
    fn a_resumed_session_gets_only_the_mcp_files_and_the_web_page() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.model = Some("claude-opus-5-5".into());
        prefs.new_sessions.handover_at = Some(300_000);
        prefs.new_sessions.outside_access = true;
        prefs.advanced.mcp_config = vec![PathBuf::from("/home/ada/mcp.json")];
        prefs.web_page.share_by_default = true;
        prefs.web_page.address = "0.0.0.0".into();
        prefs.web_page.port = Some(9000);
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, None, || None);
        assert_eq!(options.model, None);
        assert_eq!(options.handover_at, None);
        assert_eq!(options.workers_handover_at, None);
        assert!(!options.allow_outside_workspace);
        assert_eq!(options.text_replies, None);
        assert_eq!(options.mcp_config.len(), 1);
        let bind = options.remote.unwrap();
        assert_eq!(bind.ip.to_string(), "0.0.0.0");
        assert_eq!(bind.port, Some(9000));
    }

    #[test]
    fn the_hour_long_cache_is_asked_for_only_where_it_applies() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.cache_1h = Some(true);
        let defaults = SessionDefaults::from_prefs(&prefs);
        let cache = |model: Option<&str>, last: Option<&str>| {
            let mut options = new_options();
            options.model = model.map(str::to_string);
            defaults.apply(&mut options, Some(Mode::Agent), || last.map(str::to_string));
            options.cache_1h
        };
        assert_eq!(cache(Some("claude-sonnet-5-5"), None), Some(true));
        assert_eq!(
            cache(Some("openrouter:anthropic/claude-opus-5.5"), None),
            Some(true)
        );
        assert_eq!(cache(Some("cc:claude-opus-5-5"), None), None);
        assert_eq!(cache(Some("claude-code:opus"), None), None);
        assert_eq!(
            cache(Some("openrouter:~anthropic/claude-opus-latest"), None),
            Some(true)
        );
        assert_eq!(cache(Some("gpt-6.1-sol"), None), None);
        assert_eq!(cache(None, Some("claude-opus-5-5")), Some(true));
        assert_eq!(cache(None, None), None);
        // Off is passed on, over ARKO_CACHE_1H.
        prefs.new_sessions.cache_1h = Some(false);
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent), || None);
        assert_eq!(options.cache_1h, Some(false));
    }

    #[test]
    fn the_web_page_is_served_from_the_start_only_when_asked() {
        let mut prefs = Prefs::default();
        assert_eq!(SessionDefaults::from_prefs(&prefs).web_page, None);
        prefs.web_page.share_by_default = true;
        let bind = SessionDefaults::from_prefs(&prefs).web_page.unwrap();
        assert_eq!(bind, arko::remote::Bind::default());
        prefs.web_page.address = "[::1]".into();
        assert_eq!(
            SessionDefaults::from_prefs(&prefs)
                .web_page
                .unwrap()
                .ip
                .to_string(),
            "::1"
        );
        prefs.web_page.address = "my-laptop".into();
        assert_eq!(SessionDefaults::from_prefs(&prefs).web_page, None);
    }

    #[test]
    fn efforts_and_empty_names_are_read_as_arko_reads_them() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.effort = Some("HIGH".into());
        prefs.new_sessions.worker_effort = Some("lots".into());
        prefs.new_sessions.model = Some("  ".into());
        let defaults = SessionDefaults::from_prefs(&prefs);
        assert_eq!(defaults.effort, Some(arko::Effort::High));
        assert_eq!(defaults.worker_effort, None);
        assert_eq!(defaults.model, None);
    }
}
