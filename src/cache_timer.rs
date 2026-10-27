//! The cache timer: while a session is idle and its provider says how long
//! the prompt cache keeps the conversation, the status bar counts down to
//! the moment it lapses ("cache 3:12"), warns in the last minute, then
//! says "cache expired" with what the next message re-caches. Hidden while
//! a turn's requests run (each one refreshes the cache) and where the
//! lifetime isn't known. A question waiting for the user stops the requests,
//! and the timer shows then.

use arko::remote::{Activity, CacheStatus};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::*;

use crate::figures::{self, Times, clock, dollars, when};
use crate::status_bar::tabular_figures;

/// The cache timer, while idle with a known lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheTimer {
    /// It lapses in this many milliseconds.
    Counting {
        left: i64,
    },
    Expired,
}

/// The last minute: the timer warns.
pub const CACHE_WARNING_MS: i64 = 60_000;

/// Whether the session's requests keep its prompt cache warm now: a turn
/// runs and doesn't wait for the user's answer to a question.
pub fn requests_run(activity: &Activity) -> bool {
    activity.working && activity.question.is_none()
}

impl CacheTimer {
    /// The timer for `cache`, hidden (`None`) while requests run (each one
    /// refreshes the cache, see [`requests_run`]) and where its lifetime
    /// isn't known.
    pub fn of(cache: &CacheStatus, requests_run: bool, now: i64) -> Option<Self> {
        if requests_run {
            return None;
        }
        let expires = expires_at(cache)?;
        let left = expires - now;
        Some(if left > 0 {
            CacheTimer::Counting { left }
        } else {
            CacheTimer::Expired
        })
    }

    /// "cache 3:12", "cache expired".
    pub fn label(self) -> String {
        match self {
            // The minutes and seconds left, counted up to the next second, so
            // the timer reads 0:01 until the moment it lapses.
            CacheTimer::Counting { left } => format!("cache {}", clock(left + 999)),
            CacheTimer::Expired => "cache expired".to_string(),
        }
    }

    /// In its last minute.
    pub fn warns(self) -> bool {
        matches!(self, CacheTimer::Counting { left } if left <= CACHE_WARNING_MS)
    }
}

/// When the main agent's prompt cache lapses (Unix milliseconds), when its
/// lifetime and its latest refresh are known.
pub fn expires_at(cache: &CacheStatus) -> Option<i64> {
    let lifetime = cache.lifetime?;
    let refreshed = cache.refreshed_at?;
    Some(refreshed + i64::try_from(lifetime.as_millis()).unwrap_or(i64::MAX / 2))
}

/// What a cache miss writes again: "about 180k tokens, ≈ $0.90", "about
/// 180k tokens of your Claude plan".
pub fn recache(cache: &CacheStatus) -> Option<String> {
    let tokens = cache.tokens?;
    let mut text = format!("about {} tokens", figures::tokens(tokens));
    if let Some(cost) = cache.miss_cost {
        text.push_str(&format!(", ≈ {}", dollars(cost)));
    } else if let Some(plan) = cache.on_plan {
        text.push_str(&format!(" of {plan}"));
    }
    Some(text)
}

/// The cache timer's tooltip.
pub fn cache_tooltip(timer: CacheTimer, cache: &CacheStatus, now: i64, times: &Times) -> String {
    match timer {
        CacheTimer::Counting { .. } => {
            let until = expires_at(cache)
                .map(|at| when(at.div_euclid(1000), now, times))
                .unwrap_or_default();
            let mut text = format!("The prompt cache keeps this conversation until {until}.");
            if let Some(recache) = recache(cache) {
                text.push_str(&format!(" Letting it expire re-caches {recache}."));
            }
            text
        }
        CacheTimer::Expired => match recache(cache) {
            Some(recache) => format!("The next message re-caches {recache}."),
            None => "The prompt cache has expired.".to_string(),
        },
    }
}

/// What the reminder says the two choices cost: "Refresh ≈ $0.04 · Letting
/// it expire re-caches ≈ $0.90". On a plan, in tokens.
pub fn refresh_choices(cache: &CacheStatus) -> String {
    match (cache.refresh_cost, cache.miss_cost) {
        (Some(refresh), Some(miss)) => format!(
            "Refresh ≈ {} · Letting it expire re-caches ≈ {}",
            dollars(refresh),
            dollars(miss)
        ),
        _ => match (cache.tokens, cache.on_plan) {
            (Some(tokens), Some(plan)) => format!(
                "Refresh reads {} cached tokens · Letting it expire writes them again, on {plan}",
                figures::tokens(tokens)
            ),
            (Some(tokens), None) => format!(
                "Letting it expire re-caches about {} tokens",
                figures::tokens(tokens)
            ),
            _ => String::new(),
        },
    }
}

/// What letting the cache lapse costs: "Letting it expire re-caches ≈
/// $0.90". On a plan, in tokens.
pub fn expiry_cost(cache: &CacheStatus) -> Option<String> {
    match (cache.miss_cost, cache.tokens, cache.on_plan) {
        (Some(miss), _, _) => Some(format!("Letting it expire re-caches ≈ {}", dollars(miss))),
        (None, Some(tokens), Some(plan)) => Some(format!(
            "Letting it expire writes {} tokens again, on {plan}",
            figures::tokens(tokens)
        )),
        (None, Some(tokens), None) => Some(format!(
            "Letting it expire re-caches about {} tokens",
            figures::tokens(tokens)
        )),
        (None, None, _) => None,
    }
}

