//! The context ring: the main agent's context against what its model
//! takes, as a ring with its label in the status bar ("142k / 1M"), a tick
//! at the size the agent hands over at, and a warning from 90% of it. Its
//! popover splits the latest request by section, measures the context
//! against the handover size, shows what the request read from the cache
//! and wrote to it and the plans' windows, and hands over on request.

use std::sync::Arc;

use arko::remote::Reading;
use gpui_kit::component::progress::{ProgressCircle, ProgressSegment, UsageBar};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::figures::{self, exact, tokens};
use crate::live::LiveSession;
use crate::status_bar::tabular_figures;

/// The main agent's context against what its model takes, with the size it
/// hands over (or Claude Code compacts) at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContextSize {
    /// The input of its latest request.
    pub tokens: usize,
    /// What its model takes, when known.
    pub window: Option<usize>,
    /// The size it hands over at, when known.
    pub handover_at: Option<usize>,
    /// Claude Code compacts the conversation at that size.
    pub compacts: bool,
    /// A chat: the model writes a summary at that size, and the chat goes
    /// on from it.
    pub chat: bool,
}

impl ContextSize {
    /// The figures of `reading`, once a request has reported a size.
    pub fn of(reading: &Reading) -> Option<Self> {
        let tokens = reading.context_tokens?;
        let handover_at = Some(reading.settings.handover_at).filter(|at| *at > 0);
        Some(Self {
            tokens,
            window: reading.context_window.filter(|window| *window > 0),
            handover_at,
            compacts: reading.models.main.starts_with("cc:") && reading.settings.cc_compact,
            chat: false,
        })
    }

    /// The same figures for a chat (`true`), which summarizes at the
    /// handover size.
    pub fn in_chat(mut self, chat: bool) -> Self {
        self.chat = chat;
        self
    }

    /// How full the window is, in percent (0-100), when the window is known.
    pub fn percent(&self) -> Option<f32> {
        self.window
            .map(|window| (self.tokens as f32 / window as f32 * 100.).clamp(0., 100.))
    }

    /// Where the handover size is on the ring, in percent of the window.
    pub fn tick(&self) -> Option<f32> {
        let (window, at) = (self.window?, self.handover_at?);
        Some((at as f32 / window as f32 * 100.).clamp(0., 100.))
    }

    /// The context is at 90% of the handover size or past it: the ring
    /// warns.
    pub fn near_handover(&self) -> bool {
        self.handover_at
            .is_some_and(|at| self.tokens.saturating_mul(10) >= at.saturating_mul(9))
    }

    /// "142k / 1M", or "142k" when the window isn't known.
    pub fn label(&self) -> String {
        match self.window {
            Some(window) => format!("{} / {}", tokens(self.tokens), tokens(window)),
            None => tokens(self.tokens),
        }
    }

    /// What the agent does at the handover size: "hands over at 950k",
    /// "compacts at 950k", "summarizes at 950k".
    pub fn at_handover(&self) -> Option<String> {
        let verb = if self.compacts {
            "compacts"
        } else if self.chat {
            "summarizes"
        } else {
            "hands over"
        };
        self.handover_at
            .map(|at| format!("{verb} at {}", tokens(at)))
    }

    /// The exact figures: "142,318 of 1,000,000 tokens · 14% · hands over
    /// at 950k".
    pub fn exact(&self) -> String {
        let mut parts = vec![match self.window {
            Some(window) => format!("{} of {} tokens", exact(self.tokens), exact(window)),
            None => format!("{} tokens", exact(self.tokens)),
        }];
        if let Some(percent) = self.percent() {
            parts.push(figures::percent(percent as f64));
        }
        parts.extend(self.at_handover());
        parts.join(" · ")
    }
}

/// The estimated split of the main agent's latest request, in tokens,
/// scaled to the size its provider counted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Split {
    pub instructions: usize,
    pub conversation: usize,
    pub tool_results: usize,
    pub images: usize,
}

