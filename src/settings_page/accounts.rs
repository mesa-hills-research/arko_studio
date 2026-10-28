//! Settings › Models & accounts: the models and effort new sessions start
//! with, and what reaches the models: the API keys (where each comes
//! from, adding, replacing and removing one), the ChatGPT logins (in the
//! order Arko uses them) and Claude Code. Everything is read and changed
//! through the app's backend, on a background thread.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{
    ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::openrouter::OpenRouterPart;
use crate::backend::account_words::{
    CACHE_1H_VARIABLE, ChatGptLogin, ClaudeCodeStatus, KeySource, KeyState, LoginSource,
    LoginState, Problem, outdated_text, source_words, tilde,
};
use crate::backend::{Accounts, ModelChoices, SharedBackend, last_model};
use crate::model_select::ModelSelect;
use crate::prefs::{self, NewSessionMode};
use crate::session_view::HomeFolder;
use crate::workers::curly;

/// The accounts the page has a row for, by the name the page points at
/// them with: each provider's key as `/keys` names it, the ChatGPT login,
/// Claude Code and OpenRouter's routing settings.
pub const ACCOUNTS: [&str; 8] = [
    "anthropic",
    "openai",
    "gemini",
    "openrouter",
    "voyage",
    CHATGPT,
    CLAUDE_CODE,
    OPENROUTER_SETTINGS,
];
pub const CHATGPT: &str = "chatgpt";
pub const CLAUDE_CODE: &str = "claude-code";
pub const OPENROUTER_SETTINGS: &str = "openrouter-settings";
/// What opens the page at OpenRouter's routing settings, as a group of
/// models would name what it needs.
pub const OPENROUTER_WAY: &str = "OpenRouter settings";

/// The account a group of models needs, by the group's name as arko's
/// model list heads it: "OpenAI, with your API key" needs the OpenAI key,
/// "Claude Code, on your Claude plan" Claude Code.
pub fn account_for(way: &str) -> Option<&'static str> {
    let way = way.trim();
    Some(if way.starts_with("Claude Code") {
        CLAUDE_CODE
    } else if way.starts_with("Anthropic") {
        "anthropic"
    } else if way.starts_with("OpenAI") && way.contains("ChatGPT") {
        CHATGPT
    } else if way.starts_with("OpenAI") {
        "openai"
    } else if way.starts_with("Google") || way.starts_with("Gemini") {
        "gemini"
    } else if way == OPENROUTER_WAY {
        OPENROUTER_SETTINGS
    } else if way.starts_with("OpenRouter") {
        "openrouter"
    } else {
        return None;
    })
}

