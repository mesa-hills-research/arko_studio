//! The workers panel: the session's workers, opened on demand at the right
//! (docked on wide windows, a sheet on narrower ones). It never opens by
//! itself.
//!
//! The list shows the workers that run or wait, and those that ended in the
//! last five minutes, then a collapsed "Finished" group with the rest. Each
//! row gives the worker's state, task, time over all its runs, its last
//! action, its context and its calls, the figures right-aligned in tabular
//! figures so they line up. Choosing a worker shows its own transcript, with
//! Stop and Message. While the worker's view has the focus, the composer
//! writes to that worker.

use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use arko::remote::{AgentStatus, Feed, WorkerState, WorkerSummary};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    spinner::Spinner,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::LiveSession;
use crate::menus;
use crate::transcript::{AgentFilter, TranscriptView};
use crate::workers::{self, Section};

/// The key contexts of the list and of a worker's view.
pub const LIST_CONTEXT: &str = "WorkersList";
pub const WORKER_CONTEXT: &str = "WorkerView";

/// How many workers' transcripts the panel keeps ready.
const KEPT_TRANSCRIPTS: usize = 3;

actions!(
    workers_panel,
    [
        SelectPrevious,
        SelectNext,
        SelectFirst,
        SelectLast,
        /// Tab: the next row, or on past the last.
        TabNext,
        /// Shift+Tab: the previous row, or back before the first.
        TabPrevious,
        OpenSelected,
        /// From a worker's view back to the list, from the list back to the
        /// composer.
        Back,
    ]
);

/// What the panel asks of the window.
#[derive(Debug, Clone, PartialEq)]
pub enum WorkersPanelEvent {
    /// Send this command line to the session (`/stop-workers r1`).
    Command(String),
    /// Message a worker: the composer takes the focus, writing to it until
    /// the user goes back to Arko (the chip's ×, Esc) or another session.
    Message(SharedString),
    /// Esc in the list: the keyboard goes back to the composer.
    Leave,
}

/// What the panel shows of the session: its workers, their times and
/// contexts, and the feed their transcripts come from.
#[derive(Clone, Default)]
pub struct PanelSource {
    pub workers: Vec<WorkerSummary>,
    pub agents: Vec<AgentStatus>,
    pub feed: Option<Feed>,
    pub images: Option<arko::images::Images>,
    /// Changes when the session (or its opening) changes: the transcripts
    /// kept are another feed's then.
    pub key: Option<(EntityId, u64)>,
}

