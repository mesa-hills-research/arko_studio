//! Cache reminders (Settings › Notifications › Cache reminders): before a
//! session's prompt cache lapses while it is idle, a notification says so.
//! Five minutes before (for caches that last 15 minutes or more) a short
//! one, and at one minute a persistent one with a live countdown, what
//! each choice costs, and Refresh, which keeps the cache warm. While a
//! question waits for the user, the last one says that answering keeps it
//! warm instead. One pair for each idle period, and none for sessions the
//! user hasn't touched for an hour. In the background the reminders go to
//! the OS's notifications too.

use std::collections::HashMap;
use std::time::Duration;

use arko::remote::CacheStatus;
use gpui_kit::component::notification::{Notification, NotificationType};
use gpui_kit::component::{
    ActiveTheme as _, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::cache_timer::{answer_choices, expires_at, expiry_cost, refresh_choices, requests_run};
use crate::figures::{self, clock};
use crate::live::{LiveEvent, LiveSession};
use crate::notify::SessionLink;
use crate::status_bar::{Clock, tabular_figures};
use crate::{notify, prefs};

/// How long before the cache lapses the first reminder comes.
pub const EARLY_MS: i64 = 5 * 60_000;
/// How long before it lapses the last one comes, and stays.
pub const FINAL_MS: i64 = 60_000;
/// Caches that last less than this get only the last reminder.
pub const EARLY_FROM_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// The longest the reminders wait before they look again. Timers stand
/// still while the computer sleeps, and the cache's lifetime runs on: a
/// reminder due meanwhile comes this long after it wakes, at the most.
pub const RECHECK_MS: i64 = 30_000;
/// Sessions the user hadn't touched for this long when their idle period
/// started get no reminders.
pub const UNTOUCHED_MS: i64 = 60 * 60_000;

/// The reminders an idle period has had.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Sent {
    pub early: bool,
    pub last: bool,
}

/// What to do about a session's cache now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Due {
    /// The first reminder.
    Early,
    /// The last one, which stays until the cache lapses.
    Last,
    /// The cache has lapsed: the last reminder goes.
    Lapsed,
}

/// What is due for `cache` at `now` (Unix milliseconds), and how long until
/// it is worth looking again. Nothing while requests run (each one
/// refreshes the cache, see [`requests_run`]), with reminders off, or where
/// the cache's lifetime
/// isn't known. Nothing new for a session the user had left alone for an
/// hour when the cache was last refreshed (`touched_at` is when they last
/// touched it).
pub fn plan(
    cache: &CacheStatus,
    requests_run: bool,
    touched_at: i64,
    sent: Sent,
    enabled: bool,
    now: i64,
) -> (Option<Due>, Option<i64>) {
    if requests_run || !enabled {
        return (None, None);
    }
    let Some(expires) = expires_at(cache) else {
        return (None, None);
    };
    if now >= expires {
        return (sent.last.then_some(Due::Lapsed), None);
    }
    let early = cache
        .lifetime
        .is_some_and(|lifetime| lifetime >= EARLY_FROM_LIFETIME)
        .then_some(expires - EARLY_MS);
    let last = expires - FINAL_MS;
    // Up to the start of the idle period: the cache's own lifetime (an hour,
    // say) and the turn before it don't count.
    let refreshed = cache.refreshed_at.unwrap_or(now);
    let touched = refreshed - touched_at < UNTOUCHED_MS;
    let due = if now >= last && !sent.last {
        touched.then_some(Due::Last)
    } else if early.is_some_and(|early| now >= early) && !sent.early && !sent.last {
        touched.then_some(Due::Early)
    } else {
        None
    };
    // The next moment something may be due: a reminder not sent yet, or the
    // lapse, which takes the last reminder away.
    let next = [
        early.filter(|early| *early > now && !sent.early),
        Some(last).filter(|last| *last > now && !sent.last),
        (sent.last || due == Some(Due::Last)).then_some(expires),
    ]
    .into_iter()
    .flatten()
    .min()
    .map(|at| at - now);
    (due, next)
}

/// The clock the reminders read, in Unix milliseconds (tests set their
/// own).
pub struct ReminderClock(pub Clock);

impl Global for ReminderClock {}

pub(crate) fn now(cx: &App) -> i64 {
    match cx.try_global::<ReminderClock>() {
        Some(clock) => (clock.0)(),
        None => figures::now_ms(),
    }
}

/// The reminders of each session the app opened.
#[derive(Default)]
struct Reminders(HashMap<EntityId, Entity<CacheReminders>>);

impl Global for Reminders {}

/// Identifies a session's reminder among the window's notifications.
struct CacheReminder;

/// One session's reminders.
pub struct CacheReminders {
    session: WeakEntity<LiveSession>,
    /// The session's key, which tells its notifications apart.
    key: u64,
    /// The idle period the reminders sent belong to: when the cache was
    /// last refreshed.
    period: Option<i64>,
    sent: Sent,
    /// The window showing the reminder (the last one stays there, and an
    /// OS copy of either waits in the notification center).
    shown_in: Option<AnyWindowHandle>,
    /// The next reminder shows without a sound or an OS copy: it comes back
    /// after a failed Refresh.
    quiet: bool,
    _timer: Option<Task<()>>,
    _refresh: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// Reminds the user of `session`'s cache, as Settings say. Called once for
/// each session the app opens.
pub fn watch(session: &Entity<LiveSession>, cx: &mut App) -> Entity<CacheReminders> {
    let reminders = cx.new(|cx| {
        let subscriptions = vec![
            cx.observe(session, |this: &mut CacheReminders, _, cx| this.check(cx)),
            cx.subscribe(session, |this, _, event: &LiveEvent, cx| match event {
                // A turn starts a new idle period when it ends.
                LiveEvent::TurnEnded => {
                    this.sent = Sent::default();
                    this.check(cx);
                }
                // A session that ended, or never opened, has no cache left.
                LiveEvent::Ended | LiveEvent::Failed(_) => {
                    this.dismiss(cx);
                    let id = cx.entity_id();
                    cx.update_global::<Reminders, _>(|reminders, _| {
                        reminders.0.retain(|_, entity| entity.entity_id() != id)
                    });
                }
                _ => {}
            }),
            cx.observe_global::<prefs::PrefsStore>(|this, cx| this.check(cx)),
        ];
        let mut this = CacheReminders {
            session: session.downgrade(),
            key: session.read(cx).key(),
            period: None,
            sent: Sent::default(),
            shown_in: None,
            quiet: false,
            _timer: None,
            _refresh: None,
            _subscriptions: subscriptions,
        };
        this.check(cx);
        this
    });
    cx.default_global::<Reminders>()
        .0
        .insert(session.entity_id(), reminders.clone());
    reminders
}

impl CacheReminders {
    /// The reminders the idle period has had.
    pub fn sent(&self) -> Sent {
        self.sent
    }

