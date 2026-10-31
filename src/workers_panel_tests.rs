//! The workers panel in a window of its own, on a real feed's workers.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Feed, WorkerChange, WorkerSummary};
use arko_agents::AgentEvent;
use arko_workflows::WorkflowEvent;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, SharedString, TestAppContext,
    WindowBounds, WindowOptions, px, size,
};

use super::{PanelRow, PanelSource, WorkersPanel, WorkersPanelEvent};
use crate::Startup;
use crate::backend::FakeBackend;
use crate::tests::press_mouse;
use crate::workers::ENDED_SHOWS_FOR_MS;

struct Panel {
    window: AnyWindowHandle,
    panel: Entity<WorkersPanel>,
    feed: Feed,
    events: Rc<RefCell<Vec<WorkersPanelEvent>>>,
    /// The panel's clock, in Unix milliseconds.
    now: Rc<RefCell<i64>>,
}

/// A frame of a 60 Hz display. The clock moves 1/60 second and the window
/// shows its next frame, which the spinners that turn in each frame ask for.
fn display_frame(cx: &mut TestAppContext, window: AnyWindowHandle) {
    cx.executor()
        .advance_clock(std::time::Duration::from_micros(16_667));
    cx.update_window(window, |_, window, cx| {
        window.simulate_next_frame(cx);
    })
    .unwrap();
    cx.run_until_parked();
}

/// A worker's text, as the workflow streams it.
fn say(feed: &Feed, agent: &str, text: &str) {
    feed.event(
        &WorkflowEvent {
            timestamp: chrono::Utc::now(),
            workflow_run_id: uuid::Uuid::nil(),
            agent_name: agent.into(),
            agent_run_id: uuid::Uuid::nil(),
            event: AgentEvent::AssistantResponseChunk(text.into()),
        },
        Some(agent),
    );
}

/// A feed whose turn started r1 (running), r2 (which finished) and r3
/// (which failed).
fn feed() -> Feed {
    let feed = Feed::new("main");
    feed.turn_started();
    feed.worker("r1", "Fix the flaky parser test", WorkerChange::Started);
    say(&feed, "r1", "Reading the parser first.");
    feed.worker("r2", "Write the changelog", WorkerChange::Started);
    feed.worker(
        "r2",
        "Write the changelog",
        WorkerChange::Finished {
            report: "Wrote the changelog for 0.4.",
        },
    );
    feed.worker("r3", "Profile the renderer", WorkerChange::Started);
    feed.worker(
        "r3",
        "Profile the renderer",
        WorkerChange::Failed {
            error: "the model failed",
        },
    );
    feed
}

fn source(feed: &Feed, key: gpui_kit::EntityId) -> PanelSource {
    let workers: Vec<WorkerSummary> = feed.subscribe().0.workers;
    PanelSource {
        workers,
        agents: Vec::new(),
        feed: Some(feed.clone()),
        images: None,
        movable: Vec::new(),
        usual: crate::workers::Usual::default(),
        billed_per_token: Default::default(),
        key: Some((key, 0)),
    }
}

fn open(cx: &mut TestAppContext) -> Panel {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let feed = feed();
    let now = Rc::new(RefCell::new(chrono::Utc::now().timestamp_millis() + 1_000));
    let events: Rc<RefCell<Vec<WorkersPanelEvent>>> = Rc::default();
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: Point::default(),
            size: size(px(400.), px(700.)),
        })),
        ..Default::default()
    };
    let (window, panel) = cx
        .update(|cx| {
            let feed = feed.clone();
            let now = now.clone();
            gpui_kit::open_window(options, cx, move |window, cx| {
                cx.new(|cx| {
                    let mut panel = WorkersPanel::new(window, cx);
                    let key = cx.entity_id();
                    panel.set_source(source(&feed, key), cx);
                    panel.set_clock(move || *now.borrow(), cx);
                    panel
                })
            })
        })
        .unwrap();
    cx.update(|cx| {
        let events = events.clone();
        cx.subscribe(&panel, move |_, event: &WorkersPanelEvent, _| {
            events.borrow_mut().push(event.clone())
        })
        .detach();
    });
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    let panel = Panel {
        window,
        panel,
        feed,
        events,
        now,
    };
    panel.draw(cx);
    panel
}

impl Panel {
    fn draw(&self, cx: &mut TestAppContext) {
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
        cx.run_until_parked();
    }

    fn rows(&self, cx: &mut TestAppContext) -> Vec<PanelRow> {
        let now = *self.now.borrow();
        cx.read(|cx| self.panel.read(cx).rows(now))
    }

    fn press(&self, cx: &mut TestAppContext, key: &str) {
        cx.update_window(self.window, |_, window, cx| window.press(key, cx))
            .unwrap();
        self.draw(cx);
    }

    fn click(&self, cx: &mut TestAppContext, id: impl Into<gpui_kit::ElementId>) {
        let id = id.into();
        cx.update_window(self.window, |_, window, cx| window.click(id, cx))
            .unwrap();
        self.draw(cx);
    }

    fn open_worker(&self, cx: &mut TestAppContext) -> Option<SharedString> {
        cx.read(|cx| self.panel.read(cx).open_worker().cloned())
    }

    /// The place of the row that shows the keyboard cursor's ring, and
    /// whether the list has the keyboard.
    fn ring(&self, cx: &mut TestAppContext) -> (Option<usize>, bool) {
        let now = *self.now.borrow();
        cx.update_window(self.window, |_, window, cx| {
            let panel = self.panel.read(cx);
            (
                panel.cursor_ring(&panel.rows(now), window, cx),
                panel.focus_handle.contains_focused(window, cx),
            )
        })
        .unwrap()
    }
}

/// The ring follows the keyboard alone. A click on a row rings neither the
/// row the keyboard was on nor the one clicked, while the button is held
/// or after it's let go.
#[gpui_kit::test]
fn a_click_rings_no_row_and_keys_ring_the_cursor(cx: &mut TestAppContext) {
    let panel = open(cx);
    *panel.now.borrow_mut() += ENDED_SHOWS_FOR_MS;
    panel.draw(cx);
    assert_eq!(
        panel.rows(cx),
        [worker("r1"), worker("r3"), PanelRow::Finished(1)]
    );
    cx.update_window(panel.window, |_, window, cx| {
        panel.panel.update(cx, |panel, cx| panel.focus(window, cx))
    })
    .unwrap();
    panel.press(cx, "down");
    assert_eq!(panel.ring(cx), (Some(1), true));

    press_mouse(cx, panel.window, "workers-finished", true);
    assert_eq!(panel.ring(cx), (None, true), "held");
    press_mouse(cx, panel.window, "workers-finished", false);
    panel.draw(cx);
    assert_eq!(panel.rows(cx).len(), 4, "the group opened");
    assert_eq!(panel.ring(cx), (None, true), "let go");

    // The keyboard goes on from the row clicked.
    panel.press(cx, "up");
    assert_eq!(panel.ring(cx), (Some(1), true));
}

