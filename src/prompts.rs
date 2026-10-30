//! What a session asks of the user, as dialogs: the credentials it can't
//! open without (a key entered in a masked field, the ChatGPT login file,
//! Claude Code installed and signed in), whether to start a project's MCP
//! servers, and whether to stop running jobs and workers to close it.
//!
//! A dialog closed without an answer leaves the question waiting: the
//! session view shows a strip to come back to it, so the user can look
//! something up (or open Settings) first.

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::menu::DropdownMenu as _;
use gpui_kit::component::{ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::SharedBackend;
use crate::live::{LiveSession, McpRequest, PromptAnswer, PromptKind};
use crate::prefs;
use crate::workers;

/// The provider keys a session can ask for, as arko names them.
const KEY_PROVIDERS: [(&str, &str); 4] = [
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("gemini", "Gemini"),
    ("openrouter", "OpenRouter"),
];

/// What the session needs, in a line: the strip's text and the dialog's
/// title.
pub fn need_title(need: &arko::Need) -> String {
    match need {
        arko::Need::Any => "Arko needs a way to reach a model.".to_string(),
        arko::Need::AnthropicKey { model } => {
            format!("{} needs an Anthropic API key.", shown(model))
        }
        arko::Need::OpenAiKey { model } => format!("{} needs an OpenAI API key.", shown(model)),
        arko::Need::GeminiKey { model } => format!("{} needs a Gemini API key.", shown(model)),
        arko::Need::OpenRouterKey { model } => {
            format!("{} needs an OpenRouter API key.", shown(model))
        }
        arko::Need::ChatGptLogin { model, .. } => {
            format!("{} needs your ChatGPT login.", shown(model))
        }
        arko::Need::ClaudeCode { model, .. } => format!("{} runs in Claude Code.", shown(model)),
    }
}

fn shown(model: &str) -> String {
    arko::models::display_name(model)
}

/// The provider whose key `need` asks for, `None` for the others.
pub fn key_provider(need: &arko::Need) -> Option<&'static str> {
    match need {
        arko::Need::AnthropicKey { .. } => Some("anthropic"),
        arko::Need::OpenAiKey { .. } => Some("openai"),
        arko::Need::GeminiKey { .. } => Some("gemini"),
        arko::Need::OpenRouterKey { .. } => Some("openrouter"),
        _ => None,
    }
}

/// Shows the dialog for the prompt `session` waits on, if any.
pub fn show(session: &Entity<LiveSession>, window: &mut Window, cx: &mut App) {
    if window.has_active_dialog(cx) {
        return;
    }
    match session.read(cx).prompt() {
        Some(PromptKind::Credentials(need)) => {
            let need = need.clone();
            show_credentials(session.clone(), need, window, cx)
        }
        Some(PromptKind::Mcp(request)) => {
            let request = request.clone();
            show_mcp(session.clone(), request, window, cx)
        }
        None => {}
    }
}

/// The form in the credentials dialog.
pub struct CredentialsForm {
    session: Entity<LiveSession>,
    need: arko::Need,
    /// The provider whose key is entered.
    provider: &'static str,
    key: Entity<InputState>,
    error: Option<SharedString>,
    busy: bool,
}

impl CredentialsForm {
    fn new(
        session: Entity<LiveSession>,
        need: arko::Need,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let key = cx.new(|cx| {
            InputState::new(window, cx)
                .masked(true)
                .placeholder("Paste the key")
        });
        key.update(cx, |key, cx| key.focus(window, cx));
        Self {
            provider: key_provider(&need).unwrap_or("anthropic"),
            session,
            need,
            key,
            error: None,
            busy: false,
        }
    }

    fn answer(&mut self, answer: PromptAnswer, window: &mut Window, cx: &mut Context<Self>) {
        self.session
            .update(cx, |session, cx| session.answer_prompt(answer, cx));
        window.close_dialog(cx);
    }