    /// Looks at the session's cache: shows what is due, and looks again
    /// when something next may be.
    fn check(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let now = now(cx);
        let enabled = prefs::get(cx).notifications.cache_reminders;
        // A refresh of the cache (a request, a Refresh here or elsewhere)
        // starts a new idle period: what was said of the one before no
        // longer holds.
        let period = session.read(cx).reading().cache.refreshed_at;
        if period != self.period {
            tracing::debug!(
                session = %self.key,
                from = ?self.period,
                to = ?period,
                lifetime_ms = session
                    .read(cx)
                    .reading()
                    .cache
                    .lifetime
                    .map(|lifetime| lifetime.as_millis() as u64),
                now,
                "cache refreshed, a new idle period"
            );
            self.period = period;
            self.sent = Sent::default();
            self.dismiss(cx);
        }
        let (due, next, busy, asking) = {
            let live = session.read(cx);
            let busy = requests_run(live.activity());
            let (due, next) = plan(
                &live.reading().cache,
                busy,
                // On screen in front of the user, it counts as touched.
                if notify::shown_in_front(&session, cx) {
                    now
                } else {
                    live.touched_at()
                },
                self.sent,
                enabled,
                now,
            );
            (due, next, busy, live.question().is_some())
        };
        if busy || !enabled {
            // A request refreshes the cache: the reminder is out of date.
            self.dismiss(cx);
            if busy {
                self.sent = Sent::default();
            }
        }
        if due.is_some() {
            // What a reminder was worked out from, for finding out why one
            // came when it did (RUST_LOG=arko_studio::cache_reminders=debug).
            let live = session.read(cx);
            let cache = &live.reading().cache;
            tracing::debug!(
                session = %live.key(),
                title = %live.title(),
                ?due,
                now,
                lifetime_ms = cache.lifetime.map(|lifetime| lifetime.as_millis() as u64),
                refreshed_at = cache.refreshed_at,
                expires_at = expires_at(cache),
                left_ms = expires_at(cache).map(|at| at - now),
                touched_at = live.touched_at(),
                sent = ?self.sent,
                "cache reminder due"
            );
        }
        match due {
            Some(Due::Early) => {
                self.sent.early = true;
                self.remind(&session, false, asking, cx);
            }
            Some(Due::Last) => {
                self.sent = Sent {
                    early: true,
                    last: true,
                };
                self.remind(&session, true, asking, cx);
            }
            Some(Due::Lapsed) => self.dismiss(cx),
            None => {}
        }
        self._timer = next.map(|wait| {
            cx.spawn(async move |this, cx| {
                cx.background_executor()
                    .timer(Duration::from_millis(wait.clamp(1, RECHECK_MS) as u64))
                    .await;
                this.update(cx, |this, cx| this.check(cx)).ok();
            })
        });
    }

    /// Takes the reminder away, from the app and from the OS's
    /// notifications.
    fn dismiss(&mut self, cx: &mut App) {
        if let Some(window) = self.shown_in.take() {
            let key = self.key;
            window
                .update(cx, |_, window, cx| {
                    window.remove_notification1::<CacheReminder>(ElementId::Integer(key), cx)
                })
                .ok();
        }
    }

    /// Shows a reminder: the first (`last` false) or the one that stays.
    /// While a question waits (`asking`), answering it keeps the cache warm,
    /// and the session takes no Refresh.
    fn remind(
        &mut self,
        session: &Entity<LiveSession>,
        last: bool,
        asking: bool,
        cx: &mut Context<Self>,
    ) {
        let (title, cache) = {
            let live = session.read(cx);
            (live.title().to_string(), live.reading().cache.clone())
        };
        let Some(expires) = expires_at(&cache) else {
            return;
        };
        // The first reminder has no Refresh: it says what the lapse costs.
        let choices = match (asking, last) {
            (true, _) => answer_choices(&cache),
            (false, true) => refresh_choices(&cache),
            (false, false) => expiry_cost(&cache).unwrap_or_default(),
        };
        let key = self.key;
        let headline = headline(expires - now(cx));
        // Kept in the history whether or not a window shows it (the one
        // that comes back after a failed Refresh is the same reminder).
        if !self.quiet {
            let id = session.read(cx).id().cloned();
            let seen = notify::shown_in_front(session, cx);
            crate::notifications::record(
                id.as_ref(),
                &title,
                &os_text(&headline, &choices),
                seen,
                cx,
            );
        }
        let Some(window) = notify::window_for(session, cx) else {
            return;
        };
        let quiet = std::mem::take(&mut self.quiet);
        let system = !quiet && !notify::app_focused(cx);
        let this = cx.entity().downgrade();
        let link = SessionLink::of(session, cx);
        let open = link.opens();
        let countdown = last.then(|| {
            let link = link.clone();
            let choices = choices.clone();
            cx.new(|cx| Countdown::new(link, choices, expires, cx))
        });
        window
            .update(cx, |_, window, cx| {
                if system {
                    window.push_notification(
                        // The same id as the app's: taking the reminder away
                        // takes it out of the OS's notifications too, and the
                        // last one takes the place of the first.
                        Notification::new()
                            .id1::<CacheReminder>(ElementId::Integer(key))
                            .title(title)
                            .message(os_text(&headline, &choices))
                            .on_click({
                                let open = open.clone();
                                move |_, window, cx| open(window, cx)
                            })
                            .system(),
                        cx,
                    );
                }
                let note = match countdown {
                    Some(countdown) if asking => last_reminder_while_asking(key, countdown),
                    Some(countdown) => last_reminder(key, countdown, move |_, cx| {
                        this.update(cx, |this, cx| this.refresh(cx)).ok();
                    }),
                    None => first_reminder(key, link, headline, choices)
                        .on_click(move |_, window, cx| open(window, cx)),
                };
                window.push_notification(note, cx);
            })
            .ok();
        self.shown_in = Some(window);
        if !quiet {
            notify::play_sound(cx);
        }
    }