fn worker(id: &str) -> PanelRow {
    PanelRow::Worker(id.into())
}

#[gpui_kit::test]
fn ended_workers_stay_five_minutes_then_fold_into_finished(cx: &mut TestAppContext) {
    let panel = open(cx);
    assert_eq!(
        panel.rows(cx),
        [worker("r1"), worker("r2"), worker("r3")],
        "the ended ones stay for now"
    );
    cx.update_window(panel.window, |_, window, _| {
        assert!(window.try_find(("worker", 0usize)).is_some());
        assert!(window.try_find("workers-finished").is_none());
    })
    .unwrap();
    // Five minutes on, the finished one moves to the collapsed group at the
    // bottom. The failed one stays until the user has opened it.
    *panel.now.borrow_mut() += ENDED_SHOWS_FOR_MS;
    panel.draw(cx);
    assert_eq!(
        panel.rows(cx),
        [worker("r1"), worker("r3"), PanelRow::Finished(1)]
    );
    panel.click(cx, ("worker", 1usize));
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r3"));
    panel.click(cx, "workers-back");
    assert_eq!(panel.rows(cx), [worker("r1"), PanelRow::Finished(2)]);
    // The group's header says one of them failed.
    let failed = cx.read(|cx| panel.panel.read(cx).finished_failures(*panel.now.borrow()));
    assert_eq!(failed, 1);
    // Opening the group lists them, still there to read.
    panel.click(cx, "workers-finished");
    let rows = panel.rows(cx);
    assert_eq!(rows[..2], [worker("r1"), PanelRow::Finished(2)]);
    assert_eq!(rows.len(), 4);
    assert!(rows.contains(&worker("r2")) && rows.contains(&worker("r3")));
}

/// A worker as a session lists it: `id`, under `parent`, in `state`, that
/// ended at `ended` if it did.
fn summary(
    id: &str,
    parent: Option<&str>,
    state: arko::remote::WorkerState,
    ended: Option<i64>,
) -> WorkerSummary {
    let mut worker = WorkerSummary::new(id);
    worker.title = format!("Task of {id}");
    worker.state = state;
    worker.parent = parent.map(str::to_string);
    worker.depth = if parent.is_some() { 2 } else { 1 };
    worker.started_at = Some(0);
    worker.ended_at = ended;
    worker
}

impl Panel {
    /// Shows `workers` in the panel, as the session's own source would.
    fn show(&self, cx: &mut TestAppContext, workers: Vec<WorkerSummary>) {
        self.panel.update(cx, |panel, cx| {
            let mut source = panel.source.clone();
            source.workers = workers;
            panel.set_source(source, cx);
        });
        self.draw(cx);
    }

    /// The bounds of the element `id` in row `row`, if it has one.
    fn bounds_in(
        &self,
        cx: &mut TestAppContext,
        row: usize,
        id: &'static str,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        cx.update_window(self.window, |_, window, _| {
            window
                .within(("worker", row))
                .try_find(id)
                .map(|found| found.bounds())
        })
        .unwrap()
    }

    /// Makes the workers start on `model` at `effort` unless told otherwise.
    fn set_usual(&self, cx: &mut TestAppContext, model: &str, effort: &str) {
        self.panel.update(cx, |panel, cx| {
            let mut source = panel.source.clone();
            source.usual = crate::workers::Usual {
                model: model.into(),
                effort: effort.into(),
            };
            panel.set_source(source, cx);
        });
        self.draw(cx);
    }

    /// What row `id` shows in its chip.
    fn chip(&self, cx: &mut TestAppContext, id: &str) -> Option<String> {
        cx.read(|cx| {
            self.panel.read(cx).row_views[id]
                .read(cx)
                .facts
                .chip
                .clone()
        })
    }

    fn row_height(&self, cx: &mut TestAppContext, row: usize) -> gpui_kit::Pixels {
        cx.update_window(self.window, |_, window, _| {
            window.find(("worker", row)).bounds().size.height
        })
        .unwrap()
    }
}

/// A worker's workers stand under it, indented, in the order of their
/// families, however the session lists them.
#[gpui_kit::test]
fn a_workers_workers_stand_under_it_indented(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let workers = vec![
        summary("r1", None, Running, None),
        summary("r4", None, Running, None),
        summary("r2", Some("r1"), Running, None),
        summary("r3", Some("r2"), Running, None),
    ];
    panel.show(cx, workers);
    assert_eq!(
        panel.rows(cx),
        [worker("r1"), worker("r2"), worker("r3"), worker("r4")]
    );
    let left =
        |cx: &mut TestAppContext, row| panel.bounds_in(cx, row, "worker-last").unwrap().left();
    let (r1, r2, r3, r4) = (left(cx, 0), left(cx, 1), left(cx, 2), left(cx, 3));
    // A level in is a mark's width (1 rem), and a worker of the main agent
    // is where it was.
    assert!(r2 > r1 && r3 > r2, "{r1:?} {r2:?} {r3:?}");
    assert_eq!(r2 - r1, r3 - r2);
    assert_eq!(r4, r1);
}

/// A worker's row joins the line of its starter's workers. That line goes on
/// past the row while another of them follows, past the row's own workers
/// too, and ends at the last one. Top-level workers have no line of their
/// own, and no line crosses the "Finished" group's header.
#[test]
fn tree_lines_join_each_worker_to_the_one_that_started_it() {
    use super::{TreeLines, tree_lines};
    let lines = |level, passing: &[bool], continues, starts| TreeLines {
        level,
        passing: passing.to_vec(),
        continues,
        starts,
    };
    let levels = [
        Some(0), // r1
        Some(1), //   r3
        Some(2), //     r7
        Some(3), //       r8
        Some(1), //   r4
        Some(0), // r2
        None,    // Finished
        Some(0), // r5
        Some(1), //   r6
        Some(1), //   r9
    ];
    assert_eq!(
        tree_lines(&levels),
        [
            // r1's line starts under its mark.
            lines(0, &[], false, true),
            // ├ r3, and its own line starts.
            lines(1, &[], true, true),
            // │ └ r7, with r1's line passing on to r4.
            lines(2, &[true], false, true),
            // │   └ r8, r3's line done with.
            lines(3, &[true, false], false, false),
            // └ r4, the last of r1's.
            lines(1, &[], false, false),
            lines(0, &[], false, false),
            TreeLines::default(),
            lines(0, &[], false, true),
            lines(1, &[], true, false),
            lines(1, &[], false, false),
        ]
    );
    // A line ends at a header, whatever comes after it.
    assert_eq!(
        tree_lines(&[Some(0), Some(1), None, Some(1)])[1],
        lines(1, &[], false, false)
    );
}

