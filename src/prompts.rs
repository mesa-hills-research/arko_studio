//! What closing sessions asks of the user, as a dialog: whether to stop the
//! background jobs and workers still running.

use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::v_flex;
use gpui_kit::component::{ActiveTheme as _, StyledExt as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs;

/// What closing some sessions would stop: each session's title, and its
/// jobs and workers.
pub type Work = Vec<(SharedString, arko::QuitAsk)>;

/// Asks before stopping running jobs and workers: `quitting` the app, or
/// closing a session. `answer` hears Quit (`true`) or Cancel. "Don't ask
/// again" is remembered only when the user quits.
pub fn confirm_stop(
    work: Work,
    quitting: bool,
    window: &mut Window,
    cx: &mut App,
    answer: impl Fn(bool, &mut Window, &mut App) + 'static,
) {
    let answer = std::rc::Rc::new(answer);
    let dont_ask = std::rc::Rc::new(std::cell::Cell::new(false));
    let quit_label = if !quitting {
        "Close Session"
    } else if cfg!(target_os = "windows") {
        "Exit"
    } else {
        "Quit"
    };
    window.open_alert_dialog(cx, move |alert, _, cx| {
        let theme = cx.theme();
        let (muted, mono) = (theme.muted_foreground, theme.mono_font_family.clone());
        let work = work.clone();
        let (ok, cancel) = (answer.clone(), answer.clone());
        let checked = dont_ask.clone();
        let remembered = dont_ask.clone();
        let several = work.len() > 1;
        alert
            .title(if quitting {
                "Stop the work still running?"
            } else {
                "Stop this session’s work?"
            })
            .description(
                "Background jobs stop for good. Workers stop too; resuming the session lets the agent continue them.",
            )
            .content(move |content, _, _| {
                let work = work.clone();
                let checked = checked.clone();
                content.child(
                    v_flex()
                        .id("quit-work")
                        .test_support()
                        .gap_2()
                        .text_sm()
                        .children(work.iter().map(|(title, ask)| {
                            v_flex()
                                .gap_0p5()
                                .when(several, |this| {
                                    this.child(div().font_medium().child(title.clone()))
                                })
                                .when(!ask.workers.is_empty(), |this| {
                                    this.child(div().text_color(muted).child(format!(
                                        "{} {}: {}",
                                        ask.workers.len(),
                                        if ask.workers.len() == 1 { "worker" } else { "workers" },
                                        ask.workers.join(", ")
                                    )))
                                })
                                .children(ask.jobs.iter().map(|job| {
                                    div()
                                        .font_family(mono.clone())
                                        .text_xs()
                                        .text_color(muted)
                                        .truncate()
                                        .child(job.command.clone())
                                }))
                        }))
                        .child(
                            Checkbox::new("dont-ask")
                                .label("Don’t ask again")
                                .checked(checked.get())
                                .on_click(move |on, window, _| {
                                    checked.set(*on);
                                    window.refresh();
                                }),
                        ),
                )
            })
            .confirm()
            .ok_text(quit_label)
            .ok_variant(ButtonVariant::Danger)
            .on_ok(move |_, window, cx| {
                if remembered.get() {
                    prefs::update(cx, |prefs| prefs.sessions.ask_before_quit = false);
                }
                ok(true, window, cx);
                true
            })
            .on_cancel(move |_, window, cx| {
                cancel(false, window, cx);
                true
            })
    });
}
