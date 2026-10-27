//! The status bar under the window. It shows only what the user may act on:
//! a turn's state while one runs, workers, unusual states such as outside
//! access, and on the right the cache timer, plan usage, cost and context.
//! An idle window with nothing unusual has an empty bar.
//!
//! The bar is a view of its own, which the window keeps from frame to frame
//! (a cached view): its clock ticks once a second only while it shows a
//! running time, and then the bar draws again, not the panes around it.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use arko::details::CostReport;
use arko::remote::{Activity, Page, Reading};
use gpui_kit::component::status_bar::StatusBar as KitStatusBar;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    notification::Notification,
    popover::Popover,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::cache_timer::{CacheTimer, cache_timer, requests_run};
use crate::context_ring::{ContextDetails, ContextSize, context_button};
use crate::figures::{self, Turn};
use crate::layout::WidthClass;
use crate::live::LiveSession;
use crate::menus;
use crate::session_view::{HomeFolder, shown_folder};
use crate::web_page::WebPagePanel;

/// Where the bar's figures come from: the shown session, or fixed ones (a
/// preview).
enum Source {
    None,
    Session(Entity<LiveSession>),
    Fixed {
        reading: Arc<Reading>,
        activity: Activity,
    },
}

/// The time the bar reads, in Unix milliseconds.
pub type Clock = Rc<dyn Fn() -> i64>;

/// The bar's height: the window lays it out at this height without drawing
/// it again.
pub const STATUS_BAR_HEIGHT: Rems = rems(1.75);

/// Digits of equal width, so a running time keeps its width as it counts.
pub fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// The status bar of one window, for the session it shows.
pub struct StatusBar {
    source: Source,
    width: WidthClass,
    clock: Clock,
    /// What `/cost` says, read when the cost's popover opens.
    cost: Option<CostReport>,
    /// The web page's popover shows.
    web_page_open: bool,
    /// It opens as soon as the page is served (Share Web Page).
    web_page_asked: bool,
    /// The session's latest Stop or Close, invalidating older popovers.
    web_page_epoch: u64,
    _session: Option<Subscription>,
    _tick: Option<Task<()>>,
    _cost: Option<Task<()>>,
}

impl StatusBar {
    pub fn new() -> Self {
        Self {
            source: Source::None,
            width: WidthClass::Wide,
            clock: Rc::new(figures::now_ms),
            cost: None,
            web_page_open: false,
            web_page_asked: false,
            web_page_epoch: 0,
            _session: None,
            _tick: None,
            _cost: None,
        }
    }

    /// A bar showing fixed figures, at the time `clock` gives (previews and
    /// screenshots). Its buttons do nothing.
    pub fn fixed(reading: Reading, activity: Activity, clock: Clock) -> Self {
        Self {
            source: Source::Fixed {
                reading: Arc::new(reading),
                activity,
            },
            clock,
            ..Self::new()
        }
    }

    /// Reads the time from `clock` (tests).
    pub fn set_clock(&mut self, clock: Clock, cx: &mut Context<Self>) {
        self.clock = clock;
        cx.notify();
    }

    /// The cost the cost's popover shows, in place of reading it (previews).
    pub fn set_cost(&mut self, cost: CostReport, cx: &mut Context<Self>) {
        self.cost = Some(cost);
        cx.notify();
    }

    /// Shows `session`'s figures, or none.
    pub fn set_session(&mut self, session: Option<Entity<LiveSession>>, cx: &mut Context<Self>) {
        let same = match (&self.source, &session) {
            (Source::Session(shown), Some(session)) => shown.entity_id() == session.entity_id(),
            (Source::None, None) => true,
            _ => false,
        };
        if same {
            return;
        }
        self.cost = None;
        self._cost = None;
        self.web_page_open = false;
        self.web_page_asked = false;
        self.web_page_epoch = session
            .as_ref()
            .map_or(0, |session| session.read(cx).sharing_epoch());
        self._session = session
            .as_ref()
            .map(|session| cx.observe(session, |_, _, cx| cx.notify()));
        self.source = match session {
            Some(session) => Source::Session(session),
            None => Source::None,
        };
        cx.notify();
    }

    pub fn set_width(&mut self, width: WidthClass, cx: &mut Context<Self>) {
        if self.width != width {
            self.width = width;
            cx.notify();
        }
    }

    fn session(&self) -> Option<&Entity<LiveSession>> {
        match &self.source {
            Source::Session(session) => Some(session),
            _ => None,
        }
    }

