//! About Arko Studio (Help menu, or the app menu on macOS): the version, the
//! license, what the app bundles under licenses of their own (as NOTICE
//! lists them), and links.

use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use gpui_kit::*;

const SOURCE_URL: &str = "https://github.com/mesa-hills-research/arko_studio";
const ARKO_URL: &str = "https://github.com/mesa-hills-research/arko";

/// Something Arko Studio bundles, and its license.
pub struct Credit {
    pub id: &'static str,
    pub name: &'static str,
    pub license: &'static str,
    pub url: &'static str,
}

/// What the app bundles: the fonts when it has them built in, the color
/// themes and the icons.
pub fn credits(fonts_bundled: bool) -> Vec<Credit> {
    let mut credits = Vec::new();
    if fonts_bundled {
        credits.push(Credit {
            id: "credit-fonts",
            name: "Work Sans and JetBrains Mono",
            license: "SIL Open Font License 1.1",
            url: "https://openfontlicense.org",
        });
    }
    credits.extend([
        Credit {
            id: "credit-themes",
            name: "Color themes, from GPUI Kit",
            license: "Apache License 2.0",
            url: "https://github.com/mesa-hills-research/gpui_kit",
        },
        Credit {
            id: "credit-icons",
            name: "Lucide icons",
            license: "ISC License",
            url: "https://lucide.dev",
        },
    ]);
    credits
}

/// Opens the About dialog over `window`.
pub fn open(window: &mut Window, cx: &mut App) {
    let fonts_bundled = !cx.global::<crate::fonts::Fonts>().bundled().is_empty();
    window.open_dialog(cx, move |dialog, _, _| {
        dialog.w(px(400.)).content(move |content, _, cx| {
            let muted = cx.theme().muted_foreground;
            content.child(
                v_flex()
                    .id("about")
                    .test_support()
                    .gap_4()
                    .child(
                        v_flex()
                            .gap_1()
                            .child(div().text_lg().font_semibold().child(crate::APP_NAME))
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                            ),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(format!(
                                "{}. Licensed under the MIT License or the Apache License 2.0, at your option.",
                                crate::COPYRIGHT
                            )),
                    )
                    .child(
                        v_flex()
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .font_medium()
                                    .text_color(muted)
                                    .child("Includes"),
                            )
                            .children(credits(fonts_bundled).into_iter().map(|credit| {
                                h_flex()
                                    .id(credit.id)
                                    .test_support()
                                    .gap_3()
                                    .justify_between()
                                    .text_sm()
                                    .child(credit.name)
                                    .child(
                                        Button::new(SharedString::from(format!("{}-link", credit.id)))
                                            .link()
                                            .small()
                                            .label(credit.license)
                                            .on_click(move |_, _, cx| cx.open_url(credit.url)),
                                    )
                            })),
                    )
                    .child(
                        h_flex()
                            .gap_4()
                            .child(link("source-link", "Source code", SOURCE_URL))
                            .child(link("arko-link", "Arko", ARKO_URL)),
                    ),
            )
        })
    });
}

fn link(id: &'static str, label: &'static str, url: &'static str) -> Button {
    Button::new(id)
        .link()
        .small()
        .label(label)
        .on_click(move |_, _, cx| cx.open_url(url))
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::credits;

    #[test]
    fn the_fonts_are_credited_when_they_are_built_in() {
        let ids = |fonts| -> Vec<&str> { credits(fonts).iter().map(|credit| credit.id).collect() };
        assert_eq!(ids(true), ["credit-fonts", "credit-themes", "credit-icons"]);
        assert_eq!(ids(false), ["credit-themes", "credit-icons"]);
    }
}
