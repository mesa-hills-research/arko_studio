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

    /// The bounds of row `row`'s line for the setup and the cost, if it has
    /// one.
    fn setup_bounds(
        &self,
        cx: &mut TestAppContext,
        row: usize,
    ) -> Option<gpui_kit::Bounds<gpui_kit::Pixels>> {
        cx.update_window(self.window, |_, window, _| {
            window
                .within(("worker", row))
                .try_find("worker-setup")
                .map(|found| found.bounds())
        })
        .unwrap()
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
    let mut workers = vec![
        summary("r1", None, Running, None),
        summary("r4", None, Running, None),
        summary("r2", Some("r1"), Running, None),
        summary("r3", Some("r2"), Running, None),
    ];
    for worker in &mut workers {
        worker.model = Some("gpt-6.1-sol".into());
    }
    panel.show(cx, workers);
    assert_eq!(
        panel.rows(cx),
        [worker("r1"), worker("r2"), worker("r3"), worker("r4")]
    );
    let left = |cx: &mut TestAppContext, row| panel.setup_bounds(cx, row).unwrap().left();
    let (r1, r2, r3, r4) = (left(cx, 0), left(cx, 1), left(cx, 2), left(cx, 3));
    // A level in is a mark's width (1 rem), and a worker of the main agent
    // is where it was.
    assert!(r2 > r1 && r3 > r2, "{r1:?} {r2:?} {r3:?}");
    assert_eq!(r2 - r1, r3 - r2);
    assert_eq!(r4, r1);
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
#[gpui_kit::test]
fn a_row_says_the_workers_model_profile_and_cost(cx: &mut TestAppContext) {
    use arko::remote::WorkerState::Running;
    let panel = open(cx);
    let mut plain = vec![
        summary("r1", None, Running, None),
        summary("r2", None, Running, None),
    ];
    // Without a model, a profile or a cost, the rows have no line for it.
    panel.show(cx, plain.clone());
    assert!(panel.setup_bounds(cx, 0).is_none());
    let short = panel.row_height(cx, 0);
    plain[0].model = Some("gpt-6.1-sol".into());
    plain[0].effort = Some("high".into());
    plain[0].profile = Some("3d-judge".into());
    plain[0].cost_billed = 0.34;
    panel.show(cx, plain);
    // Every row has the line, so they stay as tall as one another.
    assert!(panel.setup_bounds(cx, 0).is_some());
    assert!(panel.setup_bounds(cx, 1).is_some());
    assert!(panel.row_height(cx, 0) > short);
    assert_eq!(panel.row_height(cx, 0), panel.row_height(cx, 1));
    let now = *panel.now.borrow();
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r1", now));
    assert_eq!(
        facts,
        "Running · sol · high · 3d-judge · 0 s · 0 calls · $0.34"
    );
    let facts = cx.read(|cx| panel.panel.read(cx).facts("r2", now));
    assert_eq!(facts, "Running · 0 s · 0 calls");
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