/// A screen reader hears whose worker a row is, as Arko's notices say it,
/// and the rows are a list of that name.
#[gpui_kit::test]
fn a_nested_row_says_whose_worker_it_is_to_a_screen_reader(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::{Failed, Running};
    let panel = open(cx);
    panel.show(
        cx,
        vec![
            summary("r1", None, Running, None),
            summary("r2", Some("r1"), Running, None),
            summary("r3", Some("r2"), Failed, Some(0)),
        ],
    );
    let named = |cx: &mut TestAppContext, row: usize| {
        cx.update_window(panel.window, |_, window, _| {
            let row = window.find(("worker", row));
            (row.role(), row.label().map(str::to_string))
        })
        .unwrap()
    };
    let item = Some(gpui_kit::Role::ListItem);
    assert_eq!(
        named(cx, 0),
        (item, Some("r1, Task of r1, Running".to_string()))
    );
    assert_eq!(
        named(cx, 1),
        (item, Some("r2 (under r1), Task of r2, Running".to_string()))
    );
    assert_eq!(
        named(cx, 2),
        (item, Some("r3 (under r2), Task of r3, Failed".to_string()))
    );
    let list = cx
        .update_window(panel.window, |_, window, _| {
            let list = window.find("workers-list");
            (list.role(), list.label().map(str::to_string))
        })
        .unwrap();
    assert_eq!(list, (Some(gpui_kit::Role::List), Some("Workers".into())));
}

/// The keyboard goes through a family in its order, a worker's worker opens
/// like any other with its own Stop, and its view says whose it is.
#[gpui_kit::test]
fn the_keyboard_goes_through_a_family_and_stops_a_nested_worker(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    panel.show(
        cx,
        vec![
            summary("r1", None, Running, None),
            summary("r2", None, Running, None),
            summary("r3", Some("r1"), Running, None),
            summary("r4", Some("r3"), Running, None),
        ],
    );
    let entity = panel.panel.clone();
    cx.update_window(panel.window, |_, window, cx| {
        entity.update(cx, |panel, cx| panel.focus(window, cx))
    })
    .unwrap();
    panel.draw(cx);
    let cursor = |cx: &mut TestAppContext| cx.read(|cx| panel.panel.read(cx).cursor.clone());
    assert_eq!(cursor(cx), Some(worker("r1")));
    let mut seen = Vec::new();
    for _ in 0..3 {
        panel.press(cx, "down");
        seen.push(cursor(cx).unwrap());
    }
    // r1, then its workers, then the next family.
    assert_eq!(seen, [worker("r3"), worker("r4"), worker("r2")]);
    panel.press(cx, "up");
    assert_eq!(cursor(cx), Some(worker("r4")));
    panel.press(cx, "enter");
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r4"));
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r4", *panel.now.borrow()));
    assert!(facts.starts_with("Running · under r3 · "), "{facts}");
    panel.click(cx, "worker-stop");
    assert_eq!(
        panel.events.borrow().last(),
        Some(&WorkersPanelEvent::Command("/stop-workers r4".into()))
    );
}