/// The page's state: the accounts and models as last read.
pub struct AccountsPage {
    accounts: Option<Accounts>,
    models: Option<ModelChoices>,
    /// The Model and Default workers' model settings.
    main_model: Entity<ModelSelect>,
    worker_model: Entity<ModelSelect>,
    /// The OpenRouter group.
    openrouter: Entity<OpenRouterPart>,
    /// The last session's main model: what "Last used" stands for.
    last_model: Option<String>,
    /// Claude Code is being checked again.
    checking: bool,
    home: Option<PathBuf>,
    /// The account the page was opened for (a model that needs it): its
    /// row is marked, and has the keyboard focus.
    target: Option<&'static str>,
    /// Each account row's focus, by [`ACCOUNTS`] name.
    focus: Vec<(&'static str, FocusHandle)>,
    _load: Option<Task<()>>,
    _openrouter: Subscription,
}

impl AccountsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let main_model = cx.new(|cx| {
            let mut select = ModelSelect::new(
                "new-sessions-model",
                "Model",
                Rc::new(|cx: &App| {
                    prefs::get(cx)
                        .new_sessions
                        .model
                        .clone()
                        .unwrap_or_default()
                        .into()
                }),
                Rc::new(|model, _, cx| prefs::update(cx, |prefs| prefs.new_sessions.model = model)),
                window,
                cx,
            );
            select.set_models("Last used", None, cx);
            select
        });
        let worker_model = cx.new(|cx| {
            let mut select = ModelSelect::new(
                "new-sessions-worker-model",
                "Default workers’ model",
                Rc::new(|cx: &App| {
                    prefs::get(cx)
                        .new_sessions
                        .worker_model
                        .clone()
                        .unwrap_or_default()
                        .into()
                }),
                Rc::new(|model, _, cx| {
                    prefs::update(cx, |prefs| prefs.new_sessions.worker_model = model)
                }),
                window,
                cx,
            );
            select.set_models("Automatic", None, cx);
            select
        });
        let openrouter = cx.new(|cx| OpenRouterPart::new(window, cx));
        let observing = cx.observe(&openrouter, |_, _, cx| cx.notify());
        let mut this = Self {
            accounts: None,
            models: None,
            main_model,
            worker_model,
            openrouter,
            last_model: None,
            checking: false,
            home: cx.global::<HomeFolder>().0.clone(),
            target: None,
            focus: ACCOUNTS
                .iter()
                .map(|account| (*account, cx.focus_handle()))
                .collect(),
            _load: None,
            _openrouter: observing,
        };
        this.refresh(cx);
        this
    }

    /// Marks `account`'s row (or none) and moves the keyboard to it.
    pub fn point_at(
        &mut self,
        account: Option<&'static str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.target = account;
        if let Some(focus) = account.and_then(|account| self.focus_of(account)) {
            // After the page is drawn with the row in it.
            window.defer(cx, move |window, cx| focus.focus(window, cx));
        }
        cx.notify();
    }

    /// The account the page points at, if any.
    pub fn target(&self) -> Option<&'static str> {
        self.target
    }

    /// The focus of `account`'s row.
    pub fn focus_of(&self, account: &str) -> Option<FocusHandle> {
        self.focus
            .iter()
            .find(|(name, _)| *name == account)
            .map(|(_, focus)| focus.clone())
    }

    /// The row's controls of `account`, focusable, and marked when the page
    /// points at it. With the keyboard on the row, Enter or Space does
    /// `primary`, what its main button does (Add…, Choose file…).
    fn row_field(
        &self,
        account: &'static str,
        controls: AnyElement,
        primary: impl Fn(&mut Window, &mut App) + 'static,
        cx: &App,
    ) -> AnyElement {
        let focus = self.focus_of(account);
        let marked = self.target == Some(account);
        let theme = cx.theme();
        let (ring, radius) = (theme.ring, theme.radius);
        h_flex()
            .id(SharedString::from(format!("account-{account}")))
            .test_support()
            .when_some(focus, |this, focus| {
                this.track_focus(&focus)
                    .on_key_down(move |event: &KeyDownEvent, window, cx| {
                        let keystroke = &event.keystroke;
                        // On the row itself: a button in it takes its own keys.
                        if focus.is_focused(window)
                            && !keystroke.modifiers.modified()
                            && matches!(keystroke.key.as_str(), "enter" | "space")
                        {
                            cx.stop_propagation();
                            primary(window, cx);
                        }
                    })
            })
            .rounded(radius)
            .p_0p5()
            .when(marked, |this| this.border_1().border_color(ring))
            .child(controls)
            .into_any_element()
    }

    /// Reads the accounts and the models again, in the background.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.openrouter.update(cx, |part, cx| part.read(cx));
        let backend = cx.global::<SharedBackend>().clone();
        let reading = cx.background_spawn(async move {
            let last = last_model(backend.data_root());
            let models = backend.models(None);
            (backend.accounts(), models, last)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (accounts, models, last) = reading.await;
            this.update(cx, |this, cx| {
                // A key, a login or Claude Code changed (here, or in the
                // terminal app): the composers list their models again.
                crate::backend::accounts_read(&accounts, cx);
                let last_used = match &last {
                    Some(model) => format!("Last used ({})", arko::models::display_name(model)),
                    None => "Last used".to_string(),
                };
                this.main_model.update(cx, |select, cx| {
                    select.set_models(last_used, Some(models.clone()), cx)
                });
                this.worker_model.update(cx, |select, cx| {
                    select.set_models("Automatic", Some(models.clone()), cx)
                });
                this.accounts = Some(accounts);
                this.models = Some(models);
                this.last_model = last;
                this.checking = false;
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn openrouter(&self) -> &Entity<OpenRouterPart> {
        &self.openrouter
    }

    pub fn accounts(&self) -> Option<&Accounts> {
        self.accounts.as_ref()
    }

    /// Checks Claude Code again (after installing it or signing in).
    pub fn check_again(&mut self, cx: &mut Context<Self>) {
        self.checking = true;
        cx.notify();
        self.refresh(cx);
    }

    /// Asks for a key for `provider` in a masked field, and saves it.
    pub fn ask_for_key(
        this: &Entity<Self>,
        provider: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = this
            .read(cx)
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.key(provider))
            .cloned()
        else {
            return;
        };
        let replacing = state.masked.is_some();
        let form = cx.new(|cx| KeyForm::new(this.clone(), state.clone(), window, cx));
        let field = form.read(cx).key.clone();
        let title = if replacing {
            format!("Replace the {} key", state.key.provider)
        } else {
            format!("Add a {} key", state.key.provider)
        };
        window.open_dialog(cx, move |dialog, _, _| {
            let form = form.clone();
            let saving = form.clone();
            dialog
                .w(px(460.))
                // Enter saves, as Save does: the dialog closes once the key
                // is saved, and stays open with why when it isn't.
                .on_ok(move |_, window, cx| {
                    saving.update(cx, |form, cx| form.save(window, cx));
                    false
                })
                .title(
                    h_flex()
                        .gap_2()
                        .child(gpui_kit::component::Icon::new(
                            gpui_kit::assets::IconName::KeyRound,
                        ))
                        .child(title.clone()),
                )
                .content(move |content, _, _| content.child(form.clone()))
        });
        // The dialog takes the focus as it opens: the field gets it back.
        field.update(cx, |field, cx| field.focus(window, cx));
    }

    /// Asks before removing the key Arko saved for `provider`, then removes
    /// it.
    pub fn confirm_remove(
        this: &Entity<Self>,
        provider: &'static str,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(state) = this
            .read(cx)
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.key(provider))
            .cloned()
        else {
            return;
        };
        let file = match &state.source {
            Some(KeySource::Arko(path)) => tilde(path, this.read(cx).home.as_deref()),
            _ => return,
        };
        let this = this.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .title(format!("Remove the {} key?", state.key.provider))
                .description(format!(
                    "Arko takes {} out of {file}. Sessions open now stop using it, and use another one if they find it in the environment or a .env file.",
                    state.variable
                ))
                .confirm()
                .ok_text("Remove")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    remove_key(this.clone(), provider, window, cx);
                    true
                })
        });
    }

    /// Asks for Codex's `auth.json` and adds it to the ChatGPT logins.
    pub fn choose_login(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use".into()),
        });
        let this = this.downgrade();
        window
            .spawn(cx, async move |cx| {
                let Ok(Ok(Some(paths))) = paths.await else {
                    return;
                };
                if let Some(path) = paths.into_iter().next() {
                    cx.update(|window, cx| {
                        if let Some(this) = this.upgrade() {
                            add_login(&this, path, window, cx);
                        }
                    })
                    .ok();
                }
            })
            .detach();
    }

    /// The page, as the accounts and the preferences are now. `revision`
    /// counts the page's changes, and its groups draw again only then (or,
    /// for New sessions, as the preferences it shows change).
    pub fn page(&self, this: &Entity<Self>, revision: u64, cx: &App) -> SettingPage {
        let chosen = &prefs::get(cx).new_sessions;
        let new_sessions = (
            revision,
            chosen.mode,
            &chosen.model,
            &chosen.effort,
            &chosen.worker_model,
            &chosen.worker_effort,
            chosen.cache_1h,
        );
        // What reaches the models comes first: a model that needs a key
        // opens the page there.
        SettingPage::new(super::PAGES[super::ACCOUNTS_PAGE])
            .icon(IconName::Bot)
            .groups([
                self.keys_group(this)
                    .key(super::group_key("API keys", &revision)),
                self.openrouter
                    .read(cx)
                    .group(
                        &self.openrouter,
                        self.focus_of(OPENROUTER_SETTINGS),
                        self.target == Some(OPENROUTER_SETTINGS),
                        cx,
                    )
                    .key(super::group_key("OpenRouter", &revision)),
                self.logins_group(this)
                    .key(super::group_key("Logins", &revision)),
                self.new_sessions_group(cx)
                    .key(super::group_key("New sessions", &new_sessions)),
            ])
    }

    fn new_sessions_group(&self, cx: &App) -> SettingGroup {
        let defaults = prefs::NewSessions::default();
        let chosen = &prefs::get(cx).new_sessions;
        let main = chosen
            .model
            .clone()
            .or_else(|| self.last_model.clone())
            .or_else(|| {
                self.models
                    .as_ref()
                    .and_then(|models| models.default.clone())
            });
        let workers = chosen.worker_model.clone().or_else(|| main.clone());
        let cache_from_env = self
            .accounts
            .as_ref()
            .and_then(|accounts| accounts.cache_1h)
            .unwrap_or(false);
        SettingGroup::new()
            .title("New sessions")
            .description(
                "What a new session starts on. Before its first message, you can pick another model in the message box.",
            )
            .items([
                super::choice(
                    "Mode",
                    &[
                        (NewSessionMode::Last, "Last used"),
                        (NewSessionMode::Agent, "Agent"),
                        (NewSessionMode::Chat, "Chat"),
                    ],
                    |prefs| prefs.new_sessions.mode,
                    |prefs, mode| prefs.new_sessions.mode = mode,
                    defaults.mode,
                )
                .description(
                    "Agent works in a folder with tools and workers. Chat has no tools. Last used starts as the last session did.",
                )
                .keywords(["default mode", "chat", "agent", "last used"]),
                SettingItem::new("Model", {
                    let select = self.main_model.clone();
                    SettingField::render(move |_, _, _| select.clone()).on_reset(
                        |cx| prefs::get(cx).new_sessions.model.is_some(),
                        |_, cx| prefs::update(cx, |prefs| prefs.new_sessions.model = None),
                    )
                })
                .description(self.model_note(chosen.model.as_deref()).unwrap_or_else(|| {
                    "The main agent’s. Last used is the last session’s model, else one your credentials reach.".into()
                }))
                .keywords(["default model", "main agent"]),
                SettingItem::new(
                    "Effort",
                    SettingField::dropdown(
                        effort_options("Default", main.as_deref(), chosen.effort.as_deref()),
                        |cx: &App| {
                            prefs::get(cx)
                                .new_sessions
                                .effort
                                .clone()
                                .unwrap_or_default()
                                .into()
                        },
                        |value: SharedString, cx: &mut App| {
                            prefs::update(cx, |prefs| {
                                prefs.new_sessions.effort =
                                    (!value.is_empty()).then(|| value.to_string())
                            })
                        },
                    )
                    .default_value(""),
                )
                .description("How much the main agent’s model reasons. Default is xhigh, or the nearest its model takes.")
                .keywords(["reasoning", "thinking"]),
                SettingItem::new("Default workers’ model", {
                    let select = self.worker_model.clone();
                    SettingField::render(move |_, _, _| select.clone()).on_reset(
                        |cx| prefs::get(cx).new_sessions.worker_model.is_some(),
                        |_, cx| {
                            prefs::update(cx, |prefs| prefs.new_sessions.worker_model = None)
                        },
                    )
                })
                .description(
                    self.model_note(chosen.worker_model.as_deref()).unwrap_or_else(|| {
                        "The model workers use when the main agent doesn’t choose one. A profile or the agent’s own choice wins. Automatic reuses the last session’s workers’ model when Model is Last used. Otherwise, workers use the main model.".into()
                    }),
                )
                .keywords(["workers", "worker model", "default"]),
                SettingItem::new(
                    "Workers’ effort",
                    SettingField::dropdown(
                        effort_options(
                            "Same as the main agent",
                            workers.as_deref(),
                            chosen.worker_effort.as_deref(),
                        ),
                        |cx: &App| {
                            prefs::get(cx)
                                .new_sessions
                                .worker_effort
                                .clone()
                                .unwrap_or_default()
                                .into()
                        },
                        |value: SharedString, cx: &mut App| {
                            prefs::update(cx, |prefs| {
                                prefs.new_sessions.worker_effort =
                                    (!value.is_empty()).then(|| value.to_string())
                            })
                        },
                    )
                    .default_value(""),
                )
                .keywords(["workers", "reasoning"]),
                SettingItem::new(
                    "Long prompt cache",
                    SettingField::switch(
                        move |cx: &App| {
                            prefs::get(cx)
                                .new_sessions
                                .cache_1h
                                .unwrap_or(cache_from_env)
                        },
                        |on: bool, cx: &mut App| {
                            prefs::update(cx, |prefs| prefs.new_sessions.cache_1h = Some(on))
                        },
                    )
                    .on_reset(
                        |cx| prefs::get(cx).new_sessions.cache_1h.is_some(),
                        |_, cx| prefs::update(cx, |prefs| prefs.new_sessions.cache_1h = None),
                    ),
                )
                .description(format!(
                    "Claude models keep their prompt cache an hour instead of 5 minutes, and OpenAI models with an API key 12 hours instead of 30. Other models keep their own. A long cache costs 2× the input price to write on Claude and 2.5× on OpenAI, instead of 1.25×. Until it’s set here, {CACHE_1H_VARIABLE} decides."
                ))
                .keywords([
                    "prompt cache",
                    "long cache",
                    "1 hour",
                    "12 hours",
                    "anthropic",
                    "openai",
                    CACHE_1H_VARIABLE,
                ]),
            ])
    }

    /// What the model `chosen` needs that isn't there, as a description.
    fn model_note(&self, chosen: Option<&str>) -> Option<String> {
        let chosen = chosen?;
        let group = self
            .models
            .as_ref()?
            .groups
            .iter()
            .find(|group| group.models.iter().any(|model| model.name == chosen))?;
        let needs = group.needs.as_ref()?;
        Some(format!(
            "{} needs {needs}. Sessions ask for it as they start, or add it above.",
            arko::models::display_name(chosen)
        ))
    }

    fn keys_group(&self, this: &Entity<Self>) -> SettingGroup {
        let rows: Vec<SettingItem> = match &self.accounts {
            None => arko::accounts::KEYS
                .iter()
                .map(|key| {
                    SettingItem::new(key.provider, SettingField::render(|_, _, _| div()))
                        .description("Checking…")
                        .keywords(key_keywords(key))
                })
                .collect(),
            Some(accounts) => accounts
                .keys
                .iter()
                .map(|state| self.key_row(this, state))
                .collect(),
        };
        SettingGroup::new()
            .title("API keys")
            .description(
                "Arko looks in the environment, then a .env file in the session’s folder or a parent, one beside the program, and Arko’s folder. Keys added here are saved in Arko’s folder, readable only by you. A key in a project’s .env reaches only that project’s sessions, so it isn’t shown here.",
            )
            .items(rows)
    }

    fn key_row(&self, this: &Entity<Self>, state: &KeyState) -> SettingItem {
        let key = state.key;
        let provider = key.id;
        let description = match (&state.masked, &state.source) {
            (Some(masked), Some(source)) => {
                let variable = if state.variable != key.name {
                    format!(" as {}", state.variable)
                } else {
                    String::new()
                };
                format!(
                    "{masked}, {}{variable}.",
                    source_words(source, self.home.as_deref())
                )
            }
            _ => format!("{} Get one at {}.", key.purpose, key.from),
        };
        let set = state.masked.is_some();
        let removable = matches!(state.source, Some(KeySource::Arko(_)));
        let (add, remove, page) = (this.clone(), this.clone(), this.clone());
        SettingItem::new(
            key.provider,
            SettingField::render(move |_, _, cx| {
                let (add, remove) = (add.clone(), remove.clone());
                let asking = add.clone();
                let primary = move |window: &mut Window, cx: &mut App| {
                    AccountsPage::ask_for_key(&asking, provider, window, cx)
                };
                let controls = h_flex()
                    .gap_1()
                    .when(removable, |this| {
                        this.child(
                            Button::new(SharedString::from(format!("remove-{provider}")))
                                .ghost()
                                .small()
                                .label("Remove")
                                .on_click(move |_, window, cx| {
                                    AccountsPage::confirm_remove(&remove, provider, window, cx)
                                }),
                        )
                    })
                    .child(
                        Button::new(SharedString::from(format!("key-{provider}")))
                            .outline()
                            .small()
                            .label(if set { "Replace…" } else { "Add…" })
                            .on_click(move |_, window, cx| {
                                AccountsPage::ask_for_key(&add, provider, window, cx)
                            }),
                    )
                    .into_any_element();
                page.read(cx).row_field(provider, controls, primary, cx)
            }),
        )
        .description(description)
        .keywords(key_keywords(key))
    }

    fn logins_group(&self, this: &Entity<Self>) -> SettingGroup {
        let home = self.home.as_deref();
        let logins: &[ChatGptLogin] = self
            .accounts
            .as_ref()
            .map_or(&[], |accounts| accounts.chatgpt.as_slice());
        let mut items: Vec<SettingItem> = if self.accounts.is_none() {
            vec![
                SettingItem::new("ChatGPT login", SettingField::render(|_, _, _| div()))
                    .description("Checking…"),
            ]
        } else if logins.is_empty() {
            vec![
                SettingItem::new("ChatGPT login", SettingField::render(|_, _, _| div()))
                    .description(
                        "There is no ChatGPT login. Sign in with “codex login”, or add the auth.json Codex keeps it in.",
                    ),
            ]
        } else {
            logins
                .iter()
                .enumerate()
                .map(|(ix, login)| self.login_item(this, ix, logins.len(), login))
                .collect()
        };
        let choose = this.clone();
        items.push(
            SettingItem::new(
                "Add a ChatGPT login",
                SettingField::render(move |_, _, _| {
                    let choose = choose.clone();
                    Button::new("choose-login")
                        .outline()
                        .small()
                        .label("Add…")
                        .on_click(move |_, window, cx| {
                            AccountsPage::choose_login(&choose, window, cx)
                        })
                }),
            )
            .description(
                "Choose the auth.json Codex keeps another ChatGPT account in. Arko uses it when the logins before it reach their limit.",
            )
            .keywords(["openai", "codex", "auth.json", "plan", "chatgpt", "sign in"]),
        );
        let claude = match &self.accounts {
            None => "Checking…".to_string(),
            Some(accounts) => claude_code_words(&accounts.claude_code, home),
        };
        let check = this.clone();
        let claude_page = this.clone();
        let checking = self.checking || self.accounts.is_none();
        items.push(
            SettingItem::new(
                "Claude Code",
                SettingField::render(move |_, _, cx| {
                    let check = check.clone();
                    let checking_again = check.clone();
                    let button = Button::new("check-claude-code")
                        .outline()
                        .small()
                        .label("Check again")
                        .loading(checking)
                        .on_click(move |_, _, cx| {
                            check.update(cx, |page, cx| page.check_again(cx));
                        })
                        .into_any_element();
                    claude_page.read(cx).row_field(
                        CLAUDE_CODE,
                        button,
                        move |_, cx| checking_again.update(cx, |page, cx| page.check_again(cx)),
                        cx,
                    )
                }),
            )
            .description(claude)
            .keywords(["cc", "claude plan", "install", "sign in", "anthropic"]),
        );
        let group = SettingGroup::new().title("Logins");
        let group = if logins.len() > 1 {
            group.description(
                "Arko uses the ChatGPT logins in order. The next one takes over when the one before it reaches its limit.",
            )
        } else {
            group
        };
        group.items(items)
    }

    /// The row of the ChatGPT login at `ix` of `count`, with Make first for
    /// the ones after the first, and Remove for the ones chosen here or
    /// in `ARKO_CODEX_AUTH` (Codex's own can't be removed).
    fn login_item(
        &self,
        this: &Entity<Self>,
        ix: usize,
        count: usize,
        login: &ChatGptLogin,
    ) -> SettingItem {
        let page = this.clone();
        let path = login.path.clone();
        let home = self.home.clone();
        let removable = login.source != LoginSource::Default;
        let title = if count == 1 {
            "ChatGPT login".to_string()
        } else {
            format!("ChatGPT login {}", ix + 1)
        };
        SettingItem::new(
            title,
            SettingField::render(move |_, _, cx| {
                let buttons = path.iter().fold(h_flex().gap_1(), |row, path| {
                    let (first, remove) = (page.clone(), page.clone());
                    let (first_path, remove_path) = (path.clone(), path.clone());
                    // The buttons say which login, as the row shows its path.
                    let shown = tilde(path, home.as_deref());
                    row.when(ix > 0, |row| {
                        row.child(
                            Button::new(("first-login", ix))
                                .ghost()
                                .small()
                                .label("Make first")
                                .accessibility_label(format!("Make {shown} first"))
                                .on_click(move |_, window, cx| {
                                    first_login(&first, first_path.clone(), window, cx)
                                }),
                        )
                    })
                    .when(removable, |row| {
                        row.child(
                            Button::new(("remove-login", ix))
                                .ghost()
                                .small()
                                .label("Remove")
                                .accessibility_label(format!("Remove {shown}"))
                                .on_click(move |_, window, cx| {
                                    remove_login(&remove, remove_path.clone(), window, cx)
                                }),
                        )
                    })
                });
                let adding = page.clone();
                let primary = move |window: &mut Window, cx: &mut App| {
                    AccountsPage::choose_login(&adding, window, cx)
                };
                // The first row is the one the page points at when a model
                // needs the login.
                if ix == 0 {
                    page.read(cx)
                        .row_field(CHATGPT, buttons.into_any_element(), primary, cx)
                } else {
                    buttons.into_any_element()
                }
            }),
        )
        .description(login_words(login, ix, count, self.home.as_deref()))
        .keywords(["openai", "codex", "auth.json", "plan", "chatgpt", "sign in"])
    }
}