impl PanelSource {
    fn of(session: &LiveSession, id: EntityId) -> Self {
        Self {
            workers: session.workers().to_vec(),
            agents: session.reading().agents.clone(),
            feed: session.feed().cloned(),
            images: session.link().map(|link| link.images.clone()),
            key: Some((id, session.opening())),
        }
    }
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelRow {
    Worker(SharedString),
    /// The "Finished (N)" group's header.
    Finished(usize),
}

struct Shown {
    id: SharedString,
    view: Entity<TranscriptView>,
}

/// Returns the time, in Unix milliseconds.
type Clock = Rc<dyn Fn() -> i64>;

pub struct WorkersPanel {
    session: Option<Entity<LiveSession>>,
    source: PanelSource,
    /// The list's focus.
    focus_handle: FocusHandle,
    /// A worker's view's focus (its transcript's is inside it).
    worker_focus: FocusHandle,
    /// The row the keyboard is on.
    cursor: Option<PanelRow>,
    /// The worker whose transcript shows, if any.
    open: Option<SharedString>,
    /// The transcripts made, the latest last.
    transcripts: Vec<Shown>,
    finished_open: bool,
    /// The workers the user opened: a failure stays in sight until then.
    seen: HashSet<String>,
    /// Each worker's row, by its id.
    row_views: HashMap<String, Entity<WorkerRow>>,
    /// Whether the panel shows.
    shown: bool,
    scroll: ScrollHandle,
    clock: Clock,
    /// Draws again while a time on show goes on.
    ticker: Option<Task<()>>,
    _session: Option<Subscription>,
    _focus: Vec<Subscription>,
}

impl EventEmitter<WorkersPanelEvent> for WorkersPanel {}

impl Focusable for WorkersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl WorkersPanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus_handle = cx.focus_handle();
        let worker_focus = cx.focus_handle();
        let focus = vec![cx.on_focus_in(&focus_handle, window, |this, _, cx| {
            if this.cursor.is_none() {
                this.cursor = this.rows(this.now()).into_iter().next();
                cx.notify();
            }
        })];
        Self {
            session: None,
            source: PanelSource::default(),
            focus_handle,
            worker_focus,
            cursor: None,
            open: None,
            transcripts: Vec::new(),
            finished_open: false,
            seen: HashSet::new(),
            row_views: HashMap::new(),
            shown: false,
            scroll: ScrollHandle::new(),
            clock: Rc::new(|| chrono::Utc::now().timestamp_millis()),
            ticker: None,
            _session: None,
            _focus: focus,
        }
    }

    /// Shows the workers of `session` (none when `None`).
    pub fn set_session(&mut self, session: Option<Entity<LiveSession>>, cx: &mut Context<Self>) {
        if self.session.as_ref().map(Entity::entity_id) == session.as_ref().map(Entity::entity_id) {
            return;
        }
        self.session = session;
        self.follow(cx);
    }

    /// Says whether the panel shows: it follows its session's workers only
    /// then.
    pub fn set_shown(&mut self, shown: bool, cx: &mut Context<Self>) {
        if self.shown != shown {
            self.shown = shown;
            self.follow(cx);
        }
    }

    /// Follows the session's workers while the panel shows.
    fn follow(&mut self, cx: &mut Context<Self>) {
        self._session = None;
        match self.session.clone() {
            Some(session) if self.shown => {
                self._session =
                    Some(cx.observe(&session, |this, session, cx| this.pull(&session, cx)));
                self.pull(&session, cx);
            }
            Some(_) => {}
            None => self.set_source(PanelSource::default(), cx),
        }
    }

    fn pull(&mut self, session: &Entity<LiveSession>, cx: &mut Context<Self>) {
        #[cfg(test)]
        PULLS.with(|pulls| pulls.set(pulls.get() + 1));
        let source = PanelSource::of(session.read(cx), session.entity_id());
        self.set_source(source, cx);
    }

    /// Shows `source`'s workers: from the session, or given (tests and
    /// screenshots).
    #[doc(hidden)]
    pub fn set_source(&mut self, source: PanelSource, cx: &mut Context<Self>) {
        if source.key != self.source.key {
            // Another session: its workers and transcripts are others.
            self.transcripts.clear();
            self.open = None;
            self.cursor = None;
            self.finished_open = false;
            self.seen.clear();
            self.row_views.clear();
        }
        let same = source.workers == self.source.workers
            && source.agents == self.source.agents
            && source.key == self.source.key;
        self.source = source;
        if !same {
            self.sync_rows(cx);
            cx.notify();
        }
    }

    /// Reads the time from `clock` instead of the system's: for tests and
    /// screenshots, whose times must not change.
    #[doc(hidden)]
    pub fn set_clock(&mut self, clock: impl Fn() -> i64 + 'static, cx: &mut Context<Self>) {
        self.clock = Rc::new(clock);
        self.sync_rows(cx);
        cx.notify();
    }

    fn now(&self) -> i64 {
        (self.clock)()
    }

    /// Whether the panel animates: its list shows a running worker's
    /// spinner.
    pub fn animates(&self) -> bool {
        self.open.is_none()
            && self
                .source
                .workers
                .iter()
                .any(|worker| worker.state == WorkerState::Running)
    }

    /// The worker whose transcript shows.
    pub fn open_worker(&self) -> Option<&SharedString> {
        self.open.as_ref()
    }

    /// The list's rows at `now`: the current workers in the order they
    /// started, then the "Finished" group.
    pub fn rows(&self, now: i64) -> Vec<PanelRow> {
        let id = |worker: &WorkerSummary| PanelRow::Worker(worker.id.clone().into());
        let mut rows: Vec<PanelRow> = self
            .source
            .workers
            .iter()
            .filter(|worker| self.section(worker, now) == Section::Current)
            .map(id)
            .collect();
        let finished: Vec<&WorkerSummary> = self
            .source
            .workers
            .iter()
            .filter(|worker| self.section(worker, now) == Section::Finished)
            .collect();
        if !finished.is_empty() {
            rows.push(PanelRow::Finished(finished.len()));
            if self.finished_open {
                // The latest to end first.
                let mut finished = finished;
                finished.sort_by_key(|worker| std::cmp::Reverse(worker.ended_at));
                rows.extend(finished.into_iter().map(id));
            }
        }
        rows
    }

    /// Where `worker` shows at `now`: a failure stays until opened.
    fn section(&self, worker: &WorkerSummary, now: i64) -> Section {
        workers::section(worker, now, self.seen.contains(&worker.id))
    }

    /// How many workers of the "Finished" group failed.
    pub fn finished_failures(&self, now: i64) -> usize {
        self.source
            .workers
            .iter()
            .filter(|worker| {
                worker.state == WorkerState::Failed
                    && self.section(worker, now) == Section::Finished
            })
            .count()
    }

    fn worker(&self, id: &str) -> Option<&WorkerSummary> {
        self.source.workers.iter().find(|worker| worker.id == id)
    }

    /// Shows the worker `id`'s own transcript.
    pub fn show_worker(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(feed) = self.source.feed.clone() else {
            return;
        };
        if let Some(index) = self.transcripts.iter().position(|shown| shown.id == id) {
            let shown = self.transcripts.remove(index);
            self.transcripts.push(shown);
        } else {
            let filter = AgentFilter::Worker(id.clone());
            let images = self.source.images.clone();
            let view = cx.new(|cx| {
                let mut view = TranscriptView::new(&feed, filter, window, cx);
                if let Some(images) = images {
                    view.set_images(images, cx);
                }
                view
            });
            self.transcripts.push(Shown {
                id: id.clone(),
                view,
            });
            if self.transcripts.len() > KEPT_TRANSCRIPTS {
                self.transcripts.remove(0);
            }
        }
        self.cursor = Some(PanelRow::Worker(id.clone()));
        self.seen.insert(id.to_string());
        self.open = Some(id);
        cx.notify();
    }

    /// The transcript of the worker shown.
    pub fn transcript(&self) -> Option<&Entity<TranscriptView>> {
        let open = self.open.as_ref()?;
        self.transcripts
            .iter()
            .find(|shown| &shown.id == open)
            .map(|shown| &shown.view)
    }

    /// Back to the list.
    pub fn show_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.take().is_some() {
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    /// Gives the panel the keyboard: the worker's view when one shows, else
    /// the list, on its first row unless it was on one.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.worker_focus.focus(window, cx);
        } else {
            if self.cursor.is_none() {
                self.cursor = self.rows(self.now()).into_iter().next();
                cx.notify();
            }
            self.focus_handle.focus(window, cx);
        }
    }

    /// Opens the worker `id` and gives its view the keyboard. The composer
    /// goes on writing to Arko: only Message writes to a worker.
    fn enter_worker(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        self.show_worker(id, window, cx);
        self.worker_focus.focus(window, cx);
    }

    pub fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.focus_handle.contains_focused(window, cx)
            || self.worker_focus.contains_focused(window, cx)
    }

    /// Stops the worker `id`.
    fn stop(&mut self, id: &str, cx: &mut Context<Self>) {
        cx.emit(WorkersPanelEvent::Command(format!("/stop-workers {id}")));
    }

    fn stop_all(&mut self, cx: &mut Context<Self>) {
        cx.emit(WorkersPanelEvent::Command("/stop-workers".into()));
    }

    fn message(&mut self, id: SharedString, cx: &mut Context<Self>) {
        cx.emit(WorkersPanelEvent::Message(id));
    }

    // ----- the keyboard -----

    fn cursor_index(&self, rows: &[PanelRow]) -> Option<usize> {
        let cursor = self.cursor.as_ref()?;
        rows.iter().position(|row| match (row, cursor) {
            // The group's count changes as workers move in.
            (PanelRow::Finished(_), PanelRow::Finished(_)) => true,
            _ => row == cursor,
        })
    }

    fn move_cursor(&mut self, step: isize, wrap: bool, cx: &mut Context<Self>) -> bool {
        let rows = self.rows(self.now());
        if rows.is_empty() {
            return false;
        }
        let last = rows.len() as isize - 1;
        let next = match self.cursor_index(&rows) {
            None if step > 0 => 0,
            None => last,
            Some(index) => index as isize + step,
        };
        let next = if wrap {
            next.rem_euclid(last + 1)
        } else if !(0..=last).contains(&next) {
            return false;
        } else {
            next
        };
        self.select(next as usize, &rows, cx);
        true
    }

    fn select(&mut self, index: usize, rows: &[PanelRow], cx: &mut Context<Self>) {
        self.cursor = rows.get(index).cloned();
        self.scroll.scroll_to_item(index);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(-1, true, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(1, true, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows(self.now());
        self.select(0, &rows, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        let rows = self.rows(self.now());
        self.select(rows.len().saturating_sub(1), &rows, cx);
    }

    fn tab_next(&mut self, _: &TabNext, window: &mut Window, cx: &mut Context<Self>) {
        if !self.move_cursor(1, false, cx) {
            window.focus_next(cx);
        }
    }

    fn tab_previous(&mut self, _: &TabPrevious, window: &mut Window, cx: &mut Context<Self>) {
        if !self.move_cursor(-1, false, cx) {
            window.focus_prev(cx);
        }
    }

    fn open_selected(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        match self.cursor.clone() {
            Some(PanelRow::Worker(id)) => self.enter_worker(id, window, cx),
            Some(PanelRow::Finished(_)) => {
                self.finished_open = !self.finished_open;
                cx.notify();
            }
            None => {}
        }
    }

    fn back(&mut self, _: &Back, window: &mut Window, cx: &mut Context<Self>) {
        if self.open.is_some() {
            self.show_list(window, cx);
        } else {
            cx.emit(WorkersPanelEvent::Leave);
        }
    }

    /// Draws again in a second while something on show counts, and when the
    /// next ended worker moves to the "Finished" group.
    fn tick(&mut self, now: i64, cx: &mut Context<Self>) {
        if self.ticker.is_some() {
            return;
        }
        let counting = self
            .source
            .workers
            .iter()
            .any(|worker| worker.state.is_active());
        let wait = if counting {
            Some(1000)
        } else {
            workers::next_move(&self.source.workers, now, |id| self.seen.contains(id))
                .map(|at| (at - now).max(0) as u64 + 1)
        };
        let Some(wait) = wait else {
            return;
        };
        self.ticker = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(wait))
                .await;
            this.update(cx, |this, cx| {
                this.ticker = None;
                this.sync_rows(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    // ----- drawing -----

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let running = self
            .source
            .workers
            .iter()
            .any(|worker| worker.state.is_active());
        let view = cx.entity().downgrade();
        h_flex()
            .flex_none()
            .h_10()
            .pl_3()
            .pr_1()
            .gap_1()
            .border_b_1()
            .border_color(theme.sidebar_border)
            .child(div().flex_1().text_sm().font_medium().child("Workers"))
            .child(
                Button::new("workers-menu")
                    .ghost()
                    .xsmall()
                    .compact()
                    .icon(Icon::new(Lucide::Ellipsis))
                    .tooltip("More")
                    .dropdown_menu(move |menu, _, _| {
                        let view = view.clone();
                        menu.item(
                            PopupMenuItem::new("Stop all workers")
                                .disabled(!running)
                                .on_click(move |_, _, cx| {
                                    view.update(cx, |this, cx| this.stop_all(cx)).ok();
                                }),
                        )
                    }),
            )
            .child(close_button())
    }

    fn render_list(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let now = self.now();
        let rows = self.rows(now);
        if rows.is_empty() {
            return div()
                .id("workers-empty")
                .test_support()
                .flex_1()
                .p_3()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("No workers in this session")
                .into_any_element();
        }
        let focused = self.focus_handle.contains_focused(window, cx);
        let cursor = focused.then(|| self.cursor_index(&rows)).flatten();
        let height = WorkerRow::height(window);
        let children: Vec<AnyElement> = rows
            .iter()
            .enumerate()
            .map(|(index, row)| {
                let selected = cursor == Some(index);
                match row {
                    PanelRow::Worker(id) => self.render_row(index, id, height, selected, cx),
                    PanelRow::Finished(count) => self.render_group(index, *count, selected, cx),
                }
            })
            .collect();
        v_flex()
            .id("workers-list")
            .test_support()
            .flex_1()
            .min_h_0()
            .p_1()
            .gap_0p5()
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .children(children)
            .into_any_element()
    }

    /// A worker's row: the frame, which shows the keyboard's place and the
    /// pointer's, around the worker's own view.
    fn render_row(
        &self,
        index: usize,
        id: &SharedString,
        height: Pixels,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.row_views.get(id.as_ref()) else {
            return div().into_any_element();
        };
        let open = id.clone();
        row_base(("worker", index), selected, cx)
            .child(
                view.clone()
                    .cached(StyleRefinement::default().w_full().h(height)),
            )
            .on_click(
                cx.listener(move |this, _, window, cx| this.enter_worker(open.clone(), window, cx)),
            )
            .into_any_element()
    }

    /// What `worker`'s row shows at `now`.
    fn row_facts(&self, worker: &WorkerSummary, now: i64) -> RowFacts {
        RowFacts {
            id: worker.id.clone().into(),
            state: worker.state,
            title: if worker.title.is_empty() {
                worker.id.clone()
            } else {
                worker.title.clone()
            },
            elapsed: workers::run_time_ms(worker, now).map(workers::elapsed_label),
            last: workers::last_line(worker)
                .unwrap_or_else(|| workers::state_label(worker.state).to_string()),
            context: self.context_label(&worker.id),
            calls: worker.tool_calls,
        }
    }

    /// Brings the workers' rows up to date: a row whose facts changed draws
    /// again, the others keep what they drew.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let mut views = std::mem::take(&mut self.row_views);
        views.retain(|id, _| self.worker(id).is_some());
        for worker in &self.source.workers {
            let facts = self.row_facts(worker, now);
            match views.get(&worker.id) {
                Some(view) => view.update(cx, |row, cx| row.set(facts, cx)),
                None => {
                    views.insert(worker.id.clone(), cx.new(|_| WorkerRow { facts }));
                }
            }
        }
        self.row_views = views;
    }

    fn render_group(
        &self,
        index: usize,
        count: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let failed = self.finished_failures(self.now());
        let theme = cx.theme();
        let open = self.finished_open;
        row_base("workers-finished", selected, cx)
            .mt_1()
            .child(
                h_flex()
                    .gap_1p5()
                    .text_xs()
                    .font_medium()
                    .text_color(theme.muted_foreground)
                    .child(
                        Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .xsmall(),
                    )
                    .child(format!("Finished ({count})"))
                    // Failures show, even folded away.
                    .when(failed > 0, |this| {
                        this.child(
                            div()
                                .text_color(theme.danger)
                                .child(format!("· {failed} failed")),
                        )
                    }),
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                let rows = this.rows(this.now());
                this.finished_open = !this.finished_open;
                this.cursor = rows.get(index).cloned();
                this.focus_handle.focus(window, cx);
                cx.notify();
            }))
            .into_any_element()
    }

    /// What a worker's view says of it: "Running · 4 m · 18 calls · 88k /
    /// 200k".
    pub fn facts(&self, id: &str, now: i64) -> String {
        let mut facts: Vec<String> = Vec::new();
        if let Some(worker) = self.worker(id) {
            facts.push(workers::state_label(worker.state).to_string());
            if let Some(total) = workers::run_time_ms(worker, now) {
                facts.push(workers::elapsed_label(total));
            }
            facts.push(match worker.tool_calls {
                1 => "1 call".to_string(),
                calls => format!("{calls} calls"),
            });
        }
        facts.extend(self.context_label(id));
        facts.join(" · ")
    }

    /// "88k / 200k": the worker's context, when known.
    fn context_label(&self, id: &str) -> Option<String> {
        let agent = self.source.agents.iter().find(|agent| agent.id == id)?;
        let tokens = agent.context_tokens?;
        Some(match agent.context_window {
            Some(window) => format!(
                "{} / {}",
                workers::tokens_label(tokens),
                workers::tokens_label(window)
            ),
            None => workers::tokens_label(tokens),
        })
    }

    fn render_worker(&self, id: SharedString, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let now = self.now();
        let worker = self.worker(&id).cloned();
        let title = worker
            .as_ref()
            .map(|worker| worker.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_default();
        let active = worker
            .as_ref()
            .is_some_and(|worker| worker.state.is_active());
        let facts = self.facts(&id, now);
        let transcript = self.transcript().map(TranscriptView::cached);
        let (stop_id, message_id) = (id.clone(), id.clone());
        let full_title: SharedString = title.clone().into();
        v_flex()
            .id("worker-view")
            .test_support()
            .key_context(WORKER_CONTEXT)
            .track_focus(&self.worker_focus)
            .on_action(cx.listener(Self::back))
            // Esc on the worker's rows goes back to the list too.
            .on_action(
                cx.listener(|this, _: &crate::transcript::Leave, window, cx| {
                    this.show_list(window, cx)
                }),
            )
            .flex_1()
            .min_h_0()
            .child(
                h_flex()
                    .flex_none()
                    .h_10()
                    .pl_1()
                    .pr_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .child(
                        Button::new("workers-back")
                            .ghost()
                            .xsmall()
                            .compact()
                            .icon(Icon::new(Lucide::ChevronLeft))
                            .tooltip_with_action("All workers", &Back, Some(WORKER_CONTEXT))
                            .on_click(
                                cx.listener(|this, _, window, cx| this.show_list(window, cx)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font_family.clone())
                            .text_sm()
                            .child(id.clone()),
                    )
                    .child(
                        div()
                            .id("worker-title")
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_sm()
                            .font_medium()
                            .child(title)
                            .tooltip(move |window, cx| {
                                Tooltip::new(full_title.clone()).build(window, cx)
                            }),
                    )
                    .child(close_button()),
            )
            .child(
                h_flex()
                    .flex_none()
                    .px_3()
                    .py_1()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.sidebar_border)
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(
                        div()
                            .id("worker-facts")
                            .test_support()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_features(tabular_figures())
                            .child(facts),
                    )
                    .child(
                        Button::new("worker-message")
                            .ghost()
                            .xsmall()
                            .label("Message")
                            .tooltip(format!("Write to {id} in the composer"))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.message(message_id.clone(), cx)
                            })),
                    )
                    .when(active, |this| {
                        this.child(
                            Button::new("worker-stop")
                                .ghost()
                                .xsmall()
                                .label("Stop")
                                .tooltip("Stop this worker (Arko can resume it)")
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.stop(&stop_id, cx)),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .bg(theme.background)
                    .children(transcript),
            )
            .into_any_element()
    }
}

/// The close button of the panel's headers.
fn close_button() -> Button {
    Button::new("close-workers")
        .ghost()
        .xsmall()
        .compact()
        .icon(IconName::Close)
        .tooltip_with_action("Close", &menus::ToggleWorkersPanel, None)
        .accessibility_label("Close workers")
        .on_click(|_, window, cx| window.dispatch_action(Box::new(menus::ToggleWorkersPanel), cx))
}

/// A row of the list: a frame that shows the keyboard's place and the
/// pointer's.
fn row_base(
    id: impl Into<ElementId>,
    selected: bool,
    cx: &App,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    let theme = cx.theme();
    v_flex()
        .id(id)
        .test_support()
        .w_full()
        .px_2()
        .py_1p5()
        .gap_0p5()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.transparent)
        .text_sm()
        .when(selected, |this| this.border_color(theme.ring))
        .hover(|this| {
            this.bg(theme.sidebar_accent.opacity(0.8))
                .text_color(theme.sidebar_accent_foreground)
        })
}