/// The card of a worker's worker is in the worker's own view, and opens the
/// worker in the panel.
#[gpui_kit::test]
fn the_card_of_a_workers_worker_opens_it_in_the_panel(cx: &mut TestAppContext) {
    use arko::remote::{Kind, WorkerInfo};
    let panel = open(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    feed.worker("r1", "Review the files", WorkerChange::Started);
    feed.worker_under("r2", "r1");
    feed.worker("r2", "Count the files", WorkerChange::Started);
    let mut info = WorkerInfo::new();
    info.depth = 2;
    feed.worker_info("r2", info);
    panel.panel.update(cx, |panel, cx| {
        let key = cx.entity_id();
        panel.set_source(source(&feed, key), cx);
    });
    panel.draw(cx);
    assert_eq!(panel.rows(cx), [worker("r1"), worker("r2")]);
    panel.click(cx, ("worker", 0usize));
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r1"));
    // The card of r2 is among r1's lines, which are the lines of its lane.
    let card = feed
        .subscribe()
        .0
        .items
        .iter()
        .find(|item| item.kind == Kind::WorkerLine && item.agent == "r1")
        .map(|item| item.id)
        .expect("r2's card is in r1's lane");
    panel.click(cx, ("worker-row", card));
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r2"));
}

/// The card of a worker's worker names the worker's model and effort only
/// when they aren't the ones the session's workers start with, as the main
/// transcript's cards do, and follows the session when they change.
#[gpui_kit::test]
fn the_card_in_a_workers_view_follows_the_models_workers_start_with(cx: &mut TestAppContext) {
    use arko::remote::{Kind, WorkerInfo};

    use crate::workers::Usual;

    let panel = open(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    feed.worker("r1", "Review the files", WorkerChange::Started);
    feed.worker_under("r2", "r1");
    feed.worker("r2", "Count the files", WorkerChange::Started);
    let mut info = WorkerInfo::new();
    info.depth = 2;
    info.model = Some("gpt-6.1-sol".into());
    info.effort = Some("high".into());
    feed.worker_info("r2", info);
    // Workers start on `model`, with high effort.
    let start_on = |cx: &mut TestAppContext, model: &str| {
        panel.panel.update(cx, |panel, cx| {
            let mut source = source(&feed, cx.entity_id());
            source.usual = Usual {
                model: model.into(),
                effort: "high".into(),
            };
            panel.set_source(source, cx);
        });
        panel.draw(cx);
    };
    start_on(cx, "gpt-6.1-sol");
    panel.click(cx, ("worker", 0usize));
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r1"));
    let card = feed
        .subscribe()
        .0
        .items
        .iter()
        .find(|item| item.kind == Kind::WorkerLine && item.agent == "r1")
        .map(|item| item.id)
        .expect("r2's card is in r1's lane");
    let chip = |cx: &mut TestAppContext| {
        cx.update_window(panel.window, |_, window, _| {
            window.try_find(("worker-setup", card)).is_some()
        })
        .unwrap()
    };
    assert!(!chip(cx), "r2 runs as workers usually do");
    start_on(cx, "gpt-6-luna");
    assert!(chip(cx), "workers start on another model now");
}

/// A family's cost reaches the rows from the feed's own sums: the worker
/// that started others says what they cost with it, and a worker of its own
/// says whose it is, in its view.
#[gpui_kit::test]
fn a_family_roots_row_says_what_its_workers_cost_with_it(cx: &mut TestAppContext) {
    use arko::remote::WorkerInfo;
    let panel = open(cx);
    let feed = Feed::new("main");
    feed.turn_started();
    feed.worker_under("r2", "r1");
    for id in ["r1", "r2", "r3"] {
        feed.worker(id, "Task", WorkerChange::Started);
    }
    let info = |parent: Option<&str>, cost: f64| {
        let mut info = WorkerInfo::new();
        info.depth = if parent.is_some() { 2 } else { 1 };
        info.costs = Some((cost, cost));
        info
    };
    feed.worker_info("r1", info(None, 0.12));
    feed.worker_info("r2", info(Some("r1"), 0.30));
    feed.worker_info("r3", info(None, 0.05));
    panel.show(cx, feed.subscribe().0.workers);
    let now = *panel.now.borrow();
    let facts =
        |cx: &mut TestAppContext, id: &str| cx.read(|cx| panel.panel.read(cx).facts(id, now));
    assert_eq!(
        facts(cx, "r1"),
        "Running · 0 s · 0 calls · $0.12 · $0.42 with its workers"
    );
    assert_eq!(
        facts(cx, "r2"),
        "Running · under r1 · 0 s · 0 calls · $0.30"
    );
    // Alone, a worker says only its own.
    assert_eq!(facts(cx, "r3"), "Running · 0 s · 0 calls · $0.05");
    // Calls a plan covered show at API prices.
    let mut covered = info(Some("r1"), 0.);
    covered.costs = Some((0., 0.9));
    feed.worker_info("r2", covered);
    panel.show(cx, feed.subscribe().0.workers);
    assert_eq!(
        facts(cx, "r1"),
        "Running · 0 s · 0 calls · $0.12 · ≈ $1.02 with its workers"
    );
}

/// A family moves to "Finished" together, with its first worker's place
/// among the other families in the order they ended.
#[gpui_kit::test]
fn a_family_moves_to_finished_together_and_stays_together(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::{Finished, Running};
    let panel = open(cx);
    let now = *panel.now.borrow();
    let long_ago = now - ENDED_SHOWS_FOR_MS - 10_000;
    panel.show(
        cx,
        vec![
            summary("r1", None, Finished, Some(long_ago - 60_000)),
            summary("r2", Some("r1"), Finished, Some(long_ago - 90_000)),
            summary("r3", None, Finished, Some(long_ago)),
            summary("r4", Some("r3"), Finished, Some(long_ago - 1_000)),
            summary("r5", None, Running, None),
        ],
    );
    // Both families ended long ago: they fold together.
    assert_eq!(panel.rows(cx), [worker("r5"), PanelRow::Finished(4)]);
    panel.click(cx, "workers-finished");
    // The family that ended last first, each family with its own workers
    // under it.
    assert_eq!(
        panel.rows(cx),
        [
            worker("r5"),
            PanelRow::Finished(4),
            worker("r3"),
            worker("r4"),
            worker("r1"),
            worker("r2"),
        ]
    );
    // A worker that ended a minute ago keeps its whole family in the list.
    panel.show(
        cx,
        vec![
            summary("r1", None, Finished, Some(long_ago)),
            summary("r2", Some("r1"), Finished, Some(now - 60_000)),
        ],
    );
    assert_eq!(panel.rows(cx), [worker("r1"), worker("r2")]);
}

/// Each row says what its worker runs on and what it has cost, on a line of
/// its own that every row has once one does, and the worker's view says it too.
/// A worker of a session opened again shows the tool calls it made before,
/// as the feed keeps them.
#[gpui_kit::test]
fn a_restored_worker_shows_its_saved_tool_calls(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Finished;
    let panel = open(cx);
    let now = *panel.now.borrow();
    panel.feed.worker_restored(
        "r4",
        "Port the lexer",
        Finished,
        Some("Ported."),
        None,
        now - 60_000,
        90_000,
        12,
    );
    let workers = panel.feed.subscribe().0.workers;
    panel.show(cx, workers);
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r4", now));
    assert!(facts.contains("12 calls"), "{facts}");
}

/// A row is two lines, whatever it says. It gives the worker's cost beside
/// its time, and names its setup in a chip only when the worker doesn't run
/// as workers usually do. Its tooltip and the name a screen reader hears
/// give the setup in full and what the worker cost with its workers, and
/// the worker's view says it all.
#[gpui_kit::test]
fn a_row_names_a_workers_setup_only_when_it_is_not_the_usual_one(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let mut plain = vec![
        summary("r1", None, Running, None),
        summary("r2", None, Running, None),
    ];
    panel.show(cx, plain.clone());
    let short = panel.row_height(cx, 0);
    plain[0].model = Some("gpt-6.1-sol".into());
    plain[0].effort = Some("high".into());
    plain[0].profile = Some("3d-judge".into());
    plain[0].cost_billed = 0.34;
    plain[0].cost_list = 0.34;
    plain[0].family_cost_billed = 0.5;
    plain[0].family_cost_list = 0.5;
    plain[1].model = Some("gpt-6.1-sol".into());
    plain[1].effort = Some("high".into());
    panel.show(cx, plain);
    assert_eq!(panel.row_height(cx, 0), short);
    assert_eq!(panel.row_height(cx, 1), short);
    // A profile is always named. A model is, until workers usually run it.
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("3d-judge"));
    assert_eq!(panel.chip(cx, "r2").as_deref(), Some("sol · high"));
    panel.set_usual(cx, "gpt-6.1-sol", "high");
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("3d-judge"));
    assert_eq!(panel.chip(cx, "r2"), None);
    assert!(panel.bounds_in(cx, 0, "worker-chip").is_some());
    assert!(panel.bounds_in(cx, 1, "worker-chip").is_none());
    let cost = cx.read(|cx| {
        let panel = panel.panel.read(cx);
        panel.row_views["r1"].read(cx).facts.cost.clone()
    });
    assert_eq!(cost.as_deref(), Some("$0.34"));
    let tip = |cx: &mut TestAppContext, id: &str| cx.read(|cx| panel.panel.read(cx).row_tip(id));
    assert_eq!(
        tip(cx, "r1").as_deref(),
        Some("Running\nsol · high · 3d-judge\n$0.34 · $0.50 with its workers")
    );
    assert_eq!(tip(cx, "r2").as_deref(), Some("Running\nsol · high"));
    let label = cx
        .update_window(panel.window, |_, window, _| {
            window.find(("worker", 0usize)).label().map(str::to_string)
        })
        .unwrap();
    assert_eq!(
        label.as_deref(),
        Some("r1, Task of r1, Running, sol, high, 3d-judge, $0.34, $0.50 with its workers")
    );
    let now = *panel.now.borrow();
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r1", now));
    assert_eq!(
        facts,
        "Running · sol · high · 3d-judge · 0 s · 0 calls · $0.34 · $0.50 with its workers"
    );
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r2", now));
    assert_eq!(facts, "Running · sol · high · 0 s · 0 calls");
    // A failure's tip ends with why it failed, whole.
    let mut failed = summary("r3", None, arko::remote::WorkerState::Failed, Some(0));
    failed.error = Some("the model failed: overloaded".into());
    panel.show(cx, vec![failed]);
    assert_eq!(
        tip(cx, "r3").as_deref(),
        Some("Failed\nthe model failed: overloaded")
    );
}