    /// Saves the key in Arko's folder, then lets the session check again.
    fn save_key(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.key.read(cx).value().trim().to_string();
        if value.is_empty() {
            self.error = Some("Paste the key first.".into());
            self.key.update(cx, |key, cx| key.focus(window, cx));
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let backend = cx.global::<SharedBackend>().clone();
        let provider = self.provider;
        let saving = cx.background_spawn(async move { backend.add_key(provider, &value) });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saving.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match saved {
                    Ok(_) => this.answer(PromptAnswer::Again, window, cx),
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

    /// Asks for Codex's `auth.json`, then lets the session check again.
    fn choose_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use".into()),
        });
        let backend = cx.global::<SharedBackend>().clone();
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let chosen = cx
                .background_spawn(async move { backend.set_chatgpt_login(&path) })
                .await;
            this.update_in(cx, |this, window, cx| match chosen {
                Ok(_) => this.answer(PromptAnswer::Again, window, cx),
                Err(why) => {
                    this.error = Some(why.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn description(&self) -> String {
        match &self.need {
            arko::Need::Any => "Add an API key, or sign in to ChatGPT (`codex login`) or Claude Code, then check again. Keys are saved in Arko’s folder, readable only by you.".to_string(),
            arko::Need::ChatGptLogin { problem, .. } => format!(
                "Arko couldn’t use your ChatGPT login ({problem}). Sign in with `codex login`, or choose Codex’s auth.json file."
            ),
            arko::Need::ClaudeCode {
                problem: arko::accounts::Problem::Missing,
                ..
            } => "Install Claude Code (see claude.com/claude-code), sign in, then check again.".to_string(),
            arko::Need::ClaudeCode {
                problem: arko::accounts::Problem::SignedOut(path),
                ..
            } => format!(
                "Claude Code ({}) isn’t signed in. Run `claude auth login` in a terminal, then check again.",
                path.display()
            ),
            arko::Need::ClaudeCode {
                problem: arko::accounts::Problem::Outdated { version },
                ..
            } => arko::accounts::outdated_text(version),
            _ => "The key is saved in Arko’s folder, readable only by you, and the session uses it at once.".to_string(),
        }
    }

    fn takes_key(&self) -> bool {
        matches!(self.need, arko::Need::Any) || key_provider(&self.need).is_some()
    }
}

impl Render for CredentialsForm {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let takes_key = self.takes_key();
        let chatgpt = matches!(self.need, arko::Need::ChatGptLogin { .. });
        v_flex()
            .id("credentials-form")
            .test_support()
            // Enter in the key's field saves it, as the button does (the
            // dialog would close unanswered).
            .when(takes_key, |this| {
                this.on_action(cx.listener(
                    |this, _: &gpui_kit::base::actions::Confirm, window, cx| {
                        this.save_key(window, cx)
                    },
                ))
            })
            .gap_3()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(self.description()),
            )
            .when(matches!(self.need, arko::Need::Any), |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .children(KEY_PROVIDERS.iter().map(|(id, name)| {
                            let id: &'static str = id;
                            Button::new(SharedString::from(format!("provider-{id}")))
                                .xsmall()
                                .label(*name)
                                .when(self.provider == id, |this| this.primary())
                                .when(self.provider != id, |this| this.ghost())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.provider = id;
                                    cx.notify();
                                }))
                        })),
                )
            })
            .when(takes_key, |this| this.child(Input::new(&self.key).small()))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .id("credentials-error")
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
                        Button::new("credentials-settings")
                            .ghost()
                            .small()
                            .label("Open Settings")
                            .tooltip("Settings › Models & accounts. The session waits.")
                            .on_click(|_, window, cx| {
                                window.close_dialog(cx);
                                window.dispatch_action(Box::new(crate::menus::OpenAccounts), cx);
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("credentials-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.answer(PromptAnswer::Cancel, window, cx)
                            })),
                    )
                    .when(chatgpt, |this| {
                        this.child(
                            Button::new("credentials-login")
                                .outline()
                                .small()
                                .label("Choose auth.json…")
                                .on_click(
                                    cx.listener(|this, _, window, cx| {
                                        this.choose_login(window, cx)
                                    }),
                                ),
                        )
                    })
                    .map(|this| {
                        if takes_key {
                            this.child(
                                Button::new("credentials-save")
                                    .primary()
                                    .small()
                                    .label("Save and continue")
                                    .loading(self.busy)
                                    .on_click(
                                        cx.listener(|this, _, window, cx| {
                                            this.save_key(window, cx)
                                        }),
                                    ),
                            )
                        } else {
                            this.child(
                                Button::new("credentials-again")
                                    .primary()
                                    .small()
                                    .label("Check again")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.answer(PromptAnswer::Again, window, cx)
                                    })),
                            )
                        }
                    }),
            )
    }
}