/// What a ChatGPT login's row says: where it is, whose it is when it is the
/// only one, and whether it works, then where it stands among the others.
fn login_words(login: &ChatGptLogin, ix: usize, count: usize, home: Option<&Path>) -> String {
    let path = login
        .path
        .as_deref()
        .map(|path| tilde(path, home))
        .unwrap_or_else(|| "Codex’s auth.json".into());
    let whose = match login.source {
        LoginSource::Default => "Codex’s own",
        LoginSource::Setting => "from ARKO_CODEX_AUTH",
        LoginSource::Entered | LoginSource::Flag => "chosen",
    };
    match (&login.state, count) {
        (LoginState::Ready, 1) => {
            format!("Signed in with {path} ({whose}). OpenAI models run on your ChatGPT plan.")
        }
        (LoginState::Ready, _) if ix == 0 => format!("Signed in with {path}. Arko uses it first."),
        (LoginState::Ready, _) => format!("Signed in with {path}."),
        (LoginState::Missing, 1) => format!(
            "No login at {path}. Sign in with “codex login”, or add the auth.json Codex keeps it in."
        ),
        (LoginState::Missing, _) => {
            format!("No login at {path}. Sign in with “codex login” again, or remove it.")
        }
        (LoginState::Invalid(why), 1) => format!("{path} ({whose}) can’t be used ({why})."),
        (LoginState::Invalid(why), _) => format!("{path} can’t be used ({why})."),
    }
}