    /// Keeps the cache warm (the reminder's Refresh). The refresh starts a
    /// new idle period, with its own reminders, once the session's status
    /// says when it was.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(session) = self.session.upgrade() else {
            return;
        };
        let refreshing = session.update(cx, |session, cx| session.refresh_cache(cx));
        // The reminder goes (the window busy with the click took it away
        // already).
        let reminded = self.shown_in.is_some();
        self.dismiss(cx);
        let window = notify::window_for(&session, cx);
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let refreshed = refreshing.await;
            this.update(cx, |this, cx| {
                let message = match refreshed {
                    Ok(arko::Refreshed::OutOfBand { cost, .. }) => Some((
                        NotificationType::Info,
                        format!("Cache refreshed ({}).", figures::dollars(cost)),
                    )),
                    // A short turn keeps it warm: the transcript shows it.
                    Ok(arko::Refreshed::Message) => None,
                    Err(why) => {
                        // The reminder comes back, with its countdown, for
                        // Refresh to be tried again.
                        if reminded {
                            this.sent.last = false;
                            this.quiet = true;
                            this.check(cx);
                        }
                        Some((NotificationType::Error, refresh_failed(&why)))
                    }
                };
                let Some((kind, text)) = message else {
                    return;
                };
                let id = session.read(cx).id().cloned();
                let seen = notify::shown_in_front(&session, cx);
                let title = session.read(cx).title().to_string();
                crate::notifications::record(id.as_ref(), &title, &text, seen, cx);
                if let Some(window) = window {
                    let note = notify::session_note(&session, kind, text, cx);
                    window
                        .update(cx, |_, window, cx| window.push_notification(note, cx))
                        .ok();
                }
            })
            .ok();
        }));
    }
}

/// A reminder's headline, `left` milliseconds before the cache lapses: "The
/// prompt cache expires in 5 minutes", "… in a minute".
pub fn headline(left: i64) -> String {
    match (left.max(0) + 59_999) / 60_000 {
        0 | 1 => "The prompt cache expires in a minute".to_string(),
        minutes => format!("The prompt cache expires in {minutes} minutes"),
    }
}

/// A reminder's text for the OS's notifications, under the session's title:
/// the headline, then `text` on a line of its own.
pub fn os_text(headline: &str, text: &str) -> String {
    if text.is_empty() {
        headline.to_string()
    } else {
        format!("{headline}\n{text}")
    }
}

/// The first reminder's card: the session's link, the headline and what
/// the lapse costs under it.
#[derive(IntoElement)]
struct ReminderText {
    link: SessionLink,
    headline: String,
    text: String,
}

impl RenderOnce for ReminderText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id("cache-reminder-text")
            .test_support()
            .gap_0p5()
            .text_sm()
            .child(div().flex().min_w_0().child(self.link.button()))
            .child(div().child(self.headline))
            .when(!self.text.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.text),
                )
            })
    }
}

/// What a failed refresh says: the session's own sentence when it tells
/// what wasn't refreshed, else why, after what.
pub fn refresh_failed(why: &str) -> String {
    if why.starts_with("The cache wasn") {
        why.to_string()
    } else {
        format!("The cache wasn’t refreshed. {why}")
    }
}

/// The last reminder's text: the session, the time left, what each choice
/// costs. It counts down each second until the cache lapses.
pub struct Countdown {
    link: SessionLink,
    choices: String,
    expires: i64,
    _tick: Task<()>,
}

impl Countdown {
    /// A countdown to `expires` (Unix milliseconds) for the session `link`
    /// leads to, with `choices` under it.
    pub fn new(link: SessionLink, choices: String, expires: i64, cx: &mut Context<Self>) -> Self {
        let tick = cx.spawn(async move |this, cx| {
            loop {
                let left = match this.update(cx, |this, cx| {
                    cx.notify();
                    this.expires - now(cx)
                }) {
                    Ok(left) if left > 0 => left,
                    _ => break,
                };
                // To the next whole second of the time left.
                let wait = match left % 1000 {
                    0 => 1000,
                    part => part,
                };
                cx.background_executor()
                    .timer(Duration::from_millis(wait as u64))
                    .await;
            }
        });
        Self {
            link,
            choices,
            expires,
            _tick: tick,
        }
    }
}

impl Render for Countdown {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left = (self.expires - now(cx)).max(0);
        let theme = cx.theme();
        v_flex()
            .id("cache-countdown")
            .test_support()
            .gap_0p5()
            .text_sm()
            .child(div().flex().min_w_0().child(self.link.button()))
            .child(
                div()
                    .font_features(tabular_figures())
                    .child(format!("The prompt cache expires in {}", clock(left + 999))),
            )
            .when(!self.choices.is_empty(), |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(self.choices.clone()),
                )
            })
    }
}