/// Digits of equal width (OpenType `tnum`), so figures in rows line up.
fn tabular_figures() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

/// A figure at the end of a row, in tabular figures: right-aligned rows of
/// them line up.
fn figures(text: impl Into<SharedString>) -> Div {
    div()
        .flex_none()
        .font_features(tabular_figures())
        .child(text.into())
}

/// `text` in a cell at least `width` wide, at its `right` or left side: the
/// cells of every row are as wide, so what is in them lines up.
fn cell(text: impl Into<SharedString>, width: Pixels, right: bool) -> Div {
    h_flex().flex_none().min_w(width).child(
        div()
            .flex_1()
            .when(right, |this| this.text_right())
            .child(text.into()),
    )
}

/// How wide `text` is among a row's figures: `text_xs`, in tabular figures.
fn figures_width(text: &'static str, window: &Window) -> Pixels {
    let mut style = window.text_style();
    style.font_size = rems(0.75).into();
    style.font_features = tabular_figures();
    let size = style.font_size.to_pixels(window.rem_size());
    window
        .text_system()
        .shape_line(text.into(), size, &[style.to_run(text.len())], None)
        .width
        // As a line of text lays itself out: in whole pixels.
        .ceil()
}

/// "18 calls", with the number right-aligned in room for three figures and
/// the word after it, so 1 to 999 calls line up.
fn calls(count: u32, window: &Window) -> impl IntoElement {
    h_flex()
        .flex_none()
        .gap_1()
        .font_features(tabular_figures())
        .child(cell(count.to_string(), figures_width("999", window), true))
        .child(cell(
            if count == 1 { "call" } else { "calls" },
            figures_width("calls", window),
            false,
        ))
}