/// What a key's row is found by.
fn key_keywords(key: &arko::accounts::Key) -> Vec<SharedString> {
    vec![
        key.id.into(),
        key.name.into(),
        "api key".into(),
        "credentials".into(),
    ]
}

/// The efforts to choose from for `model`, `first` standing for the
/// default, with `chosen` kept even when the model doesn't take it.
pub(super) fn effort_options(
    first: &str,
    model: Option<&str>,
    chosen: Option<&str>,
) -> Vec<(SharedString, SharedString)> {
    let mut efforts: Vec<String> = match model {
        Some(model) => {
            arko::commands::dynamic_choices(&arko::remote::Reading::default(), "/model", &[model])
        }
        None => ["low", "medium", "high", "xhigh", "max"]
            .map(String::from)
            .to_vec(),
    };
    if let Some(chosen) = chosen
        && !efforts.iter().any(|effort| effort == chosen)
    {
        efforts.push(chosen.to_string());
    }
    std::iter::once((
        SharedString::default(),
        SharedString::from(first.to_string()),
    ))
    .chain(
        efforts
            .into_iter()
            .map(|effort| (effort.clone().into(), effort.into())),
    )
    .collect()
}

/// Where Claude Code is and whether it's signed in, or how to install it
/// and sign in.
pub fn claude_code_words(status: &ClaudeCodeStatus, home: Option<&std::path::Path>) -> String {
    let Some(path) = &status.path else {
        return "Not installed. Install it with “npm install -g @anthropic-ai/claude-code” or Anthropic’s installer, sign in with “claude auth login”, then check again. Arko reaches Claude models on your Claude plan through it.".into();
    };
    // `claude --version` says "2.1.42 (Claude Code)": the number is enough.
    let version = status
        .version
        .as_deref()
        .map(|version| version.trim().trim_end_matches("(Claude Code)").trim())
        .filter(|version| !version.is_empty());
    let found = match version {
        Some(version) => format!("{} ({version})", tilde(path, home)),
        None => tilde(path, home),
    };
    match &status.ready {
        Ok(ready) => {
            let how = match (&ready.method, &ready.plan) {
                (Some(method), Some(plan)) if method == "claude.ai" => {
                    format!("signed in to your Claude {} plan", capitalized(plan))
                }
                (Some(method), _) => format!("signed in with {method}"),
                (None, _) => "signed in".to_string(),
            };
            let mut text = format!("{found}, {how}.");
            if let Some(warning) = ready.warning() {
                text.push(' ');
                text.push_str(&warning);
            }
            text
        }
        Err(Problem::SignedOut(_)) => format!(
            "{found}, not signed in. Run “claude auth login” in a terminal, then check again."
        ),
        Err(Problem::Missing) => {
            format!("{found} doesn’t run. Install it again, then check again.")
        }
        // Arko's sentence names the version, so the path adds nothing.
        Err(Problem::Outdated { version }) => outdated_text(version),
    }
}

