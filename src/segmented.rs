//! The look of GPUI Kit's segmented tabs, for a segmented control Studio
//! draws itself because the kit's tabs can't carry its roles and keys.
//!
//! shortcut: this mirrors the kit's private segmented-tab rule, so use the
//! kit's own helpers if it exposes them.

use gpui_kit::component::{ActiveTheme as _, StyledExt as _, Theme, concentric_radius, v_flex};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;

/// The least contrast of the chosen segment's fill with the track.
const SEGMENT_FILL_MIN_CONTRAST: f32 = 1.15;
/// The least contrast of the chosen segment's outline with its fill and with
/// the track.
const SEGMENT_OUTLINE_MIN_CONTRAST: f32 = 1.4;
/// The least contrast of the track's outline with the window.
const TRACK_OUTLINE_MIN_CONTRAST: f32 = 1.2;
/// The most contrast of the track's outline with the window.
const TRACK_OUTLINE_MAX_CONTRAST: f32 = 2.;

/// The track behind the segments, as an opaque color. It's the window
/// background unless the theme sets `tab_bar.segmented.background`.
fn track(theme: &Theme) -> Hsla {
    theme.background.blend(theme.tab_bar_segmented)
}

/// The fill and the outline of the chosen segment.
///
/// The fill is `secondary`. Where that sits too close to the track, it's the
/// foreground at 8% opacity on a light theme and 10% on a dark one, stronger
/// where needed to stand out from the track. The outline is the foreground at
/// 30% opacity on a light theme and 25% on a dark one, stronger where needed
/// to stand out from both the fill and the track.
fn selection(theme: &Theme) -> (Hsla, Hsla) {
    let track = track(theme);
    let dark = theme.is_dark();
    let mut fill = track.blend(theme.secondary);
    if contrast_ratio(fill, track) < SEGMENT_FILL_MIN_CONTRAST {
        let mut alpha: f32 = if dark { 0.1 } else { 0.08 };
        let tint = |alpha: f32| track.blend(theme.foreground.opacity(alpha));
        while alpha < 1. && contrast_ratio(tint(alpha), track) < SEGMENT_FILL_MIN_CONTRAST {
            alpha = (alpha + 0.01).min(1.);
        }
        fill = tint(alpha);
    }
    let mut alpha: f32 = if dark { 0.25 } else { 0.3 };
    let faint = |alpha: f32| {
        let line = fill.blend(theme.foreground.opacity(alpha));
        contrast_ratio(line, fill).min(contrast_ratio(line, track)) < SEGMENT_OUTLINE_MIN_CONTRAST
    };
    while alpha < 1. && faint(alpha) {
        alpha = (alpha + 0.05).min(1.);
    }
    (fill, theme.foreground.opacity(alpha))
}

/// The track's 1 px outline, in the theme's border color. A strong border is
/// faded toward the window, so no theme frames the control heavily. A faint
/// one gives way to the foreground at the least opacity that shows.
fn track_outline(theme: &Theme) -> Hsla {
    let track = track(theme);
    let edge = |color: Hsla| contrast_ratio(track.blend(color), theme.background);
    if edge(theme.border) < TRACK_OUTLINE_MIN_CONTRAST {
        let mut alpha: f32 = 0.15;
        while alpha < 1. && edge(theme.foreground.opacity(alpha)) < TRACK_OUTLINE_MIN_CONTRAST {
            alpha = (alpha + 0.01).min(1.);
        }
        return theme.foreground.opacity(alpha);
    }
    let mut alpha: f32 = 1.;
    while alpha > 0. && edge(theme.border.opacity(alpha)) > TRACK_OUTLINE_MAX_CONTRAST {
        alpha = (alpha - 0.05).max(0.);
    }
    theme.border.opacity(alpha)
}

/// The WCAG 2 contrast ratio of two opaque colors, from 1 to 21.
pub(crate) fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
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

/// The gap between the track's outer edge and each segment's edge, on every
/// side, where the kit's default segmented tabs have it.
pub(crate) const SEGMENT_INSET: Pixels = px(4.);

/// The track, outlined, 32 px tall around its segments like the kit's default
/// segmented tabs.
pub(crate) fn track_style<E: Styled>(track: E, cx: &App) -> E {
    let theme = cx.theme();
    // The padding and the 1 px border make up the inset.
    track
        .p(SEGMENT_INSET - px(1.))
        .gap(px(2.))
        .rounded(theme.radius_lg)
        .border_1()
        .border_color(track_outline(theme))
        .bg(theme.tokens.tab_bar_segmented)
}

/// A segment, filled and outlined when `chosen`. Other
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
        // Exactly concentric with the track's corners, so it takes no minimum.
        .rounded(concentric_radius(theme.radius_lg, SEGMENT_INSET))
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
            this.font_medium().bg(fill).border_color(if disabled {
                outline.opacity(0.5)
            } else {
                outline
            })
        })
        // Every segment keeps a hover style, an empty one when chosen or
        // disabled, because GPUI keeps an element's hover state current only
        // while it has one. A segment chosen under the pointer would otherwise
        // keep the hover color once the other was chosen.
        .hover(move |style| {
            if chosen || disabled {
                style
            } else {
                style.text_color(foreground)
            }
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

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        SEGMENT_FILL_MIN_CONTRAST, SEGMENT_OUTLINE_MIN_CONTRAST, TRACK_OUTLINE_MAX_CONTRAST,
        TRACK_OUTLINE_MIN_CONTRAST, contrast_ratio, selection, track, track_outline,
    };
    use gpui_kit::component::Theme;
    use std::rc::Rc;

    /// In every bundled theme, the track's outline shows without framing the
    /// control heavily, the chosen segment's fill and outline stand out, and
    /// both labels are legible.
    #[test]
    fn segments_keep_their_contrast_in_every_bundled_theme() {
        let configs = crate::appearance::parse_bundled();
        assert!(configs.len() >= 71, "{} themes read", configs.len());
        let mut failures = Vec::new();
        for config in configs {
            let name = config.name.clone();
            let mut theme = Theme::default();
            theme.apply_config(&Rc::new(config));
            let track = track(&theme);
            let (fill, outline) = selection(&theme);
            let edge = track.blend(track_outline(&theme));
            let line = fill.blend(outline);
            let checks = [
                (
                    "track outline",
                    contrast_ratio(edge, theme.background),
                    TRACK_OUTLINE_MIN_CONTRAST,
                    TRACK_OUTLINE_MAX_CONTRAST,
                ),
                (
                    "fill",
                    contrast_ratio(fill, track),
                    SEGMENT_FILL_MIN_CONTRAST,
                    21.,
                ),
                (
                    "outline on fill",
                    contrast_ratio(line, fill),
                    SEGMENT_OUTLINE_MIN_CONTRAST,
                    21.,
                ),
                (
                    "outline on track",
                    contrast_ratio(line, track),
                    SEGMENT_OUTLINE_MIN_CONTRAST,
                    21.,
                ),
                (
                    "chosen label",
                    contrast_ratio(fill.blend(theme.foreground), fill),
                    4.5,
                    21.,
                ),
                (
                    "other labels",
                    contrast_ratio(track.blend(theme.muted_foreground), track),
                    4.5,
                    21.,
                ),
            ];
            for (what, ratio, least, most) in checks {
                if !(least..=most).contains(&ratio) {
                    failures.push(format!("{name}: {what} {ratio:.2}"));
                }
            }
        }
        assert!(failures.is_empty(), "{failures:#?}");
    }
}