/// A worker's state as a mark: a spinner while it runs, a ring while it
/// waits, a filled circle once it ended (in `success` when it finished, in
/// `danger` when it failed).
fn state_mark(state: WorkerState, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let dot = |color: Hsla| div().size_2().rounded_full().bg(color);
    let mark: AnyElement = match state {
        WorkerState::Running => Spinner::new()
            .xsmall()
            .color(theme.muted_foreground)
            .into_any_element(),
        WorkerState::Queued => div()
            .size_2()
            .rounded_full()
            .border_1()
            .border_color(theme.muted_foreground)
            .into_any_element(),
        WorkerState::Finished => dot(theme.success).into_any_element(),
        WorkerState::Failed => dot(theme.danger).into_any_element(),
        WorkerState::Stopped => dot(theme.muted_foreground).into_any_element(),
    };
    let label = workers::state_label(state);
    div()
        .id("worker-state")
        .flex_none()
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
        .tooltip(move |window, cx| Tooltip::new(label).build(window, cx))
}

/// What a worker's row shows.
#[derive(Clone, PartialEq)]
struct RowFacts {
    id: SharedString,
    state: WorkerState,
    title: String,
    elapsed: Option<String>,
    last: String,
    context: Option<String>,
    calls: u32,
}

/// A worker's row, as a view of its own: it draws again only when what it
/// shows changes. A running worker's spinner, which asks for a frame 30
/// times a second, draws its own row again and leaves the others as they
/// were drawn.
struct WorkerRow {
    facts: RowFacts,
}