/// `word` with a capital first letter: a plan's name ("max" is "Max").
fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn remove_key(
    this: Entity<AccountsPage>,
    provider: &'static str,
    window: &mut Window,
    cx: &mut App,
) {
    let backend = cx.global::<SharedBackend>().clone();
    let removing = cx.background_spawn(async move { backend.remove_key(provider) });
    window
        .spawn(cx, async move |cx| {
            let removed = removing.await;
            cx.update(|window, cx| {
                match removed {
                    Ok(said) => window.push_notification(said, cx),
                    Err(why) => window.push_notification(Notification::error(why), cx),
                }
                this.update(cx, |page, cx| page.refresh(cx));
            })
            .ok();
        })
        .detach();
}

/// Adds the Codex auth file at `path` to the ChatGPT logins, saying how it
/// went.
pub fn add_login(this: &Entity<AccountsPage>, path: PathBuf, window: &mut Window, cx: &mut App) {
    change_logins(
        this,
        move |backend| backend.add_chatgpt_login(&path),
        window,
        cx,
    );
}

/// Takes the login at `path` out of the ChatGPT logins, saying how it went.
fn remove_login(this: &Entity<AccountsPage>, path: PathBuf, window: &mut Window, cx: &mut App) {
    change_logins(
        this,
        move |backend| backend.remove_chatgpt_login(&path),
        window,
        cx,
    );
}

