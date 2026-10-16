//! The window's three width classes, and short relative times.

use std::time::SystemTime;

use chrono::{DateTime, Datelike as _, Local};
use gpui_kit::Pixels;

/// How the window lays out at its width.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidthClass {
    /// Below 700 px: the sidebar and the workers panel are sheets over the
    /// transcript, and the status bar keeps only its icons.
    Narrow,
    /// 700 to 1200 px (half a screen): the sidebar docks, the workers panel
    /// is a sheet.
    Half,
    /// 1200 px and up: both dock.
    Wide,
}

pub const NARROW_BELOW: f32 = 700.;
pub const WIDE_FROM: f32 = 1200.;

impl WidthClass {
    pub fn of(width: Pixels) -> Self {
        let width = f32::from(width);
        if width < NARROW_BELOW {
            WidthClass::Narrow
        } else if width < WIDE_FROM {
            WidthClass::Half
        } else {
            WidthClass::Wide
        }
    }

    /// Whether the sidebar docks beside the transcript (else it's a sheet).
    pub fn docks_sidebar(self) -> bool {
        self != WidthClass::Narrow
    }

    /// Whether the workers panel docks (else it's a sheet).
    pub fn docks_panel(self) -> bool {
        self == WidthClass::Wide
    }
}

/// A short time for a list: "now", "5m", "3h", "4d", "Sep 12", "Sep 2025".
pub fn relative_time(then: SystemTime, now: SystemTime) -> String {
    let seconds = now
        .duration_since(then)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0);
    const MINUTE: u64 = 60;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    match seconds {
        s if s < MINUTE => "now".to_string(),
        s if s < HOUR => format!("{}m", s / MINUTE),
        s if s < DAY => format!("{}h", s / HOUR),
        s if s < 7 * DAY => format!("{}d", s / DAY),
        _ => {
            let then: DateTime<Local> = then.into();
            let now: DateTime<Local> = now.into();
            if then.year() == now.year() {
                then.format("%b %-d").to_string()
            } else {
                then.format("%b %Y").to_string()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::px;
    use std::time::Duration;

    #[test]
    fn widths_fall_in_their_classes() {
        assert_eq!(WidthClass::of(px(400.)), WidthClass::Narrow);
        assert_eq!(WidthClass::of(px(699.)), WidthClass::Narrow);
        assert_eq!(WidthClass::of(px(700.)), WidthClass::Half);
        assert_eq!(WidthClass::of(px(1199.)), WidthClass::Half);
        assert_eq!(WidthClass::of(px(1200.)), WidthClass::Wide);
        assert!(!WidthClass::Narrow.docks_sidebar());
        assert!(WidthClass::Half.docks_sidebar() && !WidthClass::Half.docks_panel());
        assert!(WidthClass::Wide.docks_panel());
    }

    #[test]
    fn relative_times_shorten_with_age() {
        let now = SystemTime::now();
        let ago = |seconds: u64| now - Duration::from_secs(seconds);
        assert_eq!(relative_time(ago(5), now), "now");
        assert_eq!(relative_time(ago(5 * 60), now), "5m");
        assert_eq!(relative_time(ago(3 * 3600), now), "3h");
        assert_eq!(relative_time(ago(4 * 86400), now), "4d");
        // A time ahead of the clock (another machine's) reads as now.
        assert_eq!(relative_time(now + Duration::from_secs(60), now), "now");
        let older = relative_time(ago(40 * 86400), now);
        assert!(
            older.chars().next().unwrap().is_ascii_uppercase(),
            "{older}"
        );
    }
}
