//! What the status bar's parts say, worked out from a session's status and
//! the time: counts, clocks and money as people read them, the turn's
//! state, the plans' usage and the cost, each with the sentences of its
//! tooltip. Pure functions, so the bar and its popovers say the same, and
//! tests check it without a window.

use arko::remote::{Activity, CostPart, ExtraUsage, PlanStatus, PlanWindow, Reading};
use chrono::{DateTime, Datelike as _, Local, NaiveDateTime, TimeDelta, TimeZone as _};
use gpui_kit::App;

use crate::prefs::{self, HourCycle};

/// The time now, in Unix milliseconds: what every figure is worked out at.
pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// A token count, short: "820", "8.2k", "142k", "1.02M", "1M".
pub fn tokens(count: usize) -> String {
    /// `value` with at most `decimals` decimals, trailing zeros dropped.
    fn trimmed(value: f64, decimals: usize) -> String {
        let text = format!("{value:.decimals$}");
        if text.contains('.') {
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            text
        }
    }
    let count = count as f64;
    if count < 1_000. {
        return format!("{count}");
    }
    // Rounded first, so that 9,960 reads "10k", not "10.0k", and 999,600
    // reads "1M", not "1000k".
    let thousands = count / 1_000.;
    if (thousands * 10.).round() / 10. < 10. {
        return format!("{}k", trimmed(thousands, 1));
    }
    if thousands.round() < 1_000. {
        return format!("{}k", thousands.round());
    }
    format!("{}M", trimmed(count / 1_000_000., 2))
}