impl WorkerRow {
    fn set(&mut self, facts: RowFacts, cx: &mut Context<Self>) {
        if self.facts != facts {
            self.facts = facts;
            cx.notify();
        }
    }

    /// The heights of its lines: the title's, in `text_sm`, and the
    /// figures', in `text_xs`.
    fn lines(window: &Window) -> (Pixels, Pixels) {
        let rem = window.rem_size();
        let line = |size: Rems| {
            let mut style = window.text_style();
            style.font_size = size.into();
            style.line_height_in_pixels(rem)
        };
        // The state's mark is 1 rem high.
        let title = line(rems(0.875)).max(rems(1.).to_pixels(rem));
        (title, line(rems(0.75)))
    }

    /// Its height: a cached view is laid out at a set size, not measured.
    fn height(window: &Window) -> Pixels {
        let (title, figures) = Self::lines(window);
        title + rems(0.125).to_pixels(window.rem_size()) + figures
    }
}

impl Render for WorkerRow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        ROWS_DRAWN.with(|drawn| {
            *drawn
                .borrow_mut()
                .entry(self.facts.id.to_string())
                .or_default() += 1
        });
        let theme = cx.theme();
        let (title_line, figures_line) = Self::lines(window);
        let facts = &self.facts;
        v_flex()
            .size_full()
            .gap_0p5()
            .child(
                h_flex()
                    .w_full()
                    .h(title_line)
                    .min_w_0()
                    .gap_1p5()
                    .child(state_mark(facts.state, cx))
                    .child(
                        div()
                            .flex_none()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(facts.id.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(facts.title.clone()),
                    )
                    .children(
                        facts
                            .elapsed
                            .clone()
                            .map(|elapsed| figures(elapsed).text_xs()),
                    ),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(figures_line)
                    .min_w_0()
                    .gap_1p5()
                    // Under the state's mark, so the line starts where the
                    // worker's id does.
                    .child(div().flex_none().w_4())
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(facts.last.clone()),
                            )
                            .children(facts.context.clone().map(figures))
                            .child(calls(facts.calls, window)),
                    ),
            )
    }
}

