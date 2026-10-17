//! Settings › Sessions: what new agent sessions and chats start with, and
//! what the app asks before it stops or removes something. An open
//! session's own live settings are in its Session info.

use gpui_kit::component::input::{Textarea, TextareaState};
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _};
use gpui_kit::*;

use super::fields::{TextKind, TextSetting, is_change};
use crate::prefs::{self, ExtraRules, NewSessions, ProjectInstructions};
use crate::writing::WritingAids;

/// The smallest handover size arko takes: a fresh conversation starts at
/// about a tenth of it.
pub const MIN_HANDOVER: usize = 20_000;

/// The longest arko waits before moving a command to the background, in
/// seconds.
pub const MAX_BACKGROUND_AFTER: u64 = 3_600;

/// arko's own time before a command moves to the background, in seconds.
pub const DEFAULT_BACKGROUND_AFTER: u64 = 600;

/// The page's state: the fields typed in.
pub struct SessionsPage {
    handover: TextSetting<usize>,
    workers_handover: TextSetting<usize>,
    background_after: TextSetting<u64>,
    chat_instructions: Entity<TextareaState>,
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
        let writing = cx.global::<WritingAids>().0.clone();
        let chat_instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(4, 12)
                .placeholder("Answer briefly, and in British English.")
                .smooth_caret(input.smooth_typing)
                .keymap(crate::composer::text_keymap(input.keymap))
                .spell_checker(writing.spell_checker())
                .default_value(new.chat_instructions.clone())
        });
        chat_instructions.update(cx, |field, cx| {
            field.set_spell_checking(input.spell_check, cx)
        });
        let subscriptions = vec![
            cx.subscribe(handover.input(), |this, _, event, cx| {
                if is_change(event) {
                    this.handover.changed(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(workers_handover.input(), |this, _, event, cx| {
                if is_change(event) {
                    this.workers_handover.changed(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(background_after.input(), |this, _, event, cx| {
                if is_change(event) {
                    this.background_after.changed(cx);
                    cx.notify();
                }
            }),
            cx.subscribe(&chat_instructions, |_, field, event, cx| {
                if is_change(event) {
                    let text = field.read(cx).value().to_string();
                    prefs::update(cx, |prefs| prefs.new_sessions.chat_instructions = text);
                }
            }),
            cx.observe_global_in::<prefs::PrefsStore>(window, |this, window, cx| {
                if !super::shown_changed(&mut this.seen, cx) {
                    return;
                }
                this.handover.follow(window, cx);
                this.workers_handover.follow(window, cx);
                this.background_after.follow(window, cx);
                let text = prefs::get(cx).new_sessions.chat_instructions.clone();
                if this.chat_instructions.read(cx).value() != text {
                    this.chat_instructions
                        .update(cx, |field, cx| field.set_value(text, window, cx));
                }
                cx.notify();
            }),
        ];
        Self {
            handover,
            workers_handover,
            background_after,
            chat_instructions,
            seen: super::shown_prefs(prefs::get(cx)),
            _subscriptions: subscriptions,
        }
    }

    /// Why each typed setting isn't saved, if one isn't: main agent's
    /// handover size, workers', background after.
    pub fn errors(&self) -> [Option<SharedString>; 3] {
        [
            self.handover.error(),
            self.workers_handover.error(),
            self.background_after.error(),
        ]
    }

    /// What's typed in each field: main agent's handover size, workers',
    /// background after.
    pub fn typed(&self, cx: &App) -> [SharedString; 3] {
        [
            self.handover.typed(cx),
            self.workers_handover.typed(cx),
            self.background_after.typed(cx),
        ]
    }

    pub fn page(&self) -> SettingPage {
        SettingPage::new(super::PAGES[3])
            .icon(IconName::FolderOpen)
            .description(
                "What new sessions start with. An open session’s own settings are in its Session info: click its title.",
            )
            .groups([self.agent_group(), self.chat_group(), confirmations_group()])
    }

    fn agent_group(&self) -> SettingGroup {
        let defaults = NewSessions::default();
        SettingGroup::new()
            .title("Agent sessions")
            .items([
                self.handover.item("Handover size", "handover-at")
                .description(format!(
                    "When the main agent’s context reaches it, the agent writes a report and goes on in a fresh conversation. Like 950k or 1.2M, at least {}k, and held under 95% of what its model takes.",
                    MIN_HANDOVER / 1000
                ))
                .keywords(["context", "tokens", "handover-at", "main agent"]),
                self.workers_handover
                    .item("Workers’ handover size", "workers-handover-at")
                .description("The same for workers, each up to what its own model takes.")
                .keywords(["context", "tokens", "handover-at", "workers"]),
                self.background_after
                    .item("Background after", "background-after")
                .description(
                    "A shell command still running then moves to the background, and the agent goes on with its work. 90s, 10m or 1h at most, or off.",
                )
                .keywords(["shell", "command", "timeout", "jobs"]),
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
                        .child(Textarea::new(&field).small())
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
/// or `1.2M`, at least [`MIN_HANDOVER`]. Empty: the model's own.
pub fn parse_handover(text: &str) -> Result<Option<usize>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let tokens = parse_size(text)
        .ok_or_else(|| "Give a number of tokens, such as 200000, 950k or 1.2M.".to_string())?;
    if tokens < MIN_HANDOVER {
        return Err(format!(
            "At least {}k: a fresh conversation would start near it.",
            MIN_HANDOVER / 1000
        ));
    }
    Ok(Some(tokens))
}

/// A number of tokens with `k` or `M` for thousands or millions, and
/// decimals with either, rounded to whole tokens ("1.2M" is 1,200,000).
fn parse_size(text: &str) -> Option<usize> {
    let text: String = text.chars().filter(|c| !matches!(c, ',' | '_')).collect();
    let text = text.trim();
    let (number, scale) = if let Some(number) = text.strip_suffix(['k', 'K']) {
        (number, 1_000u128)
    } else if let Some(number) = text.strip_suffix(['m', 'M']) {
        (number, 1_000_000)
    } else {
        (text, 1)
    };
    let number = number.trim();
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    let digits = |part: &str| part.chars().all(|c| c.is_ascii_digit());
    // Tokens are whole: decimals only with k or M.
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || (scale == 1 && number.contains('.'))
    {
        return None;
    }
    let whole: u128 = if whole.is_empty() {
        0
    } else {
        whole.parse().ok()?
    };
    // The fraction's digits past what a token can show round away.
    let fraction = &fraction[..fraction.len().min(12)];
    let part = if fraction.is_empty() {
        0
    } else {
        let tenths = 10u128.pow(fraction.len() as u32);
        (fraction.parse::<u128>().ok()? * scale * 2 + tenths) / (2 * tenths)
    };
    usize::try_from(whole.checked_mul(scale)?.checked_add(part)?).ok()
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
    let text = text.trim().to_ascii_lowercase();
    if text.is_empty() {
        return Ok(None);
    }
    if text == "off" {
        return Ok(Some(0));
    }
    let invalid = || {
        format!(
            "Give a time such as 90s, 10m or 1h (at most {}m), or off.",
            MAX_BACKGROUND_AFTER / 60
        )
    };
    let (number, unit) = text.split_at(text.len().saturating_sub(1));
    let scale = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        _ => return Err(invalid()),
    };
    let seconds = number
        .trim()
        .parse::<u64>()
        .ok()
        .and_then(|number| number.checked_mul(scale))
        .filter(|seconds| (1..=MAX_BACKGROUND_AFTER).contains(seconds))
        .ok_or_else(invalid)?;
    Ok(Some(seconds))
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
    use super::{parse_background_after, parse_handover, shown_background_after, shown_tokens};

    #[test]
    fn handover_sizes_read_as_arko_reads_them() {
        assert_eq!(parse_handover(""), Ok(None));
        assert_eq!(parse_handover("200000"), Ok(Some(200_000)));
        assert_eq!(parse_handover(" 200,000 "), Ok(Some(200_000)));
        assert_eq!(parse_handover("200_000"), Ok(Some(200_000)));
        assert_eq!(parse_handover("950k"), Ok(Some(950_000)));
        assert_eq!(parse_handover("950 K"), Ok(Some(950_000)));
        assert!(parse_handover("lots").is_err());
        assert!(parse_handover("-5").is_err());
        assert!(
            parse_handover("5k")
                .unwrap_err()
                .starts_with("At least 20k")
        );
        assert!(parse_handover("0").unwrap_err().starts_with("At least 20k"));
        assert!(parse_handover("99999999999999999999999k").is_err());
        for tokens in [20_000, 950_000, 123_456, 1_000_000, 1_200_000, 1_250_000] {
            assert_eq!(parse_handover(&shown_tokens(tokens)), Ok(Some(tokens)));
        }
    }

    /// Millions, and decimals with k or M, rounded to whole tokens.
    #[test]
    fn handover_sizes_take_millions_and_decimals() {
        assert_eq!(parse_handover("1M"), Ok(Some(1_000_000)));
        assert_eq!(parse_handover("1m"), Ok(Some(1_000_000)));
        assert_eq!(parse_handover("1.2M"), Ok(Some(1_200_000)));
        assert_eq!(parse_handover("0.95M"), Ok(Some(950_000)));
        assert_eq!(parse_handover(".5M"), Ok(Some(500_000)));
        assert_eq!(parse_handover("250.5k"), Ok(Some(250_500)));
        assert_eq!(parse_handover("1.2345678M"), Ok(Some(1_234_568)));
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
        assert!(parse_background_after("ten").is_err());
        for seconds in [0, 90, 600, 3_600] {
            assert_eq!(
                parse_background_after(&shown_background_after(seconds)),
                Ok(Some(seconds))
            );
        }
    }
}