/// A count in full, with thousands separators: "142,318".
pub fn exact(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// A length of time on a clock: "0:05", "1:23", "12:00", "1:02:03".
pub fn clock(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Dollars, to the cent: "$1.20", and "< $0.01" for less than a cent.
pub fn dollars(amount: f64) -> String {
    if amount > 0. && amount < 0.005 {
        "< $0.01".to_string()
    } else {
        format!("${amount:.2}")
    }
}

/// A percentage, whole: "14%".
pub fn percent(value: f64) -> String {
    format!("{}%", value.round() as i64)
}

/// The user's time zone.
#[derive(Debug, Clone)]
pub enum Zone {
    /// A zone of the time zone database, which names its times ("MDT").
    Named(jiff::tz::TimeZone),
    /// The system's zone as chrono reads it, for a system whose zone the
    /// database doesn't have. Its times go without a name.
    Unnamed,
}

impl Zone {
    /// The system's zone now. It is looked up again every few minutes, so a
    /// computer that moves to another zone shows its times.
    pub fn system() -> Self {
        jiff::tz::TimeZone::try_system().map_or(Zone::Unnamed, Zone::Named)
    }

    /// The time on a wall clock at `ms` (Unix milliseconds), and the zone's
    /// abbreviation then ("MDT" in summer, "MST" in winter). Zones whose
    /// abbreviation is a number ("+04") go without one.
    fn at(&self, ms: i64) -> Option<(NaiveDateTime, Option<String>)> {
        match self {
            Zone::Named(zone) => {
                let instant = jiff::Timestamp::from_millisecond(ms).ok()?;
                let info = zone.to_offset_info(instant);
                let offset = TimeDelta::seconds(info.offset().seconds().into());
                let wall = DateTime::from_timestamp_millis(ms)?.naive_utc() + offset;
                let name = info.abbreviation();
                let named = !name.is_empty() && name.chars().all(|c| c.is_ascii_alphabetic());
                Some((wall, named.then(|| name.to_string())))
            }
            Zone::Unnamed => Some((Local.timestamp_millis_opt(ms).single()?.naive_local(), None)),
        }
    }
}

/// How times of day read: on the clock Settings › Appearance chooses, in the
/// user's zone.
#[derive(Debug, Clone)]
pub struct Times {
    pub hour_cycle: HourCycle,
    pub zone: Zone,
}

impl Times {
    /// As the preferences say, in the system's zone.
    pub fn of(cx: &App) -> Self {
        Self {
            hour_cycle: prefs::get(cx).appearance.hour_cycle,
            zone: Zone::system(),
        }
    }

    /// A time of day: "8:00PM", or "20:00" on a 24-hour clock.
    fn time_of_day(&self, at: NaiveDateTime) -> String {
        match self.hour_cycle {
            HourCycle::Twelve => at.format("%-I:%M%p").to_string(),
            HourCycle::TwentyFour => at.format("%H:%M").to_string(),
        }
    }
}

/// A time ahead, said by how far off its day is.
enum Ahead {
    /// Its time of day and zone: "8:00PM MDT".
    Today(String),
    /// Its weekday, time of day and zone: "Mon 9:00AM MDT".
    ThisWeek(String),
    /// Its date alone: "Oct 20", "Jan 4, 2027".
    Later(String),
}

/// `unix_seconds`, a time ahead of `now_ms`, as [`Ahead`] says it. Times
/// ahead name their zone, and times past don't.
fn ahead(unix_seconds: i64, now_ms: i64, times: &Times) -> Option<Ahead> {
    let (then, zone) = times.zone.at(unix_seconds.checked_mul(1000)?)?;
    let (now, _) = times.zone.at(now_ms)?;
    let time = || match &zone {
        Some(zone) => format!("{} {zone}", times.time_of_day(then)),
        None => times.time_of_day(then),
    };
    let days = (then.date() - now.date()).num_days();
    Some(if days == 0 {
        Ahead::Today(time())
    } else if (1..7).contains(&days) {
        Ahead::ThisWeek(format!("{} {}", then.format("%a"), time()))
    } else if then.year() == now.year() {
        Ahead::Later(then.format("%b %-d").to_string())
    } else {
        Ahead::Later(then.format("%b %-d, %Y").to_string())
    })
}

/// A local time ahead, for a Unix time in seconds: "8:00PM MDT" today, "Mon
/// 9:00AM MDT" within a week, else "Oct 20".
pub fn when(unix_seconds: i64, now_ms: i64, times: &Times) -> String {
    match ahead(unix_seconds, now_ms, times) {
        Some(Ahead::Today(text) | Ahead::ThisWeek(text) | Ahead::Later(text)) => text,
        None => String::new(),
    }
}

/// When a plan's window resets, after "Resets": "at 8:00PM MDT", "Mon
/// 9:00AM MDT", "on Oct 20".
pub fn resets(unix_seconds: i64, now_ms: i64, times: &Times) -> String {
    match ahead(unix_seconds, now_ms, times) {
        Some(Ahead::Today(time)) => format!("at {time}"),
        Some(Ahead::ThisWeek(text)) => text,
        Some(Ahead::Later(date)) => format!("on {date}"),
        None => String::new(),
    }
}

/// How long ago `then_ms` was at `now_ms` (Unix milliseconds) for a list of
/// what happened: "Just now", "5 min ago", then once it is more than an
/// hour back the time of day, with the date when it isn't today ("2:05PM",
/// "Oct 15 2:05PM"). A time past goes without its zone.
pub fn ago(then_ms: i64, now_ms: i64, times: &Times) -> String {
    let minutes = (now_ms - then_ms).max(0) / 60_000;
    match minutes {
        0 => "Just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        _ => {
            let (Some((then, _)), Some((now, _))) = (times.zone.at(then_ms), times.zone.at(now_ms))
            else {
                return String::new();
            };
            let time = times.time_of_day(then);
            if then.date() == now.date() {
                time
            } else if then.year() == now.year() {
                format!("{} {time}", then.format("%b %-d"))
            } else {
                format!("{} {time}", then.format("%b %-d, %Y"))
            }
        }
    }
}

/// The date and time of day at `ms` (Unix milliseconds), for a record such
/// as an export: "2026-10-17 9:30AM", or "2026-10-17 09:30" on a 24-hour
/// clock.
pub fn date_and_time(ms: i64, times: &Times) -> String {
    times
        .zone
        .at(ms)
        .map(|(at, _)| format!("{} {}", at.format("%Y-%m-%d"), times.time_of_day(at)))
        .unwrap_or_default()
}

/// What the agent does: shown at the bar's left, nothing when idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// A turn runs, since this Unix time in milliseconds (when known).
    Working { since: Option<i64> },
    /// The agent asked a question and waits for the answer.
    Waiting,
}

