//! Find's highlights take the theme's search colors.

use std::rc::Rc;

use gpui_kit::component::{ActiveTheme as _, Colorize as _, Theme};
use gpui_kit::{Hsla, TestAppContext, hsla};

use super::find::{
    Code, Ink, Look, MATCH_CONTRAST, MarkdownInks, TableRow, Text, composite, readable_on,
};
use gpui_kit::component::contrast_ratio;

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
/// sidebar's over it in the workers panel), in the user's bubble, in a
/// call's details, whose fill may be translucent, and in a call's line and
/// on a worker's card, normally and hovered. In Markdown too, a reply's and
/// a thinking's, in their text, quotes and links, in inline code and code
/// blocks on their own backgrounds, and in a table's body and head, which
/// GPUI Kit paints on the theme's surface. Hovered, it reads as
/// well as any one color can where none reads at once on both fills. A color
/// that reads there already stays as it is.
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
            // As a ghost button hovers, and as GPUI Kit's outline button
            // paints, normally and hovered.
            let hover = if theme.is_dark() {
                theme.accent.opacity(0.5)
            } else {
                theme.accent
            };
            let header = [none, hover];
            let card = [
                theme.input_background(),
                theme.input.mix_oklab(theme.transparent, 0.5),
            ];
            // Each text, and the fills under it, as the rows paint them.
            let mut inks_to_check = vec![
                ("plain", Ink::new(theme.foreground), vec![none]),
                ("muted", Ink::new(theme.muted_foreground), vec![none]),
                ("warning", Ink::new(theme.warning), vec![none]),
                ("failure", Ink::new(theme.danger), vec![none]),
                ("bubble", Ink::on(theme.foreground, bubble), vec![bubble]),
                (
                    "details",
                    Ink::on(theme.foreground, theme.muted),
                    vec![theme.muted],
                ),
                (
                    "call line",
                    super::row::call_line_ink(theme.muted_foreground, &theme),
                    header.to_vec(),
                ),
                (
                    "call failure",
                    super::row::call_line_ink(theme.danger, &theme),
                    header.to_vec(),
                ),
                (
                    "worker card",
                    super::row::worker_title_ink(&theme),
                    card.to_vec(),
                ),
            ];
            // Markdown's parts, as GPUI Kit paints them, and the look that
            // gives each its ink. A thinking's own text is muted.
            let md = MarkdownInks::of(false, cx);
            let thinking = MarkdownInks::of(true, cx);
            let code = theme.accent;
            let table = theme.popover;
            let head = composite(table, theme.table_head);
            let text = Look::default();
            let quote = Look {
                text: Text::Quote,
                ..text
            };
            let link = Look {
                text: Text::Link,
                ..text
            };
            let body = Look {
                table: Some(TableRow::Body),
                ..text
            };
            let in_head = Look {
                table: Some(TableRow::Head),
                ..text
            };
            let inline = |look: Look| Look {
                code: Some(Code::Inline),
                ..look
            };
            let block = |look: Look| Look {
                code: Some(Code::Block),
                ..look
            };
            let markdown = [
                ("reply", md, text, Ink::new(theme.foreground), none),
                (
                    "thinking",
                    thinking,
                    text,
                    Ink::new(theme.muted_foreground),
                    none,
                ),
                ("quote", md, quote, Ink::new(theme.muted_foreground), none),
                ("link", md, link, Ink::new(theme.link), none),
                (
                    "inline code",
                    md,
                    inline(text),
                    Ink::on(theme.foreground, code),
                    code,
                ),
                (
                    "inline code in a thinking",
                    thinking,
                    inline(text),
                    Ink::on(theme.muted_foreground, code),
                    code,
                ),
                (
                    "inline code in a quote",
                    md,
                    inline(quote),
                    Ink::on(theme.muted_foreground, code),
                    code,
                ),
                (
                    "inline code in a link",
                    md,
                    inline(link),
                    Ink::on(theme.link, code),
                    code,
                ),
                (
                    "code block",
                    md,
                    block(text),
                    Ink::on(theme.foreground, theme.editor_background()),
                    theme.editor_background(),
                ),
                (
                    "code block in a thinking",
                    thinking,
                    block(text),
                    Ink::on(theme.muted_foreground, theme.editor_background()),
                    theme.editor_background(),
                ),
                (
                    "code block in a quote",
                    md,
                    block(quote),
                    Ink::on(theme.muted_foreground, theme.editor_background()),
                    theme.editor_background(),
                ),
                ("table", md, body, Ink::on(theme.foreground, table), table),
                (
                    "table in a thinking",
                    thinking,
                    body,
                    Ink::on(theme.muted_foreground, table),
                    table,
                ),
                (
                    "link in a table",
                    md,
                    Look {
                        text: Text::Link,
                        ..body
                    },
                    Ink::on(theme.link, table),
                    table,
                ),
                (
                    "inline code in a table",
                    md,
                    inline(body),
                    Ink::on(theme.foreground, composite(table, code)),
                    composite(table, code),
                ),
                (
                    "table head",
                    md,
                    in_head,
                    Ink::on(theme.table_head_foreground, head),
                    head,
                ),
                (
                    "link in a table head",
                    md,
                    Look {
                        text: Text::Link,
                        ..in_head
                    },
                    Ink::on(theme.link, head),
                    head,
                ),
                (
                    "inline code in a table head",
                    md,
                    inline(in_head),
                    Ink::on(theme.table_head_foreground, composite(head, code)),
                    composite(head, code),
                ),
            ];
            for (what, inks, look, ink, fill) in markdown {
                assert_eq!(inks.ink(look), ink, "{name}, {what}");
                inks_to_check.push((what, ink, vec![fill]));
            }
            let (all, current) = super::find::match_colors(cx);
            for (what, ink, fills) in inks_to_check {
                // Painted over the window's background, or over the sidebar
                // painted over it, the first fill as the control shows
                // normally.
                let surfaces: Vec<(bool, Hsla)> =
                    [theme.background, theme.background.blend(theme.sidebar)]
                        .into_iter()
                        .flat_map(|transcript| {
                            fills
                                .iter()
                                .enumerate()
                                .map(move |(at, fill)| (at == 0, transcript.blend(*fill)))
                        })
                        .collect();
                for highlight in [all, current] {
                    // The least `text` reads at, on the normal fills or on
                    // all of them.
                    let least = |text: Hsla, normal_only: bool| {
                        surfaces
                            .iter()
                            .filter(|(normal, _)| !normal_only || *normal)
                            .map(|(_, surface)| {
                                let under = surface.blend(highlight);
                                contrast_ratio(under.blend(text), under)
                            })
                            .fold(f32::INFINITY, f32::min)
                    };
                    let text = readable_on(ink, highlight, cx);
                    assert!(
                        least(text, true) >= MATCH_CONTRAST,
                        "{name}, {what}: {:.2}",
                        least(text, true)
                    );
                    // Hovered too, as well as any color can.
                    let (black, white) = (gpui_kit::black(), gpui_kit::white());
                    let best = least(black, false).max(least(white, false));
                    assert!(
                        least(text, false) >= best.min(MATCH_CONTRAST),
                        "{name}, {what} hovered: {:.2} of {best:.2}",
                        least(text, false)
                    );
                    if least(ink.text, false) >= MATCH_CONTRAST {
                        assert_eq!(text, ink.text, "{name}, {what}");
                    }
                }
            }
        }
    });
}

/// A match's shade is worked out once for its colors, its highlight and
/// the theme, and again when the theme changes.
#[gpui_kit::test]
fn shades_are_worked_out_once_a_theme(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        let worked = || super::find::shades_worked();
        let (all, _) = super::find::match_colors(cx);
        let ink = Ink::new(cx.theme().muted_foreground);
        let before = worked();
        let shade = readable_on(ink, all, cx);
        assert_eq!(worked(), before + 1);
        assert_eq!(readable_on(ink, all, cx), shade);
        assert_eq!(worked(), before + 1, "the second is the first's");
        let themes = crate::appearance::parse_bundled();
        let other = themes
            .into_iter()
            .find(|theme| theme.name.as_ref() == "Ayu Light")
            .unwrap();
        Theme::update(cx, |theme| theme.apply_config(&Rc::new(other)));
        let (all, _) = super::find::match_colors(cx);
        readable_on(Ink::new(cx.theme().muted_foreground), all, cx);
        assert_eq!(worked(), before + 2);
    });
}
