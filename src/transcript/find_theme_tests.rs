//! Find's highlights take the theme's search colors.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{Hsla, TestAppContext, hsla};

use super::find::{Ink, MATCH_CONTRAST, readable_on};
use crate::segmented::contrast_ratio;

/// Matches are yellow, the current one stronger, unless the theme sets its
/// own colors for them.
#[gpui_kit::test]
fn find_highlights_use_the_themes_search_colors(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let yellow = cx.theme().yellow;
        assert_eq!(
            super::find::match_colors(cx),
            (yellow.opacity(0.25), yellow.opacity(0.55))
        );
        let (all, current) = (hsla(0.55, 0.8, 0.5, 0.3), hsla(0.55, 0.8, 0.5, 0.6));
        let theme = Theme::global_mut(cx);
        theme.search_match = all;
        theme.search_match_active = current;
        assert_eq!(super::find::match_colors(cx), (all, current));
    });
}

/// The text of every match reads on its highlight in every bundled theme,
/// the current match's and the others': plain and muted text, a warning's
/// and a failure's, on the transcript (the window's background or the
/// sidebar's over it in the workers panel), in the user's bubble and in a
/// call's details, whose fill may be translucent. A color that reads there
/// already stays as it is.
#[gpui_kit::test]
fn matched_text_reads_in_every_bundled_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for config in crate::appearance::parse_bundled() {
            let name = config.name.clone();
            Theme::update(cx, |theme| theme.apply_config(&Rc::new(config)));
            let theme = cx.theme().clone();
            let bubble = super::row::bubble_color(&theme);
            let none = gpui_kit::transparent_black();
            // Each text, and the fills under it, as the rows paint them.
            let inks = [
                ("plain", Ink::new(theme.foreground), none),
                ("muted", Ink::new(theme.muted_foreground), none),
                ("warning", Ink::new(theme.warning), none),
                ("failure", Ink::new(theme.danger), none),
                ("bubble", Ink::on(theme.foreground, bubble), bubble),
                (
                    "details",
                    Ink::on(theme.foreground, theme.muted),
                    theme.muted,
                ),
            ];
            let (all, current) = super::find::match_colors(cx);
            for (what, ink, fill) in inks {
                // Painted over the window's background, or over the sidebar
                // painted over it.
                let surfaces = [theme.background, theme.background.blend(theme.sidebar)]
                    .map(|transcript| transcript.blend(fill));
                for highlight in [all, current] {
                    let contrast = |text: Hsla| {
                        surfaces
                            .iter()
                            .map(|surface| {
                                let under = surface.blend(highlight);
                                contrast_ratio(under.blend(text), under)
                            })
                            .fold(f32::INFINITY, f32::min)
                    };
                    let text = readable_on(ink, highlight, cx);
                    assert!(
                        contrast(text) >= MATCH_CONTRAST,
                        "{name}, {what}: {:.2}",
                        contrast(text)
                    );
                    if contrast(ink.text) >= MATCH_CONTRAST {
                        assert_eq!(text, ink.text, "{name}, {what}");
                    }
                }
            }
        }
    });
}
