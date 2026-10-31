//! The cache timer: while a session is idle and its provider says how long
//! the prompt cache keeps the conversation, the status bar counts down to
//! the moment it lapses ("cache 3:12"), warns in the last minute, then
//! says "cache expired" with what the next message re-caches. Hidden while
//! a turn's requests run (each one refreshes the cache) and where the
//! lifetime isn't known. A question waiting for the user stops the requests,
//! and the timer shows then.
//!
//! Arko works the timer out ([`arko::cache_timer`]), counting from the
//! latest request's send time and lapsing a safety margin early, so the
//! app, the terminal and the web page say the same. The app shows it with
//! the user's clock.

use arko::cache_timer::{CacheTimer, recache};
use arko::remote::CacheStatus;
use gpui_kit::component::tooltip::{ManagedTooltipExt as _, Tooltip};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _, h_flex};
use gpui_kit::*;

use crate::figures::{Times, when};
use crate::status_bar::tabular_figures;

/// The cache timer's tooltip.
pub fn cache_tooltip(timer: CacheTimer, cache: &CacheStatus, now: i64, times: &Times) -> String {
    match timer {
        CacheTimer::Counting { .. } => {
            let until = cache
                .expires_at()
                .map(|at| when(at.div_euclid(1000), now, times))
                .unwrap_or_default();
            let mut text = format!("The prompt cache keeps this conversation until {until}.");
            if let Some(recache) = recache(cache) {
                text.push_str(&format!(" Letting it expire re-caches {recache}."));
            }
            text
        }
        _ => match recache(cache) {
            Some(recache) => format!("The next message re-caches {recache}."),
            None => "The prompt cache has expired.".to_string(),
        },
    }
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
        .managed_tooltip(move |window, cx| {
            Tooltip::new(cache_tooltip(timer, &cache, now, &Times::of(cx))).build(window, cx)
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::cache_tooltip;
    use crate::figures::{Times, Zone};
    use crate::prefs::HourCycle;
    use arko::cache_timer::CacheTimer;
    use arko::remote::CacheStatus;
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

    /// The timer lapses 30 seconds before the cache's lifetime ends, as
    /// Arko counts it.
    #[test]
    fn the_cache_timer_lapses_early_as_arko_counts_it() {
        let cache = cache(5, 0);
        let timer = CacheTimer::of(&cache, false, 0).unwrap();
        assert_eq!(timer.label(), "cache 4:30");
        assert_eq!(
            CacheTimer::of(&cache, false, 270_000),
            Some(CacheTimer::Expired)
        );
    }

    #[test]
    fn the_cache_timers_tooltip_says_until_when_and_what_a_miss_costs() {
        let times = Times {
            hour_cycle: HourCycle::Twelve,
            zone: Zone::Named(jiff::tz::TimeZone::get("America/Denver").unwrap()),
        };
        assert_eq!(
            cache_tooltip(CacheTimer::Expired, &cache(5, 0), 400_000, &times),
            "The next message re-caches about 180k tokens, ≈ $0.90."
        );
        // Refreshed at 2:00PM in Denver on Oct 16, 2026, for five minutes:
        // a time ahead, so it names the zone. It lapses 30 seconds early.
        let refreshed = 1_792_180_800_000;
        let counting = cache_tooltip(
            CacheTimer::Counting { left: 270_000 },
            &cache(5, refreshed),
            refreshed,
            &times,
        );
        assert!(
            counting.starts_with("The prompt cache keeps this conversation until 2:04PM MDT."),
            "{counting}"
        );
        let mut on_plan = cache(5, 0);
        on_plan.miss_cost = None;
        on_plan.refresh_cost = None;
        on_plan.on_plan = Some("your Claude plan");
        assert_eq!(
            cache_tooltip(CacheTimer::Expired, &on_plan, 400_000, &times),
            "The next message re-caches about 180k tokens of your Claude plan."
        );
    }
}
