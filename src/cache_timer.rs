//! The cache timer: while a session is idle and its provider says how long
//! the prompt cache keeps the conversation, the status bar counts down to
//! the moment it lapses ("cache 3:12"), warns in the last minute, then
//! says "cache expired" with what the next message re-caches. Hidden while
//! a turn's requests run (each one refreshes the cache) and where the
//! lifetime isn't known. A question waiting for the user stops the requests,
//! and the timer shows then. While keep-warm will refresh the cache before
//! it lapses, the timer says so instead ("cache warm until 7:00AM MDT"),
//! without the warning. A click on it opens a menu that keeps the cache
//! warm for an hour or eight, or stops keep-warm, as on the web page.
//!
//! Arko works the timer out ([`arko::cache_timer`]), counting from the
//! latest request's send time and lapsing a safety margin early, so the
//! app, the terminal and the web page say the same. The app shows it with
//! the user's clock.

use arko::cache_timer::{CacheTimer, KEEP_WARM_WAITS, recache, refresh_each};
use arko::remote::CacheStatus;
use gpui_kit::component::button::{Button, ButtonVariants as _};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::{ActiveTheme as _, Icon, Sizable as _};
use gpui_kit::*;

use crate::figures::{Times, when};
use crate::live::LiveSession;
use crate::status_bar::{run, tabular_figures};

/// The timer's words, arko's with keep-warm's end on the user's clock:
/// "cache 3:12", "cache warm until 7:00AM MDT", "cache warm, 9 refreshes
/// left".
pub fn cache_label(timer: CacheTimer, now: i64, times: &Times) -> String {
    match timer {
        CacheTimer::KeptWarm {
            ends_at: Some(at), ..
        } => format!("cache warm until {}", when(at.div_euclid(1000), now, times)),
        timer => timer.label(),
    }
}

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
            if cache.keep_warm.is_some_and(|kept| kept.waits) {
                text.push(' ');
                text.push_str(&KEEP_WARM_WAITS.replace('\'', "’"));
            }
            text
        }
        CacheTimer::KeptWarm { ends_at, left } => {
            let mut text = "Keep-warm refreshes the prompt cache before it expires".to_string();
            match (ends_at, left) {
                (Some(at), _) => text.push_str(&format!(
                    ", until {}",
                    when(at.div_euclid(1000), now, times)
                )),
                (None, Some(1)) => text.push_str(" once more"),
                (None, Some(left)) => text.push_str(&format!(", {left} more times")),
                (None, None) => {}
            }
            text.push('.');
            if let Some(each) = refresh_each(cache) {
                text.push(' ');
                text.push_str(&each);
            }
            text
        }
        _ => match recache(cache) {
            Some(recache) => format!("The next message re-caches {recache}."),
            None => "The prompt cache has expired.".to_string(),
        },
    }
}

/// The session's keep-warm, for its Session info: "On until 7:00AM MDT",
/// "On for 9 more refreshes", "Off".
pub fn keep_warm_state(cache: &CacheStatus, now: i64, times: &Times) -> String {
    let Some(kept) = cache.keep_warm else {
        return "Off".to_string();
    };
    match (kept.ends_at, kept.left) {
        (Some(at), _) => format!("On until {}", when(at.div_euclid(1000), now, times)),
        (None, Some(1)) => "On for 1 more refresh".to_string(),
        (None, Some(left)) => format!("On for {left} more refreshes"),
        (None, None) => "On".to_string(),
    }
}

/// Whose choice the session's keep-warm is, for its Session info.
pub fn keep_warm_origin(kept: &arko::keep_warm::Status) -> &'static str {
    if kept.follows_setting {
        "Set in Settings for every session."
    } else {
        "Set for this session."
    }
}

/// A choice of the timer's menu, with its name there, its name beside
/// "Keep warm" in Session info, and the command it sends.
pub type KeepWarmChoice = (&'static str, &'static str, &'static str);

/// What the timer's menu offers, Stop while keep-warm is `on`, else an hour
/// or eight of it, as on the web page.
pub fn keep_warm_choices(on: bool) -> &'static [KeepWarmChoice] {
    if on {
        &[("Stop", "Stop", "/keep-warm off")]
    } else {
        &[
            ("Keep warm for 1 hour", "For 1 hour", "/keep-warm 1h"),
            ("Keep warm for 8 hours", "For 8 hours", "/keep-warm 8h"),
        ]
    }
}