/// A worker on a model billed per token says so in its view as Arko's list
/// of workers does, and in its row's chip in a word, "API", even when it
/// runs the usual model. Its tooltip and a screen reader say it in full. One
/// on a plan says nothing of it.
#[gpui_kit::test]
fn a_worker_billed_per_token_says_so(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let mut api = summary("r1", None, Running, None);
    api.model = Some("claude-opus-5-5".into());
    api.effort = Some("high".into());
    let mut plan = summary("r2", None, Running, None);
    plan.model = Some("cc:claude-opus-5-5".into());
    panel.panel.update(cx, |panel, cx| {
        let mut source = panel.source.clone();
        source.workers = vec![api, plan];
        source.billed_per_token = ["r1".to_string()].into();
        panel.set_source(source, cx);
    });
    panel.draw(cx);
    let now = *panel.now.borrow();
    let facts =
        |id: &str, cx: &mut TestAppContext| cx.read(|cx| panel.panel.read(cx).facts(id, now));
    assert_eq!(
        facts("r1", cx),
        "Running · opus · high · billed per token · 0 s · 0 calls"
    );
    assert_eq!(facts("r2", cx), "Running · cc:opus · 0 s · 0 calls");
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("opus · high · API"));
    assert_eq!(panel.chip(cx, "r2").as_deref(), Some("cc:opus"));
    panel.set_usual(cx, "claude-opus-5-5", "high");
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("API"));
    assert_eq!(panel.chip(cx, "r2").as_deref(), Some("cc:opus"));
    let tip = |cx: &mut TestAppContext, id: &str| cx.read(|cx| panel.panel.read(cx).row_tip(id));
    assert_eq!(
        tip(cx, "r1").as_deref(),
        Some("Running\nopus · high · billed per token")
    );
    let label = |cx: &mut TestAppContext, row: usize| {
        cx.update_window(panel.window, |_, window, _| {
            window.find(("worker", row)).label().map(str::to_string)
        })
        .unwrap()
    };
    assert_eq!(
        label(cx, 0).as_deref(),
        Some("r1, Task of r1, Running, opus, high, billed per token")
    );
    assert_eq!(
        label(cx, 1).as_deref(),
        Some("r2, Task of r2, Running, cc:opus")
    );
}

/// A worker's billing that changes on its own (the session switched how
/// it reaches OpenAI) shows at once in its row.
#[gpui_kit::test]
fn a_change_of_billing_alone_shows_in_the_row(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let mut sol = summary("r1", None, Running, None);
    sol.model = Some("gpt-6.1-sol".into());
    panel.show(cx, vec![sol]);
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("sol"));
    panel.panel.update(cx, |panel, cx| {
        let mut source = panel.source.clone();
        source.billed_per_token = ["r1".to_string()].into();
        panel.set_source(source, cx);
    });
    panel.draw(cx);
    assert_eq!(panel.chip(cx, "r1").as_deref(), Some("sol · API"));
}

/// When a row's second line is short of room, the context and the calls
/// give way first, each whole, the context before the calls, and the last
/// action takes the room they leave. It keeps a few words until they've
/// gone, and the chip shortens last of all, except that it never takes more
/// than 45% of the line.
#[gpui_kit::test]
fn a_short_line_gives_up_the_figures_then_the_last_action_then_the_chip(cx: &mut TestAppContext) {
    use arko::remote::{AgentStatus, WorkerState::Running};
    let panel = open(cx);
    let mut worker = summary("r1", None, Running, None);
    worker.profile = Some("impl".into());
    worker.last = Some("read src/render/scene.rs:120-180".into());
    worker.tool_calls = 142;
    panel.panel.update(cx, |panel, cx| {
        let mut source = panel.source.clone();
        source.workers = vec![worker];
        source.agents = vec![AgentStatus {
            id: "r1".into(),
            context_tokens: Some(152_000),
            context_window: Some(1_000_000),
            handover_at: None,
            compacts: false,
        }];
        panel.set_source(source, cx);
    });
    let shown = |part: Option<gpui_kit::Bounds<gpui_kit::Pixels>>,
                 line: gpui_kit::Bounds<gpui_kit::Pixels>| {
        part.is_some_and(|part| {
            part.top() == line.top() && part.left() >= line.left() && part.right() <= line.right()
        })
    };
    let few_words = px(16. * 6.);
    let (mut chip_room, mut chip_shortened, mut figures_went) = (None, false, false);
    for width in (60..=700).rev().step_by(4) {
        cx.simulate_window_resize(panel.window, size(px(width as f32), px(700.)));
        panel.draw(cx);
        let bounds = |cx: &mut TestAppContext, id| panel.bounds_in(cx, 0, id);
        let line = bounds(cx, "worker-figures").unwrap();
        let context = shown(bounds(cx, "worker-context"), line);
        let calls = shown(bounds(cx, "worker-calls"), line);
        let last_bounds = bounds(cx, "worker-last").unwrap();
        let last = last_bounds.size.width;
        let chip = bounds(cx, "worker-chip").unwrap().size.width;
        let room = *chip_room.get_or_insert(chip);
        if width == 700 {
            assert!(context && calls, "both figures show at {width}");
        }
        assert!(calls || !context, "the context goes first, at {width}");
        if context || calls {
            assert!(last >= few_words - px(0.5), "{last:?} at {width}");
        } else {
            figures_went = true;
        }
        // Up to the next figure shown, or to the end of the line.
        let next = if context {
            bounds(cx, "worker-context").unwrap().left()
        } else if calls {
            bounds(cx, "worker-calls").unwrap().left()
        } else {
            line.right() + px(8.)
        };
        assert!(
            next - last_bounds.right() <= px(8.5),
            "{last_bounds:?} before {next:?} at {width}"
        );
        if chip < room - px(0.5) {
            chip_shortened = true;
            assert!(!context && !calls, "the chip shortens last, at {width}");
            let line = line.right() - bounds(cx, "worker-chip").unwrap().left();
            assert!(
                last < px(0.5) || chip <= line * 0.45 + px(0.5),
                "{last:?} beside {chip:?} at {width}"
            );
        }
    }
    assert!(figures_went && chip_shortened);
    // A long chip keeps to its share of the line.
    panel.panel.update(cx, |panel, cx| {
        let mut source = panel.source.clone();
        source.workers[0].profile = Some("a-profile-with-a-long-long-name".into());
        panel.set_source(source, cx);
    });
    cx.simulate_window_resize(panel.window, size(px(400.), px(700.)));
    panel.draw(cx);
    let chip = panel.bounds_in(cx, 0, "worker-chip").unwrap();
    let line = panel.bounds_in(cx, 0, "worker-figures").unwrap().right() - chip.left();
    assert!(
        chip.size.width <= line * 0.45 + px(0.5),
        "{chip:?} in {line:?}"
    );
    let last = panel.bounds_in(cx, 0, "worker-last").unwrap().size.width;
    assert!(last >= few_words - px(0.5), "{last:?}");
}

