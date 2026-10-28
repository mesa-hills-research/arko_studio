//! Settings › Sessions: what new agent sessions and chats start with, and
//! what the app asks before it stops or removes something. An open
//! session's own live settings are in its Session info.

use gpui_kit::component::input::{InputEvent, Textarea, TextareaState};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, IconName};
use gpui_kit::*;

use super::fields::{self, TextKind, TextSetting};
use crate::prefs::{self, ExtraRules, NewSessions, ProjectInstructions};
use crate::writing::WritingAids;

mod names;

use crate::workers::{
    billing_about, budget_about, copies_about, curly, depth_about, models_about, retry_cap_about,
};
use arko::commands::MIN_HANDOVER_SIZE;
use arko::monitors::Wakes;
use arko::workers;
use arko::{ModelChoice, WorkerBilling};

/// arko's own time before a command moves to the background, in seconds.
pub const DEFAULT_BACKGROUND_AFTER: u64 = 600;

/// The page's state: the fields typed in.
pub struct SessionsPage {
    handover: TextSetting<usize>,
    workers_handover: TextSetting<usize>,
    background_after: TextSetting<u64>,
    monitor_wakes: TextSetting<Wakes>,
    chat_instructions: Entity<TextareaState>,
    /// The chat instructions the field last showed or saved.
    instructions_seen: String,
    /// Saves the chat instructions once the typing rests.
    instructions_typed: Option<Task<()>>,
    /// How new sessions are named, as the page last read it.
    names: names::Names,
    /// The preferences as the page last showed them.
    seen: prefs::Prefs,
    _subscriptions: Vec<Subscription>,
}

