//! What a line of work says and opens to, what the find reads of it, and
//! how an open thought, an open count and Messages' live line behave.

use arko::remote::{Delivery, Feed, Kind, Source};

use super::model::RowKey;
use super::tests::{item_ids, say, think};
use crate::prefs::{self, TranscriptView as View};

fn ids_of(feed: &Feed, kind: Kind) -> Vec<u64> {
    item_ids(feed)
        .into_iter()
        .filter(|(_, found)| *found == kind)
        .map(|(id, _)| id)
        .collect()
}

/// The thoughts a reader meets, one line each, cut at word ends to lengths
/// around what fits the column. Drawn in the app's fonts, each whose line
/// is cut opens.
#[cfg(target_os = "linux")]
#[test]
fn a_thought_whose_line_is_cut_opens() {
    use super::height_tests::real;
    use gpui_kit::component::ActiveTheme as _;
    use gpui_kit::{FontStyle, TextRun, font};

    const THOUGHTS: [&str; 4] = [
        "The user wants the stale cache test fixed, so I should read the test first and then look at how the cache decides that a value has expired before changing anything else in the module.",
        "Now WAIT for the build, then run cargo test --locked --lib transcript:: and look at which of the FOUR goldens changed, since the MAIN one should stay as it was before this.",
        "Wow, the migration will add MANY NEW COLUMNS: OWNER, GROUP, MODE, MTIME and CTIME, which the old rows lack, so a default must come from somewhere, and NULL is the honest one.",
        "Reading the README again to confirm the install command and the minimum supported version before I write the docs page, since the changelog and the README disagree about it today.",
    ];
    let mut cx = real::app();
    cx.update(|cx| prefs::update(cx, |prefs| prefs.view = View::Quiet));
    let feed = Feed::new("main");
    feed.user("Go", Source::App, Delivery::Turn);
    feed.turn_started();
    let mut texts = Vec::new();
    for thought in THOUGHTS {
        let ends = thought
            .char_indices()
            .filter(|(at, c)| *c == ' ' && (70..=150).contains(at))
            .map(|(at, _)| at);
        for end in ends {
            think(&feed, &thought[..end]);
            say(&feed, "main", "ok");
            texts.push(thought[..end].to_string());
        }
    }
    let width = 800.;
    let (window, view) = real::open(&mut cx, &feed, (width, 600.), true);
    real::settle(&mut cx, window);
    let thoughts = ids_of(&feed, Kind::Thinking);
    let mut cut = 0;
    for (id, text) in thoughts.iter().zip(&texts) {
        // Each line drawn, so its width is known.
        cx.update(|cx| {
            view.update(cx, |view, cx| {
                let ix = view
                    .rows
                    .iter()
                    .position(|row| row.key == RowKey::Thinking(*id))
                    .unwrap();
                view.list.scroll_to(ix, gpui_kit::px(0.));
                cx.notify();
            })
        });
        real::settle(&mut cx, window);
        let (opens, room, shaped) = cx
            .update_window(window, |_, window, cx| {
                let view = view.read(cx);
                let m = view.metrics.clone().unwrap();
                let mut italic = font(cx.theme().font_family.clone());
                italic.style = FontStyle::Italic;
                let run = TextRun {
                    len: text.len(),
                    font: italic,
                    color: gpui_kit::Hsla::default(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                let size = gpui_kit::px(m.rem * super::row::THOUGHT_SIZE);
                let shaped = window.text_system().layout_line(text, size, &[run], None);
                (
                    view.thought_opens(*id, false),
                    super::row::room(width, false, m.rem),
                    f32::from(shaped.width),
                )
            })
            .unwrap();
        if shaped > room {
            cut += 1;
            assert!(opens, "{text:?} is cut ({shaped} px in {room}) and opens");
        }
    }
    assert!(cut > 0);
}