impl Split {
    /// The split of `reading`'s latest request, when it measured one: its
    /// sections in bytes turned into tokens, then scaled so they add up to
    /// the context's size (the provider's count).
    pub fn of(reading: &Reading) -> Option<Self> {
        let request = reading.last_request.as_ref()?;
        let sizes = request.sizes.as_ref()?;
        let per_token = if request.bytes_per_token > 0. {
            request.bytes_per_token
        } else {
            4.
        };
        let estimate = |bytes: usize| bytes as f64 / per_token;
        let parts = [
            estimate(sizes.system_prompt + sizes.tool_definitions),
            estimate(sizes.user_text + sizes.assistant_text + sizes.tool_calls),
            estimate(sizes.tool_results),
            sizes.image_tokens as f64,
        ];
        let sum: f64 = parts.iter().sum();
        if sum <= 0. {
            return None;
        }
        let scale = reading
            .context_tokens
            .map_or(1., |measured| measured as f64 / sum);
        let [instructions, conversation, tool_results, images] =
            parts.map(|part| (part * scale).round() as usize);
        Some(Self {
            instructions,
            conversation,
            tool_results,
            images,
        })
    }

    /// Each part with its name, in the order they are drawn.
    pub fn parts(&self) -> [(&'static str, usize); 4] {
        [
            ("System prompt and tools", self.instructions),
            ("Conversation", self.conversation),
            ("Tool results", self.tool_results),
            ("Images", self.images),
        ]
    }
}

/// The ring and its label, which open the context's popover. `details`
/// gives the popover's content as it draws, from the latest status.
pub fn context_button(
    context: ContextSize,
    details: impl Fn(&App) -> ContextDetails + 'static,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let color = if context.near_handover() {
        theme.warning
    } else {
        theme.progress_bar
    };
    let ring = ProgressCircle::new("context-ring")
        .size(rems(1.))
        .value(context.percent().unwrap_or(0.))
        .color(color)
        .when_some(context.tick(), |this, tick| this.tick(tick))
        .accessibility_label("Context")
        .accessibility_value(context.exact());
    Popover::new("context-popover")
        .anchor(Anchor::BottomRight)
        .trigger(
            Button::new("status-context")
                .ghost()
                .xsmall()
                .tooltip(context.exact())
                .child(
                    h_flex()
                        .gap_1p5()
                        .font_features(tabular_figures())
                        .when(context.window.is_some(), |this| this.child(ring))
                        .child(context.label()),
                ),
        )
        .content(move |_, _, cx| details(cx))
        .into_any_element()
}

/// The context's popover: the estimated split of the main agent's latest
/// request, the context against the handover size, what the request read
/// from the cache and wrote to it, the plans' windows, and Hand over now.
#[derive(IntoElement)]
pub struct ContextDetails {
    reading: Arc<Reading>,
    now: i64,
    /// The session Hand over now acts on (none: a preview).
    session: Option<Entity<LiveSession>>,
}

impl ContextDetails {
    pub fn new(reading: &Arc<Reading>, now: i64) -> Self {
        Self {
            reading: reading.clone(),
            now,
            session: None,
        }
    }

    /// The session whose context it is: Hand over now acts on it.
    pub fn session(mut self, session: &Entity<LiveSession>) -> Self {
        self.session = Some(session.clone());
        self
    }
}

impl RenderOnce for ContextDetails {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let reading = &self.reading;
        let chat = self
            .session
            .as_ref()
            .is_some_and(|session| session.read(cx).mode() == crate::backend::Mode::Chat);
        let context = ContextSize::of(reading).map(|context| context.in_chat(chat));
        let muted = theme.muted_foreground;
        let colors = [theme.chart_1, theme.chart_2, theme.chart_3, theme.chart_4];
        let window = context.and_then(|context| context.window);
        // Where the bars measure from: the model's window, else the handover
        // size, else the context itself.
        let whole = window
            .or(context.and_then(|context| context.handover_at))
            .or(context.map(|context| context.tokens))
            .unwrap_or(1)
            .max(1) as f32;
        let share = |tokens: usize| tokens as f32 / whole * 100.;