/// A worker narrowed to some skills and servers says how many in its row's
/// chip and its view, with the full sentence on hover. One that has what its
/// starter has reads as before.
#[gpui_kit::test]
fn a_narrowed_workers_row_counts_its_skills_and_servers(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let mut narrowed = summary("r1", None, Running, None);
    narrowed.model = Some("cc:claude-sonnet-5-5".into());
    narrowed.effort = Some("xhigh".into());
    narrowed.profile = Some("impl-light".into());
    let mut plain = narrowed.clone();
    plain.id = "r2".into();
    narrowed.skills = Some(vec!["ponytail".into(), "release-notes".into()]);
    narrowed.mcp = Some(Vec::new());
    panel.show(cx, vec![narrowed, plain]);
    let now = *panel.now.borrow();
    let facts =
        |id: &str, cx: &mut TestAppContext| cx.read(|cx| panel.panel.read(cx).facts(id, now));
    assert_eq!(
        facts("r1", cx),
        "Running · cc:sonnet · xhigh · impl-light · 2 skills · no MCP · 0 s · 0 calls"
    );
    assert_eq!(
        facts("r2", cx),
        "Running · cc:sonnet · xhigh · impl-light · 0 s · 0 calls"
    );
    assert_eq!(
        panel.chip(cx, "r1").as_deref(),
        Some("impl-light · 2 skills · no MCP")
    );
    // The row's tooltip says it in full.
    let tip = |id: &str, cx: &mut TestAppContext| cx.read(|cx| panel.panel.read(cx).row_tip(id));
    assert_eq!(
        tip("r1", cx).as_deref(),
        Some(
            "Running\nRuns cc:claude-sonnet-5-5 at xhigh effort, profile impl-light, with only the skills ponytail and release-notes and no MCP servers."
        )
    );
    assert_eq!(
        tip("r2", cx).as_deref(),
        Some("Running\ncc:sonnet · xhigh · impl-light")
    );
}

#[gpui_kit::test]
fn the_keyboard_moves_through_the_list_and_opens_a_worker(cx: &mut TestAppContext) {
    let panel = open(cx);
    let entity = panel.panel.clone();
    cx.update_window(panel.window, |_, window, cx| {
        entity.update(cx, |panel, cx| panel.focus(window, cx))
    })
    .unwrap();
    panel.draw(cx);
    // The focus lands on the first worker. ↓ and Tab move on, ↑ back.
    panel.press(cx, "down");
    panel.press(cx, "tab");
    panel.press(cx, "up");
    let cursor = cx.read(|cx| panel.panel.read(cx).cursor.clone());
    assert_eq!(cursor, Some(worker("r2")));
    panel.press(cx, "enter");
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r2"));
    // Its own transcript shows, ending with its report.
    let transcript = cx.read(|cx| panel.panel.read(cx).transcript().cloned());
    assert!(transcript.is_some());
    // Esc goes back to the list, and from the list to the composer.
    panel.press(cx, "escape");
    assert_eq!(panel.open_worker(cx), None);
    panel.press(cx, "escape");
    assert_eq!(
        panel.events.borrow().last(),
        Some(&WorkersPanelEvent::Leave)
    );
}

#[gpui_kit::test]
fn a_workers_view_writes_to_it_only_through_message(cx: &mut TestAppContext) {
    let panel = open(cx);
    panel.click(cx, ("worker", 0usize));
    assert_eq!(panel.open_worker(cx).as_deref(), Some("r1"));
    // Viewing a worker leaves the composer writing to Arko.
    assert!(
        panel.events.borrow().is_empty(),
        "{:?}",
        panel.events.borrow()
    );
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r1", *panel.now.borrow()));
    assert!(facts.starts_with("Running · 1 s · 0 calls"), "{facts}");
    panel.click(cx, "worker-stop");
    panel.click(cx, "worker-message");
    assert_eq!(
        *panel.events.borrow(),
        [
            WorkersPanelEvent::Command("/stop-workers r1".into()),
            WorkersPanelEvent::Message("r1".into())
        ]
    );
    // Back to the list: the composer keeps writing to whom the user chose.
    panel.click(cx, "workers-back");
    assert_eq!(panel.events.borrow().len(), 2);
    // A finished worker has no Stop.
    panel.click(cx, ("worker", 1usize));
    cx.update_window(panel.window, |_, window, _| {
        assert!(window.try_find("worker-stop").is_none());
        assert!(window.try_find("worker-message").is_some());
    })
    .unwrap();
    let _ = &panel.feed;
}

#[gpui_kit::test]
fn stop_all_workers_is_in_the_headers_menu(cx: &mut TestAppContext) {
    let panel = open(cx);
    panel.click(cx, "workers-menu");
    cx.update_window(panel.window, |_, window, cx| {
        window.within("popup-menu").click(0usize, cx)
    })
    .unwrap();
    panel.draw(cx);
    assert!(
        panel
            .events
            .borrow()
            .contains(&WorkersPanelEvent::Command("/stop-workers".into()))
    );
}