fn show_credentials(
    session: Entity<LiveSession>,
    need: arko::Need,
    window: &mut Window,
    cx: &mut App,
) {
    let title = need_title(&need);
    let form = cx.new(|cx| CredentialsForm::new(session, need, window, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let form = form.clone();
        dialog
            .w(px(460.))
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
}

fn show_mcp(session: Entity<LiveSession>, request: McpRequest, window: &mut Window, cx: &mut App) {
    window.open_dialog(cx, move |dialog, _, cx| {
        let theme = cx.theme();
        let request = request.clone();
        let (start, refuse) = (session.clone(), session.clone());
        dialog
            .w(px(520.))
            .title(
                h_flex()
                    .gap_2()
                    .child(gpui_kit::component::Icon::new(gpui_kit::assets::IconName::Plug))
                    .child("Start this project’s MCP servers?"),
            )
            .content({
                let muted = theme.muted_foreground;
                let mono = theme.mono_font_family.clone();
                let mono_features = theme.mono_font_features.clone();
                move |content, _, _| {
                    let request = request.clone();
                    content.child(
                        v_flex()
                            .id("mcp-approval")
                            .test_support()
                            .gap_2()
                            .text_sm()
                            .child(div().text_color(muted).child(format!(
                                "{} asks to run these programs on your computer. Arko remembers your answer until the file changes.",
                                request.file.display()
                            )))
                            .children(request.servers.iter().map(|(name, command)| {
                                v_flex()
                                    .gap_0p5()
                                    .child(div().font_medium().child(name.clone()))
                                    .child(
                                        div()
                                            .font_family(mono.clone())
                                            .font_features(mono_features.clone())
                                            .text_xs()
                                            .text_color(muted)
                                            .child(command.clone()),
                                    )
                            }))
                            .when(!request.skipped.is_empty(), |this| {
                                this.child(div().text_xs().text_color(muted).child(format!(
                                    "Left out {}",
                                    request.skipped.join(", ")
                                )))
                            }),
                    )
                }
            })
            .footer(
                h_flex()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("mcp-refuse")
                            .ghost()
                            .small()
                            .label("Don’t start")
                            .on_click(move |_, window, cx| {
                                refuse.update(cx, |session, cx| {
                                    session.answer_prompt(PromptAnswer::Mcp(Some(false)), cx)
                                });
                                window.close_dialog(cx);
                            }),
                    )
                    .child(
                        Button::new("mcp-start")
                            .primary()
                            .small()
                            .label("Start")
                            .on_click(move |_, window, cx| {
                                start.update(cx, |session, cx| {
                                    session.answer_prompt(PromptAnswer::Mcp(Some(true)), cx)
                                });
                                window.close_dialog(cx);
                            }),
                    ),
            )
    });
}

/// A token count as the context ring labels it: "8.2k", "142k", "1.02M".
pub fn short_tokens(tokens: usize) -> String {
    // The unit is picked after rounding: 999,600 is 1.00M, not 1000k.
    let thousands = tokens as f64 / 1e3;
    if tokens < 1_000 {
        tokens.to_string()
    } else if (thousands * 10.).round() < 100. {
        format!("{thousands:.1}k")
    } else if thousands.round() < 1_000. {
        format!("{}k", thousands.round())
    } else {
        format!("{:.2}M", tokens as f64 / 1e6)
    }
}

/// What switching costs, in a sentence: the conversation is read again
/// without its cache.
pub fn switch_cost_text(cost: Option<&arko::details::SwitchCost>) -> String {
    match cost {
        Some(cost) => match (cost.dollars, cost.plan) {
            (Some(dollars), _) => format!(
                "Arko re-reads about {} tokens of this conversation without its cache, costing ≈ ${dollars:.2}.",
                short_tokens(cost.tokens)
            ),
            (None, Some(plan)) => format!(
                "Arko re-reads about {} tokens of this conversation without its cache, using {plan}.",
                short_tokens(cost.tokens)
            ),
            (None, None) => format!(
                "Arko re-reads about {} tokens of this conversation without its cache.",
                short_tokens(cost.tokens)
            ),
        },
        None => "Arko re-reads this conversation without its cache.".to_string(),
    }
}

/// Asks before switching an open session between Chat and Agent: what
/// changes, what it costs, and for Agent the folder it works in (`folders`:
/// the choices, the first chosen). `answer` hears the folder (none for
/// Chat) on Switch. "Don't ask again" counts only on Switch. With
/// `folder_only` (the user said not to ask again), it asks only for the
/// folder: without the cost, and without "Don't ask again".
#[allow(clippy::too_many_arguments)]
pub fn confirm_switch(
    to: crate::backend::Mode,
    cost: Option<arko::details::SwitchCost>,
    folder_only: bool,
    folders: Vec<std::path::PathBuf>,
    home: Option<std::path::PathBuf>,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(Option<std::path::PathBuf>, &mut Window, &mut App) + 'static,
) {
    use crate::backend::Mode;
    let answer = std::rc::Rc::new(answer);
    let dont_ask = std::rc::Rc::new(std::cell::Cell::new(false));
    let folder = std::rc::Rc::new(std::cell::RefCell::new(folders.first().cloned()));
    let (title, change) = match to {
        Mode::Agent => (
            "Switch to Agent mode?",
            "From your next message the agent has its tools and Arko’s agent instructions, and works in the folder below.",
        ),
        Mode::Chat => (
            "Switch to Chat mode?",
            "From your next message Arko answers without tools, as a chat.",
        ),
    };
    let description = if folder_only {
        change.to_string()
    } else {
        format!("{change} {}", switch_cost_text(cost.as_ref()))
    };
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let (checked, remembered) = (dont_ask.clone(), dont_ask.clone());
        let (chosen, shown) = (folder.clone(), folder.clone());
        let folders = folders.clone();
        let home = home.clone();
        let ok = answer.clone();
        alert
            .title(title)
            .description(description.clone())
            .content(move |content, _, _| {
                let checked = checked.clone();
                let current = shown.borrow().clone();
                let choices = folders.clone();
                let chosen = chosen.clone();
                let home = home.clone();
                content.child(
                    v_flex()
                        .id("switch-mode")
                        .test_support()
                        .gap_2()
                        .text_sm()
                        .when(to == Mode::Agent, |this| {
                            let label = current
                                .as_deref()
                                .map(|folder| {
                                    crate::session_view::shown_folder(folder, home.as_deref())
                                })
                                .unwrap_or_else(|| "Choose a folder".to_string());
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .child(div().text_color(muted).child("Folder"))
                                    .child(
                                        Button::new("switch-folder")
                                            .outline()
                                            .small()
                                            .label(label)
                                            .dropdown_caret(true)
                                            .dropdown_menu(move |menu, _, _| {
                                                let menu = choices.iter().fold(menu, |menu, path| {
                                                    let chosen = chosen.clone();
                                                    let path = path.clone();
                                                    menu.item(
                                                        gpui_kit::component::menu::PopupMenuItem::new(
                                                            crate::session_view::shown_folder(
                                                                &path,
                                                                home.as_deref(),
                                                            ),
                                                        )
                                                        .on_click(move |_, window, _| {
                                                            *chosen.borrow_mut() =
                                                                Some(path.clone());
                                                            window.refresh();
                                                        }),
                                                    )
                                                });
                                                let chosen = chosen.clone();
                                                menu.separator().item(
                                                    gpui_kit::component::menu::PopupMenuItem::new(
                                                        "Browse…",
                                                    )
                                                    .on_click(move |_, window, cx| {
                                                        browse_into(chosen.clone(), window, cx)
                                                    }),
                                                )
                                            }),
                                    ),
                            )
                        })
                        .when(!folder_only, |this| {
                            this.child(
                                Checkbox::new("dont-ask-switch")
                                    .label("Don’t ask again")
                                    .checked(checked.get())
                                    .on_click(move |on, window, _| {
                                        checked.set(*on);
                                        window.refresh();
                                    }),
                            )
                        }),
                )
            })
            .confirm()
            .ok_text("Switch")
            .on_ok({
                let folder = folder.clone();
                move |_, window, cx| {
                    let chosen = folder.borrow().clone();
                    if to == Mode::Agent && chosen.is_none() {
                        window.push_notification("Choose a folder for the agent first.", cx);
                        return false;
                    }
                    if remembered.get() {
                        prefs::update(cx, |prefs| prefs.sessions.ask_before_switch = false);
                    }
                    ok(chosen.filter(|_| to == Mode::Agent), window, cx);
                    true
                }
            })
    });
}

