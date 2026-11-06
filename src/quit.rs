//! Closing sessions, one or all of them (quitting the app). What would stop
//! is asked about first, in one dialog for every session: the background
//! jobs (they stop for good) and the workers (the agent can continue them
//! when the session resumes). Sessions with nothing running close at once.
//! If the engine finds more running than the user agreed to stop, it asks
//! again for that session.

use std::time::Duration;

use gpui_kit::component::{WindowExt as _, notification::Notification};
use gpui_kit::*;

use crate::live::{LiveEvent, LiveSession, LiveSessions};
use crate::prefs;
use crate::prompts::{self, Work};

/// How long quitting waits for the sessions to shut down (the crew waits
/// up to 12 s for its workers).
pub const QUIT_WAIT: Duration = Duration::from_secs(15);

/// The closes under way, kept until they finish.
#[derive(Default)]
struct Closings(Vec<Entity<Closing>>);

impl Global for Closings {}

/// Whether any session of this app runs (and would have to close).
pub fn any_running(cx: &App) -> bool {
    LiveSessions::all(cx)
        .iter()
        .any(|session| session.read(cx).is_running())
}

/// Quits the app: every session closes first, asking about what runs.
pub fn quit(window: &mut Window, cx: &mut App) {
    close(LiveSessions::all(cx), true, window, cx);
}

/// What runs in a window.
type InWindow = Box<dyn FnOnce(&mut Window, &mut App)>;

/// What runs once a close finished, in the window that started it.
struct Then {
    run: InWindow,
    /// What the window says didn't happen when the sessions don't close in
    /// time ("It wasn’t deleted.").
    not_done: &'static str,
}

/// Closes `sessions`, and with `quitting` quits the app once they all ended.
pub fn close(
    sessions: Vec<Entity<LiveSession>>,
    quitting: bool,
    window: &mut Window,
    cx: &mut App,
) {
    close_and(sessions, quitting, None, window, cx);
}

/// Closes `sessions` and runs `then` once they all ended. A close the user
/// cancels drops `then`, so nothing is left waiting for a later close. So
/// does one that doesn't finish within [`QUIT_WAIT`], and the window then
/// says which session didn't close, and `not_done`.
pub fn close_then(
    sessions: Vec<Entity<LiveSession>>,
    not_done: &'static str,
    window: &mut Window,
    cx: &mut App,
    then: impl FnOnce(&mut Window, &mut App) + 'static,
) {
    let then = Then {
        run: Box::new(then),
        not_done,
    };
    close_and(sessions, false, Some(then), window, cx);
}

fn close_and(
    sessions: Vec<Entity<LiveSession>>,
    quitting: bool,
    then: Option<Then>,
    window: &mut Window,
    cx: &mut App,
) {
    let running: Vec<Entity<LiveSession>> = sessions
        .into_iter()
        .filter(|session| {
            let session = session.read(cx);
            session.is_running() || *session.phase() == crate::live::Phase::Sample
        })
        .collect();
    if running.is_empty() {
        if let Some(then) = then {
            (then.run)(window, cx);
        }
        if quitting {
            cx.quit();
        }
        return;
    }
    let work: Work = running
        .iter()
        .filter_map(|session| {
            let session = session.read(cx);
            let work = session.running_work();
            (!work.jobs.is_empty() || !work.workers.is_empty())
                .then(|| (session.title().clone(), work))
        })
        .collect();
    if work.is_empty() || !prefs::get(cx).sessions.ask_before_quit {
        start(running, quitting, then, window, cx);
    } else {
        // The answer comes once, and `then` goes with it.
        let then = std::cell::RefCell::new(then);
        prompts::confirm_stop(work, quitting, window, cx, move |agreed, window, cx| {
            let then = then.take();
            if agreed {
                start(running.clone(), quitting, then, window, cx);
            }
        });
    }
}

fn start(
    sessions: Vec<Entity<LiveSession>>,
    quitting: bool,
    then: Option<Then>,
    window: &mut Window,
    cx: &mut App,
) {
    if cx.try_global::<Closings>().is_none() {
        cx.set_global(Closings::default());
    }
    let handle = window.window_handle();
    let closing = cx.new(|cx| Closing::new(sessions.clone(), quitting, then, handle, cx));
    cx.update_global::<Closings, _>(|closings, _| closings.0.push(closing.clone()));
    for session in sessions {
        session.update(cx, |session, cx| {
            let agreed = session.running_work();
            session.close(agreed, cx);
        });
    }
    closing.update(cx, |closing, cx| closing.check(cx));
}