#[cfg(test)]
#[path = "workers_panel_tests.rs"]
mod tests;

#[cfg(test)]
thread_local! {
    /// How many times each worker's row was drawn.
    static ROWS_DRAWN: std::cell::RefCell<std::collections::HashMap<String, usize>> =
        std::cell::RefCell::default();
    /// How many times the panel read its session's workers.
    static PULLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Render for WorkersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let now = self.now();
        self.tick(now, cx);
        let theme = cx.theme();
        let (background, foreground) = (theme.sidebar, theme.sidebar_foreground);
        // A worker of another session, or one gone: back to the list.
        if self
            .open
            .as_ref()
            .is_some_and(|open| self.worker(open).is_none() || self.transcript().is_none())
        {
            self.open = None;
        }
        let body = match self.open.clone() {
            Some(id) => self.render_worker(id, cx),
            None => v_flex()
                .id("workers-list-view")
                .key_context(LIST_CONTEXT)
                .track_focus(&self.focus_handle)
                .on_action(cx.listener(Self::select_previous))
                .on_action(cx.listener(Self::select_next))
                .on_action(cx.listener(Self::select_first))
                .on_action(cx.listener(Self::select_last))
                .on_action(cx.listener(Self::tab_next))
                .on_action(cx.listener(Self::tab_previous))
                .on_action(cx.listener(Self::open_selected))
                .on_action(cx.listener(Self::back))
                .flex_1()
                .min_h_0()
                .child(self.render_header(cx))
                .child(self.render_list(window, cx))
                .into_any_element(),
        };
        v_flex()
            .id("workers-panel")
            .test_support()
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(body)
    }
}