impl SessionsPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let new = prefs::get(cx).new_sessions.clone();
        let input = prefs::get(cx).input.clone();
        let handover = TextSetting::new(
            TextKind {
                get: |prefs| prefs.new_sessions.handover_at,
                set: |prefs, value| prefs.new_sessions.handover_at = value,
                parse: parse_handover,
                show: |tokens| shown_tokens(*tokens),
            },
            "Its model’s",
            window,
            cx,
        );
        let workers_handover = TextSetting::new(
            TextKind {
                get: |prefs| prefs.new_sessions.workers_handover_at,
                set: |prefs, value| prefs.new_sessions.workers_handover_at = value,
                parse: parse_handover,
                show: |tokens| shown_tokens(*tokens),
            },
            "The main agent’s",
            window,
            cx,
        );
        let background_after = TextSetting::new(
            TextKind {
                get: |prefs| prefs.new_sessions.background_after,
                set: |prefs, value| prefs.new_sessions.background_after = value,
                parse: parse_background_after,
                show: |seconds| shown_background_after(*seconds),
            },
            &shown_background_after(DEFAULT_BACKGROUND_AFTER),
            window,
            cx,
        );
        let monitor_wakes = TextSetting::new(
            TextKind {
                get: |prefs| {
                    prefs
                        .new_sessions
                        .monitor_wakes
                        .as_deref()
                        .and_then(|wakes| arko::monitors::parse_wakes(wakes).ok())
                },
                set: |prefs, value| {
                    prefs.new_sessions.monitor_wakes = value.map(|wakes| wakes.shown())
                },
                parse: parse_monitor_wakes,
                show: |wakes| wakes.shown(),
            },
            &Wakes::default().shown(),
            window,
            cx,
        );
        let writing = cx.global::<WritingAids>().0.clone();
        let chat_instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .placeholder("Answer briefly, and in British English.")
                .keymap(crate::composer::text_keymap(input.keymap))
                .spell_checker(writing.spell_checker())
                .default_value(new.chat_instructions.clone())
        });
        chat_instructions.update(cx, |field, cx| {
            field.set_spell_checking(input.spell_check, cx)
        });
        let subscriptions = vec![
            fields::subscribe(|page: &mut Self| &mut page.handover, handover.input(), cx),
            fields::subscribe(
                |page: &mut Self| &mut page.workers_handover,
                workers_handover.input(),
                cx,
            ),
            fields::subscribe(
                |page: &mut Self| &mut page.background_after,
                background_after.input(),
                cx,
            ),
            fields::subscribe(
                |page: &mut Self| &mut page.monitor_wakes,
                monitor_wakes.input(),
                cx,
            ),
            // Saved once the typing rests, or as the field is left.
            cx.subscribe(&chat_instructions, |this, _, event, cx| match event {
                InputEvent::Change => {
                    this.instructions_typed = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(fields::TYPING_REST).await;
                        this.update(cx, |this, cx| this.save_instructions(cx)).ok();
                    }));
                }
                InputEvent::Blur if this.instructions_typed.is_some() => {
                    this.instructions_typed = None;
                    this.save_instructions(cx);
                }
                _ => {}
            }),
            // A key or a login added or removed changes what a way lacks.
            cx.observe_global::<crate::backend::AccountsSeen>(|this, cx| this.refresh_names(cx)),
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                if !super::shown_changed(&mut this.seen, cx) {
                    return;
                }
                this.handover.follow(window, cx);
                this.workers_handover.follow(window, cx);
                this.background_after.follow(window, cx);
                this.monitor_wakes.follow(window, cx);
                this.follow_input(cx);
                // Instructions changed elsewhere (a reset) show, and what's
                // typed but not saved yet stays.
                let text = prefs::get(cx).new_sessions.chat_instructions.clone();
                if text != this.instructions_seen {
                    this.instructions_seen = text.clone();
                    this.instructions_typed = None;
                    if this.chat_instructions.read(cx).value() != text {
                        this.chat_instructions
                            .update(cx, |field, cx| field.set_value(text, window, cx));
                    }
                }
                cx.notify();
            }),
        ];
        // Settings closed while typing: what's typed is saved.
        cx.on_release(|this, cx| {
            this.handover.commit(cx);
            this.workers_handover.commit(cx);
            this.background_after.commit(cx);
            this.monitor_wakes.commit(cx);
            if this.instructions_typed.take().is_some() {
                this.save_instructions(cx);
            }
        })
        .detach();
        let mut this = Self {
            handover,
            workers_handover,
            background_after,
            monitor_wakes,
            chat_instructions,
            instructions_seen: new.chat_instructions,
            instructions_typed: None,
            names: names::Names::new(),
            seen: super::shown_prefs(prefs::get(cx)),
            _subscriptions: subscriptions,
        };
        this.refresh_names(cx);
        this
    }

    /// Follows the keybindings and spell check settings for chat instructions.
    fn follow_input(&mut self, cx: &mut Context<Self>) {
        let input = prefs::get(cx).input.clone();
        self.chat_instructions.update(cx, |field, cx| {
            let keymap = crate::composer::text_keymap(input.keymap);
            if field.current_keymap() != keymap {
                field.set_keymap(keymap, cx);
            }
            if field.is_spell_checking() != input.spell_check {
                field.set_spell_checking(input.spell_check, cx);
            }
        });
    }

    /// Whether the chat instructions type with smooth typing, the keys and
    /// spell check Settings › Input sets (tests).
    #[cfg(test)]
    pub fn instructions_input(&self, cx: &App) -> (bool, gpui_kit::component::input::Keymap, bool) {
        let field = self.chat_instructions.read(cx);
        (
            field.has_smooth_caret(),
            field.current_keymap(),
            field.is_spell_checking(),
        )
    }

    /// Saves the chat instructions typed.
    fn save_instructions(&mut self, cx: &mut App) {
        let text = self.chat_instructions.read(cx).value().to_string();
        self.instructions_seen = text.clone();
        prefs::update(cx, |prefs| prefs.new_sessions.chat_instructions = text);
    }

    /// Why each typed setting isn't saved, if one isn't: main agent's
    /// handover size, workers', background after, monitor wakes.
    pub fn errors(&self) -> [Option<SharedString>; 4] {
        [
            self.handover.error(),
            self.workers_handover.error(),
            self.background_after.error(),
            self.monitor_wakes.error(),
        ]
    }

    /// What's typed in each field: main agent's handover size, workers',
    /// background after, monitor wakes.
    pub fn typed(&self, cx: &App) -> [SharedString; 4] {
        [
            self.handover.typed(cx),
            self.workers_handover.typed(cx),
            self.background_after.typed(cx),
            self.monitor_wakes.typed(cx),
        ]
    }

    pub fn page(&self, this: &Entity<Self>, cx: &App) -> SettingPage {
        let prefs = prefs::get(cx);
        let new = &prefs.new_sessions;
        let errors = self.errors();
        // What the groups show, which they draw again only when it changes.
        // Their fields draw themselves as they're typed in.
        let agent = (
            (
                new.handover_at,
                new.workers_handover_at,
                new.background_after,
                new.monitor_wakes.clone(),
            ),
            (
                new.worker_models,
                new.worker_billing,
                new.api_budget,
                new.worker_depth,
                new.worker_retry_cap,
                new.message_copies,
            ),
            (
                new.outside_access,
                new.text_replies,
                new.cc_compact,
                new.extra_rules,
                new.project_instructions,
                new.worker_project_instructions,
            ),
            errors.each_ref().map(|error| error.as_deref()),
        );
        SettingPage::new(super::PAGES[super::SESSIONS_PAGE])
            .icon(IconName::FolderOpen)
            .description(
                "What new sessions start with. Click an open session’s title to change its settings in Session info.",
            )
            .groups([
                self.agent_group(cx)
                    .key(super::group_key("Agent sessions", &agent)),
                self.chat_group().key(super::group_key("Chats", &())),
                self.names_group(this),
                confirmations_group()
                    .key(super::group_key("Confirmations", &prefs.sessions)),
            ])
    }

    fn agent_group(&self, cx: &App) -> SettingGroup {
        let defaults = NewSessions::default();
        let new = &prefs::get(cx).new_sessions;
        SettingGroup::new()
            .title("Agent sessions")
            .items([
                self.handover.item("Handover size", "handover-at")
                .description(format!(
                    "When the main agent’s context reaches it, the agent writes a report and goes on in a fresh conversation. Like 950k or 1.2M, at least {}k, and held under 95% of what its model takes.",
                    MIN_HANDOVER_SIZE / 1000
                ))
                .keywords(["context", "tokens", "handover-at", "main agent"]),
                self.workers_handover
                    .item("Workers’ handover size", "workers-handover-at")
                .description("The same for workers, each up to what its own model takes.")
                .keywords(["context", "tokens", "handover-at", "workers"]),
                super::choice_of(
                    workers::MODELS_NAME,
                    ModelChoice::ALL
                        .map(|choice| (choice, SharedString::from(curly(choice.label()))))
                        .to_vec(),
                    |prefs| prefs.new_sessions.worker_models,
                    |prefs, choice| prefs.new_sessions.worker_models = choice,
                    defaults.worker_models,
                )
                .description(models_about(new.worker_models))
                .keywords(["workers", "profiles", "model", "worker-models", "agent chooses"]),
                super::choice_of(
                    workers::BILLING_NAME,
                    WorkerBilling::ALL
                        .map(|billing| (billing, SharedString::from(billing.label())))
                        .to_vec(),
                    |prefs| prefs.new_sessions.worker_billing.unwrap_or_default(),
                    |prefs, billing| prefs.new_sessions.worker_billing = Some(billing),
                    WorkerBilling::default(),
                )
                .description(billing_about(new.worker_billing.unwrap_or_default()))
                .keywords(["workers", "billing", "api", "per token", "plans", "worker-billing"]),
                super::switch(
                    workers::BUDGET_NAME,
                    "",
                    |prefs| prefs.new_sessions.api_budget.unwrap_or(false),
                    |prefs, on| prefs.new_sessions.api_budget = Some(on),
                    false,
                )
                .description(budget_about(
                    new.api_budget
                        .unwrap_or(false)
                        .then_some(workers::DEFAULT_BUDGET),
                ))
                .keywords(["workers", "budget", "api", "spend", "dollars", "api-budget"]),
                super::choice_of(
                    workers::DEPTH_NAME,
                    (1..=workers::MAX_DEPTH)
                        .map(|depth| (depth, SharedString::from(depth.to_string())))
                        .collect(),
                    |prefs| prefs.new_sessions.worker_depth,
                    |prefs, depth| prefs.new_sessions.worker_depth = depth,
                    defaults.worker_depth,
                )
                .description(depth_about())
                .keywords(["workers", "nested", "levels", "worker-depth"]),
                super::switch(
                    workers::RETRY_CAP_NAME,
                    "",
                    |prefs| prefs.new_sessions.worker_retry_cap,
                    |prefs, on| prefs.new_sessions.worker_retry_cap = on,
                    defaults.worker_retry_cap,
                )
                .description(retry_cap_about(new.worker_retry_cap))
                .keywords(["workers", "retry", "failed requests", "worker-retry-cap"]),
                super::switch(
                    workers::COPIES_NAME,
                    "",
                    |prefs| prefs.new_sessions.message_copies,
                    |prefs, on| prefs.new_sessions.message_copies = on,
                    defaults.message_copies,
                )
                .description(copies_about(new.message_copies))
                .keywords(["workers", "messages", "copy", "message-copies"]),
                self.background_after
                    .item("Background after", "background-after")
                .description(
                    "A shell command still running then moves to the background, and the agent goes on with its work. 90s, 10m or 1h at most, or off.",
                )
                .keywords(["shell", "command", "timeout", "jobs"]),
                self.monitor_wakes
                    .item(arko::monitors::NAME, "monitor-wakes")
                .description(
                    "A monitor’s warning before it ends wakes an idle agent for this long after your last message, so it can keep the monitor. Auto lasts while the prompt cache from that message does. Or 30m, 2h or 24h at most, always, or off.",
                )
                .keywords(["monitor", "warning", "wake", "idle", "jobs"]),
                super::switch(
                    "Outside access",
                    "The agents may change files outside the session’s folder. The status bar says so while it’s on.",
                    |prefs| prefs.new_sessions.outside_access,
                    |prefs, on| prefs.new_sessions.outside_access = on,
                    defaults.outside_access,
                )
                .keywords(["workspace", "files", "permissions"]),
                super::switch(
                    "Text replies",
                    "The main agent ends its turns with a reply in text. Off, it ends them by calling complete_task.",
                    |prefs| prefs.new_sessions.text_replies,
                    |prefs, on| prefs.new_sessions.text_replies = on,
                    defaults.text_replies,
                )
                .keywords(["complete_task", "replies"]),
                super::switch(
                    "Claude Code compacts",
                    "At the handover size, Claude Code compacts the conversation of agents on cc: models. Off, they hand over as on other providers.",
                    |prefs| prefs.new_sessions.cc_compact,
                    |prefs, on| prefs.new_sessions.cc_compact = on,
                    defaults.cc_compact,
                )
                .keywords(["cc", "compaction", "compact", "handover"]),
                super::choice(
                    "Extra rules",
                    &[
                        (ExtraRules::Small, "Small models"),
                        (ExtraRules::All, "Every agent"),
                        (ExtraRules::Off, "No agent"),
                    ],
                    |prefs| prefs.new_sessions.extra_rules,
                    |prefs, rules| prefs.new_sessions.extra_rules = rules,
                    defaults.extra_rules,
                )
                .description(
                    "Plain, explicit rules in the agents’ instructions, which small models (Haiku, luna) do better with.",
                )
                .keywords(["small models", "instructions", "haiku"]),
                super::choice(
                    "Project instructions",
                    &[
                        (ProjectInstructions::One, "AGENTS.md, else CLAUDE.md"),
                        (ProjectInstructions::Both, "Both"),
                        (ProjectInstructions::Off, "Neither"),
                    ],
                    |prefs| prefs.new_sessions.project_instructions,
                    |prefs, files| prefs.new_sessions.project_instructions = files,
                    defaults.project_instructions,
                )
                .description(
                    "Which of the project’s files the main agent gets with its first message.",
                )
                .keywords(["AGENTS.md", "CLAUDE.md", "project"]),
                super::switch(
                    "Workers get them too",
                    "Each worker gets the project’s instructions with its first message.",
                    |prefs| prefs.new_sessions.worker_project_instructions,
                    |prefs, on| prefs.new_sessions.worker_project_instructions = on,
                    defaults.worker_project_instructions,
                )
                .keywords(["AGENTS.md", "CLAUDE.md", "workers"]),
            ])
    }

    fn chat_group(&self) -> SettingGroup {
        let field = self.chat_instructions.clone();
        SettingGroup::new().title("Chats").items([
            SettingItem::new(
                "Custom instructions",
                SettingField::render(move |_, _, _| {
                    div()
                        .id("chat-instructions")
                        .test_support()
                        .w_full()
                        .child(Textarea::new(&field))
                })
                .on_reset(
                    |cx| !prefs::get(cx).new_sessions.chat_instructions.is_empty(),
                    |_, cx| {
                        prefs::update(cx, |prefs| {
                            prefs.new_sessions.chat_instructions.clear()
                        })
                    },
                ),
            )
            .layout(Axis::Vertical)
            .description(
                "What a new chat’s model is told after Arko’s own instructions. An open chat takes changes in its Session info, from its next conversation.",
            )
            .keywords(["system prompt", "chat"]),
            SettingItem::new(
                "At the context limit",
                SettingField::render(|_, _, cx| {
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Summarize and continue")
                }),
            )
            .description(
                "At the main agent’s handover size, the model writes a summary of the chat, and the chat goes on from it in a fresh conversation.",
            )
            .keywords(["handover", "summary", "context"]),
        ])
    }
}