/// Keeps `session`'s cache warm now (Session › Refresh Cache), from
/// `window`, saying how it went as the reminder's Refresh does.
pub fn refresh(session: &Entity<LiveSession>, window: &mut Window, cx: &mut App) {
    let reminders = cx
        .try_global::<Reminders>()
        .and_then(|reminders| reminders.0.get(&session.entity_id()).cloned())
        .unwrap_or_else(|| watch(session, cx));
    let key = session.read(cx).key();
    // (This window is busy with the command: its reminder goes through it.)
    window.remove_notification1::<CacheReminder>(ElementId::Integer(key), cx);
    reminders.update(cx, |reminders, cx| reminders.refresh(cx));
}

/// The first reminder: the session's link, the `headline` ("The prompt
/// cache expires in 5 minutes") and what the lapse costs (`text`). `key`
/// tells the session's reminder apart.
pub fn first_reminder(key: u64, link: SessionLink, headline: String, text: String) -> Notification {
    Notification::new()
        .id1::<CacheReminder>(ElementId::Integer(key))
        .with_type(NotificationType::Warning)
        .content(move |_, _, _| {
            ReminderText {
                link: link.clone(),
                headline: headline.clone(),
                text: text.clone(),
            }
            .into_any_element()
        })
}

/// The last reminder: what the prompt cache's lapse is about, a live
/// countdown with what each choice costs, and Refresh, which runs
/// `refresh` (the reminder goes first). It stays until it is closed or the
/// cache lapses. `key` tells the session's reminder apart.
pub fn last_reminder(
    key: u64,
    countdown: Entity<Countdown>,
    refresh: impl Fn(&mut Window, &mut App) + 'static,
) -> Notification {
    let refresh = std::rc::Rc::new(refresh);
    last_reminder_while_asking(key, countdown).action(move |_, _, _| {
        let refresh = refresh.clone();
        Button::new(("refresh-cache", key))
            .primary()
            .small()
            .label("Refresh")
            .on_click(move |_, window, cx| {
                // (This window is busy with the click: the reminder goes
                // through it.)
                window.remove_notification1::<CacheReminder>(ElementId::Integer(key), cx);
                refresh(window, cx);
            })
    })
}

/// The last reminder while a question waits: what the prompt cache's lapse
/// is about and a live countdown, with what answering and letting it lapse
/// do. It stays until it is closed or the cache lapses.
pub fn last_reminder_while_asking(key: u64, countdown: Entity<Countdown>) -> Notification {
    Notification::new()
        .id1::<CacheReminder>(ElementId::Integer(key))
        .with_type(NotificationType::Warning)
        .content(move |_, _, _| countdown.clone().into_any_element())
        .autohide(false)
}