    /// The bar shows a chat's figures.
    fn is_chat(&self, cx: &App) -> bool {
        self.session()
            .is_some_and(|session| session.read(cx).mode() == crate::backend::Mode::Chat)
    }

    /// Shares the page and shows its links only after the engine answers.
    pub fn share_web_page(
        &mut self,
        bind: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self.session().cloned() else {
            return;
        };
        if session.read(cx).reading().page.is_some() {
            self.web_page_open = true;
            cx.notify();
            return;
        }
        let answer = crate::web_page::share(&session, bind, cx);
        let epoch = session.read(cx).sharing_epoch();
        self.web_page_epoch = epoch;
        let this = cx.entity().downgrade();
        window
            .spawn(cx, async move |cx| {
                let answer = answer.await;
                cx.update(|window, cx| {
                    // Stop and Close deliberately cancel earlier shares.
                    if session.read(cx).sharing_epoch() != epoch {
                        return;
                    }
                    match answer {
                        Ok(_) => {
                            this.update(cx, |this, cx| {
                                if this.session().is_some_and(|shown| shown == &session) {
                                    this.web_page_asked = true;
                                    cx.notify();
                                }
                            })
                            .ok();
                        }
                        Err(why) => window.push_notification(Notification::error(why), cx),
                    }
                })
                .ok();
            })
            .detach();
    }

    /// Whether the web page's popover shows.
    pub fn is_web_page_open(&self) -> bool {
        self.web_page_open
    }

    /// Whether its clock ticks: a running time shows.
    pub fn is_ticking(&self) -> bool {
        self._tick.is_some()
    }

    /// What `/cost` said when the cost's popover last opened.
    pub fn cost(&self) -> Option<&CostReport> {
        self.cost.as_ref()
    }

    /// The figures shown: the session's latest status and activity.
    fn status(&self, cx: &App) -> Option<(Arc<Reading>, Activity)> {
        match &self.source {
            Source::None => None,
            Source::Session(session) => {
                let session = session.read(cx);
                Some((session.reading().clone(), session.activity().clone()))
            }
            Source::Fixed { reading, activity } => Some((reading.clone(), activity.clone())),
        }
    }