/// The More button's tooltip would cover Stop all workers. While the menu is
/// open, the pointer leaving the button and coming back shows no tooltip,
/// and once the menu closes the tooltip shows again.
#[gpui_kit::test]
fn the_headers_more_button_shows_no_tooltip_over_its_open_menu(cx: &mut TestAppContext) {
    let panel = open(cx);
    let window = panel.window;
    let tooltip = |cx: &mut TestAppContext| {
        gpui_kit::VisualTestContext::from_window(window, cx)
            .debug_bounds("tooltip")
            .is_some()
    };
    let menu_open = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, _| {
            window.try_find("popup-menu").is_some()
        })
        .unwrap()
    };
    // Longer than a tooltip's show delay and its grace period.
    let rest = |cx: &mut TestAppContext| {
        for _ in 0..10 {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(100));
            cx.run_until_parked();
            cx.update_window(window, |_, window, cx| window.render_frame(cx))
                .unwrap();
        }
    };
    let rest_on_button = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.hover("workers-menu", cx))
            .unwrap();
        rest(cx);
    };
    // Down in the empty part of the panel.
    let rest_away = |cx: &mut TestAppContext| {
        gpui_kit::VisualTestContext::from_window(window, cx).simulate_mouse_move(
            gpui_kit::point(px(20.), px(680.)),
            None,
            gpui_kit::Modifiers::default(),
        );
        rest(cx);
    };

    rest_on_button(cx);
    assert!(tooltip(cx), "the button shows its tooltip");

    panel.click(cx, "workers-menu");
    assert!(menu_open(cx));
    assert!(!tooltip(cx), "opening the menu hides the tooltip");
    rest_away(cx);
    rest_on_button(cx);
    assert!(menu_open(cx), "the menu stays open");
    assert!(!tooltip(cx), "and the button shows no tooltip over it");

    panel.press(cx, "escape");
    assert!(!menu_open(cx));
    rest_away(cx);
    rest_on_button(cx);
    assert!(tooltip(cx), "with the menu closed, the tooltip shows again");
}

/// On the real engine: while the composer writes to a worker, what is sent
/// goes as `/message r1 …`, and Esc goes back to the main agent.
#[gpui_kit::test]
fn the_composer_writes_to_the_worker_the_panel_shows(cx: &mut TestAppContext) {
    use crate::backend::ArkoBackend;
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};

    let mock = Mock::start(|_, _| reply("a1", "Done."));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    let feed_has = |cx: &mut TestAppContext, text: &str| {
        cx.read(|cx| {
            view.read(cx)
                .live_session()
                .and_then(|session| session.read(cx).feed().cloned())
                .is_some_and(|feed| {
                    feed.subscribe()
                        .0
                        .items
                        .iter()
                        .any(|item| item.text.contains(text))
                })
        })
    };
    act(cx, window, |window, cx| window.input("Hello.", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply", |cx| feed_has(cx, "Done."));

    // The panel shows r1 with the focus: the composer writes to it.
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| {
            view.set_target(Some("r1".into()), window, cx)
        })
    });
    assert_eq!(cx.read(|cx| view.read(cx).placeholder(cx)), "Message r1");
    act(cx, window, |window, _| {
        assert!(window.try_find("composer-target").is_some());
    });
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| view.focus_composer(window, cx));
        window.input("Check the tests too.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    // This session has no worker r1: the engine says the message didn't
    // reach it, so it went as `/message r1 …`.
    wait_until(cx, "the message to r1", |cx| {
        feed_has(cx, "didn't reach r1")
    });
    assert!(
        !mock.was_sent("Check the tests too."),
        "not to the main agent"
    );
    // The engine may still be finishing that command when it says so.
    wait_until(cx, "the session to be idle", |cx| {
        cx.read(|cx| {
            view.read(cx)
                .live_session()
                .is_some_and(|session| !session.read(cx).is_working())
        })
    });
    // Esc in the composer writes to the main agent again.
    act(cx, window, |window, cx| window.press("escape", cx));
    assert_eq!(cx.read(|cx| view.read(cx).target().cloned()), None);
    assert_eq!(cx.read(|cx| view.read(cx).placeholder(cx)), "Message Arko");
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// Writing to a worker and editing a message are two modes Esc ends: one
/// at a time. Choosing a worker ends an edit.
#[gpui_kit::test]
fn writing_to_a_worker_ends_an_edit(cx: &mut TestAppContext) {
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};

    let mock = Mock::start(|_, _| reply("a1", "Done."));
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(
        crate::backend::ArkoBackend::new(temp.path().join("arko")).with_configure(mock.configure()),
    );
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| window.input("Hello.", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            view.read(cx).transcript(cx).is_some_and(|transcript| {
                transcript.read(cx).last_reply().is_some()
                    && transcript.read(cx).busy_reason().is_none()
            })
        })
    });
    act(cx, window, |window, cx| window.press("alt-up", cx));
    assert!(cx.read(|cx| view.read(cx).is_editing()));
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| {
            view.set_target(Some("r1".into()), window, cx)
        })
    });
    assert!(!cx.read(|cx| view.read(cx).is_editing()));
    assert_eq!(
        cx.read(|cx| view.read(cx).target().cloned()).as_deref(),
        Some("r1")
    );
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// A running worker's spinner turns as it paints. It draws no row again, its own
/// included, apart from the time that goes on.
#[gpui_kit::test]
fn a_spinner_draws_no_row(cx: &mut TestAppContext) {
    let panel = open(cx);
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::On
        });
        cx.set_reduce_motion(false);
    });
    panel.draw(cx);
    super::ROWS_DRAWN.with(|drawn| drawn.borrow_mut().clear());
    // A second of a 60 Hz display.
    for _ in 0..60 {
        display_frame(cx, panel.window);
    }
    let drawn = super::ROWS_DRAWN.with(|drawn| drawn.borrow().clone());
    // r1 runs, and its spinner turns, with its time going on once in a
    // second. r2 and r3 ended, and nothing of theirs changed.
    assert!(drawn.get("r1").is_none_or(|&count| count <= 2), "{drawn:?}");
    assert_eq!(drawn.get("r2"), None, "{drawn:?}");
    assert_eq!(drawn.get("r3"), None, "{drawn:?}");
}