/// A switch of the main agent's model once the conversation has started.
pub struct ModelSwitch {
    /// The model, as the model list names it.
    pub label: String,
    /// The effort it starts at, one of `efforts` when there are any.
    pub effort: Option<String>,
    /// The efforts to choose from in the dialog. None when the effort was
    /// chosen already, or the model takes none.
    pub efforts: Vec<String>,
    /// The model stays, and only the effort changes.
    pub keeps_model: bool,
}

/// The dialog's title and description for `switch`.
pub fn model_switch_text(switch: &ModelSwitch) -> (String, String) {
    let label = &switch.label;
    match (&switch.effort, switch.keeps_model) {
        (Some(effort), true) => (
            format!("Switch to {effort} effort?"),
            format!(
                "A new conversation starts at {effort} effort, carrying this one over. This one stays in the conversation list."
            ),
        ),
        _ => (
            format!("Switch to {label}?"),
            format!(
                "A new conversation starts on {label}, carrying this one over. This one stays in the conversation list."
            ),
        ),
    }
}

/// Asks before the main agent moves to another model (or effort) in a new
/// conversation that carries the current one over. `answer` hears the
/// effort on Switch. "Don't ask again" counts only on Switch.
pub fn confirm_model_switch(
    switch: ModelSwitch,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(Option<String>, &mut Window, &mut App) + 'static,
) {
    let (title, description) = model_switch_text(&switch);
    let answer = std::rc::Rc::new(answer);
    let dont_ask = std::rc::Rc::new(std::cell::Cell::new(false));
    let effort = std::rc::Rc::new(std::cell::RefCell::new(switch.effort.clone()));
    let efforts = switch.efforts;
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (checked, remembered) = (dont_ask.clone(), dont_ask.clone());
        let (chosen, shown) = (effort.clone(), effort.clone());
        let efforts = efforts.clone();
        let ok = answer.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .content(move |content, _, _| {
                let checked = checked.clone();
                let current = shown.borrow().clone();
                let efforts = efforts.clone();
                let chosen = chosen.clone();
                content.child(
                    v_flex()
                        .id("switch-model")
                        .test_support()
                        .gap_2()
                        .text_sm()
                        .when_some(current.filter(|_| !efforts.is_empty()), |this, current| {
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .child(div().text_color(muted).child("Effort"))
                                    .child(
                                        Button::new("switch-effort")
                                            .outline()
                                            .small()
                                            // Named with its label, as heard.
                                            .accessibility_label(format!("Effort, {current}"))
                                            .label(current)
                                            .dropdown_caret(true)
                                            .dropdown_menu(move |menu, _, _| {
                                                efforts.iter().fold(menu, |menu, effort| {
                                                    let chosen = chosen.clone();
                                                    let effort = effort.clone();
                                                    menu.item(
                                                        gpui_kit::component::menu::PopupMenuItem::new(
                                                            effort.clone(),
                                                        )
                                                        .on_click(move |_, window, _| {
                                                            *chosen.borrow_mut() =
                                                                Some(effort.clone());
                                                            window.refresh();
                                                        }),
                                                    )
                                                })
                                            }),
                                    ),
                            )
                        })
                        .child(
                            Checkbox::new("dont-ask-switch")
                                .label("Don’t ask again")
                                .checked(checked.get())
                                .on_click(move |on, window, _| {
                                    checked.set(*on);
                                    window.refresh();
                                }),
                        ),
                )
            })
            .confirm()
            .ok_text("Switch")
            .on_ok({
                let effort = effort.clone();
                move |_, window, cx| {
                    if remembered.get() {
                        prefs::update(cx, |prefs| prefs.sessions.ask_before_switch = false);
                    }
                    ok(effort.borrow().clone(), window, cx);
                    true
                }
            })
    });
}