    /// Reads what `/cost` says, off the UI thread, for the cost's popover.
    fn load_cost(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self
            .session()
            .and_then(|session| session.read(cx).link())
            .map(|link| link.details.clone())
        else {
            return;
        };
        let load = cx.background_spawn(async move { details.cost() });
        self._cost = Some(cx.spawn(async move |this, cx| {
            let cost = load.await;
            this.update(cx, |this, cx| {
                this.cost = Some(cost);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Keeps the clock ticking while a running time shows, and stops it
    /// when none does.
    fn schedule_tick(&mut self, next: Option<i64>, cx: &mut Context<Self>) {
        let Some(next) = next else {
            self._tick = None;
            return;
        };
        if self._tick.is_some() {
            return;
        }
        self._tick = Some(cx.spawn(async move |this, cx| {
            let mut wait = next;
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(wait.clamp(1, 1000) as u64))
                    .await;
                let next = this.update(cx, |this, cx| {
                    cx.notify();
                    this.status(cx).and_then(|(reading, activity)| {
                        next_change(&reading, &activity, (this.clock)())
                    })
                });
                match next {
                    Ok(Some(next)) => wait = next,
                    _ => break,
                }
            }
            this.update(cx, |this, _| this._tick = None).ok();
        }));
    }
}

impl Default for StatusBar {
    fn default() -> Self {
        Self::new()
    }
}

/// How long until a running time the bar shows changes, in milliseconds:
/// the turn's clock and the cache timer move each second. `None` when
/// nothing shown moves.
pub fn next_change(reading: &Reading, activity: &Activity, now: i64) -> Option<i64> {
    let turn = match Turn::of(activity) {
        Some(Turn::Working { since: Some(since) }) => Some(1000 - (now - since).rem_euclid(1000)),
        _ => None,
    };
    let cache = match CacheTimer::of(&reading.cache, requests_run(activity), now) {
        Some(CacheTimer::Counting { left }) => Some(match left % 1000 {
            0 => 1000,
            part => part,
        }),
        _ => None,
    };
    turn.into_iter().chain(cache).min()
}

/// Sends a command line to the session, as if typed, and says why not when
/// it can't.
pub(crate) fn run(session: &Entity<LiveSession>, command: &str, window: &mut Window, cx: &mut App) {
    let input = arko::Input::Message(arko::Message {
        text: command.to_string(),
        ..Default::default()
    });
    if let Err(why) = session.update(cx, |session, cx| session.send(input, cx)) {
        window.push_notification(why, cx);
    }
}

/// "r1", "r1 and r2", "r1, r2, r3, r4, r5 and 2 more".
fn names(ids: &[String]) -> String {
    const SHOWN: usize = 5;
    let shown: Vec<&str> = ids.iter().take(SHOWN).map(String::as_str).collect();
    let more = ids.len().saturating_sub(SHOWN);
    match (shown.as_slice(), more) {
        ([], _) => String::new(),
        ([one], _) => one.to_string(),
        (all, 0) => format!(
            "{} and {}",
            all[..all.len() - 1].join(", "),
            all[all.len() - 1]
        ),
        (all, more) => format!("{} and {more} more", all.join(", ")),
    }
}

/// The Outside access popover's heading and what it says under it, with the
/// session's folder as it shows (`None` before it has one).
fn outside_words(folder: Option<&str>) -> (&'static str, String) {
    let text = match folder {
        Some(folder) => format!(
            "The agents may create, change and delete files outside {folder} too, wherever your account can."
        ),
        None => "The agents may create, change and delete files outside the session’s folder too, wherever your account can.".to_string(),
    };
    ("Outside access", text)
}

/// A popover's heading and the sentences under it.
fn heading(title: impl Into<SharedString>, text: impl Into<SharedString>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(div().text_sm().font_medium().child(title.into()))
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(text.into()),
        )
}

impl StatusBar {
    fn render_turn(&self, turn: Turn, now: i64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .id("status-turn")
            .test_support()
            .gap_1()
            .font_features(tabular_figures())
            .when(turn == Turn::Waiting, |this| {
                this.text_color(theme.foreground).child(
                    Icon::new(gpui_kit::assets::IconName::MessageCircleQuestionMark)
                        .xsmall()
                        .text_color(theme.warning),
                )
            })
            .child(turn.label(now))
            .into_any_element()
    }

    fn render_workers(&self, running: &[String]) -> AnyElement {
        let count = running.len();
        let label = format!("{count} worker{}", if count == 1 { "" } else { "s" });
        let tooltip = format!("{} running · show the workers", names(running));
        Button::new("status-workers")
            .ghost()
            .xsmall()
            .icon(IconName::Bot)
            .label(label)
            .tooltip_with_action(tooltip, &menus::ToggleWorkersPanel, None)
            .on_click(|_, window, cx| {
                window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx)
            })
            .into_any_element()
    }

    fn render_outside(&self, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session().cloned();
        let folder = session
            .as_ref()
            .and_then(|session| session.read(cx).workspace().cloned())
            .map(|folder| shown_folder(&folder, cx.global::<HomeFolder>().0.as_deref()));
        let warning = cx.theme().warning;
        Popover::new("outside-popover")
            .anchor(Anchor::BottomLeft)
            .trigger(
                Button::new("status-outside")
                    .ghost()
                    .xsmall()
                    .icon(Icon::new(IconName::TriangleAlert).text_color(warning))
                    .text_color(warning)
                    .when(!narrow, |this| this.label("Outside access"))
                    .when(narrow, |this| this.tooltip("Outside access is on"))
                    .accessibility_label("Outside access"),
            )
            .content(move |_, _, cx| {
                let (title, text) = outside_words(folder.as_deref());
                let session = session.clone();
                v_flex()
                    .id("outside-details")
                    .test_support()
                    .w(rems(18.))
                    .gap_3()
                    .child(heading(title, text, cx))
                    .child(
                        h_flex().justify_end().child(
                            Button::new("outside-off")
                                .outline()
                                .small()
                                .label("Turn off")
                                .disabled(session.is_none())
                                .on_click(cx.listener(move |state, _, window, cx| {
                                    if let Some(session) = &session {
                                        run(session, "/outside off", window, cx);
                                    }
                                    state.dismiss(window, cx);
                                })),
                        ),
                    )
            })
            .into_any_element()
    }

    fn render_web_page(&self, page: &Page, narrow: bool, cx: &mut Context<Self>) -> AnyElement {
        let session = self.session().cloned();
        let page = page.clone();
        let view = cx.entity().downgrade();
        Popover::new("web-page-popover")
            .anchor(Anchor::BottomLeft)
            .open(self.web_page_open)
            .on_open_change(move |open, _, cx| {
                let open = *open;
                view.update(cx, |this, cx| {
                    this.web_page_open = open;
                    cx.notify();
                })
                .ok();
            })
            .trigger(
                Button::new("status-web-page")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Globe)
                    .when(!narrow, |this| this.label("Web page shared"))
                    .when(narrow, |this| this.tooltip("Web page shared"))
                    .accessibility_label("Web page shared"),
            )
            .content(move |_, _, cx| {
                let panel = WebPagePanel::new(&page)
                    .explained()
                    .on_settings(cx.listener(|state, _, window, cx| {
                        // Once the focus is back in the window: sent from
                        // inside the popover, the action would go with it.
                        state.dismiss(window, cx);
                        window.defer(cx, |window, cx| {
                            window.dispatch_action(Box::new(menus::OpenWebPageSettings), cx)
                        });
                    }));
                match &session {
                    Some(session) => panel.session(session),
                    None => panel,
                }
            })
            .into_any_element()
    }

    fn render_plan(&self, reading: &Reading, now: i64, cx: &App) -> Option<AnyElement> {
        let (_, window) = figures::plan_alert(&reading.plans)?;
        let tooltip = figures::plans_tooltip(&reading.plans, now, &figures::Times::of(cx));
        Some(
            div()
                .id("status-plan")
                .test_support()
                .font_features(tabular_figures())
                .text_color(cx.theme().warning)
                .child(figures::plan_label(window))
                .tooltip(move |window, cx| {
                    gpui_kit::component::tooltip::Tooltip::new(tooltip.clone()).build(window, cx)
                })
                .into_any_element(),
        )
    }

    fn render_cost(&self, reading: &Reading, cx: &mut Context<Self>) -> Option<AnyElement> {
        let label = figures::cost_label(reading)?;
        let this = cx.entity();
        let view = this.downgrade();
        Some(
            Popover::new("cost-popover")
                .anchor(Anchor::BottomRight)
                .on_open_change(move |open, _, cx| {
                    if *open {
                        view.update(cx, |this, cx| this.load_cost(cx)).ok();
                    }
                })
                .trigger(
                    Button::new("status-cost")
                        .ghost()
                        .xsmall()
                        .label(label)
                        .tooltip(figures::cost_tooltip(reading)),
                )
                .content(move |_, _, cx| CostDetails::new(this.read(cx).cost.clone()))
                .into_any_element(),
        )
    }

    fn render_context(&self, context: ContextSize, cx: &mut Context<Self>) -> AnyElement {
        let this = cx.entity();
        let clock = self.clock.clone();
        context_button(
            context,
            move |cx| {
                let bar = this.read(cx);
                let reading = bar
                    .status(cx)
                    .map(|(reading, _)| reading)
                    .unwrap_or_default();
                let details = ContextDetails::new(&reading, clock());
                match bar.session() {
                    Some(session) => details.session(session),
                    None => details,
                }
            },
            cx,
        )
    }
}

/// What the bar shows, part by part: each `None` (or `false`) while hidden.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Shown {
    /// "Working 1:23", "Waiting for your answer".
    pub turn: Option<String>,
    /// The running workers.
    pub workers: Vec<String>,
    pub outside: bool,
    pub web_page: bool,
    /// "cache 3:12", and whether it warns.
    pub cache: Option<(String, bool)>,
    /// "5 h limit 84%".
    pub plan: Option<String>,
    /// "$1.20".
    pub cost: Option<String>,
    /// "142k / 1M", and whether it warns.
    pub context: Option<(String, bool)>,
}

