//! The look of GPUI Kit's segmented tabs, for a segmented control Studio
//! draws itself because the kit's tabs can't carry its roles and keys.
//!
//! shortcut: this mirrors the kit's private segmented-tab rule, so use the
//! kit's own helpers if it exposes them.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, Theme, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// The least contrast of the chosen segment's outline with its fill and with
/// the track.
const SEGMENT_OUTLINE_MIN_CONTRAST: f32 = 1.4;
/// The most contrast of the track's outline with the track.
const TRACK_OUTLINE_MAX_CONTRAST: f32 = 1.25;

/// The track behind the segments, as an opaque color.
fn track(theme: &Theme) -> Hsla {
    theme.background.blend(theme.tab_bar_segmented)
}

/// The fill and the outline of the chosen segment.
///
/// The fill is the window background on a light theme and a step lighter
/// than the track on a dark one. The outline is the foreground at 30% opacity
/// on a light theme and 25% on a dark one. Where a theme's colors sit close
/// together it gets stronger, until it stands out from both the fill and the
/// track.
fn selection(theme: &Theme) -> (Hsla, Hsla) {
    let track = track(theme);
    let (fill, mut alpha) = if theme.is_dark() {
        (track.blend(theme.foreground.opacity(0.16)), 0.25)
    } else {
        (theme.background, 0.3)
    };
    let faint = |alpha: f32| {
        let line = fill.blend(theme.foreground.opacity(alpha));
        contrast_ratio(line, fill).min(contrast_ratio(line, track)) < SEGMENT_OUTLINE_MIN_CONTRAST
    };
    while alpha < 1. && faint(alpha) {
        alpha = (alpha + 0.05).min(1.);
    }
    (fill, theme.foreground.opacity(alpha))
}

/// The track's hairline, in the theme's border color. A strong border is
/// faded toward the track, so no theme frames the control heavily.
fn track_outline(theme: &Theme) -> Hsla {
    let track = track(theme);
    let mut alpha = 1.;
    while alpha > 0.
        && contrast_ratio(track.blend(theme.border.opacity(alpha)), track)
            > TRACK_OUTLINE_MAX_CONTRAST
    {
        alpha = (alpha - 0.05).max(0.);
    }
    theme.border.opacity(alpha)
}

/// The WCAG 2 contrast ratio of two opaque colors, from 1 to 21.
fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    fn luminance(color: Hsla) -> f32 {
        let channel = |c: f32| {
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        let rgb = color.to_rgb();
        0.2126 * channel(rgb.r) + 0.7152 * channel(rgb.g) + 0.0722 * channel(rgb.b)
    }
    let (a, b) = (luminance(a), luminance(b));
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

/// The track, outlined, 32 px tall around its segments like the kit's default
/// segmented tabs.
pub(crate) fn track_style<E: Styled>(track: E, cx: &App) -> E {
    let theme = cx.theme();
    // The 1 px border and 3 px padding put the 24 px segments 4 px inside the
    // edge, where the kit's sit.
    track
        .p(px(3.))
        .gap(px(2.))
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(track_outline(theme))
        .bg(theme.tokens.tab_bar_segmented)
}

/// A segment, filled, outlined and lightly raised when `chosen`. Other
/// segments use muted text that brightens on hover, unless `disabled`. A
/// disabled choice keeps its fill with a fainter outline.
pub(crate) fn segment_style<E: Styled + InteractiveElement + FluentBuilder>(
    segment: E,
    chosen: bool,
    disabled: bool,
    cx: &App,
) -> E {
    let theme = cx.theme();
    let (fill, outline) = selection(theme);
    let foreground = theme.foreground;
    segment
        .h(px(24.))
        .px_3()
        .rounded((theme.radius_lg - px(2.)).max(px(0.)))
        .border_1()
        .border_color(theme.transparent)
        .text_sm()
        .text_color(if disabled {
            theme.muted_foreground.opacity(0.65)
        } else if chosen {
            foreground
        } else {
            theme.muted_foreground
        })
        .when(chosen, |this| {
            this.font_medium().bg(fill).map(|this| {
                if disabled {
                    this.border_color(outline.opacity(0.5))
                } else {
                    this.border_color(outline).shadow(raised_shadow())
                }
            })
        })
        .when(!chosen && !disabled, |this| {
            this.hover(move |style| style.text_color(foreground))
        })
}

/// A segment's label, as wide as its medium weight. The chosen segment's
/// label is medium weight, and a hidden medium copy holds every segment at
/// that width, so a new choice never moves the other segments.
pub(crate) fn segment_label(label: impl Into<SharedString>) -> Div {
    let label = label.into();
    v_flex()
        .items_center()
        .child(label.clone())
        .child(div().h_0().overflow_hidden().font_medium().child(label))
}

/// The light shadow of the chosen segment, tight because the track around it
/// is only a few pixels wider.
fn raised_shadow() -> Vec<BoxShadow> {
    let ink = hsla(0., 0., 0., 0.1);
    vec![
        BoxShadow::new(px(0.), px(1.), ink).blur_radius(px(1.5)),
        BoxShadow::new(px(0.), px(1.), ink)
            .blur_radius(px(1.))
            .spread_radius(px(-1.)),
    ]
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        SEGMENT_OUTLINE_MIN_CONTRAST, TRACK_OUTLINE_MAX_CONTRAST, contrast_ratio, selection, track,
        track_outline,
    };
    use gpui_kit::component::Theme;
    use std::rc::Rc;

    /// In every bundled theme, the chosen segment's outline stands out from
    /// its fill and from the track, and the track's own outline stays fainter
    /// than that.
    #[test]
    fn outlines_are_visible_and_subtle_in_every_bundled_theme() {
        let configs = crate::appearance::parse_bundled();
        assert!(configs.len() >= 71, "{} themes read", configs.len());
        let mut failures = Vec::new();
        for config in configs {
            let name = config.name.clone();
            let mut theme = Theme::default();
            theme.apply_config(&Rc::new(config));
            let (fill, outline) = selection(&theme);
            let track = track(&theme);
            let line = fill.blend(outline);
            for (surface, color) in [("fill", fill), ("track", track)] {
                let ratio = contrast_ratio(line, color);
                if ratio < SEGMENT_OUTLINE_MIN_CONTRAST {
                    failures.push(format!("{name}: outline on {surface} {ratio:.2}"));
                }
            }
            let ratio = contrast_ratio(track.blend(track_outline(&theme)), track);
            if ratio > TRACK_OUTLINE_MAX_CONTRAST {
                failures.push(format!("{name}: track outline {ratio:.2}"));
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