fn confirmations_group() -> SettingGroup {
    let defaults = prefs::SessionPrefs::default();
    SettingGroup::new().title("Confirmations").items([
        super::switch(
            "Ask about running work",
            "Before quitting or closing a session stops background jobs and workers.",
            |prefs| prefs.sessions.ask_before_quit,
            |prefs, on| prefs.sessions.ask_before_quit = on,
            defaults.ask_before_quit,
        )
        .keywords(["quit", "close", "jobs", "workers"]),
        super::switch(
            "Ask before switching modes",
            "Before switching an open session between Chat and Agent, say what it costs.",
            |prefs| prefs.sessions.ask_before_switch,
            |prefs, on| prefs.sessions.ask_before_switch = on,
            defaults.ask_before_switch,
        )
        .keywords(["chat", "agent", "mode", "cache"]),
        super::switch(
            "Ask before editing a message",
            "Editing or regenerating a message removes the messages after it.",
            |prefs| prefs.sessions.ask_before_edit,
            |prefs, on| prefs.sessions.ask_before_edit = on,
            defaults.ask_before_edit,
        )
        .keywords(["edit", "regenerate", "branch"]),
    ])
}

/// A handover size: `200000`, `200,000`, `200_000`, `950k`, `250.5k`, `1M`
/// or `1.2M`, at least [`MIN_HANDOVER_SIZE`]. Empty: the model's own.
pub fn parse_handover(text: &str) -> Result<Option<usize>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    arko::commands::parse_handover_size(text).map(Some)
}