impl Turn {
    pub fn of(activity: &Activity) -> Option<Self> {
        if activity.question.is_some() {
            Some(Turn::Waiting)
        } else if activity.working {
            Some(Turn::Working {
                since: activity.since,
            })
        } else {
            None
        }
    }

    /// "Working 1:23", "Waiting for your answer".
    pub fn label(self, now: i64) -> String {
        match self {
            Turn::Working { since: Some(since) } => format!("Working {}", clock(now - since)),
            Turn::Working { since: None } => "Working".to_string(),
            Turn::Waiting => "Waiting for your answer".to_string(),
        }
    }
}

/// The plan's window to warn of: the fullest one at 80% or more.
pub fn plan_alert(plans: &[PlanStatus]) -> Option<(&PlanStatus, &PlanWindow)> {
    plans
        .iter()
        .flat_map(|plan| plan.windows.iter().map(move |window| (plan, window)))
        .filter(|(_, window)| window.used_percent >= 80.)
        .max_by(|a, b| a.1.used_percent.total_cmp(&b.1.used_percent))
}

/// "5 h limit 84%".
pub fn plan_label(window: &PlanWindow) -> String {
    format!("{} limit {}", window.label, percent(window.used_percent))
}

/// One window of a plan, for a tooltip or a popover: "5 h limit · 84% used,
/// resets at 8:00PM MDT".
pub fn plan_window_line(window: &PlanWindow, now: i64, times: &Times) -> String {
    let mut text = format!(
        "{} limit · {} used",
        window.label,
        percent(window.used_percent)
    );
    if let Some(at) = window.resets_at {
        text.push_str(&format!(", resets {}", resets(at, now, times)));
    }
    text
}

/// What a plan has besides its windows, in words: a ChatGPT plan's credits
/// ("credits left 12.50", "has credits", "unlimited credits") and how many
/// of its logins are set aside, a Claude plan's extra usage ("extra usage
/// $271.40 of $500.00"). `None` when it has none. Arko gives credits and
/// extra usage only when they are on.
pub fn plan_extras(plan: &PlanStatus) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(credits) = &plan.credits {
        parts.push(match credits.balance.as_deref().map(str::trim) {
            _ if credits.unlimited => "unlimited credits".to_string(),
            Some(balance) if !balance.is_empty() => format!("credits left {balance}"),
            _ => "has credits".to_string(),
        });
    }
    if let Some(extra) = &plan.extra_usage {
        parts.push(extra_usage_words(extra));
    }
    if plan.set_aside > 0 {
        parts.push(format!("{} set aside", plan.set_aside));
    }
    (!parts.is_empty()).then(|| parts.join(", "))
}

/// A Claude account's extra usage as Arko's lines say it: "extra usage
/// $271.40 of $500.00" with its amounts, "extra usage $271.40 used" without
/// a limit, "extra usage in use" or "extra usage on" when they aren't known.
/// The amounts are in minor units of the currency (cents for USD).
fn extra_usage_words(extra: &ExtraUsage) -> String {
    let places = extra.decimal_places.unwrap_or(2) as usize;
    let (before, after) = match extra.currency.as_deref() {
        None | Some("USD") => ("$", String::new()),
        Some(code) => ("", format!(" {code}")),
    };
    let amount = |minor: f64| format!("{before}{:.places$}", minor / 10_f64.powi(places as i32));
    match (extra.used_credits, extra.monthly_limit) {
        (Some(used), Some(limit)) => {
            format!("extra usage {} of {}{after}", amount(used), amount(limit))
        }
        (Some(used), None) => format!("extra usage {}{after} used", amount(used)),
        _ if extra.in_use => "extra usage in use".to_string(),
        _ => "extra usage on".to_string(),
    }
}

