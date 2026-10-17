//! Find's highlights take the theme's search colors.

use gpui_kit::component::{ActiveTheme as _, Theme};
use gpui_kit::{TestAppContext, hsla};

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