/// Sessions closing, and whether the app quits after.
struct Closing {
    sessions: Vec<Entity<LiveSession>>,
    quitting: bool,
    /// Runs once every session ended. Dropped when the user keeps one.
    then: Option<Then>,
    window: AnyWindowHandle,
    _subscriptions: Vec<Subscription>,
    _cap: Option<Task<()>>,
}

impl Closing {
    fn new(
        sessions: Vec<Entity<LiveSession>>,
        quitting: bool,
        then: Option<Then>,
        window: AnyWindowHandle,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = sessions
            .iter()
            .map(|session| cx.subscribe(session, Self::on_event))
            .collect();
        // Neither quitting nor what waits for the close waits forever for a
        // session that doesn't end.
        let cap = (quitting || then.is_some()).then(|| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(QUIT_WAIT).await;
                this.update(cx, |this, cx| this.gave_up(cx)).ok();
            })
        });
        Self {
            sessions,
            quitting,
            then,
            window,
            _subscriptions: subscriptions,
            _cap: cap,
        }
    }

    fn on_event(
        &mut self,
        session: Entity<LiveSession>,
        event: &LiveEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            LiveEvent::Ended | LiveEvent::Failed(_) => self.check(cx),
            LiveEvent::QuitAsked => {
                if !prefs::get(cx).sessions.ask_before_quit {
                    session.update(cx, |session, cx| session.agree_to_quit(cx));
                    return;
                }
                let Some(ask) = session.read(cx).quit_ask().cloned() else {
                    return;
                };
                let work = vec![(session.read(cx).title().clone(), ask)];
                let quitting = self.quitting;
                let this = cx.entity().downgrade();
                self.window
                    .update(cx, |_, window, cx| {
                        prompts::confirm_stop(work, quitting, window, cx, move |agreed, _, cx| {
                            if agreed {
                                session.update(cx, |session, cx| session.agree_to_quit(cx));
                            } else {
                                session.update(cx, |session, cx| session.cancel_close(cx));
                                // That session stays, and so does the app.
                                this.update(cx, |this, cx| {
                                    this.sessions.retain(|kept| kept != &session);
                                    this.quitting = false;
                                    this.then = None;
                                    this.check(cx);
                                })
                                .ok();
                            }
                        });
                    })
                    .ok();
            }
            _ => {}
        }
    }

    /// Done once every session ended: the app quits, if that was the aim.
    fn check(&mut self, cx: &mut Context<Self>) {
        self.sessions
            .retain(|session| session.read(cx).is_running());
        if !self.sessions.is_empty() {
            return;
        }
        let me = cx.entity();
        cx.update_global::<Closings, _>(|closings, _| closings.0.retain(|closing| closing != &me));
        if let Some(then) = self.then.take() {
            let window = self.window;
            cx.defer(move |cx| {
                window
                    .update(cx, |_, window, cx| (then.run)(window, cx))
                    .ok();
            });
        }
        if self.quitting {
            cx.quit();
        }
    }

    /// [`QUIT_WAIT`] passed with sessions still running. Quitting quits
    /// anyway. What waited for the close doesn't run, and the window says
    /// which session didn't close.
    fn gave_up(&mut self, cx: &mut Context<Self>) {
        self.sessions
            .retain(|session| session.read(cx).is_running());
        let Some(title) = self
            .sessions
            .first()
            .map(|session| session.read(cx).title().clone())
        else {
            return;
        };
        if self.quitting {
            tracing::warn!("quitting with sessions that didn't shut down in time");
            cx.quit();
            return;
        }
        if let Some(then) = self.then.take() {
            let words = format!("“{title}” didn’t close in time. {}", then.not_done);
            self.window
                .update(cx, |_, window, cx| {
                    window.push_notification(Notification::error(words), cx)
                })
                .ok();
        }
        let me = cx.entity();
        cx.update_global::<Closings, _>(|closings, _| closings.0.retain(|closing| closing != &me));
    }
}