/// The panel reads its session's workers only while it shows.
#[gpui_kit::test]
fn a_closed_panel_leaves_the_sessions_workers_alone(cx: &mut TestAppContext) {
    use crate::testing::{Mock, reply, wait_until};
    use crate::tests::{act, open};

    let mock = Mock::start(|_, _| reply("a1", "Done."));
    let temp = tempfile::tempdir().unwrap();
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(
        crate::backend::ArkoBackend::new(temp.path().join("arko")).with_configure(mock.configure()),
    );
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    let pulls = || super::PULLS.with(|pulls| pulls.get());
    let before = pulls();
    act(cx, window, |window, cx| window.input("Hello.", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            view.read(cx).transcript(cx).is_some_and(|transcript| {
                transcript.read(cx).last_reply().is_some()
                    && transcript.read(cx).busy_reason().is_none()
            })
        })
    });
    assert_eq!(pulls(), before, "the panel is closed");
    // Opened, it reads them, and follows them from then on.
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ToggleWorkersPanel), cx)
    });
    assert!(pulls() > before);
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// With the Vim keymap, Escape leaves insert mode first. Escape in normal
/// mode then writes to Arko again, as the strip and the chip say.
#[gpui_kit::test]
fn with_vim_escape_in_normal_mode_writes_to_arko_again(cx: &mut TestAppContext) {
    use crate::tests::{act, open};

    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| prefs.input.keymap = crate::prefs::Keymap::Vim)
    });
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let target = |cx: &mut TestAppContext| cx.read(|cx| view.read(cx).target().cloned());
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| {
            view.set_target(Some("r1".into()), window, cx);
            view.focus_composer(window, cx);
        })
    });
    act(cx, window, |window, cx| window.press("i", cx));
    act(cx, window, |window, cx| window.input("Check the tests", cx));
    act(cx, window, |window, cx| window.press("escape", cx));
    assert_eq!(target(cx).as_deref(), Some("r1"), "out of insert mode");
    act(cx, window, |window, cx| window.press("escape", cx));
    assert_eq!(target(cx), None);
}

/// Docked in the window, with a worker running: its spinner turns as it
/// paints, so no row is drawn again for it, though the window draws for it.
#[gpui_kit::test]
fn in_the_window_a_spinner_draws_no_row(cx: &mut TestAppContext) {
    use crate::tests::{act, open};

    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::On
        });
        cx.set_reduce_motion(false);
    });
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ToggleWorkersPanel), cx)
    });
    let panel = cx.read(|cx| workspace.read(cx).workers_panel().clone());
    let feed = feed();
    let now = chrono::Utc::now().timestamp_millis() + 1_000;
    act(cx, window, |_, cx| {
        panel.update(cx, |panel, cx| {
            let key = cx.entity_id();
            panel.set_source(source(&feed, key), cx);
            panel.set_clock(move || now, cx);
        })
    });
    super::ROWS_DRAWN.with(|drawn| drawn.borrow_mut().clear());
    for _ in 0..60 {
        display_frame(cx, window);
    }
    let drawn = super::ROWS_DRAWN.with(|drawn| drawn.borrow().clone());
    assert!(drawn.get("r1").is_none_or(|&count| count <= 2), "{drawn:?}");
    assert_eq!(drawn.get("r2"), None, "{drawn:?}");
    assert_eq!(drawn.get("r3"), None, "{drawn:?}");
}

/// Docked in the window and kept from frame to frame, the panel shows a
/// running worker's time going on, and draws again for it only, once in a
/// second, not with every frame of the window around it or of its spinner.
#[gpui_kit::test]
fn in_the_window_a_running_workers_time_goes_on(cx: &mut TestAppContext) {
    use std::cell::Cell;

    use crate::tests::{act, open};

    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::On
        });
        cx.set_reduce_motion(false);
    });
    let (window, workspace) = open(cx, 1300.);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ToggleWorkersPanel), cx)
    });
    let panel = cx.read(|cx| workspace.read(cx).workers_panel().clone());
    let feed = feed();
    let clock = Rc::new(Cell::new(chrono::Utc::now().timestamp_millis() + 1_000));
    let now = clock.clone();
    act(cx, window, |_, cx| {
        panel.update(cx, |panel, cx| {
            let key = cx.entity_id();
            panel.set_source(source(&feed, key), cx);
            panel.set_clock(move || now.get(), cx);
        })
    });
    let elapsed = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let row = panel.read(cx).row_views.get("r1").unwrap().clone();
            row.read(cx).facts.elapsed.clone()
        })
    };
    let before = elapsed(cx);
    super::ROWS_DRAWN.with(|drawn| drawn.borrow_mut().clear());
    // Three seconds of a 60 Hz display.
    for _ in 0..180 {
        clock.set(clock.get() + 1000 / 60);
        display_frame(cx, window);
    }
    assert_ne!(elapsed(cx), before, "the time goes on");
    let rows = super::ROWS_DRAWN.with(|drawn| drawn.borrow().clone());
    assert!(
        rows.get("r1")
            .is_some_and(|&count| (2..=5).contains(&count)),
        "{rows:?}"
    );
    // With the spinner still, frames for the rest of the window (the status
    // bar's here) leave the panel as it was drawn.
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::Off
        });
        cx.set_reduce_motion(true);
    });
    act(cx, window, |_, _| {});
    let status = cx.read(|cx| workspace.read(cx).status_bar().clone());
    super::PANEL_DRAWN.with(|drawn| drawn.set(0));
    for _ in 0..10 {
        cx.update(|cx| status.update(cx, |_, cx| cx.notify()));
        cx.update_window(window, |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }
    let drawn = super::PANEL_DRAWN.with(|drawn| drawn.get());
    assert_eq!(drawn, 0, "the panel drew {drawn} times");
}

/// Back and Forward retrace the views the user left, and leaving a view
/// for another drops what Forward had.
#[test]
fn the_view_history_steps_back_and_forward() {
    let worker = |id: &str| Some(SharedString::from(id.to_string()));
    let mut history = super::ViewHistory::default();
    // The list, then r1, then r2.
    history.leave(None);
    history.leave(worker("r1"));
    assert_eq!(history.step(worker("r2"), false), Some(worker("r1")));
    assert_eq!(history.step(worker("r1"), false), Some(None));
    assert_eq!(history.step(None, false), None, "nothing before the first");
    assert_eq!(history.step(None, true), Some(worker("r1")));
    // r3 from r1: r2 is no longer ahead.
    history.leave(worker("r1"));
    assert_eq!(history.step(worker("r3"), true), None);
    assert_eq!(history.step(worker("r3"), false), Some(worker("r1")));
    assert_eq!(history.step(worker("r1"), true), Some(worker("r3")));
}