        let split = context.map(|context| {
            let bar = UsageBar::new("context-split", "Context").value_text(context.label());
            let bar = match Split::of(reading) {
                Some(split) => bar.segments(
                    split
                        .parts()
                        .into_iter()
                        .zip(colors)
                        .filter(|((_, tokens), _)| *tokens > 0)
                        .map(|((name, tokens), color)| {
                            ProgressSegment::new(share(tokens), color)
                                .with_label(name)
                                .with_value_text(format!("about {}", figures::tokens(tokens)))
                        }),
                ),
                None => bar.value(share(context.tokens)),
            };
            // The tick marks the handover size, which the bar under it
            // measures against.
            bar.when_some(context.tick().filter(|_| window.is_some()), |bar, tick| {
                bar.tick(tick)
            })
        });
        let handover = context.and_then(|context| {
            let at = context.handover_at?;
            let percent = (context.tokens as f32 / at as f32 * 100.).clamp(0., 100.);
            let verb = if context.compacts {
                "Until compaction"
            } else if context.chat {
                "Until the summary"
            } else {
                "Until the handover"
            };
            Some(
                UsageBar::new("context-handover", verb)
                    .small()
                    .value(percent)
                    .value_text(format!(
                        "{} / {}",
                        figures::tokens(context.tokens),
                        figures::tokens(at)
                    ))
                    .when(context.near_handover(), |bar| bar.color(theme.warning)),
            )
        });
        let cache = match (reading.cache.read, reading.cache.written) {
            (None, None) => None,
            (read, written) => Some(format!(
                "Last request’s cache · {} read · {} written",
                figures::tokens(read.unwrap_or(0)),
                figures::tokens(written.unwrap_or(0))
            )),
        };
        let plans: Vec<AnyElement> = reading
            .plans
            .iter()
            .enumerate()
            .map(|(ix, plan)| {
                v_flex()
                    .gap_2()
                    .child(div().text_xs().text_color(muted).child(plan.plan.clone()))
                    .children(plan.windows.iter().enumerate().map(|(jx, window)| {
                        UsageBar::new(
                            ("plan-window", ix * 16 + jx),
                            format!("{} limit", window.label),
                        )
                        .small()
                        .value(window.used_percent as f32)
                        .value_text(figures::percent(window.used_percent))
                        .when(window.used_percent >= 80., |bar| bar.color(theme.warning))
                        .when_some(window.resets_at, |bar, at| {
                            let over = figures::starts_over(at, self.now);
                            bar.description(format!("Starts over {over}"))
                        })
                    }))
                    // A ChatGPT plan's credits, and the logins set aside.
                    .children(figures::plan_extras(plan).map(|extras| {
                        let mut chars = extras.chars();
                        let line: String = chars
                            .next()
                            .map(|first| first.to_uppercase().chain(chars).collect())
                            .unwrap_or_default();
                        div()
                            .id(SharedString::from(format!("plan-extras-{ix}")))
                            .test_support()
                            .text_xs()
                            .text_color(muted)
                            .child(line)
                    }))
                    .into_any_element()
            })
            .collect();
        let session = self.session.clone();
        v_flex()
            .id("context-details")
            .test_support()
            .w(rems(20.))
            .gap_3()
            .children(split)
            .children(handover)
            .when_some(cache, |this, cache| {
                this.child(div().text_xs().text_color(muted).child(cache))
            })
            .children(plans)
            .child(
                h_flex().justify_end().child(
                    Button::new("hand-over-now")
                        .outline()
                        .small()
                        .label(if chat { "Summarize now" } else { "Hand over now" })
                        .tooltip(if chat {
                            "The model writes a summary, and the chat goes on from it in a fresh conversation"
                        } else {
                            "The agent writes a report and goes on from it in a fresh conversation"
                        })
                        .disabled(session.is_none())
                        .on_click(move |_, window, cx| {
                            if let Some(session) = &session {
                                crate::status_bar::run(session, "/handover", window, cx);
                            }
                        }),
                ),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{ContextSize, Split};
    use arko::remote::{LiveSettings, Models, Reading};

    fn reading(tokens: usize, window: usize, handover: usize) -> Reading {
        Reading {
            context_tokens: Some(tokens),
            context_window: Some(window),
            settings: LiveSettings {
                handover_at: handover,
                ..LiveSettings::default()
            },
            ..Reading::default()
        }
    }

    #[test]
    fn the_context_reads_against_the_window_with_the_handover_marked() {
        let context = ContextSize::of(&reading(142_318, 1_000_000, 950_000)).unwrap();
        assert_eq!(context.label(), "142k / 1M");
        assert!((context.percent().unwrap() - 14.2318).abs() < 1e-4);
        assert_eq!(context.tick(), Some(95.));
        assert!(!context.near_handover());
        assert_eq!(
            context.exact(),
            "142,318 of 1,000,000 tokens · 14% · hands over at 950k"
        );
        // From 90% of the handover size, the ring warns.
        assert!(
            ContextSize::of(&reading(855_000, 1_000_000, 950_000))
                .unwrap()
                .near_handover()
        );
        assert!(
            !ContextSize::of(&reading(854_000, 1_000_000, 950_000))
                .unwrap()
                .near_handover()
        );
        // Nothing measured yet: no ring.
        assert_eq!(ContextSize::of(&Reading::default()), None);
        // Claude Code compacting.
        let mut cc = reading(1_000, 200_000, 150_000);
        cc.models = Models {
            main: "cc:claude-opus-5-5".into(),
            ..Models::default()
        };
        cc.settings.cc_compact = true;
        assert_eq!(
            ContextSize::of(&cc).unwrap().at_handover().as_deref(),
            Some("compacts at 150k")
        );
        // No window known: the count alone.
        let mut unknown = reading(8_200, 0, 0);
        unknown.context_window = None;
        let context = ContextSize::of(&unknown).unwrap();
        assert_eq!(context.label(), "8.2k");
        assert_eq!(context.percent(), None);
        assert_eq!(context.exact(), "8,200 tokens");
    }

    /// A chat summarizes at the handover size.
    #[test]
    fn a_chat_summarizes_at_the_handover_size() {
        let context = ContextSize::of(&reading(142_318, 1_000_000, 950_000))
            .unwrap()
            .in_chat(true);
        assert_eq!(context.at_handover().as_deref(), Some("summarizes at 950k"));
        assert!(context.exact().ends_with("summarizes at 950k"));
    }

    #[test]
    fn the_split_is_scaled_to_the_measured_size() {
        let mut reading = reading(10_000, 200_000, 150_000);
        reading.last_request = Some(arko::remote::LastRequest {
            at: 0,
            model: "claude-opus-5-5".into(),
            sizes: Some(Default::default()),
            bytes_per_token: 4.,
        });
        assert_eq!(Split::of(&reading), None, "nothing measured");
        let sizes = reading
            .last_request
            .as_mut()
            .and_then(|request| request.sizes.as_mut())
            .unwrap();
        sizes.system_prompt = 4_000;
        sizes.tool_definitions = 4_000;
        sizes.user_text = 8_000;
        sizes.tool_results = 16_000;
        sizes.image_tokens = 2_000;
        // 2k + 2k + 4k + 2k estimated, 10k measured: as is.
        assert_eq!(
            Split::of(&reading),
            Some(Split {
                instructions: 2_000,
                conversation: 2_000,
                tool_results: 4_000,
                images: 2_000,
            })
        );
        // Measured at twice the estimate: each part doubles.
        reading.context_tokens = Some(20_000);
        assert_eq!(Split::of(&reading).unwrap().tool_results, 8_000);
    }
}
