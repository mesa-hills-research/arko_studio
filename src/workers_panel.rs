//! The workers panel: the session's workers, opened on demand at the right
//! (docked on wide windows, a sheet on narrower ones). It never opens by
//! itself.
//!
//! The list shows the workers that run or wait, and those that ended in the
//! last five minutes, then a collapsed "Finished" group with the rest. Each
//! row gives the worker's state, task, cost and time over all its runs, then
//! its setup when it isn't the usual one, its last action, its context and
//! its calls, the figures right-aligned in tabular figures so they line up.
//! A worker's workers stand under it, joined to it by tree lines. Choosing a
//! worker shows its own transcript, with Stop and Message. While the
//! worker's view has the focus, the composer writes to that worker.

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
    nested_radius,
    tooltip::{ManagedTooltipExt as _, Tooltip},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::live::LiveSession;
use crate::menus;
use crate::transcript::{AgentFilter, TranscriptEvent, TranscriptView};
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
        /// The view the panel showed before this one, a worker's or the
        /// list.
        GoBack,
        /// The view that Back left.
        GoForward,
    ]
);

/// Makes Back and Forward, from the keys and from a mouse's side buttons,
/// step through `panel`'s views wherever in `element` they're pressed. The
/// panel takes them itself (it may be a sheet, outside the window's
/// panes), and the window takes them everywhere else.
pub fn step_through_views<E: InteractiveElement>(element: E, panel: &Entity<WorkersPanel>) -> E {
    let go = |forward: bool| {
        let panel = panel.downgrade();
        move |window: &mut Window, cx: &mut App| {
            panel
                .update(cx, |panel, cx| panel.go(forward, window, cx))
                .ok();
        }
    };
    // A press over the docked panel steps once, not again in the window.
    let press = |forward: bool| {
        let go = go(forward);
        move |_: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            go(window, cx);
        }
    };
    let (back, forward) = (go(false), go(true));
    element
        .on_action(move |_: &GoBack, window, cx| back(window, cx))
        .on_action(move |_: &GoForward, window, cx| forward(window, cx))
        .on_mouse_down(
            MouseButton::Navigate(NavigationDirection::Back),
            press(false),
        )
        .on_mouse_down(
            MouseButton::Navigate(NavigationDirection::Forward),
            press(true),
        )
}

/// The panel's views the user left, the list (`None`) or a worker's, for
/// Back and Forward.
#[derive(Debug, Default)]
struct ViewHistory {
    back: Vec<Option<SharedString>>,
    forward: Vec<Option<SharedString>>,
}

impl ViewHistory {
    /// The user went from `from` to another view, so Forward has nothing.
    fn leave(&mut self, from: Option<SharedString>) {
        self.back.push(from);
        self.forward.clear();
    }

    /// The view before `current` (after it, `forward`), if there is one.
    fn step(
        &mut self,
        current: Option<SharedString>,
        forward: bool,
    ) -> Option<Option<SharedString>> {
        let (from, to) = if forward {
            (&mut self.forward, &mut self.back)
        } else {
            (&mut self.back, &mut self.forward)
        };
        let view = from.pop()?;
        to.push(current);
        Some(view)
    }
}

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
    /// The running calls that can move to the background, by item.
    pub movable: Vec<u64>,
    /// What workers start with unless told otherwise, which the rows and
    /// the cards in a worker's transcript name only when a worker runs
    /// another.
    pub usual: workers::Usual,
    /// The workers whose model is billed per token, by id.
    pub billed_per_token: HashSet<String>,
    /// Changes when the session (or its opening) changes: the transcripts
    /// kept are another feed's then.
    pub key: Option<(EntityId, u64)>,
}