impl StatusBar {
    /// What the bar shows now. Narrow windows keep the context, the cost
    /// and the warnings.
    pub fn shown(&self, cx: &App) -> Shown {
        let Some((reading, activity)) = self.status(cx) else {
            return Shown::default();
        };
        let now = (self.clock)();
        let wide = self.width != WidthClass::Narrow;
        let cache = CacheTimer::of(&reading.cache, requests_run(&activity), now);
        Shown {
            turn: Turn::of(&activity)
                .filter(|_| wide)
                .map(|turn| turn.label(now)),
            workers: if wide {
                reading
                    .workers_running
                    .iter()
                    .map(|(id, _)| id.clone())
                    .collect()
            } else {
                Vec::new()
            },
            outside: reading.settings.outside,
            web_page: reading.page.is_some(),
            cache: cache
                .filter(|_| wide)
                .map(|timer| (timer.label(), timer.warns())),
            plan: figures::plan_alert(&reading.plans)
                .filter(|_| wide)
                .map(|(_, window)| figures::plan_label(window)),
            cost: figures::cost_label(&reading),
            context: ContextSize::of(&reading)
                .map(|context| context.in_chat(self.is_chat(cx)))
                .map(|context| (context.label(), context.near_handover())),
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("StatusBar");
        let mut bar = KitStatusBar::new()
            .w_full()
            .h(STATUS_BAR_HEIGHT)
            .flex_none();
        let unread = crate::notifications::unread(cx);
        let Some((reading, activity)) = self.status(cx) else {
            self.schedule_tick(None, cx);
            if unread > 0 {
                bar = bar.right(unread_button(unread));
            }
            return bar;
        };
        let now = (self.clock)();
        self.schedule_tick(next_change(&reading, &activity, now), cx);
        let shown = self.shown(cx);
        let narrow = self.width == WidthClass::Narrow;
        if let Some(turn) = Turn::of(&activity).filter(|_| shown.turn.is_some()) {
            bar = bar.left(self.render_turn(turn, now, cx));
        }
        if !shown.workers.is_empty() {
            bar = bar.left(self.render_workers(&shown.workers));
        }
        if shown.outside {
            bar = bar.left(self.render_outside(narrow, cx));
        }
        if let Some(session) = self.session() {
            let epoch = session.read(cx).sharing_epoch();
            if self.web_page_epoch != epoch {
                self.web_page_epoch = epoch;
                self.web_page_asked = false;
                self.web_page_open = false;
            }
        }
        match &reading.page {
            Some(page) => {
                // Shared at the user's asking: its links show at once.
                if std::mem::take(&mut self.web_page_asked) {
                    self.web_page_open = true;
                }
                bar = bar.left(self.render_web_page(page, narrow, cx));
            }
            None => self.web_page_open = false,
        }
        if let Some(timer) = CacheTimer::of(&reading.cache, requests_run(&activity), now)
            .filter(|_| shown.cache.is_some())
        {
            bar = bar.right(cache_timer(timer, &reading.cache, now, cx));
        }
        if shown.plan.is_some()
            && let Some(plan) = self.render_plan(&reading, now, cx)
        {
            bar = bar.right(plan);
        }
        if let Some(cost) = self.render_cost(&reading, cx) {
            bar = bar.right(cost);
        }
        if let Some(context) =
            ContextSize::of(&reading).map(|context| context.in_chat(self.is_chat(cx)))
        {
            bar = bar.right(self.render_context(context, cx));
        }
        // Last, at the right edge, and only while something is unread.
        if unread > 0 {
            bar = bar.right(unread_button(unread));
        }
        bar
    }
}

/// The count of the notifications not seen yet, which opens the
/// notifications panel.
fn unread_button(unread: usize) -> AnyElement {
    let label = match unread {
        1 => "1 unread notification".to_string(),
        count => format!("{count} unread notifications"),
    };
    Button::new("status-notifications")
        .ghost()
        .xsmall()
        .icon(Icon::new(gpui_kit::assets::IconName::Bell))
        .label(unread.to_string())
        .tooltip_with_action(label.clone(), &menus::ToggleNotifications, None)
        .accessibility_label(label)
        .on_click(|_, window, cx| window.dispatch_action(Box::new(menus::ToggleNotifications), cx))
        .into_any_element()
}

/// The cost's popover: what was billed and what it would cost at API
/// prices, the main agent's part and the workers', the search indexes'
/// embeddings and the plans' usage.
#[derive(IntoElement)]
pub struct CostDetails {
    report: Option<CostReport>,
}

impl CostDetails {
    /// `report` as `/cost` says it, `None` while it is read.
    pub fn new(report: Option<CostReport>) -> Self {
        Self { report }
    }
}

impl RenderOnce for CostDetails {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let row = |name: &str, amount: String, detail: String| {
            h_flex()
                .gap_2()
                .text_xs()
                .child(div().w(rems(6.5)).flex_none().child(name.to_string()))
                .child(
                    div()
                        .w(rems(4.5))
                        .flex_none()
                        .text_right()
                        .font_features(tabular_figures())
                        .child(amount),
                )
                .child(div().flex_1().min_w_0().text_color(muted).child(detail))
        };
        let tokens = |input: usize, output: usize| {
            format!(
                "{} in, {} out",
                figures::tokens(input),
                figures::tokens(output)
            )
        };
        let body = match &self.report {
            None => div()
                .text_xs()
                .text_color(muted)
                .child("Reading…")
                .into_any_element(),
            Some(report) => {
                let billed = report.billed();
                let api = report.main.api + report.workers.api;
                let plan = report.main.plan.or(report.workers.plan);
                let part = |share: &arko::details::CostShare| match share.plan {
                    Some(_) => format!("≈ {}", figures::dollars(share.api)),
                    None => figures::dollars(share.billed),
                };
                let mut lines = vec![
                    div()
                        .text_sm()
                        .child(format!("{} billed", figures::dollars(billed)))
                        .into_any_element(),
                ];
                if let Some(plan) = plan {
                    lines.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .child(format!(
                                "≈ {} at API prices. The {plan} covers {}",
                                figures::dollars(api),
                                if report.main.plan.is_some() && report.workers.plan.is_some()
                                    || report.workers.input + report.workers.output == 0
                                    || report.main.input + report.main.output == 0
                                {
                                    "it"
                                } else {
                                    "part of it"
                                }
                            ))
                            .into_any_element(),
                    );
                }
                let mut rows = vec![row(
                    "Main agent",
                    part(&report.main),
                    tokens(report.main.input, report.main.output),
                )];
                if report.workers.input + report.workers.output > 0 {
                    rows.push(row(
                        "Workers",
                        part(&report.workers),
                        tokens(report.workers.input, report.workers.output),
                    ));
                }
                if let Some((tokens, dollars)) = report.embeddings {
                    rows.push(row(
                        "Search indexes",
                        figures::dollars(dollars),
                        format!("{} tokens", figures::tokens(tokens as usize)),
                    ));
                }
                lines.push(v_flex().gap_1().children(rows).into_any_element());
                if !report.plans.is_empty() {
                    lines.push(
                        div()
                            .text_xs()
                            .text_color(muted)
                            .whitespace_normal()
                            .child(figures::plans_tooltip(
                                &report.plans,
                                figures::now_ms(),
                                &figures::Times::of(cx),
                            ))
                            .into_any_element(),
                    );
                }
                v_flex().gap_2().children(lines).into_any_element()
            }
        };
        v_flex()
            .id("cost-details")
            .test_support()
            .w(rems(20.))
            .gap_2()
            .child(div().text_sm().font_medium().child("Cost"))
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{names, next_change, outside_words};
    use arko::remote::{Activity, Reading};
    use std::time::Duration;