/// The plans' windows, a plan a line: "Claude plan · 5 h limit · 84% used,
/// resets at 8:00PM MDT · 7 d limit · 40% used", and what the plan has
/// besides (see [`plan_extras`]), "· credits left 12.50, 1 set aside".
pub fn plans_tooltip(plans: &[PlanStatus], now: i64, times: &Times) -> String {
    plans
        .iter()
        .map(|plan| {
            let mut parts: Vec<String> = plan
                .windows
                .iter()
                .map(|window| plan_window_line(window, now, times))
                .collect();
            parts.extend(plan_extras(plan));
            format!("{} · {}", plan.plan, parts.join(" · "))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A part of the cost ran on a plan: its list price is more than it billed.
fn on_plan(part: &CostPart) -> bool {
    part.list_price > part.billed + 1e-9
}

/// The cost the bar shows: what was billed, else (all of it on a plan)
/// what it would cost at API prices, "≈ $3.40". Nothing before anything
/// cost.
pub fn cost_label(reading: &Reading) -> Option<String> {
    if reading.cost >= 0.005 {
        Some(dollars(reading.cost))
    } else {
        reading
            .api_price
            .filter(|price| *price >= 0.005)
            .map(|price| format!("≈ {}", dollars(price)))
    }
}

/// The cost's tooltip: billed against API prices, the main agent's part
/// and the workers'.
pub fn cost_tooltip(reading: &Reading) -> String {
    let costs = &reading.costs;
    let part = |name: &str, part: &CostPart| {
        if on_plan(part) {
            format!("{name} ≈ {} (plan)", dollars(part.list_price))
        } else {
            format!("{name} {}", dollars(part.billed))
        }
    };
    let mut lines = vec![format!("Billed {}", dollars(reading.cost))];
    if let Some(api) = reading.api_price {
        lines.push(format!("≈ {} at API prices", dollars(api)));
    }
    let workers_used = costs.workers.calls > 0;
    if workers_used {
        lines.push(format!(
            "{}, {}",
            part("main agent", &costs.main),
            part("workers", &costs.workers)
        ));
    }
    lines.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use arko::remote::Question;

    #[test]
    fn token_counts_shorten_by_size() {
        assert_eq!(tokens(820), "820");
        assert_eq!(tokens(8_200), "8.2k");
        assert_eq!(tokens(8_000), "8k");
        assert_eq!(tokens(9_960), "10k");
        assert_eq!(tokens(142_318), "142k");
        assert_eq!(tokens(999_600), "1M");
        assert_eq!(tokens(1_000_000), "1M");
        assert_eq!(tokens(1_020_000), "1.02M");
        assert_eq!(tokens(200_000), "200k");
        assert_eq!(exact(142_318), "142,318");
        assert_eq!(exact(1_000_000), "1,000,000");
        assert_eq!(exact(999), "999");
    }

    #[test]
    fn clocks_and_money_read_as_people_write_them() {
        assert_eq!(clock(5_400), "0:05");
        assert_eq!(clock(83_000), "1:23");
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(clock(-5), "0:00");
        assert_eq!(dollars(1.2), "$1.20");
        assert_eq!(dollars(0.004), "< $0.01");
        assert_eq!(dollars(0.), "$0.00");
        assert_eq!(percent(14.4), "14%");
    }

    /// Times in `zone`, a zone of the time zone database, on `hour_cycle`.
    fn times_in(zone: &str, hour_cycle: HourCycle) -> Times {
        Times {
            hour_cycle,
            zone: Zone::Named(jiff::tz::TimeZone::get(zone).unwrap()),
        }
    }

    /// Times in Denver, which keeps daylight time from March to November.
    fn denver(hour_cycle: HourCycle) -> Times {
        times_in("America/Denver", hour_cycle)
    }

    /// The Unix milliseconds of a UTC time.
    fn utc(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(year, month, day, hour, minute, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn times_ahead_say_the_day_only_when_it_isnt_today_and_name_the_zone() {
        let twelve = denver(HourCycle::Twelve);
        let twenty_four = denver(HourCycle::TwentyFour);
        // Friday, Oct 16, 2026, 9:00AM in Denver (MDT, UTC-6).
        let now = utc(2026, 10, 16, 15, 0);
        let today = utc(2026, 10, 16, 20, 0) / 1000;
        let monday = utc(2026, 10, 19, 15, 0) / 1000;
        let later = utc(2026, 10, 30, 15, 0) / 1000;
        let next_year = utc(2027, 1, 4, 15, 0) / 1000;
        assert_eq!(when(today, now, &twelve), "2:00PM MDT");
        assert_eq!(when(monday, now, &twelve), "Mon 9:00AM MDT");
        assert_eq!(when(later, now, &twelve), "Oct 30");
        assert_eq!(when(next_year, now, &twelve), "Jan 4, 2027");
        assert_eq!(resets(today, now, &twelve), "at 2:00PM MDT");
        assert_eq!(resets(monday, now, &twelve), "Mon 9:00AM MDT");
        assert_eq!(resets(later, now, &twelve), "on Oct 30");
        // A 24-hour clock keeps its own form.
        assert_eq!(when(today, now, &twenty_four), "14:00 MDT");
        assert_eq!(resets(monday, now, &twenty_four), "Mon 09:00 MDT");
    }

    /// The zone is named for the instant shown, on either side of a change
    /// to or from daylight time.
    #[test]
    fn times_ahead_name_standard_or_daylight_time_as_it_is_then() {
        let times = denver(HourCycle::Twelve);
        // Mid-winter and mid-summer, a reset at 8:00PM.
        let winter_noon = utc(2026, 1, 15, 19, 0);
        assert_eq!(
            resets(utc(2026, 1, 16, 3, 0) / 1000, winter_noon, &times),
            "at 8:00PM MST"
        );
        let summer_noon = utc(2026, 7, 15, 18, 0);
        assert_eq!(
            resets(utc(2026, 7, 16, 2, 0) / 1000, summer_noon, &times),
            "at 8:00PM MDT"
        );
        // Mar 8, 2026: the clocks go from 2:00AM MST to 3:00AM MDT.
        let spring = utc(2026, 3, 8, 8, 0);
        assert_eq!(
            resets(utc(2026, 3, 8, 8, 30) / 1000, spring, &times),
            "at 1:30AM MST"
        );
        assert_eq!(
            resets(utc(2026, 3, 9, 2, 0) / 1000, spring, &times),
            "at 8:00PM MDT"
        );
        // Nov 1, 2026: 1:30AM comes twice, first in daylight time.
        let fall = utc(2026, 11, 1, 6, 30);
        assert_eq!(
            resets(utc(2026, 11, 1, 7, 30) / 1000, fall, &times),
            "at 1:30AM MDT"
        );
        assert_eq!(
            resets(utc(2026, 11, 1, 8, 30) / 1000, fall, &times),
            "at 1:30AM MST"
        );
        let window = PlanWindow {
            label: "5 h".into(),
            used_percent: 84.,
            resets_at: Some(utc(2026, 7, 16, 2, 0) / 1000),
        };
        assert_eq!(
            plan_window_line(&window, summer_noon, &times),
            "5 h limit · 84% used, resets at 8:00PM MDT"
        );
    }

    #[test]
    fn midnight_and_noon_read_12_00am_and_12_00pm() {
        // Jul 15, 2026, at midnight and at noon in Denver.
        let midnight = utc(2026, 7, 15, 6, 0);
        let noon = utc(2026, 7, 15, 18, 0);
        let twelve = denver(HourCycle::Twelve);
        assert_eq!(when(midnight / 1000, midnight, &twelve), "12:00AM MDT");
        assert_eq!(when(noon / 1000, midnight, &twelve), "12:00PM MDT");
        assert_eq!(ago(midnight, noon, &twelve), "12:00AM");
        let twenty_four = denver(HourCycle::TwentyFour);
        assert_eq!(when(midnight / 1000, midnight, &twenty_four), "00:00 MDT");
        assert_eq!(when(noon / 1000, midnight, &twenty_four), "12:00 MDT");
    }

    #[test]
    fn recent_times_are_relative_and_older_ones_say_the_time_without_the_zone() {
        let twelve = denver(HourCycle::Twelve);
        let ago = |then: i64, now: i64| ago(then, now, &twelve);
        // Friday, Oct 16, 2026, 2:30PM in Denver.
        let now = utc(2026, 10, 16, 20, 30);
        assert_eq!(ago(now, now), "Just now");
        assert_eq!(ago(now - 59_000, now), "Just now");
        // A time ahead of the clock (another machine's) reads as now.
        assert_eq!(ago(now + 60_000, now), "Just now");
        assert_eq!(ago(now - 60_000, now), "1 min ago");
        assert_eq!(ago(now - 59 * 60_000, now), "59 min ago");
        assert_eq!(ago(now - 90 * 60_000, now), "1:00PM");
        assert_eq!(ago(now - 24 * 3_600_000, now), "Oct 15 2:30PM");
        assert_eq!(ago(now - 400 * 24 * 3_600_000, now), "Sep 11, 2025 2:30PM");
        let twenty_four = denver(HourCycle::TwentyFour);
        assert_eq!(super::ago(now - 90 * 60_000, now, &twenty_four), "13:00");
        assert_eq!(
            super::ago(now - 24 * 3_600_000, now, &twenty_four),
            "Oct 15 14:30"
        );
        // An export's stamp.
        assert_eq!(date_and_time(now, &twelve), "2026-10-16 2:30PM");
        assert_eq!(date_and_time(now, &twenty_four), "2026-10-16 14:30");
    }

    /// Without the time zone database's zone, times are the system's and go
    /// unnamed, and so do those of a zone whose abbreviation is a number.
    #[test]
    fn a_zone_without_a_lettered_name_goes_unnamed() {
        let unnamed = Times {
            hour_cycle: HourCycle::Twelve,
            zone: Zone::Unnamed,
        };
        let now = Local
            .with_ymd_and_hms(2026, 10, 16, 9, 0, 0)
            .unwrap()
            .timestamp_millis();
        let at = Local
            .with_ymd_and_hms(2026, 10, 16, 14, 0, 0)
            .unwrap()
            .timestamp();
        assert_eq!(resets(at, now, &unnamed), "at 2:00PM");
        // São Paulo calls its time "-03".
        let sao_paulo = times_in("America/Sao_Paulo", HourCycle::Twelve);
        let now = utc(2026, 10, 16, 12, 0);
        assert_eq!(
            when(utc(2026, 10, 16, 23, 0) / 1000, now, &sao_paulo),
            "8:00PM"
        );
    }

    #[test]
    fn the_turn_shows_while_it_runs_or_waits() {
        let mut activity = Activity::default();
        assert_eq!(Turn::of(&activity), None);
        activity.working = true;
        activity.since = Some(1_000);
        let turn = Turn::of(&activity).unwrap();
        assert_eq!(turn.label(84_500), "Working 1:23");
        activity.question = Some(Question {
            id: 1,
            text: "Tabs or spaces?".into(),
        });
        assert_eq!(
            Turn::of(&activity).unwrap().label(0),
            "Waiting for your answer"
        );
    }

    /// A plan with no credits, no extra usage and no login set aside.
    fn plan(name: &str, windows: Vec<(f64, &str)>) -> PlanStatus {
        PlanStatus {
            plan: name.into(),
            windows: windows
                .into_iter()
                .map(|(used_percent, label)| PlanWindow {
                    label: label.into(),
                    used_percent,
                    resets_at: None,
                })
                .collect(),
            credits: None,
            set_aside: 0,
            extra_usage: None,
            text: String::new(),
        }
    }

    #[test]
    fn plan_usage_shows_from_80_percent() {
        let plan = |windows: Vec<(f64, &str)>| plan("Claude plan", windows);
        assert_eq!(plan_alert(&[plan(vec![(79.9, "5 h"), (40., "7 d")])]), None);
        let plans = [plan(vec![(84., "5 h"), (91., "7 d")])];
        let (_, window) = plan_alert(&plans).unwrap();
        assert_eq!(plan_label(window), "7 d limit 91%");
        assert_eq!(
            plans_tooltip(&plans, 0, &denver(HourCycle::Twelve)),
            "Claude plan · 5 h limit · 84% used · 7 d limit · 91% used"
        );
    }

    /// A ChatGPT plan's credits and the logins set aside, and a Claude
    /// plan's extra usage, show with the plan's windows in Arko's words.
    #[test]
    fn what_a_plan_has_besides_its_windows_is_said() {
        use arko::remote::PlanCredits;

        let mut chatgpt = plan("ChatGPT plan", vec![(12., "5 h"), (82., "7 d")]);
        assert_eq!(plan_extras(&chatgpt), None);
        chatgpt.credits = Some(PlanCredits {
            has_credits: true,
            unlimited: false,
            balance: Some("12.50".into()),
        });
        chatgpt.set_aside = 1;
        assert_eq!(
            plan_extras(&chatgpt).as_deref(),
            Some("credits left 12.50, 1 set aside")
        );
        assert_eq!(
            plans_tooltip(
                std::slice::from_ref(&chatgpt),
                0,
                &denver(HourCycle::Twelve)
            ),
            "ChatGPT plan · 5 h limit · 12% used · 7 d limit · 82% used · credits left 12.50, 1 set aside"
        );
        let credits = |unlimited, balance: Option<&str>| {
            let mut plan = plan("ChatGPT plan", vec![(12., "5 h")]);
            plan.credits = Some(PlanCredits {
                has_credits: true,
                unlimited,
                balance: balance.map(str::to_string),
            });
            plan_extras(&plan)
        };
        assert_eq!(credits(true, None).as_deref(), Some("unlimited credits"));
        assert_eq!(credits(false, None).as_deref(), Some("has credits"));

        let mut claude = plan("Claude plan", vec![(30., "5 h")]);
        let mut extra = ExtraUsage {
            enabled: Some(true),
            ..ExtraUsage::default()
        };
        let mut said = |extra: &ExtraUsage| {
            claude.extra_usage = Some(extra.clone());
            plan_extras(&claude)
        };
        assert_eq!(said(&extra).as_deref(), Some("extra usage on"));
        extra.in_use = true;
        assert_eq!(said(&extra).as_deref(), Some("extra usage in use"));
        extra.used_credits = Some(27_140.);
        assert_eq!(said(&extra).as_deref(), Some("extra usage $271.40 used"));
        extra.monthly_limit = Some(50_000.);
        extra.currency = Some("USD".into());
        extra.decimal_places = Some(2);
        assert_eq!(
            said(&extra).as_deref(),
            Some("extra usage $271.40 of $500.00")
        );
        // A currency with no minor unit shows its code after the amounts.
        extra.used_credits = Some(3_500.);
        extra.monthly_limit = Some(10_000.);
        extra.currency = Some("JPY".into());
        extra.decimal_places = Some(0);
        assert_eq!(
            said(&extra).as_deref(),
            Some("extra usage 3500 of 10000 JPY")
        );
    }

    #[test]
    fn the_cost_shows_what_was_billed_else_the_api_price() {
        let mut reading = Reading::default();
        assert_eq!(cost_label(&reading), None, "nothing yet");
        reading.cost = 1.2;
        reading.costs.main = CostPart {
            billed: 1.0,
            list_price: 1.0,
            calls: 3,
            ..CostPart::default()
        };
        reading.costs.workers = CostPart {
            billed: 0.2,
            list_price: 0.2,
            calls: 1,
            ..CostPart::default()
        };
        assert_eq!(cost_label(&reading).as_deref(), Some("$1.20"));
        assert_eq!(
            cost_tooltip(&reading),
            "Billed $1.20 · main agent $1.00, workers $0.20"
        );
        // All on a plan.
        let mut plan = Reading {
            api_price: Some(3.4),
            ..Reading::default()
        };
        plan.costs.main = CostPart {
            list_price: 3.4,
            calls: 4,
            ..CostPart::default()
        };
        assert_eq!(cost_label(&plan).as_deref(), Some("≈ $3.40"));
        assert_eq!(cost_tooltip(&plan), "Billed $0.00 · ≈ $3.40 at API prices");
    }
}