/// The timer, with what it means in its tooltip, and its menu, which sends
/// keep-warm's commands to `session`.
pub fn cache_timer(
    timer: CacheTimer,
    cache: &CacheStatus,
    now: i64,
    session: Option<Entity<LiveSession>>,
    cx: &App,
) -> AnyElement {
    let theme = cx.theme();
    let times = Times::of(cx);
    let choices = keep_warm_choices(cache.keep_warm.is_some());
    // What a refresh costs, beside the choices that start them.
    let each = refresh_each(cache).filter(|_| cache.keep_warm.is_none());
    Button::new("status-cache")
        .ghost()
        .xsmall()
        .font_features(tabular_figures())
        .text_color(if timer.warns() {
            theme.warning
        } else {
            theme.muted_foreground
        })
        .icon(Icon::new(gpui_kit::assets::IconName::Timer).xsmall())
        .label(cache_label(timer, now, &times))
        .tooltip(cache_tooltip(timer, cache, now, &times))
        .dropdown_menu_with_anchor(Anchor::BottomRight, move |mut menu, _, _| {
            for (name, _, command) in choices {
                let session = session.clone();
                menu = menu.item(
                    PopupMenuItem::new(*name)
                        .disabled(session.is_none())
                        .on_click(move |_, window, cx| {
                            if let Some(session) = &session {
                                run(session, command, window, cx);
                            }
                        }),
                );
            }
            if let Some(each) = &each {
                menu = menu
                    .item(PopupMenuItem::separator())
                    .item(PopupMenuItem::label(each.clone()));
            }
            menu
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{cache_label, cache_tooltip, keep_warm_choices, keep_warm_origin, keep_warm_state};
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
            keep_warm: None,
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

    /// While keep-warm will refresh the cache before it lapses, the timer
    /// says until when on the user's clock, or how many refreshes are left,
    /// and its tooltip what each refresh costs.
    #[test]
    fn keep_warm_says_until_when_on_the_users_clock() {
        let mut times = Times {
            hour_cycle: HourCycle::Twelve,
            zone: Zone::Named(jiff::tz::TimeZone::get("America/Denver").unwrap()),
        };
        // 2:00PM in Denver on Oct 16, 2026, kept warm for eight hours.
        let now = 1_792_180_800_000;
        let until = CacheTimer::KeptWarm {
            ends_at: Some(now + 8 * 3_600_000),
            left: None,
        };
        assert_eq!(
            cache_label(until, now, &times),
            "cache warm until 10:00PM MDT"
        );
        assert_eq!(
            cache_tooltip(until, &cache(5, now), now, &times),
            "Keep-warm refreshes the prompt cache before it expires, until 10:00PM MDT. Each refresh ≈ $0.04."
        );
        times.hour_cycle = HourCycle::TwentyFour;
        assert_eq!(
            cache_label(until, now, &times),
            "cache warm until 22:00 MDT"
        );
        // Past midnight, it names the day.
        let tomorrow = CacheTimer::KeptWarm {
            ends_at: Some(now + 12 * 3_600_000),
            left: None,
        };
        assert_eq!(
            cache_label(tomorrow, now, &times),
            "cache warm until Sat 02:00 MDT"
        );
        let counted = CacheTimer::KeptWarm {
            ends_at: None,
            left: Some(9),
        };
        assert_eq!(
            cache_label(counted, now, &times),
            "cache warm, 9 refreshes left"
        );
        assert!(!counted.warns());
        let mut on_plan = cache(5, now);
        on_plan.refresh_cost = None;
        on_plan.on_plan = Some("your Claude plan");
        assert_eq!(
            cache_tooltip(counted, &on_plan, now, &times),
            "Keep-warm refreshes the prompt cache before it expires, 9 more times. Each refresh reads 180k cached tokens of your Claude plan."
        );
        let last = CacheTimer::KeptWarm {
            ends_at: None,
            left: Some(1),
        };
        assert_eq!(cache_label(last, now, &times), "cache warm, 1 refresh left");
    }

    /// Session info says until when keep-warm keeps the cache warm, or how
    /// many refreshes it has left, and whose choice it is. While it waits for
    /// the next message, the countdown's tooltip says so.
    #[test]
    fn keep_warm_says_its_state_and_whose_choice_it_is() {
        let times = Times {
            hour_cycle: HourCycle::Twelve,
            zone: Zone::Named(jiff::tz::TimeZone::get("America/Denver").unwrap()),
        };
        let now = 1_792_180_800_000;
        let mut kept = arko::keep_warm::Status::default();
        kept.ends_at = Some(now + 8 * 3_600_000);
        kept.next_at = Some(now + 240_000);
        let mut cache = cache(5, now);
        cache.keep_warm = Some(kept);
        assert_eq!(keep_warm_state(&cache, now, &times), "On until 10:00PM MDT");
        assert_eq!(keep_warm_origin(&kept), "Set for this session.");
        kept.follows_setting = true;
        assert_eq!(
            keep_warm_origin(&kept),
            "Set in Settings for every session."
        );
        kept.ends_at = None;
        kept.left = Some(9);
        cache.keep_warm = Some(kept);
        assert_eq!(
            keep_warm_state(&cache, now, &times),
            "On for 9 more refreshes"
        );
        cache.keep_warm = None;
        assert_eq!(keep_warm_state(&cache, now, &times), "Off");

        kept.next_at = None;
        kept.waits = true;
        cache.keep_warm = Some(kept);
        let counting = cache_tooltip(CacheTimer::Counting { left: 60_000 }, &cache, now, &times);
        assert!(
            counting.ends_with(
                " Keep-warm waits for your next message, as Arko can’t tell how long ago the cache was used."
            ),
            "{counting}"
        );
    }

    /// The timer's menu stops keep-warm while it's on, and otherwise starts
    /// it for an hour or eight, with the web page's commands.
    #[test]
    fn the_timers_menu_offers_what_the_web_pages_does() {
        assert_eq!(
            keep_warm_choices(false),
            [
                ("Keep warm for 1 hour", "For 1 hour", "/keep-warm 1h"),
                ("Keep warm for 8 hours", "For 8 hours", "/keep-warm 8h")
            ]
        );
        assert_eq!(
            keep_warm_choices(true),
            [("Stop", "Stop", "/keep-warm off")]
        );
    }
}