/// The dialog's title and description before a message continues
/// `worker`, with Arko's `warning` of what that costs.
pub fn continue_text(worker: &str, warning: &str) -> (String, String) {
    (format!("Continue {worker}?"), workers::curly(warning))
}

/// Asks before a message continues `worker`, a finished worker whose prompt
/// cache expired, with Arko's `warning` of what that costs. `answer` hears
/// Continue.
pub fn confirm_continue(
    worker: &str,
    warning: &str,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(&mut Window, &mut App) + 'static,
) {
    let (title, description) = continue_text(worker, warning);
    let answer = std::rc::Rc::new(answer);
    window.open_alert_dialog(cx, move |alert, _, _| {
        let ok = answer.clone();
        alert
            .title(title.clone())
            .description(description.clone())
            .confirm()
            .ok_text("Continue")
            .on_ok(move |_, window, cx| {
                ok(window, cx);
                true
            })
    });
}

/// Asks for a folder, and puts it in `chosen`.
fn browse_into(
    chosen: std::rc::Rc<std::cell::RefCell<Option<std::path::PathBuf>>>,
    window: &mut Window,
    cx: &mut App,
) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Choose".into()),
    });
    window
        .spawn(cx, async move |cx| {
            if let Ok(Ok(Some(paths))) = paths.await
                && let Some(folder) = paths.into_iter().next()
            {
                *chosen.borrow_mut() = Some(folder);
                cx.update(|window, _| window.refresh()).ok();
            }
        })
        .detach();
}