/// A session's reminders (tests).
#[cfg(test)]
pub fn of(session: &Entity<LiveSession>, cx: &App) -> Option<Entity<CacheReminders>> {
    cx.try_global::<Reminders>()?
        .0
        .get(&session.entity_id())
        .cloned()
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{Due, EARLY_MS, FINAL_MS, ReminderClock, Sent, UNTOUCHED_MS, plan};
    use crate::Startup;
    use crate::backend::FakeBackend;
    use crate::live::{LiveEvent, LiveSession};
    use crate::tests::open;
    use arko::remote::{Activity, CacheStatus, Question, Reading};
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, AppContext as _, ElementId, Entity, TestAppContext};
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    fn cache(minutes: u64) -> CacheStatus {
        CacheStatus {
            lifetime: Some(Duration::from_secs(minutes * 60)),
            refreshed_at: Some(0),
            tokens: Some(180_000),
            miss_cost: Some(0.9),
            refresh_cost: Some(0.04),
            ..CacheStatus::default()
        }
    }

    const HOUR: i64 = 3_600_000;

    #[test]
    fn an_hour_long_cache_gets_both_reminders() {
        let cache = cache(60);
        let none = Sent::default();
        // At first: nothing, look again at 55 minutes.
        assert_eq!(
            plan(&cache, false, 0, none, true, 0),
            (None, Some(HOUR - EARLY_MS))
        );
        // At 55 minutes, the first, then the last at 59.
        assert_eq!(
            plan(&cache, false, 0, none, true, HOUR - EARLY_MS),
            (Some(Due::Early), Some(EARLY_MS - FINAL_MS))
        );
        let early = Sent {
            early: true,
            last: false,
        };
        assert_eq!(
            plan(&cache, false, 0, early, true, HOUR - FINAL_MS),
            (Some(Due::Last), Some(FINAL_MS))
        );
        // It lapses: the last one goes.
        let both = Sent {
            early: true,
            last: true,
        };
        assert_eq!(
            plan(&cache, false, 0, both, true, HOUR),
            (Some(Due::Lapsed), None)
        );
        assert_eq!(plan(&cache, false, 0, both, true, HOUR - 30_000).0, None);
    }

    #[test]
    fn a_5_minute_cache_gets_only_the_last_reminder() {
        let cache = cache(5);
        let none = Sent::default();
        assert_eq!(
            plan(&cache, false, 0, none, true, 0),
            (None, Some(4 * 60_000))
        );
        assert_eq!(
            plan(&cache, false, 0, none, true, 4 * 60_000).0,
            Some(Due::Last)
        );
    }

    #[test]
    fn no_reminders_while_working_off_unknown_or_untouched() {
        let cache = cache(5);
        let none = Sent::default();
        let at = 4 * 60_000 + 1;
        assert_eq!(plan(&cache, true, 0, none, true, at), (None, None));
        assert_eq!(plan(&cache, false, 0, none, false, at), (None, None));
        let unknown = CacheStatus {
            lifetime: None,
            ..cache.clone()
        };
        assert_eq!(plan(&unknown, false, 0, none, true, at), (None, None));
        // Last touched an hour before the cache was refreshed: nothing.
        assert_eq!(plan(&cache, false, -UNTOUCHED_MS, none, true, at).0, None);
        assert_eq!(
            plan(&cache, false, -UNTOUCHED_MS + 1, none, true, at).0,
            Some(Due::Last)
        );
    }

    /// The hour without the user counts up to the cache's last refresh:
    /// a long turn or an hour-long cache doesn't use it up.
    #[test]
    fn the_hour_left_alone_counts_up_to_the_idle_period() {
        let minute = 60_000;
        let hour = |refreshed_at| CacheStatus {
            refreshed_at: Some(refreshed_at),
            ..cache(60)
        };
        // Sent at 0:00, the turn's last request at 2:00: the cache lapses at
        // 62:00, the last reminder at 61:00.
        let early = Sent {
            early: true,
            last: false,
        };
        assert_eq!(
            plan(&hour(2 * minute), false, 0, early, true, 61 * minute).0,
            Some(Due::Last)
        );
        // A 6-minute turn: the first reminder at 61:00.
        assert_eq!(
            plan(
                &hour(6 * minute),
                false,
                0,
                Sent::default(),
                true,
                61 * minute
            )
            .0,
            Some(Due::Early)
        );
    }

    /// A session showing its cache's figures, its reminders on the test's
    /// clock, in a window in front.
    struct Reminded {
        window: AnyWindowHandle,
        session: Entity<LiveSession>,
        now: Rc<Cell<i64>>,
    }

    const NOW: i64 = 1_800_000_000_000;

    impl Reminded {
        fn new(cx: &mut TestAppContext, lifetime_minutes: u64, refreshed_ago: i64) -> Self {
            let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
            cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
            let (window, _) = open(cx, 1300.);
            cx.update_window(window, |_, window, _| window.activate_window())
                .unwrap();
            let now = Rc::new(Cell::new(NOW));
            let clock = now.clone();
            cx.update(|cx| cx.set_global(ReminderClock(Rc::new(move || clock.get()))));
            let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
            let reading = Reading {
                title: "Fonts".into(),
                cache: CacheStatus {
                    refreshed_at: Some(NOW - refreshed_ago),
                    ..cache(lifetime_minutes)
                },
                ..Reading::default()
            };
            session.update(cx, |session, cx| {
                session.set_touched_at(NOW);
                session.set_status(reading, Activity::default(), cx);
            });
            cx.update(|cx| super::watch(&session, cx));
            cx.run_until_parked();
            Self {
                window,
                session,
                now,
            }
        }

        /// Moves both clocks on by `ms`, a second at a time: the wall clock
        /// the reminders read, and GPUI's (its timers).
        fn advance(&self, cx: &mut TestAppContext, ms: i64) {
            let mut left = ms;
            while left > 0 {
                let step = left.min(1_000);
                left -= step;
                self.now.set(self.now.get() + step);
                cx.executor()
                    .advance_clock(Duration::from_millis(step as u64));
                cx.run_until_parked();
            }
        }

        fn sent(&self, cx: &mut TestAppContext) -> Sent {
            cx.read(|cx| super::of(&self.session, cx).unwrap().read(cx).sent())
        }

        /// The session's status says its cache was refreshed `ago`
        /// milliseconds before now.
        fn refreshed(&self, cx: &mut TestAppContext, lifetime_minutes: u64, ago: i64) {
            let reading = Reading {
                cache: CacheStatus {
                    refreshed_at: Some(self.now.get() - ago),
                    ..cache(lifetime_minutes)
                },
                ..Reading::default()
            };
            self.session.update(cx, |session, cx| {
                session.set_status(reading, Activity::default(), cx)
            });
            cx.run_until_parked();
        }

        fn click(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) {
            cx.update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window.click(id, cx);
            })
            .unwrap();
            cx.run_until_parked();
        }

        fn notes(&self, cx: &mut TestAppContext) -> usize {
            cx.update_window(self.window, |_, window, cx| window.notifications(cx).len())
                .unwrap()
        }

        fn shows(&self, cx: &mut TestAppContext, id: impl Into<ElementId>) -> bool {
            cx.update_window(self.window, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).is_some()
            })
            .unwrap()
        }
    }

    /// Each reminder is kept in the history with its session's title and
    /// what it said, the first with what the lapse costs and the last with
    /// what each choice costs. The one that comes back after a failed
    /// Refresh isn't kept again, and the failure is.
    #[gpui_kit::test]
    fn reminders_are_kept_in_the_history(cx: &mut TestAppContext) {
        use crate::notifications::{entries, unread};

        // 6 minutes left of an hour's.
        let reminded = Reminded::new(cx, 60, 54 * 60_000);
        reminded.advance(cx, 60_000);
        reminded.advance(cx, 4 * 60_000);
        let kept = cx.read(|cx| entries(cx).to_vec());
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].title, "Fonts");
        assert_eq!(kept[0].session.as_deref(), Some("s1"));
        assert_eq!(
            kept[0].message,
            "The prompt cache expires in 5 minutes\nLetting it expire re-caches ≈ $0.90"
        );
        assert_eq!(
            kept[1].message,
            "The prompt cache expires in a minute\nRefresh ≈ $0.04 · Letting it expire re-caches ≈ $0.90"
        );
        assert!(kept[0].at < kept[1].at);
        assert_eq!(cx.read(unread), 2);
        // Refresh fails (this session never opened): the reminder comes
        // back, and the failure is the one new entry.
        let key = cx.read(|cx| reminded.session.read(cx).key());
        reminded.click(cx, ("refresh-cache", key));
        reminded.advance(cx, 1_000);
        let kept = cx.read(|cx| entries(cx).to_vec());
        assert_eq!(kept.len(), 3);
        assert!(kept[2].message.starts_with("The cache wasn’t refreshed."));
        assert_eq!(kept[2].title, "Fonts");
    }

    /// A 5-minute cache: at its last minute a reminder comes that counts
    /// down, with Refresh. It goes when the cache lapses.
    #[gpui_kit::test]
    fn the_last_minute_brings_a_reminder_that_counts_down(cx: &mut TestAppContext) {
        // 70 s left.
        let reminded = Reminded::new(cx, 5, 230_000);
        assert_eq!(reminded.sent(cx), Sent::default());
        assert_eq!(reminded.notes(cx), 0);
        reminded.advance(cx, 10_000);
        assert!(reminded.sent(cx).last);
        assert_eq!(reminded.notes(cx), 1);
        assert!(reminded.shows(cx, "cache-countdown"));
        let key = cx.read(|cx| reminded.session.read(cx).key());
        assert!(reminded.shows(cx, ("refresh-cache", key)));
        // It lapses: the reminder goes.
        reminded.advance(cx, 61_000);
        reminded.advance(cx, 1_000);
        assert!(!reminded.shows(cx, "cache-countdown"));
    }

    /// The headline says the minutes left, and the OS shows it under the
    /// session's title, with what the lapse costs on a line of its own.
    #[test]
    fn reminders_read_well() {
        assert_eq!(
            super::headline(EARLY_MS),
            "The prompt cache expires in 5 minutes"
        );
        // Late, after the computer slept.
        assert_eq!(
            super::headline(3 * 60_000 - 500),
            "The prompt cache expires in 3 minutes"
        );
        assert_eq!(
            super::headline(FINAL_MS),
            "The prompt cache expires in a minute"
        );
        assert_eq!(
            super::os_text("The prompt cache expires in a minute", "Refresh ≈ $0.04"),
            "The prompt cache expires in a minute\nRefresh ≈ $0.04"
        );
        assert_eq!(
            super::os_text("The prompt cache expires in a minute", ""),
            "The prompt cache expires in a minute"
        );
    }

    /// A failed refresh says why once: the session's sentence as it is,
    /// the app's own after what failed.
    #[test]
    fn a_failed_refresh_says_why_once() {
        assert_eq!(
            super::refresh_failed("The cache wasn't refreshed: the server is overloaded"),
            "The cache wasn't refreshed: the server is overloaded"
        );
        assert_eq!(
            super::refresh_failed("Wait for the turn to end."),
            "The cache wasn’t refreshed. Wait for the turn to end."
        );
    }

    /// Refresh asks the session to keep the cache warm, and says why it
    /// couldn't (this session never opened). The reminder comes back, its
    /// countdown running on, so Refresh can be tried again.
    #[gpui_kit::test]
    fn refresh_asks_the_session_and_says_why_not(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 250_000);
        assert!(reminded.sent(cx).last);
        let key = cx.read(|cx| reminded.session.read(cx).key());
        reminded.click(cx, ("refresh-cache", key));
        reminded.advance(cx, 1_000);
        assert!(reminded.sent(cx).last);
        assert!(reminded.shows(cx, "cache-countdown"));
        assert!(reminded.shows(cx, ("refresh-cache", key)));
        // The reminder, and why the refresh failed.
        assert_eq!(reminded.notes(cx), 2);
    }

    /// Refresh keeps the cache warm out of band. arko answers before its
    /// status says when the cache was refreshed: the reminder stays away,
    /// and the new idle period gets its own.
    #[gpui_kit::test]
    fn a_refresh_starts_an_idle_period_with_reminders_of_its_own(cx: &mut TestAppContext) {
        // 50 s left: the last reminder shows.
        let reminded = Reminded::new(cx, 5, 250_000);
        assert!(reminded.sent(cx).last);
        reminded.session.update(cx, |session, _| {
            session.answer_refresh_with(Ok(arko::Refreshed::OutOfBand {
                read: 180_000,
                written: 0,
                cost: 0.04,
            }))
        });
        let key = cx.read(|cx| reminded.session.read(cx).key());
        reminded.click(cx, ("refresh-cache", key));
        reminded.advance(cx, 300);
        assert!(
            !reminded.shows(cx, "cache-countdown"),
            "the reminder came back"
        );
        assert_eq!(reminded.notes(cx), 1, "only “Cache refreshed” shows");
        // arko's status, half a second after the refresh.
        reminded.advance(cx, 200);
        reminded.refreshed(cx, 5, 500);
        assert_eq!(reminded.sent(cx), Sent::default());
        // The new idle period's last minute.
        reminded.advance(cx, 4 * 60_000);
        assert!(reminded.sent(cx).last);
        assert!(reminded.shows(cx, "cache-countdown"));
    }

    /// In the background the reminder goes to the OS's notifications too,
    /// and leaves them with the lapse.
    #[gpui_kit::test]
    fn an_os_copy_leaves_with_the_reminder(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 230_000);
        gpui_kit::VisualTestContext::from_window(reminded.window, cx).deactivate_window();
        reminded.advance(cx, 10_000);
        assert!(reminded.sent(cx).last);
        assert_eq!(cx.delivered_system_notifications().len(), 1);
        reminded.advance(cx, 61_000);
        assert!(!reminded.shows(cx, "cache-countdown"));
        assert!(cx.delivered_system_notifications().is_empty());
    }

    /// A session that ended or didn't open leaves no reminders behind.
    #[gpui_kit::test]
    fn sessions_gone_leave_no_reminders_behind(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 230_000);
        let other = cx.new(|cx| LiveSession::for_tests("s2", cx));
        cx.update(|cx| super::watch(&other, cx));
        for (session, event) in [
            (&reminded.session, LiveEvent::Ended),
            (&other, LiveEvent::Failed(None)),
        ] {
            assert!(cx.read(|cx| super::of(session, cx).is_some()));
            session.update(cx, |_, cx| cx.emit(event));
            cx.run_until_parked();
            assert!(cx.read(|cx| super::of(session, cx).is_none()));
        }
    }

    /// A reminder due while the computer slept comes after it wakes: its
    /// timers stood still, the cache's lifetime didn't.
    #[gpui_kit::test]
    fn a_reminder_due_during_sleep_comes_after_waking(cx: &mut TestAppContext) {
        // An hour-long cache. The computer sleeps 57 minutes, and wakes with
        // 3 minutes left: the first reminder is overdue.
        let reminded = Reminded::new(cx, 60, 0);
        reminded.now.set(reminded.now.get() + 57 * 60_000);
        reminded.advance(cx, 30_000);
        assert!(reminded.sent(cx).early);
        assert_eq!(reminded.notes(cx), 1);
    }

    /// While a question waits, no request keeps the cache warm: the last
    /// reminder comes, saying answering keeps it warm, without Refresh (the
    /// session takes none in the middle of a turn).
    #[gpui_kit::test]
    fn a_question_waiting_gets_a_reminder_to_answer(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 230_000);
        let asking = Activity {
            working: true,
            since: Some(NOW - 240_000),
            question: Some(Question {
                id: 1,
                text: "Tabs or spaces?".into(),
            }),
            ..Activity::default()
        };
        let reading = cx.read(|cx| reminded.session.read(cx).reading().clone());
        reminded.session.update(cx, |session, cx| {
            session.set_status((*reading).clone(), asking, cx)
        });
        reminded.advance(cx, 10_000);
        assert!(reminded.sent(cx).last);
        assert!(reminded.shows(cx, "cache-countdown"));
        let key = cx.read(|cx| reminded.session.read(cx).key());
        assert!(!reminded.shows(cx, ("refresh-cache", key)));
    }

    /// An hour-long cache gets a first reminder 5 minutes before.
    #[gpui_kit::test]
    fn an_hour_long_cache_is_reminded_twice(cx: &mut TestAppContext) {
        // 6 minutes left.
        let reminded = Reminded::new(cx, 60, 54 * 60_000);
        assert_eq!(reminded.sent(cx), Sent::default());
        reminded.advance(cx, 60_000);
        assert_eq!(
            reminded.sent(cx),
            Sent {
                early: true,
                last: false
            }
        );
        assert_eq!(reminded.notes(cx), 1);
        // The session's title, and what the lapse costs. Refresh comes with
        // the last one.
        assert!(reminded.shows(cx, "cache-reminder-text"));
        let key = cx.read(|cx| reminded.session.read(cx).key());
        assert!(!reminded.shows(cx, ("refresh-cache", key)));
        reminded.advance(cx, 4 * 60_000);
        assert!(reminded.sent(cx).last);
        assert!(reminded.shows(cx, "cache-countdown"));
    }

    /// None for a session the user left alone for an hour, nor with
    /// reminders off.
    #[gpui_kit::test]
    fn sessions_left_alone_or_reminders_off_get_none(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 230_000);
        // Left alone for an hour before the cache's last refresh.
        reminded.session.update(cx, |session, _| {
            session.set_touched_at(NOW - 230_000 - UNTOUCHED_MS)
        });
        reminded.advance(cx, 10_000);
        assert_eq!(reminded.sent(cx), Sent::default());
        assert_eq!(reminded.notes(cx), 0);
        cx.update(|cx| {
            crate::prefs::update(cx, |prefs| prefs.notifications.cache_reminders = false)
        });
        reminded
            .session
            .update(cx, |session, _| session.set_touched_at(NOW));
        reminded.advance(cx, 1_000);
        assert_eq!(reminded.notes(cx), 0);
    }

    /// A session on screen in the window in front counts as touched, how
    /// long ago the user last sent it something.
    #[gpui_kit::test]
    fn a_session_on_screen_counts_as_touched(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 5, 230_000);
        let workspace = cx
            .update_window(reminded.window, |root, _, cx| {
                root.downcast::<gpui_kit::base::Root>()
                    .unwrap()
                    .read(cx)
                    .view()
                    .clone()
                    .downcast::<crate::workspace::Workspace>()
                    .unwrap()
            })
            .unwrap();
        cx.update_window(reminded.window, |_, window, cx| {
            workspace.update(cx, |workspace, cx| {
                workspace.show_session(reminded.session.clone(), window, cx)
            })
        })
        .unwrap();
        reminded.session.update(cx, |session, _| {
            session.set_touched_at(NOW - 3 * UNTOUCHED_MS)
        });
        reminded.advance(cx, 10_000);
        assert!(reminded.sent(cx).last);
    }

    /// The reminders of the sessions open in the app, the window in front
    /// and the one behind it showing two of them.
    struct Apart {
        front: AnyWindowHandle,
        front_workspace: Entity<crate::workspace::Workspace>,
        /// In front, behind and shown nowhere.
        sessions: [Entity<LiveSession>; 3],
        titles: [String; 3],
        now: Rc<Cell<i64>>,
    }

    impl Apart {
        fn new(cx: &mut TestAppContext) -> Self {
            use crate::backend::Backend as _;
            let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
            let entries = backend.sessions().unwrap();
            cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
            let (front, front_workspace) = open(cx, 1300.);
            let (behind, behind_workspace) = open(cx, 1300.);
            let now = Rc::new(Cell::new(NOW));
            let clock = now.clone();
            cx.update(|cx| cx.set_global(ReminderClock(Rc::new(move || clock.get()))));
            // Each is watched from the moment it opens. (The third sample
            // session is open elsewhere.)
            let chosen = [0, 1, 3];
            let sessions: [Entity<LiveSession>; 3] = std::array::from_fn(|index| {
                cx.update(|cx| LiveSession::resume(&entries[chosen[index]], cx))
            });
            let titles = std::array::from_fn(|index| entries[chosen[index]].title.clone());
            cx.update_window(front, |_, window, cx| {
                front_workspace.update(cx, |workspace, cx| {
                    workspace.show_session(sessions[0].clone(), window, cx)
                })
            })
            .unwrap();
            cx.update_window(behind, |_, window, cx| {
                behind_workspace.update(cx, |workspace, cx| {
                    workspace.show_session(sessions[1].clone(), window, cx)
                })
            })
            .unwrap();
            cx.update_window(front, |_, window, _| window.activate_window())
                .unwrap();
            cx.run_until_parked();
            Self {
                front,
                front_workspace,
                sessions,
                titles,
                now,
            }
        }

        /// The session's cache lasts `lifetime_minutes` and was refreshed
        /// `ago` milliseconds before now.
        fn cache(&self, cx: &mut TestAppContext, session: usize, lifetime_minutes: u64, ago: i64) {
            let reading = Reading {
                cache: CacheStatus {
                    refreshed_at: Some(self.now.get() - ago),
                    ..cache(lifetime_minutes)
                },
                ..Reading::default()
            };
            self.sessions[session].update(cx, |session, cx| {
                session.set_touched_at(NOW);
                session.set_status(reading, Activity::default(), cx)
            });
            cx.run_until_parked();
        }

        fn sent(&self, cx: &mut TestAppContext, session: usize) -> Sent {
            cx.read(|cx| {
                super::of(&self.sessions[session], cx)
                    .unwrap()
                    .read(cx)
                    .sent()
            })
        }

        fn key(&self, cx: &mut TestAppContext, session: usize) -> u64 {
            cx.read(|cx| self.sessions[session].read(cx).key())
        }
    }

    /// Reminders come for every session open in the app, the one in front of
    /// the user and those out of view. Each card names its session in a link
    /// that brings it forward.
    #[gpui_kit::test]
    fn reminders_come_for_sessions_out_of_view_too(cx: &mut TestAppContext) {
        let apart = Apart::new(cx);
        // 50 s of a 5-minute cache, then 4 minutes of an hour's, twice.
        apart.cache(cx, 0, 5, 250_000);
        apart.cache(cx, 1, 60, 56 * 60_000);
        apart.cache(cx, 2, 60, 56 * 60_000);
        assert!(apart.sent(cx, 0).last);
        assert_eq!(
            apart.sent(cx, 1),
            Sent {
                early: true,
                last: false
            }
        );
        assert!(apart.sent(cx, 2).early);
        // The cards come up in the window in front, where the user looks.
        let notes = cx
            .update_window(apart.front, |_, window, cx| window.notifications(cx).len())
            .unwrap();
        assert_eq!(notes, 3);
        for session in 0..3 {
            let key = apart.key(cx, session);
            let label = cx
                .update_window(apart.front, |_, window, cx| {
                    window.render_frame(cx);
                    window.find(("open-session", key)).label().map(String::from)
                })
                .unwrap();
            assert_eq!(
                label,
                Some(format!("Open session {}", apart.titles[session]))
            );
        }
        // The link of the session no window shows brings it forward.
        let key = apart.key(cx, 2);
        cx.update_window(apart.front, |_, window, cx| {
            window.render_frame(cx);
            window.click(("open-session", key), cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            crate::tests::open_title(&apart.front_workspace, cx),
            Some(apart.titles[2].clone())
        );
    }

    /// Sessions that only sit in the sidebar have no running agent, so no
    /// cache to remind of.
    #[gpui_kit::test]
    fn a_session_only_in_the_sidebar_has_no_reminders(cx: &mut TestAppContext) {
        let apart = Apart::new(cx);
        apart.cache(cx, 0, 5, 250_000);
        let listed = cx.read(|cx| {
            crate::live::LiveSessions::all(cx)
                .iter()
                .filter_map(|session| session.read(cx).id().cloned())
                .count()
        });
        // The three opened here; the sample sidebar lists more.
        assert_eq!(listed, 3);
        let more = cx.read(|cx| {
            apart
                .front_workspace
                .read(cx)
                .sidebar()
                .read(cx)
                .sessions()
                .len()
        });
        assert!(more > listed);
    }

    /// A one-hour cache is reminded of twice, at 5 minutes and at 1 minute
    /// left, and the reminders say so. The OS shows the session's title
    /// above them.
    #[gpui_kit::test]
    fn an_hour_long_cache_is_reminded_at_five_and_at_one_minute_left(cx: &mut TestAppContext) {
        let reminded = Reminded::new(cx, 60, 0);
        gpui_kit::VisualTestContext::from_window(reminded.window, cx).deactivate_window();
        let mut posted: Vec<(i64, String, String)> = Vec::new();
        let start = reminded.now.get();
        // Every 5 seconds for an hour and a minute.
        for _ in 0..(61 * 12) {
            reminded.advance(cx, 5_000);
            for shown in cx.shown_system_notifications().iter().skip(posted.len()) {
                posted.push((
                    reminded.now.get() - start,
                    shown.title.to_string(),
                    shown.body.to_string(),
                ));
            }
        }
        assert_eq!(posted.len(), 2, "{posted:?}");
        let (first, last) = (&posted[0], &posted[1]);
        // The checks look again at least every 30 seconds.
        assert!(
            (55 * 60_000..=55 * 60_000 + 30_000).contains(&first.0),
            "{first:?}"
        );
        assert!(
            (59 * 60_000..=59 * 60_000 + 30_000).contains(&last.0),
            "{last:?}"
        );
        assert_eq!(first.1, "Fonts");
        assert!(
            first
                .2
                .starts_with("The prompt cache expires in 5 minutes\n")
        );
        assert_eq!(last.1, "Fonts");
        assert!(last.2.starts_with("The prompt cache expires in a minute\n"));
    }

    #[test]
    fn a_lapse_without_a_reminder_shown_does_nothing() {
        assert_eq!(
            plan(&cache(5), false, 0, Sent::default(), true, 300_000),
            (None, None)
        );
    }
}