impl PanelSource {
    fn of(session: &LiveSession, id: EntityId) -> Self {
        let models = &session.reading().models;
        Self {
            billed_per_token: session
                .workers()
                .iter()
                .filter(|worker| workers::billed_per_token(worker, models))
                .map(|worker| worker.id.clone())
                .collect(),
            workers: session.workers().to_vec(),
            agents: session.reading().agents.clone(),
            feed: session.feed().cloned(),
            images: session.link().map(|link| link.images.clone()),
            movable: session.reading().movable.clone(),
            usual: workers::Usual::of(&session.reading().models),
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
    _events: Subscription,
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
    /// The views before and after the one that shows.
    history: ViewHistory,
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
            history: ViewHistory::default(),
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
            self.history = ViewHistory::default();
            self.cursor = None;
            self.finished_open = false;
            self.seen.clear();
            self.row_views.clear();
        }
        // The rows' chips name what isn't usual, and how a worker is billed.
        let same = source.workers == self.source.workers
            && source.agents == self.source.agents
            && source.usual == self.source.usual
            && source.billed_per_token == self.source.billed_per_token
            && source.key == self.source.key;
        if source.movable != self.source.movable {
            for shown in &self.transcripts {
                shown
                    .view
                    .update(cx, |view, cx| view.set_movable(&source.movable, cx));
            }
        }
        if source.usual != self.source.usual {
            for shown in &self.transcripts {
                shown
                    .view
                    .update(cx, |view, cx| view.set_usual(&source.usual, cx));
            }
        }
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

    /// The worker whose transcript shows.
    pub fn open_worker(&self) -> Option<&SharedString> {
        self.open.as_ref()
    }

    /// The list's rows at `now`: the current workers in the order they
    /// started, each followed by the workers it started, then the "Finished"
    /// group. A family moves to the group together.
    pub fn rows(&self, now: i64) -> Vec<PanelRow> {
        let workers = &self.source.workers;
        let tree = workers::tree(workers);
        let sections = self.sections(&tree, now);
        let id = |index: usize| PanelRow::Worker(workers[index].id.clone().into());
        let mut rows: Vec<PanelRow> = tree
            .iter()
            .filter(|placed| sections[placed.index] == Section::Current)
            .map(|placed| id(placed.index))
            .collect();
        let finished: Vec<&workers::Placed> = tree
            .iter()
            .filter(|placed| sections[placed.index] == Section::Finished)
            .collect();
        if !finished.is_empty() {
            rows.push(PanelRow::Finished(finished.len()));
            if self.finished_open {
                // The family whose first worker ended last first, each family
                // in its own order.
                let mut roots: Vec<usize> = finished
                    .iter()
                    .filter(|placed| placed.root == placed.index)
                    .map(|placed| placed.index)
                    .collect();
                roots.sort_by_key(|root| std::cmp::Reverse(workers[*root].ended_at));
                for root in roots {
                    rows.extend(
                        finished
                            .iter()
                            .filter(|placed| placed.root == root)
                            .map(|placed| id(placed.index)),
                    );
                }
            }
        }
        rows
    }

    /// Where each worker shows at `now`: a failure stays until opened, and a
    /// worker's workers stay with it.
    fn sections(&self, tree: &[workers::Placed], now: i64) -> Vec<Section> {
        workers::family_sections(&self.source.workers, tree, now, |id| self.seen.contains(id))
    }

    /// How many workers of the "Finished" group failed.
    pub fn finished_failures(&self, now: i64) -> usize {
        let tree = workers::tree(&self.source.workers);
        let sections = self.sections(&tree, now);
        tree.iter()
            .filter(|placed| {
                sections[placed.index] == Section::Finished
                    && self.source.workers[placed.index].state == WorkerState::Failed
            })
            .count()
    }

    fn worker(&self, id: &str) -> Option<&WorkerSummary> {
        self.source.workers.iter().find(|worker| worker.id == id)
    }

    /// Shows the worker `id`'s own transcript. Back returns to the view
    /// before.
    pub fn show_worker(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if self.source.feed.is_some() && self.open.as_ref() != Some(&id) {
            self.history.leave(self.open.clone());
        }
        self.open_transcript(id, window, cx);
    }

    /// Shows the worker `id`'s own transcript, making it unless it's kept.
    fn open_transcript(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
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
            let (movable, usual) = (self.source.movable.clone(), self.source.usual.clone());
            view.update(cx, |view, cx| {
                view.set_movable(&movable, cx);
                view.set_usual(&usual, cx);
            });
            let events = cx.subscribe_in(&view, window, Self::on_transcript_event);
            self.transcripts.push(Shown {
                id: id.clone(),
                view,
                _events: events,
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

    /// Moves a worker's call to the background, stops its job or opens a
    /// worker, as its transcript's row asks. The card of a worker it started
    /// opens that worker's transcript.
    fn on_transcript_event(
        &mut self,
        _: &Entity<TranscriptView>,
        event: &TranscriptEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let TranscriptEvent::OpenWorker(id) = event {
            self.enter_worker(id.clone(), window, cx);
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        match event {
            TranscriptEvent::MoveToBackground(item) => {
                session.update(cx, |session, cx| session.move_call(*item, cx));
            }
            TranscriptEvent::StopJob(job) => {
                session.update(cx, |session, cx| session.stop_job(job, cx));
            }
            _ => {}
        }
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
        if let Some(open) = self.open.take() {
            self.history.leave(Some(open));
            self.focus_handle.focus(window, cx);
            cx.notify();
        }
    }

    /// Shows the view the panel showed before this one (after it,
    /// `forward`), while the panel shows. A worker that is gone shows the
    /// list. The keyboard stays where it is, unless it was in the panel,
    /// where it goes to the view shown.
    pub fn go(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.shown {
            return;
        }
        let Some(view) = self.history.step(self.open.clone(), forward) else {
            return;
        };
        let had_focus = self.contains_focus(window, cx);
        match view.filter(|id| self.worker(id).is_some()) {
            Some(id) => self.open_transcript(id, window, cx),
            None => {
                self.open = None;
                cx.notify();
            }
        }
        if had_focus {
            self.focus(window, cx);
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

    /// The place among `rows` of the row that shows the keyboard cursor's
    /// ring. It shows while the list has the keyboard and the last input
    /// was a key, so a click rings no row (its press focuses the list while
    /// the cursor is still on the row before).
    fn cursor_ring(&self, rows: &[PanelRow], window: &Window, cx: &App) -> Option<usize> {
        let keyed =
            window.last_input_was_keyboard() && self.focus_handle.contains_focused(window, cx);
        keyed.then(|| self.cursor_index(rows)).flatten()
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
                .child("No workers")
                .into_any_element();
        }
        let cursor = self.cursor_ring(&rows, window, cx);
        let height = WorkerRow::height(window);
        let levels: Vec<Option<usize>> = rows
            .iter()
            .map(|row| match row {
                PanelRow::Worker(id) => self
                    .row_views
                    .get(id.as_ref())
                    .map(|view| view.read(cx).facts.level),
                PanelRow::Finished(_) => None,
            })
            .collect();
        let lines = tree_lines(&levels);
        let geometry = LineGeometry::new(window, cx);
        let children: Vec<AnyElement> = rows
            .iter()
            .zip(&lines)
            .enumerate()
            .map(|(index, (row, lines))| {
                let selected = cursor == Some(index);
                match row {
                    PanelRow::Worker(id) => {
                        let lines = lines.guides(&geometry);
                        self.render_row(index, id, height, selected, lines, cx)
                    }
                    PanelRow::Finished(count) => self.render_group(index, *count, selected, cx),
                }
            })
            .collect();
        v_flex()
            .id("workers-list")
            .test_support()
            .role(Role::List)
            .aria_label(SharedString::from("Workers"))
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
    /// pointer's, around the worker's own view, with the tree `lines` that
    /// join it to its family.
    fn render_row(
        &self,
        index: usize,
        id: &SharedString,
        height: Pixels,
        selected: bool,
        lines: Vec<Div>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(view) = self.row_views.get(id.as_ref()) else {
            return div().into_any_element();
        };
        let open = id.clone();
        let label = self
            .worker(id)
            .map(|worker| self.row_label(worker))
            .unwrap_or_default();
        let tip = (id.clone(), cx.entity().downgrade());
        row_base(("worker", index), selected, cx)
            .role(Role::ListItem)
            .aria_label(SharedString::from(label))
            .children(lines)
            .child(
                view.clone()
                    .cached(StyleRefinement::default().w_full().h(height)),
            )
            .managed_tooltip(move |window, cx| {
                let (id, panel) = &tip;
                let text = panel
                    .read_with(cx, |panel, _| panel.row_tip(id))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                Tooltip::new(text).build(window, cx)
            })
            .on_click(
                cx.listener(move |this, _, window, cx| this.enter_worker(open.clone(), window, cx)),
            )
            .into_any_element()
    }

    /// What `worker`'s row shows at `now`, `level` workers down the list's
    /// families.
    fn row_facts(&self, worker: &WorkerSummary, level: usize, now: i64) -> RowFacts {
        RowFacts {
            id: worker.id.clone().into(),
            state: worker.state,
            title: if worker.title.is_empty() {
                worker.id.clone()
            } else {
                worker.title.clone()
            },
            elapsed: workers::run_time_ms(worker, now).map(workers::elapsed_label),
            cost: workers::cost_label(worker),
            last: workers::last_line(worker)
                .unwrap_or_else(|| workers::state_label(worker.state).to_string()),
            chip: self.chip(worker),
            context: self.context_label(&worker.id),
            calls: worker.tool_calls,
            level,
        }
    }

    /// The setup that sets `worker` apart in its row, when it isn't the one
    /// workers usually run ([`workers::card_setup`]), and "API" for a model
    /// billed per token, as the model lists say it.
    fn chip(&self, worker: &WorkerSummary) -> Option<String> {
        let parts: Vec<String> = workers::card_setup(worker, &self.source.usual)
            .into_iter()
            .chain(
                self.billed_per_token(worker)
                    .then(|| workers::BILLED_PER_TOKEN_SHORT.to_string()),
            )
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    /// What the pointer resting on worker `id`'s row shows, a line each for
    /// its state, its setup in full, what it cost alone and with its
    /// workers, and why it failed.
    fn row_tip(&self, id: &str) -> Option<String> {
        let worker = self.worker(id)?;
        let mut lines = vec![workers::state_label(worker.state).to_string()];
        lines.extend(match workers::narrowed_sentence(worker) {
            Some(sentence) if self.billed_per_token(worker) => {
                Some(format!("{sentence} Billed per token."))
            }
            Some(sentence) => Some(sentence),
            None => self.setup(worker, workers::BILLED_PER_TOKEN),
        });
        lines.extend(workers::costs_label(worker));
        lines.extend(workers::failure(worker).map(str::to_string));
        Some(lines.join("\n"))
    }

    /// `worker`'s setup in a few words ([`workers::setup_label`]), and
    /// `billed` for a model billed per token: "billed per token" as Arko's
    /// list of workers says it, or "API" where room is short.
    fn setup(&self, worker: &WorkerSummary, billed: &str) -> Option<String> {
        let parts: Vec<String> = workers::setup_label(worker)
            .into_iter()
            .chain(self.billed_per_token(worker).then(|| billed.to_string()))
            .collect();
        (!parts.is_empty()).then(|| parts.join(" · "))
    }

    fn billed_per_token(&self, worker: &WorkerSummary) -> bool {
        self.source.billed_per_token.contains(&worker.id)
    }

    /// A worker's row as a screen reader names it ([`workers::row_label`]),
    /// then its setup, "billed per token" for a model billed per token, and
    /// what it cost, which the row shows in part.
    fn row_label(&self, worker: &WorkerSummary) -> String {
        let mut parts = vec![workers::row_label(worker)];
        parts.extend(workers::setup_words(worker));
        if self.billed_per_token(worker) {
            parts.push(workers::BILLED_PER_TOKEN.to_string());
        }
        parts.extend(workers::cost_label(worker));
        parts.extend(workers::family_cost_label(worker));
        parts.join(", ")
    }

    /// Brings the workers' rows up to date: a row whose facts changed draws
    /// again, the others keep what they drew.
    fn sync_rows(&mut self, cx: &mut Context<Self>) {
        let now = self.now();
        let tree = workers::tree(&self.source.workers);
        let mut views = std::mem::take(&mut self.row_views);
        views.retain(|id, _| self.worker(id).is_some());
        for placed in &tree {
            let worker = &self.source.workers[placed.index];
            let facts = self.row_facts(worker, placed.level, now);
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

    /// What a worker's view says of it: "Running · under r1 · sol · high · 4 m
    /// · 18 calls · 88k / 200k · $0.12 · $0.42 with its workers".
    pub fn facts(&self, id: &str, now: i64) -> String {
        let mut facts: Vec<String> = Vec::new();
        let worker = self.worker(id);
        if let Some(worker) = worker {
            facts.push(workers::state_label(worker.state).to_string());
            facts.extend(
                worker
                    .parent
                    .as_ref()
                    .map(|parent| format!("under {parent}")),
            );
            facts.extend(self.setup(worker, workers::BILLED_PER_TOKEN));
            if let Some(total) = workers::run_time_ms(worker, now) {
                facts.push(workers::elapsed_label(total));
            }
            facts.push(match worker.tool_calls {
                1 => "1 call".to_string(),
                calls => format!("{calls} calls"),
            });
        }
        facts.extend(self.context_label(id));
        facts.extend(worker.and_then(workers::costs_label));
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
        // The skills and servers it was narrowed to, in full.
        let narrowed = worker.as_ref().and_then(workers::narrowed_sentence);
        let with_workers = self
            .source
            .workers
            .iter()
            .any(|other| other.parent.as_deref() == Some(id.as_ref()));
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
                            .managed_tooltip(move |window, cx| {
                                Tooltip::new(full_title.clone()).build(window, cx)
                            })
                            .child(title),
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
                            .when_some(narrowed, |this, narrowed| {
                                this.managed_tooltip(move |window, cx| {
                                    Tooltip::new(narrowed.clone()).build(window, cx)
                                })
                            })
                            .child(facts),
                    )
                    .child(
                        Button::new("worker-message")
                            .ghost()
                            .xsmall()
                            .label("Message")
                            .tooltip(format!("Write to {id} in the message box"))
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
                                .tooltip(workers::stop_tip(with_workers))
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

/// The tree lines beside a worker's row, which join it to the worker that
/// started it and to that worker's other workers, as in a file tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct TreeLines {
    /// How many workers the row's worker is under.
    level: usize,
    /// For each level from 1 up to the row's own, whether the line of that
    /// level's workers runs past the row to another worker further down.
    passing: Vec<bool>,
    /// Whether the line from the row's starter goes on past the row to
    /// another of its workers (├), or ends at the row (└).
    continues: bool,
    /// Whether a line leaves from under the row's mark down to its workers.
    starts: bool,
}

/// The tree lines of each row of a list of rows `levels` deep, in the order
/// they show, each worker followed by its workers. `None` is a row that isn't
/// a worker's, such as a group's header, which no line crosses.
fn tree_lines(levels: &[Option<usize>]) -> Vec<TreeLines> {
    let mut lines = vec![TreeLines::default(); levels.len()];
    // Seen from the row reached, going up the list: for each level, whether a
    // row of that level lies further down with no row less deep in between.
    let mut below: Vec<bool> = Vec::new();
    let mut next: Option<usize> = None;
    for (index, level) in levels.iter().enumerate().rev() {
        let Some(level) = *level else {
            below.clear();
            next = None;
            continue;
        };
        below.resize(below.len().max(level + 1), false);
        lines[index] = TreeLines {
            level,
            passing: below.get(1..level).unwrap_or_default().to_vec(),
            continues: level > 0 && below[level],
            starts: next == Some(level + 1),
        };
        // Deeper lines stop at this row, and its own goes on up from it.
        below.truncate(level + 1);
        below[level] = true;
        next = Some(level);
    }
    lines
}

/// Where a row's tree lines go in its frame, measured from inside the
/// frame's border.
struct LineGeometry {
    /// How far a level stands in, a mark's width.
    indent: Pixels,
    /// Half a mark's width and height.
    half: Pixels,
    /// The middle of a top-level worker's mark, from the frame's left.
    mark_x: Pixels,
    /// The middle of a row's mark, from the frame's top.
    mark_y: Pixels,
    /// How far a line goes on above the frame to meet the row above, across
    /// the frame's border and the gap between rows.
    above: Pixels,
    /// How far a line goes on below the frame, across its border, where the
    /// next row's line takes over.
    below: Pixels,
    /// The curve where an edge leaves its line.
    radius: Pixels,
    color: Hsla,
}

impl LineGeometry {
    fn new(window: &Window, cx: &App) -> Self {
        let theme = cx.theme();
        let rem = window.rem_size();
        let (title, _) = WorkerRow::lines(window);
        // `row_base`'s padding, and the list's gap between rows.
        let (padding_x, padding_y, gap) = (rem * 0.5, rem * 0.375, rem * 0.125);
        let border = px(1.);
        let half = rem * 0.5;
        Self {
            indent: rem,
            half,
            mark_x: padding_x + half,
            mark_y: padding_y + title / 2.,
            above: border + gap,
            below: border,
            radius: nested_radius(theme.radius, padding_x),
            // Quiet, behind the marks and the text. A light theme's border
            // nearly vanishes on its panel, so the lines take a little of
            // the muted text's colour there.
            color: if theme.is_dark() {
                theme.sidebar_border
            } else {
                theme
                    .sidebar_border
                    .blend(theme.muted_foreground.opacity(0.25))
            },
        }
    }
}

impl TreeLines {
    /// The lines, as 1 px strokes placed in the row's frame.
    fn guides(&self, geometry: &LineGeometry) -> Vec<Div> {
        let g = geometry;
        let stroke = px(1.);
        // The line of `level`'s workers runs under the marks of their
        // starters, one level less in.
        let column = |level: usize| g.mark_x + g.indent * (level - 1) as f32;
        let line = |x: Pixels| div().absolute().left(x - stroke / 2.).w(stroke).bg(g.color);
        let mut guides: Vec<Div> = (1..)
            .zip(&self.passing)
            .filter(|(_, passing)| **passing)
            .map(|(level, _)| line(column(level)).top(-g.above).bottom(-g.below))
            .collect();
        if self.level > 0 {
            let x = column(self.level);
            // Down from the row above, then across to the left of the mark,
            // turning in a small curve.
            guides.push(
                div()
                    .absolute()
                    .left(x - stroke / 2.)
                    .top(-g.above)
                    .w(g.indent - g.half + stroke / 2.)
                    .h(g.above + g.mark_y + stroke / 2.)
                    .border_l(stroke)
                    .border_b(stroke)
                    .rounded_bl(g.radius)
                    .border_color(g.color),
            );
            if self.continues {
                let top = g.mark_y + stroke / 2. - g.radius;
                guides.push(line(x).top(top).bottom(-g.below));
            }
        }
        if self.starts {
            let x = column(self.level + 1);
            guides.push(line(x).top(g.mark_y + g.half).bottom(-g.below));
        }
        guides
    }
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
fn calls(count: u32, window: &Window) -> Div {
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
/// `danger` when it failed). The row's tooltip names it.
fn state_mark(state: WorkerState, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let dot = |color: Hsla| div().size_2().rounded_full().bg(color);
    let mark: AnyElement = match state {
        WorkerState::Running => crate::spinner::spinner()
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
        // Stopped, and a state a later arko adds.
        _ => dot(theme.muted_foreground).into_any_element(),
    };
    div()
        .flex_none()
        .size_4()
        .flex()
        .items_center()
        .justify_center()
        .child(mark)
}

/// What a worker's row shows.
#[derive(Clone, PartialEq)]
struct RowFacts {
    id: SharedString,
    state: WorkerState,
    title: String,
    elapsed: Option<String>,
    /// What its model calls cost, beside its time.
    cost: Option<String>,
    last: String,
    /// Its setup, when it isn't the one workers usually run, before its last
    /// action.
    chip: Option<String>,
    context: Option<String>,
    calls: u32,
    /// How many workers' workers it is: it stands in that far.
    level: usize,
}

/// A worker's row, as a view of its own: it draws again only when what it
/// shows changes. A running worker's spinner turns as it paints, so it draws
/// no row again.
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
        let rem = window.rem_size();
        let (title_line, figures_line) = Self::lines(window);
        let facts = &self.facts;
        v_flex()
            .size_full()
            .gap_0p5()
            // A worker's workers stand in under it, a mark's width a level.
            .pl(rems(facts.level as f32))
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
                            .cost
                            .clone()
                            .map(|cost| figures(cost).text_xs().text_color(theme.muted_foreground)),
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
                            .children(facts.chip.clone().map(|chip| {
                                // It takes at most 45% of the line, so the
                                // last action keeps a share (the tooltip
                                // has it whole), and shortens further only
                                // once the rest of the line has gone.
                                div()
                                    .id("worker-chip")
                                    .test_support()
                                    .min_w_0()
                                    .max_w(relative(0.45))
                                    .px_1()
                                    .rounded(nested_radius(theme.radius, rems(0.5).to_pixels(rem)))
                                    .bg(theme.muted)
                                    .truncate()
                                    .child(chip)
                            }))
                            .child(
                                // The context, then the calls, go down to
                                // a line out of sight, each whole, when
                                // the last action would have less than a
                                // few words' room. It takes the room they
                                // leave.
                                give_way(figures_line)
                                    .id("worker-figures")
                                    .test_support()
                                    .flex_1()
                                    .child(
                                        give_way(figures_line)
                                            .flex_grow(1.)
                                            .flex_basis(rems(6.))
                                            .child(
                                                div()
                                                    .id("worker-last")
                                                    .test_support()
                                                    .flex_grow(1.)
                                                    .flex_basis(rems(6.))
                                                    .min_w_0()
                                                    .truncate()
                                                    .child(facts.last.clone()),
                                            )
                                            .children(facts.context.clone().map(|context| {
                                                figures(context).id("worker-context").test_support()
                                            })),
                                    )
                                    .child(
                                        calls(facts.calls, window)
                                            .id("worker-calls")
                                            .test_support(),
                                    ),
                            ),
                    ),
            )
    }
}

/// A line `height` high whose items that have no room go down to a second
/// line, out of sight.
fn give_way(height: Pixels) -> Div {
    h_flex()
        .min_w_0()
        .h(height)
        .overflow_hidden()
        .flex_wrap()
        .content_start()
        .gap_x_2()
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
    /// How many times the panel was drawn.
    static PANEL_DRAWN: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Render for WorkersPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        PANEL_DRAWN.with(|drawn| drawn.set(drawn.get() + 1));
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
            .map(|this| step_through_views(this, &cx.entity()))
            .size_full()
            .bg(background)
            .text_color(foreground)
            .child(body)
    }
}