/// What closing some sessions would stop: each session's title, and its
/// jobs and workers.
pub type Work = Vec<(SharedString, arko::QuitAsk)>;

/// Asks before stopping running jobs and workers: `quitting` the app, or
/// closing a session. `answer` hears Quit (`true`) or Cancel. "Don't ask
/// again" is remembered only when the user quits.
pub fn confirm_stop(
    work: Work,
    quitting: bool,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(bool, &mut Window, &mut App) + 'static,
) {
    let answer = std::rc::Rc::new(answer);
    let dont_ask = std::rc::Rc::new(std::cell::Cell::new(false));
    let quit_label = if !quitting {
        "Close Session"
    } else if cfg!(target_os = "windows") {
        "Exit"
    } else {
        "Quit"
    };
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let theme = cx.theme();
        let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
        let mono_features = theme.mono_font_features.clone();
        let work = work.clone();
        let (ok, cancel) = (answer.clone(), answer.clone());
        let checked = dont_ask.clone();
        let remembered = dont_ask.clone();
        let several = work.len() > 1;
        alert
            .title(if quitting {
                "Stop the work still running?"
            } else {
                "Stop this session’s work?"
            })
            .description(
                "Background jobs stop for good. Workers stop too, and resuming the session lets the agent continue them.",
            )
            .content(move |content, _, _| {
                let work = work.clone();
                let checked = checked.clone();
                content.child(
                    v_flex()
                        .id("quit-work")
                        .test_support()
                        .gap_2()
                        .text_sm()
                        .children(work.iter().map(|(title, ask)| {
                            v_flex()
                                .gap_0p5()
                                .when(several, |this| {
                                    this.child(div().font_medium().child(title.clone()))
                                })
                                .when(!ask.workers.is_empty(), |this| {
                                    this.child(
                                        div()
                                            .text_color(muted)
                                            .child(workers::running_text(&ask.workers)),
                                    )
                                })
                                .children(ask.jobs.iter().map(|job| {
                                    div()
                                        .font_family(mono.clone())
                                        .font_features(mono_features.clone())
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(job.command.clone())
                                }))
                        }))
                        .child(
                            Checkbox::new("dont-ask")
                                .label("Don’t ask again")
                                .checked(checked.get())
                                .on_click(move |on, window, _| {
                                    checked.set(*on);
                                    window.refresh();
                                }),
                        ),
                )
            })
            .confirm()
            .ok_text(quit_label)
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                if remembered.get() {
                    prefs::update(cx, |prefs| prefs.sessions.ask_before_quit = false);
                }
                ok(true, window, cx);
                true
            })
            .on_cancel(move |_, window, cx| {
                cancel(false, window, cx);
                true
            })
    });
}

