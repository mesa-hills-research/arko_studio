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
use crate::workers::{ENDED_SHOWS_FOR_MS, WorkerTimes};

struct Panel {
    window: AnyWindowHandle,
    panel: Entity<WorkersPanel>,
    feed: Feed,
    events: Rc<RefCell<Vec<WorkersPanelEvent>>>,
    /// The panel's clock, in Unix milliseconds.
    now: Rc<RefCell<i64>>,
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
    let mut times = WorkerTimes::default();
    for worker in &workers {
        times.observe(worker);
    }
    PanelSource {
        workers,
        times,
        agents: Vec::new(),
        feed: Some(feed.clone()),
        images: None,
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

/// A running worker's spinner draws its own row again, not the others'.
#[gpui_kit::test]
fn a_spinner_draws_only_its_own_row(cx: &mut TestAppContext) {
    let panel = open(cx);
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.appearance.motion = crate::prefs::Motion::On
        });
        cx.set_reduce_motion(false);
    });
    panel.draw(cx);
    super::ROWS_DRAWN.with(|drawn| drawn.borrow_mut().clear());
    // A second, at 30 frames a second.
    for _ in 0..30 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1000 / 30 + 1));
        cx.run_until_parked();
    }
    let drawn = super::ROWS_DRAWN.with(|drawn| drawn.borrow().clone());
    // r1 runs, and its spinner turns. r2 and r3 ended: nothing of theirs
    // changed, so their rows aren't drawn again.
    assert!(
        drawn.get("r1").is_some_and(|&count| count >= 10),
        "{drawn:?}"
    );
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

/// Docked in the window, with a worker running: its spinner draws its own
/// row, not the others, though the window keeps the panel from frame to
/// frame otherwise.
#[gpui_kit::test]
fn in_the_window_a_spinner_draws_only_its_own_row(cx: &mut TestAppContext) {
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
    for _ in 0..30 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(1000 / 30 + 1));
        cx.run_until_parked();
    }
    let drawn = super::ROWS_DRAWN.with(|drawn| drawn.borrow().clone());
    assert!(
        drawn.get("r1").is_some_and(|&count| count >= 10),
        "{drawn:?}"
    );
    assert_eq!(drawn.get("r2"), None, "{drawn:?}");
    assert_eq!(drawn.get("r3"), None, "{drawn:?}");
}
