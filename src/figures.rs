//! What the status bar's parts say, worked out from a session's status and
//! the time: counts, clocks and money as people read them, the turn's
//! state, the plans' usage and the cost, each with the sentences of its
//! tooltip. Pure functions, so the bar and its popovers say the same, and
//! tests check it without a window.

use arko::remote::{Activity, CostPart, ExtraUsage, PlanStatus, PlanWindow, Reading};
use chrono::{DateTime, Datelike as _, Local, TimeZone as _};

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

/// A local time of day for a Unix time in seconds: "14:00" today, "Mon
/// 09:00" within a week, else "Oct 20".
pub fn when(unix_seconds: i64, now_ms: i64) -> String {
    let (Some(then), Some(now)) = (
        Local.timestamp_opt(unix_seconds, 0).single(),
        Local.timestamp_millis_opt(now_ms).single(),
    ) else {
        return String::new();
    };
    when_local(then, now)
}

/// How long ago `then_ms` was at `now_ms` (Unix milliseconds) for a list of
/// what happened: "Just now", "5 min ago", then once it is more than an
/// hour back the time of day, with the date when it isn't today ("14:05",
/// "Oct 15 14:05").
pub fn ago(then_ms: i64, now_ms: i64) -> String {
    let minutes = (now_ms - then_ms).max(0) / 60_000;
    match minutes {
        0 => "Just now".to_string(),
        1..=59 => format!("{minutes} min ago"),
        _ => {
            let (Some(then), Some(now)) = (
                Local.timestamp_millis_opt(then_ms).single(),
                Local.timestamp_millis_opt(now_ms).single(),
            ) else {
                return String::new();
            };
            if then.date_naive() == now.date_naive() {
                then.format("%H:%M").to_string()
            } else if then.year() == now.year() {
                then.format("%b %-d %H:%M").to_string()
            } else {
                then.format("%b %-d, %Y %H:%M").to_string()
            }
        }
    }
}

fn when_local(then: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = (then.date_naive() - now.date_naive()).num_days();
    if days == 0 {
        then.format("%H:%M").to_string()
    } else if (1..7).contains(&days) {
        then.format("%a %H:%M").to_string()
    } else if then.year() == now.year() {
        then.format("%b %-d").to_string()
    } else {
        then.format("%b %-d, %Y").to_string()
    }
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
/// starts over at 14:00".
pub fn plan_window_line(window: &PlanWindow, now: i64) -> String {
    let mut text = format!(
        "{} limit · {} used",
        window.label,
        percent(window.used_percent)
    );
    if let Some(at) = window.resets_at {
        text.push_str(&format!(", starts over {}", starts_over(at, now)));
    }
    text
}

/// "at 14:00", "Mon 09:00", "on Oct 20".
pub fn starts_over(unix_seconds: i64, now: i64) -> String {
    let at = when(unix_seconds, now);
    if at.contains(':') && !at.contains(' ') {
        format!("at {at}")
    } else if at.contains(':') {
        at
    } else {
        format!("on {at}")
    }
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
/// starts over at 14:00 · 7 d limit · 40% used", and what the plan has
/// besides (see [`plan_extras`]), "· credits left 12.50, 1 set aside".
pub fn plans_tooltip(plans: &[PlanStatus], now: i64) -> String {
    plans
        .iter()
        .map(|plan| {
            let mut parts: Vec<String> = plan
                .windows
                .iter()
                .map(|window| plan_window_line(window, now))
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

    #[test]
    fn local_times_say_the_day_only_when_it_isnt_today() {
        let at = |day: u32, hour: u32| Local.with_ymd_and_hms(2026, 10, day, hour, 0, 0).unwrap();
        let now = at(16, 9);
        assert_eq!(when_local(at(16, 14), now), "14:00");
        assert_eq!(when_local(at(19, 9), now), "Mon 09:00");
        assert_eq!(when_local(at(30, 9), now), "Oct 30");
    }

    #[test]
    fn recent_times_are_relative_and_older_ones_say_the_time() {
        let now = Local
            .with_ymd_and_hms(2026, 10, 16, 14, 30, 0)
            .unwrap()
            .timestamp_millis();
        assert_eq!(ago(now, now), "Just now");
        assert_eq!(ago(now - 59_000, now), "Just now");
        // A time ahead of the clock (another machine's) reads as now.
        assert_eq!(ago(now + 60_000, now), "Just now");
        assert_eq!(ago(now - 60_000, now), "1 min ago");
        assert_eq!(ago(now - 59 * 60_000, now), "59 min ago");
        assert_eq!(ago(now - 90 * 60_000, now), "13:00");
        assert_eq!(ago(now - 24 * 3_600_000, now), "Oct 15 14:30");
        assert_eq!(ago(now - 400 * 24 * 3_600_000, now), "Sep 11, 2025 14:30");
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
            plans_tooltip(&plans, 0),
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
            plans_tooltip(std::slice::from_ref(&chatgpt), 0),
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