/// Puts the login at `path` first among the ChatGPT logins, saying how it
/// went.
fn first_login(this: &Entity<AccountsPage>, path: PathBuf, window: &mut Window, cx: &mut App) {
    change_logins(
        this,
        move |backend| backend.first_chatgpt_login(&path),
        window,
        cx,
    );
}

/// Changes the ChatGPT logins with `change`, off the UI thread, says how it
/// went, and reads the account rows again.
fn change_logins(
    this: &Entity<AccountsPage>,
    change: impl FnOnce(&SharedBackend) -> Result<String, String> + Send + 'static,
    window: &mut Window,
    cx: &mut App,
) {
    let backend = cx.global::<SharedBackend>().clone();
    let changing = cx.background_spawn(async move { change(&backend) });
    let this = this.clone();
    window
        .spawn(cx, async move |cx| {
            let changed = changing.await;
            cx.update(|window, cx| {
                match changed {
                    Ok(said) => window.push_notification(said, cx),
                    Err(why) => window.push_notification(Notification::error(curly(&why)), cx),
                }
                this.update(cx, |page, cx| page.refresh(cx));
            })
            .ok();
        })
        .detach();
}

/// The dialog's form for a key: a masked field, what's wrong with what was
/// pasted, and Save.
pub struct KeyForm {
    page: Entity<AccountsPage>,
    state: KeyState,
    key: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
}