#[cfg(test)]
mod tests {
    use super::{ModelSwitch, continue_text, model_switch_text, short_tokens, switch_cost_text};

    /// The question names the worker, and says in Arko's words, with the
    /// app's apostrophes, how long ago its cache expired and what
    /// continuing re-reads.
    #[test]
    fn continuing_a_worker_says_what_it_re_reads() {
        let worker = crate::workers::ended_with_cache("r3", 2 * 3_600_000 + 60_000, 340_000);
        let cache = worker.cache().unwrap();
        assert_eq!(
            continue_text("r3", &cache.warning(chrono::Utc::now())),
            (
                "Continue r3?".to_string(),
                "r3’s cache expired 2 hours ago. Continuing re-reads about 340k tokens without it (≈ $1.36 at API prices).".to_string()
            )
        );
    }

    /// A switch of model says what starts, on what, and where the current
    /// conversation stays.
    #[test]
    fn a_model_switch_says_a_new_conversation_starts() {
        let switch = |effort: Option<&str>, keeps_model| ModelSwitch {
            label: "GPT-6.1 Sol".into(),
            effort: effort.map(String::from),
            efforts: Vec::new(),
            keeps_model,
        };
        assert_eq!(
            model_switch_text(&switch(Some("high"), false)),
            (
                "Switch to GPT-6.1 Sol?".to_string(),
                "A new conversation starts on GPT-6.1 Sol, carrying this one over. This one stays in the conversation list.".to_string()
            )
        );
        assert_eq!(
            model_switch_text(&switch(Some("low"), true)),
            (
                "Switch to low effort?".to_string(),
                "A new conversation starts at low effort, carrying this one over. This one stays in the conversation list.".to_string()
            )
        );
    }

    #[test]
    fn token_counts_read_as_the_context_ring_says() {
        assert_eq!(short_tokens(820), "820");
        assert_eq!(short_tokens(8_200), "8.2k");
        assert_eq!(short_tokens(142_318), "142k");
        assert_eq!(short_tokens(1_020_000), "1.02M");
        assert_eq!(short_tokens(9_960), "10k");
        assert_eq!(short_tokens(999_600), "1.00M");
    }

    #[test]
    fn a_switch_says_what_it_costs() {
        let cost = |dollars, plan| arko::details::SwitchCost {
            tokens: 180_000,
            dollars,
            plan,
        };
        assert_eq!(
            switch_cost_text(Some(&cost(Some(0.9), None))),
            "Arko re-reads about 180k tokens of this conversation without its cache, costing ≈ $0.90."
        );
        assert_eq!(
            switch_cost_text(Some(&cost(None, Some("your Claude plan")))),
            "Arko re-reads about 180k tokens of this conversation without its cache, using your Claude plan."
        );
        assert_eq!(
            switch_cost_text(None),
            "Arko re-reads this conversation without its cache."
        );
    }
}
