//! Find's highlights take the theme's search colors.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{TestAppContext, hsla};

use super::find::{MATCH_CONTRAST, readable_on};
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

/// The text of a warning's matches reads on its highlight in every bundled
/// theme, the current match's and the others'. A warning color that reads
/// there already stays as it is.
#[gpui_kit::test]
fn matches_on_warning_lines_read_in_every_bundled_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        for config in crate::appearance::parse_bundled() {
            let name = config.name.clone();
            Theme::update(cx, |theme| theme.apply_config(&Rc::new(config)));
            let theme = cx.theme().clone();
            let (all, current) = super::find::match_colors(cx);
            for highlight in [all, current] {
                let background = theme.background.blend(highlight);
                let contrast = |text| contrast_ratio(background.blend(text), background);
                let text = readable_on(theme.warning, highlight, cx);
                assert!(
                    contrast(text) >= MATCH_CONTRAST,
                    "{name}: {:.2}",
                    contrast(text)
                );
                if contrast(theme.warning) >= MATCH_CONTRAST {
                    assert_eq!(text, theme.warning, "{name}");
                }
            }
        }
    });
}