    #[test]
    fn running_times_tick_on_their_seconds() {
        let mut activity = Activity {
            working: true,
            since: Some(10_300),
            ..Activity::default()
        };
        let reading = Reading::default();
        // 1.7 s into the turn: the clock moves in 300 ms.
        assert_eq!(next_change(&reading, &activity, 12_000), Some(300));
        // Idle with no cache known: nothing moves.
        activity.working = false;
        assert_eq!(next_change(&reading, &activity, 12_000), None);
        // The cache lapses at 300 s: at 12.25 s it moves in 750 ms.
        let mut reading = Reading::default();
        reading.cache.lifetime = Some(Duration::from_secs(300));
        reading.cache.refreshed_at = Some(0);
        assert_eq!(next_change(&reading, &activity, 12_250), Some(750));
        // Expired: still.
        assert_eq!(next_change(&reading, &activity, 400_000), None);
    }

    #[test]
    fn running_workers_are_named_five_at_most() {
        let ids = |n: usize| (1..=n).map(|n| format!("r{n}")).collect::<Vec<_>>();
        assert_eq!(names(&ids(1)), "r1");
        assert_eq!(names(&ids(2)), "r1 and r2");
        assert_eq!(names(&ids(3)), "r1, r2 and r3");
        assert_eq!(names(&ids(7)), "r1, r2, r3, r4, r5 and 2 more");
    }

    /// The popover's heading is the button's name, and the sentence under
    /// it says where the agents may reach.
    #[test]
    fn the_outside_popover_is_headed_by_its_buttons_name() {
        let (title, text) = outside_words(Some("~/code/app"));
        assert_eq!(title, "Outside access");
        assert!(text.contains("outside ~/code/app too"), "{text}");
        let (_, text) = outside_words(None);
        assert!(text.contains("outside the session’s folder too"), "{text}");
    }
}