/// A number of tokens as the field shows it: `1.2M`, `950k`, or the
/// number.
pub fn shown_tokens(tokens: usize) -> String {
    if tokens >= 1_000_000 && tokens.is_multiple_of(100_000) {
        let tenths = tokens / 100_000;
        match tenths % 10 {
            0 => format!("{}M", tenths / 10),
            rest => format!("{}.{rest}M", tenths / 10),
        }
    } else if tokens.is_multiple_of(1000) {
        format!("{}k", tokens / 1000)
    } else {
        tokens.to_string()
    }
}

/// When commands move to the background, as arko's `/background-after`
/// reads it: `90s`, `10m`, `1h` or `off` (0), at most an hour. Empty:
/// arko's own.
pub fn parse_background_after(text: &str) -> Result<Option<u64>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    arko::commands::parse_background_after(text).map(Some)
}

/// How long monitors' warnings wake the agent, as arko's `/monitor-wakes`
/// reads it: `auto`, `30m`, `2h` or `24h` at most, `always`, or `off`.
/// Empty: arko's own (`auto`).
pub fn parse_monitor_wakes(text: &str) -> Result<Option<Wakes>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    arko::monitors::parse_wakes(text).map(Some)
}

/// A time in seconds as the field shows it: `90s`, `10m`, `1h`, or `off`.
pub fn shown_background_after(seconds: u64) -> String {
    match seconds {
        0 => "off".into(),
        seconds if seconds.is_multiple_of(3_600) => format!("{}h", seconds / 3_600),
        seconds if seconds.is_multiple_of(60) => format!("{}m", seconds / 60),
        seconds => format!("{seconds}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        parse_background_after, parse_handover, parse_monitor_wakes, shown_background_after,
        shown_tokens,
    };

    #[test]
    fn handover_sizes_read_as_arko_reads_them() {
        assert_eq!(parse_handover(""), Ok(None));
        assert_eq!(parse_handover("200000"), Ok(Some(200_000)));
        assert_eq!(parse_handover(" 200,000 "), Ok(Some(200_000)));
        assert_eq!(parse_handover("200_000"), Ok(Some(200_000)));
        assert_eq!(parse_handover("950k"), Ok(Some(950_000)));
        assert!(parse_handover("950 K").is_err());
        assert!(parse_handover("lots").is_err());
        assert!(parse_handover("-5").is_err());
        for text in ["5k", "0", "1,2k", "1.2345678M"] {
            assert_eq!(
                parse_handover(text),
                arko::commands::parse_handover_size(text).map(Some)
            );
        }
        assert!(parse_handover("99999999999999999999999k").is_err());
        for tokens in [20_000, 950_000, 123_456, 1_000_000, 1_200_000, 1_250_000] {
            assert_eq!(parse_handover(&shown_tokens(tokens)), Ok(Some(tokens)));
        }
    }

    /// Millions and decimals with k or M must name whole tokens.
    #[test]
    fn handover_sizes_take_millions_and_decimals() {
        assert_eq!(parse_handover("1M"), Ok(Some(1_000_000)));
        assert_eq!(parse_handover("1m"), Ok(Some(1_000_000)));
        assert_eq!(parse_handover("1.2M"), Ok(Some(1_200_000)));
        assert_eq!(parse_handover("0.95M"), Ok(Some(950_000)));
        assert!(parse_handover(".5M").is_err());
        assert_eq!(parse_handover("250.5k"), Ok(Some(250_500)));
        assert!(parse_handover("1.2345678M").is_err());
        assert!(parse_handover("200000.5").is_err(), "tokens are whole");
        assert!(parse_handover("1.2.3M").is_err());
        assert!(parse_handover("M").is_err());
        assert_eq!(shown_tokens(1_200_000), "1.2M");
        assert_eq!(shown_tokens(1_000_000), "1M");
        assert_eq!(shown_tokens(1_250_000), "1250k");
        assert_eq!(shown_tokens(950_000), "950k");
    }

    #[test]
    fn background_times_read_as_arko_reads_them() {
        assert_eq!(parse_background_after(""), Ok(None));
        assert_eq!(parse_background_after("off"), Ok(Some(0)));
        assert_eq!(parse_background_after("90s"), Ok(Some(90)));
        assert_eq!(parse_background_after("10M"), Ok(Some(600)));
        assert_eq!(parse_background_after("1h"), Ok(Some(3_600)));
        assert!(parse_background_after("2h").is_err());
        assert!(parse_background_after("0s").is_err());
        for text in ["ten", "é", "2h"] {
            assert_eq!(
                parse_background_after(text),
                arko::commands::parse_background_after(text).map(Some)
            );
        }
        for seconds in [0, 90, 600, 3_600] {
            assert_eq!(
                parse_background_after(&shown_background_after(seconds)),
                Ok(Some(seconds))
            );
        }
    }

    #[test]
    fn monitor_wakes_read_as_arko_reads_them() {
        use arko::monitors::Wakes;

        assert_eq!(parse_monitor_wakes(""), Ok(None));
        assert_eq!(parse_monitor_wakes("2h"), Ok(Some(Wakes::For(7_200))));
        assert_eq!(parse_monitor_wakes(" Always "), Ok(Some(Wakes::Always)));
        assert_eq!(parse_monitor_wakes("off"), Ok(Some(Wakes::Never)));
        assert_eq!(parse_monitor_wakes("auto"), Ok(Some(Wakes::Auto)));
        for text in ["soon", "25h", "0m"] {
            assert!(parse_monitor_wakes(text).is_err(), "{text}");
        }
        for wakes in [Wakes::Never, Wakes::For(1_800), Wakes::Auto, Wakes::Always] {
            assert_eq!(parse_monitor_wakes(&wakes.shown()), Ok(Some(wakes)));
        }
    }
}