impl KeyForm {
    fn new(
        page: Entity<AccountsPage>,
        state: KeyState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prefix = state.key.prefix;
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder(format!("{prefix}…"))
        });
        Self {
            page,
            state,
            key,
            error: None,
            busy: false,
        }
    }

    fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Saving already (Enter pressed again).
        if self.busy {
            return;
        }
        let value = self.key.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.error = Some("Paste the key first.".into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let backend = cx.global::<SharedBackend>().clone();
        let provider = self.state.key.id;
        let saving = cx.background_spawn(async move { backend.add_key(provider, &value) });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saving.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match saved {
                    Ok(said) => {
                        window.close_dialog(cx);
                        window.push_notification(said, cx);
                        this.page.update(cx, |page, cx| page.refresh(cx));
                    }
                    Err(why) => {
                        this.error = Some(why.into());
                        this.key.update(cx, |key, cx| key.focus(window, cx));
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }
}

/// What a key saved in Arko's folder does to the key in use, `state`, if
/// there is one. A key from the environment or another `.env` file comes
/// before Arko's folder's when Arko starts, so the new one wins only until
/// the app quits.
pub fn replacing_words(state: &KeyState, home: Option<&std::path::Path>) -> Option<String> {
    match (&state.masked, &state.source) {
        (Some(masked), Some(KeySource::Arko(_))) => {
            Some(format!("It replaces {masked} in Arko’s folder."))
        }
        (Some(masked), Some(source)) => Some(format!(
            "Arko uses it in place of {masked} until Arko Studio quits. After that, {} {} comes first again, so change it there to replace it for good.",
            state.variable,
            source_words(source, home)
        )),
        _ => None,
    }
}

impl Render for KeyForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let replacing = replacing_words(&self.state, self.page.read(cx).home.as_deref());
        let theme = cx.theme();
        let key = self.state.key;
        v_flex()
            .id("key-form")
            .test_support()
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{} Get one at {}. It’s saved in Arko’s folder, readable only by you, and open sessions use it at once.",
                        key.purpose, key.from
                    )),
            )
            .when_some(replacing, |this, text| {
                this.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(text),
                )
            })
            .child(Input::new(&self.key).small())
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("key-error")
                        .test_support()
                        .text_sm()
                        .text_color(theme.danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("key-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    )
                    .child(
                        Button::new("key-save")
                            .primary()
                            .small()
                            .label("Save")
                            .loading(self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{claude_code_words, effort_options, login_words, replacing_words};
    use crate::backend::account_words::{
        ChatGptLogin, ClaudeCodeStatus, LoginSource, LoginState, Problem, Ready,
    };
    use std::path::{Path, PathBuf};

    fn status(path: Option<&str>, ready: Result<Ready, Problem>) -> ClaudeCodeStatus {
        ClaudeCodeStatus {
            path: path.map(PathBuf::from),
            on_path: true,
            version: Some("2.1.42 (Claude Code)".into()),
            ready,
        }
    }

    #[test]
    fn claude_code_says_where_it_is_and_how_it_signs_in() {
        let home = Some(Path::new("/home/ada"));
        let path = "/home/ada/.local/bin/claude";
        let ready = Ready {
            path: path.into(),
            method: Some("claude.ai".into()),
            plan: Some("max".into()),
        };
        assert_eq!(
            claude_code_words(&status(Some(path), Ok(ready)), home),
            "~/.local/bin/claude (2.1.42), signed in to your Claude Max plan."
        );
        let words = claude_code_words(
            &status(Some(path), Err(Problem::SignedOut(path.into()))),
            home,
        );
        assert!(
            words.ends_with(
                "not signed in. Run “claude auth login” in a terminal, then check again."
            ),
            "{words}"
        );
        let words = claude_code_words(&status(None, Err(Problem::Missing)), home);
        assert!(words.starts_with("Not installed."), "{words}");
        // A Claude Code too old says so in Arko's words.
        let outdated = Problem::Outdated {
            version: "2.1.296".into(),
        };
        let words = claude_code_words(&status(Some(path), Err(outdated)), home);
        assert!(
            words.starts_with("Claude Code 2.1.296 has a security flaw")
                && words.contains("claude update"),
            "{words}"
        );
        let api = Ready {
            path: path.into(),
            method: Some("apiKey".into()),
            plan: None,
        };
        let words = claude_code_words(&status(Some(path), Ok(api)), home);
        assert!(words.contains("may be billed per token"), "{words}");
    }

    /// A ChatGPT login's row says whose it is when it is alone, and where it
    /// stands when it is one of several.
    #[test]
    fn a_chatgpt_logins_row_says_where_it_stands() {
        let home = Some(Path::new("/home/ada"));
        let login = |path: &str, source, state| ChatGptLogin {
            path: Some(PathBuf::from(path)),
            source,
            state,
        };
        let own = login(
            "/home/ada/.codex/auth.json",
            LoginSource::Default,
            LoginState::Ready,
        );
        assert_eq!(
            login_words(&own, 0, 1, home),
            "Signed in with ~/.codex/auth.json (Codex’s own). OpenAI models run on your ChatGPT plan."
        );
        let second = login(
            "/home/ada/.codex-2/auth.json",
            LoginSource::Entered,
            LoginState::Ready,
        );
        assert_eq!(
            login_words(&own, 0, 2, home),
            "Signed in with ~/.codex/auth.json. Arko uses it first."
        );
        assert_eq!(
            login_words(&second, 1, 2, home),
            "Signed in with ~/.codex-2/auth.json."
        );
        let lost = login(
            "/home/ada/.codex-2/auth.json",
            LoginSource::Entered,
            LoginState::Missing,
        );
        assert_eq!(
            login_words(&lost, 1, 2, home),
            "No login at ~/.codex-2/auth.json. Sign in with “codex login” again, or remove it."
        );
        assert!(
            login_words(&lost, 0, 1, home).ends_with("or add the auth.json Codex keeps it in.")
        );
    }

    /// A key saved in Arko's folder replaces one saved there for good, and
    /// one from the environment or another `.env` only until the app quits.
    #[test]
    fn a_replaced_key_says_for_how_long() {
        let home = Path::new("/home/ada");
        let sample = crate::backend::Accounts::sample(home, Path::new("/home/ada/.local/arko"));
        let words = |id: &str| replacing_words(sample.key(id).unwrap(), Some(home));
        assert_eq!(
            words("anthropic").as_deref(),
            Some("It replaces sk-ant-…3f9a in Arko’s folder.")
        );
        assert_eq!(
            words("openai").as_deref(),
            Some(
                "Arko uses it in place of sk-…b20c until Arko Studio quits. After that, OPENAI_API_KEY from the environment comes first again, so change it there to replace it for good."
            )
        );
        assert!(
            words("gemini")
                .unwrap()
                .contains("GEMINI_API_KEY from /opt/arko-studio/.env comes first again")
        );
        assert_eq!(words("voyage"), None);
    }

    #[test]
    fn efforts_are_the_models_own() {
        let options = effort_options("Default", Some("gemini"), None);
        let values: Vec<&str> = options.iter().map(|(value, _)| value.as_ref()).collect();
        assert_eq!(values, ["", "low", "medium", "high"]);
        // A chosen effort the model doesn't take still shows.
        let options = effort_options("Default", Some("gemini"), Some("max"));
        assert_eq!(options.last().unwrap().0.as_ref(), "max");
        assert_eq!(effort_options("Default", None, None).len(), 6);
    }
}
