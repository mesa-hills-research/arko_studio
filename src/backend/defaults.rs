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
    /// How long monitors' warnings wake the agent. `None`: arko's own.
    pub monitor_wakes: Option<arko::monitors::Wakes>,
    /// Who chooses the workers' models.
    pub worker_models: arko::ModelChoice,
    /// Whether workers may run on models billed per token. `None`: as
    /// `ARKO_WORKER_BILLING` says.
    pub worker_billing: Option<arko::WorkerBilling>,
    /// Whether the daily API budget is on, at Arko's default amount.
    /// `None`: as `ARKO_API_BUDGET` says.
    pub api_budget: Option<bool>,
    /// How many levels of workers, when it is a depth a session can have.
    pub worker_depth: Option<u32>,
    /// Whether a worker fails after Arko's number of failed model requests
    /// in a row.
    pub worker_retry_cap: bool,
    /// Whether the agent that started a worker gets a copy of the user's
    /// messages to it.
    pub message_copies: bool,
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
    /// Whether every session gets Arko's built-in skills. `None`: as
    /// `ARKO_BUILTIN_SKILLS` says.
    pub builtin_skills: Option<bool>,
    /// The built-in skills every session leaves out, by name, when Arko
    /// knows them. `None`: as `ARKO_BUILTIN_SKILLS_OFF` says.
    pub builtin_skills_off: Option<Vec<String>>,
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
            mode: match new.starting_mode() {
                prefs::NewSessionMode::Chat => Mode::Chat,
                _ => Mode::Agent,
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
            monitor_wakes: new
                .monitor_wakes
                .as_deref()
                .and_then(|wakes| arko::monitors::parse_wakes(wakes).ok()),
            worker_models: new.worker_models,
            worker_billing: new.worker_billing,
            api_budget: new.api_budget,
            worker_depth: arko::workers::check_depth(new.worker_depth).ok(),
            worker_retry_cap: new.worker_retry_cap,
            message_copies: new.message_copies,
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
            builtin_skills: prefs.extensions.builtin_skills,
            // A name Arko doesn't know is a warning in every session, and
            // the page has no switch to clear it.
            builtin_skills_off: prefs.extensions.builtin_skills_off.as_ref().map(|names| {
                names
                    .iter()
                    .filter(|name| arko::builtin_skills::names().contains(&name.as_str()))
                    .cloned()
                    .collect()
            }),
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
    /// user didn't choose in its composer. The engine resolves any model
    /// left unchosen, then decides which settings that model supports.
    pub fn apply(&self, options: &mut arko::Options, new: Option<Mode>) {
        options.mcp_config.extend(self.mcp_config.iter().cloned());
        // A resumed session takes the choice for the conversations that
        // start after, so it applies to every session as it opens.
        options.builtin_skills = self.builtin_skills;
        options.builtin_skills_off = self.builtin_skills_off.clone();
        if options.remote.is_none() {
            options.remote = self.web_page;
            options.remote_port_fallback = self.web_page.is_some();
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
        // The engine checks capability after resolving its default model,
        // including the first session with no saved or chosen model.
        options.cache_1h = self.cache_1h;
        options.handover_at = self.handover_at;
        options.workers_handover_at = self.workers_handover_at;
        options.background_after = self.background_after;
        options.monitor_wakes = self.monitor_wakes;
        options.worker_models = Some(self.worker_models);
        options.worker_billing = self.worker_billing;
        options.api_budget = self
            .api_budget
            .map(|on| on.then_some(arko::workers::DEFAULT_BUDGET));
        options.worker_depth = self.worker_depth;
        options.worker_retry_cap = self
            .worker_retry_cap
            .then_some(arko::workers::DEFAULT_RETRY_CAP);
        options.message_copies = Some(self.message_copies);
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

/// Where the web page is served, from the preferences: `None` when the
/// address isn't an IP address.
pub fn web_page_bind(page: &prefs::WebPagePrefs) -> Option<arko::remote::Bind> {
    let address = page.address();
    let address = address.trim_start_matches('[').trim_end_matches(']');
    Some(arko::remote::Bind {
        ip: address.parse::<IpAddr>().ok()?,
        port: page.port,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_first_session_keeps_its_long_cache_preference() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.cache_1h = Some(true);
        let mut options = arko::Options::default();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.model, None);
        assert_eq!(options.cache_1h, Some(true));
    }

    use super::*;

    fn new_options() -> arko::Options {
        arko::Options::default()
    }

    #[test]
    fn the_defaults_leave_every_choice_to_arko() {
        let mut options = new_options();
        SessionDefaults::default().apply(&mut options, Some(Mode::Agent));
        let plain = new_options();
        assert_eq!(options.model, plain.model);
        assert_eq!(options.worker_model, plain.worker_model);
        assert_eq!(options.cache_1h, None);
        assert_eq!(options.handover_at, None);
        assert_eq!(options.background_after, None);
        assert_eq!(options.monitor_wakes, None);
        // Arko's own choices for the workers, passed on as set.
        assert_eq!(options.worker_models, Some(arko::ModelChoice::Agent));
        assert_eq!(options.worker_depth, Some(arko::workers::DEFAULT_DEPTH));
        assert!(!options.allow_outside_workspace);
        assert_eq!(options.text_replies, Some(true));
        assert_eq!(options.cc_compact, Some(true));
        assert_eq!(options.extra_rules, Some(arko::ExtraRules::Small));
        assert_eq!(options.project_instructions, Some(arko::ProjectFiles::One));
        assert_eq!(options.remote, None);
        assert!(options.mcp_config.is_empty());
        assert_eq!(options.builtin_skills, None);
        assert_eq!(options.builtin_skills_off, None);
    }

    /// The built-in skill choice goes to a new and to a resumed session, a
    /// chat too. A name Arko doesn't know is dropped, since Arko would warn
    /// of it in every session.
    #[test]
    fn the_built_in_skill_choice_reaches_new_and_resumed_sessions() {
        let mut prefs = Prefs::default();
        prefs.extensions.builtin_skills = Some(false);
        prefs.extensions.builtin_skills_off = Some(vec![
            "repo-docs-style".into(),
            "dropped-by-a-later-arko".into(),
        ]);
        let defaults = SessionDefaults::from_prefs(&prefs);
        for mode in [Some(Mode::Agent), Some(Mode::Chat), None] {
            let mut options = new_options();
            defaults.apply(&mut options, mode);
            assert_eq!(options.builtin_skills, Some(false), "{mode:?}");
            assert_eq!(
                options.builtin_skills_off,
                Some(vec!["repo-docs-style".to_string()]),
                "{mode:?}"
            );
        }
        // Switched back on, it is passed on too, so a resumed session that
        // saved them off gets them again.
        prefs.extensions.builtin_skills = Some(true);
        prefs.extensions.builtin_skills_off = Some(Vec::new());
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, None);
        assert_eq!(options.builtin_skills, Some(true));
        assert_eq!(options.builtin_skills_off, Some(Vec::new()));
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
        new.monitor_wakes = Some("always".into());
        new.worker_models = arko::ModelChoice::Profiles;
        new.worker_depth = 3;
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
        defaults.apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.model.as_deref(), Some("claude-opus-5-5"));
        assert_eq!(options.worker_model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(options.worker_effort, Some(arko::Effort::Medium));
        assert_eq!(options.cache_1h, Some(true));
        assert_eq!(options.handover_at, Some(950_000));
        assert_eq!(options.workers_handover_at, Some(300_000));
        assert_eq!(options.background_after, Some(0));
        assert_eq!(options.monitor_wakes, Some(arko::monitors::Wakes::Always));
        assert_eq!(options.worker_models, Some(arko::ModelChoice::Profiles));
        assert_eq!(options.worker_depth, Some(3));
        assert!(options.allow_outside_workspace);
        assert_eq!(options.text_replies, Some(false));
        assert_eq!(options.cc_compact, Some(false));
        assert_eq!(options.extra_rules, Some(arko::ExtraRules::All));
        assert_eq!(options.project_instructions, Some(arko::ProjectFiles::Both));
        assert_eq!(options.worker_project_instructions, Some(false));
        assert_eq!(options.chat_instructions, None, "an agent has no chat's");
        assert_eq!(options.mcp_config, [PathBuf::from("/home/ada/mcp.json")]);
    }

    /// A new session has no worker retry cap unless the preference turns it
    /// on, at Arko's default, and a resumed session keeps its own.
    #[test]
    fn the_worker_retry_cap_is_off_in_a_new_session_unless_chosen() {
        let mut prefs = Prefs::default();
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.worker_retry_cap, None);
        prefs.new_sessions.worker_retry_cap = true;
        let defaults = SessionDefaults::from_prefs(&prefs);
        let mut options = new_options();
        defaults.apply(&mut options, Some(Mode::Agent));
        assert_eq!(
            options.worker_retry_cap,
            Some(arko::workers::DEFAULT_RETRY_CAP)
        );
        let mut options = new_options();
        defaults.apply(&mut options, None);
        assert_eq!(options.worker_retry_cap, None);
    }

    /// A new session starts with message copies off unless the preference
    /// turns them on, and a resumed session keeps its own.
    #[test]
    fn message_copies_are_off_in_a_new_session_unless_chosen() {
        let mut prefs = Prefs::default();
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.message_copies, Some(false));
        prefs.new_sessions.message_copies = true;
        let defaults = SessionDefaults::from_prefs(&prefs);
        let mut options = new_options();
        defaults.apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.message_copies, Some(true));
        let mut options = new_options();
        defaults.apply(&mut options, None);
        assert_eq!(options.message_copies, None);
    }

    /// Worker billing and the daily API budget are left to Arko (its
    /// environment, else ask first and no budget) until the preferences
    /// choose them. On, the budget is Arko's default amount. A resumed
    /// session keeps its own.
    #[test]
    fn worker_billing_and_the_budget_are_arkos_until_chosen() {
        let mut prefs = Prefs::default();
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.worker_billing, None);
        assert_eq!(options.api_budget, None);
        prefs.new_sessions.worker_billing = Some(arko::WorkerBilling::Plans);
        prefs.new_sessions.api_budget = Some(true);
        let defaults = SessionDefaults::from_prefs(&prefs);
        let mut options = new_options();
        defaults.apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.worker_billing, Some(arko::WorkerBilling::Plans));
        assert_eq!(
            options.api_budget,
            Some(Some(arko::workers::DEFAULT_BUDGET))
        );
        let mut options = new_options();
        defaults.apply(&mut options, None);
        assert_eq!(options.worker_billing, None);
        assert_eq!(options.api_budget, None);
        // Off is passed on, over ARKO_API_BUDGET.
        prefs.new_sessions.api_budget = Some(false);
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.api_budget, Some(None));
    }

    /// The model chosen in the composer wins over the default.
    #[test]
    fn what_the_composer_chose_stays() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.model = Some("claude-opus-5-5".into());
        let mut options = new_options();
        options.model = Some("sonnet".into());
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.model.as_deref(), Some("sonnet"));
    }

    /// A depth a hand-edited file holds that a session can't have is left to
    /// Arko, which would refuse the number.
    #[test]
    fn a_worker_depth_out_of_range_is_left_to_arko() {
        for depth in [0, 5, 100] {
            let mut prefs = Prefs::default();
            prefs.new_sessions.worker_depth = depth;
            let mut options = new_options();
            SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
            assert_eq!(options.worker_depth, None, "{depth}");
        }
    }

    #[test]
    fn a_new_chat_gets_its_instructions_and_no_agent_settings() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.chat_instructions = "  Answer briefly.\n".into();
        prefs.new_sessions.outside_access = true;
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Chat));
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
        prefs.new_sessions.worker_models = arko::ModelChoice::Session;
        prefs.new_sessions.outside_access = true;
        prefs.advanced.mcp_config = vec![PathBuf::from("/home/ada/mcp.json")];
        prefs.web_page.share_by_default = true;
        prefs.web_page.address = Some("0.0.0.0".into());
        prefs.web_page.port = Some(9000);
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, None);
        assert_eq!(options.model, None);
        assert_eq!(options.handover_at, None);
        assert_eq!(options.workers_handover_at, None);
        assert_eq!(options.worker_models, None);
        assert_eq!(options.worker_depth, None);
        assert!(!options.allow_outside_workspace);
        assert_eq!(options.text_replies, None);
        assert_eq!(options.mcp_config.len(), 1);
        let bind = options.remote.unwrap();
        assert_eq!(bind.ip.to_string(), "0.0.0.0");
        assert_eq!(bind.port, Some(9000));
    }

    #[test]
    fn the_engine_applies_the_cache_preference_after_choosing_its_models() {
        let mut prefs = Prefs::default();
        prefs.new_sessions.cache_1h = Some(true);
        let defaults = SessionDefaults::from_prefs(&prefs);
        for model in [
            None,
            Some("claude-sonnet-5-5"),
            Some("cc:opus"),
            Some("gpt-6.1-sol"),
        ] {
            let mut options = new_options();
            options.model = model.map(str::to_string);
            defaults.apply(&mut options, Some(Mode::Agent));
            assert_eq!(options.cache_1h, Some(true));
        }
        // Off is passed on, over ARKO_CACHE_1H.
        prefs.new_sessions.cache_1h = Some(false);
        let mut options = new_options();
        SessionDefaults::from_prefs(&prefs).apply(&mut options, Some(Mode::Agent));
        assert_eq!(options.cache_1h, Some(false));
    }

    #[test]
    fn the_web_page_is_served_from_the_start_only_when_asked() {
        let mut prefs = Prefs::default();
        assert_eq!(SessionDefaults::from_prefs(&prefs).web_page, None);
        prefs.web_page.share_by_default = true;
        // Every network this computer is on, by default.
        let bind = SessionDefaults::from_prefs(&prefs).web_page.unwrap();
        assert_eq!(bind.ip.to_string(), "0.0.0.0");
        assert_eq!(bind.port, None);
        prefs.web_page.address = Some("127.0.0.1".into());
        let bind = SessionDefaults::from_prefs(&prefs).web_page.unwrap();
        assert_eq!(bind, arko::remote::Bind::default(), "arko's own");
        prefs.web_page.address = Some("[::1]".into());
        assert_eq!(
            SessionDefaults::from_prefs(&prefs)
                .web_page
                .unwrap()
                .ip
                .to_string(),
            "::1"
        );
        prefs.web_page.address = Some("my-laptop".into());
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