/// What the reminder says while a question waits: answering keeps the
/// cache warm, and what letting it lapse costs.
pub fn answer_choices(cache: &CacheStatus) -> String {
    let mut text = "Answering before then keeps it warm".to_string();
    if let Some(expiry) = expiry_cost(cache) {
        text.push_str(" · ");
        text.push_str(&expiry);
    }
    text
}

/// The timer, with what it means in its tooltip.
pub fn cache_timer(timer: CacheTimer, cache: &CacheStatus, now: i64, cx: &App) -> AnyElement {
    let theme = cx.theme();
    // Written when it shows, not with every second's draw.
    let cache = cache.clone();
    h_flex()
        .id("status-cache")
        .test_support()
        .gap_1()
        .font_features(tabular_figures())
        .text_color(if timer.warns() {
            theme.warning
        } else {
            theme.muted_foreground
        })
        .child(Icon::new(gpui_kit::assets::IconName::Timer).xsmall())
        .child(timer.label())
        .tooltip(move |window, cx| {
            Tooltip::new(cache_tooltip(timer, &cache, now, &Times::of(cx))).build(window, cx)
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        CacheTimer, answer_choices, cache_tooltip, expiry_cost, recache, refresh_choices,
        requests_run,
    };
    use crate::figures::{Times, Zone};
    use crate::prefs::HourCycle;
    use arko::remote::{Activity, CacheStatus, Question};
    use std::time::Duration;

    fn cache(lifetime_minutes: u64, refreshed_at: i64) -> CacheStatus {
        CacheStatus {
            lifetime: Some(Duration::from_secs(lifetime_minutes * 60)),
            refreshed_at: Some(refreshed_at),
            tokens: Some(180_000),
            read: Some(170_000),
            written: Some(10_000),
            miss_cost: Some(0.9),
            refresh_cost: Some(0.04),
            on_plan: None,
        }
    }

    #[test]
    fn the_cache_timer_counts_down_while_idle() {
        let cache = cache(5, 0);
        // 3:12 left.
        let timer = CacheTimer::of(&cache, false, 300_000 - 192_000).unwrap();
        assert_eq!(timer.label(), "cache 3:12");
        assert!(!timer.warns());
        // Half a second left reads 0:01, and it warns.
        let timer = CacheTimer::of(&cache, false, 299_500).unwrap();
        assert_eq!(timer.label(), "cache 0:01");
        assert!(timer.warns());
        assert_eq!(
            CacheTimer::of(&cache, false, 300_000),
            Some(CacheTimer::Expired)
        );
        assert_eq!(CacheTimer::Expired.label(), "cache expired");
        // Hidden while working, and when the lifetime isn't known.
        assert_eq!(CacheTimer::of(&cache, true, 1_000), None);
        let unknown = CacheStatus {
            lifetime: None,
            ..cache.clone()
        };
        assert_eq!(CacheTimer::of(&unknown, false, 1_000), None);
    }

    /// A question waiting for the user's answer stops the turn's requests:
    /// the cache counts down then.
    #[test]
    fn requests_stop_while_a_question_waits() {
        let mut activity = Activity {
            working: true,
            ..Activity::default()
        };
        assert!(requests_run(&activity));
        activity.question = Some(Question {
            id: 1,
            text: "Tabs or spaces?".into(),
        });
        assert!(!requests_run(&activity));
        assert!(!requests_run(&Activity::default()));
        let cache = cache(5, 0);
        assert_eq!(
            answer_choices(&cache),
            "Answering before then keeps it warm · Letting it expire re-caches ≈ $0.90"
        );
    }

    #[test]
    fn the_cache_says_what_each_choice_costs() {
        let mut cache = cache(5, 0);
        assert_eq!(
            refresh_choices(&cache),
            "Refresh ≈ $0.04 · Letting it expire re-caches ≈ $0.90"
        );
        let times = Times {
            hour_cycle: HourCycle::Twelve,
            zone: Zone::Named(jiff::tz::TimeZone::get("America/Denver").unwrap()),
        };
        assert_eq!(
            cache_tooltip(CacheTimer::Expired, &cache, 400_000, &times),
            "The next message re-caches about 180k tokens, ≈ $0.90."
        );
        // Refreshed at 2:00PM in Denver on Oct 16, 2026, for five minutes:
        // a time ahead, so it names the zone.
        let refreshed = 1_792_180_800_000;
        let counting = cache_tooltip(
            CacheTimer::Counting { left: 300_000 },
            &self::cache(5, refreshed),
            refreshed,
            &times,
        );
        assert!(
            counting.starts_with("The prompt cache keeps this conversation until 2:05PM MDT."),
            "{counting}"
        );
        cache.miss_cost = None;
        cache.refresh_cost = None;
        cache.on_plan = Some("your Claude plan");
        assert_eq!(
            recache(&cache).as_deref(),
            Some("about 180k tokens of your Claude plan")
        );
        assert_eq!(
            refresh_choices(&cache),
            "Refresh reads 180k cached tokens · Letting it expire writes them again, on your Claude plan"
        );
        assert_eq!(
            expiry_cost(&cache).as_deref(),
            Some("Letting it expire writes 180k tokens again, on your Claude plan")
        );
    }
}
