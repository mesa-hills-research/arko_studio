//! The status bar: what it shows for a session's status, how its clock
//! ticks, and what its buttons and popovers do.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use arko::remote::{Activity, CostPart, Link, LinkKind, Page, PlanStatus, PlanWindow, Reading};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, Entity, Point, TestAppContext, WindowBounds,
    WindowOptions, px, size,
};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::live::LiveSession;
use crate::status_bar::{Shown, StatusBar};
use crate::tests::{act, open};

/// The time the tests start at, in Unix milliseconds.
const NOW: i64 = 1_800_000_000_000;

struct Bar {
    window: AnyWindowHandle,
    bar: Entity<StatusBar>,
    session: Entity<LiveSession>,
    now: Rc<Cell<i64>>,
}

impl Bar {
    fn show(&self, cx: &mut TestAppContext, reading: Reading, activity: Activity) {
        self.session
            .update(cx, |session, cx| session.set_status(reading, activity, cx));
        cx.run_until_parked();
    }

    fn shown(&self, cx: &mut TestAppContext) -> Shown {
        cx.read(|cx| self.bar.read(cx).shown(cx))
    }

    /// Moves both clocks on by `ms`: GPUI's (its timers) and the bar's.
    fn advance(&self, cx: &mut TestAppContext, ms: i64) {
        self.now.set(self.now.get() + ms);
        cx.executor()
            .advance_clock(Duration::from_millis(ms as u64));
        cx.run_until_parked();
    }

    fn find(&self, cx: &mut TestAppContext, id: &'static str) -> bool {
        cx.update_window(self.window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }
}

fn start(cx: &mut TestAppContext) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
}

/// A status bar alone in a window, showing a session that hasn't opened
/// (what it's sent waits in its queue), at [`NOW`].
fn bar(cx: &mut TestAppContext, width: f32) -> Bar {
    start(cx);
    let now = Rc::new(Cell::new(NOW));
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    let (window, bar) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(width), px(400.)),
            })),
            ..Default::default()
        };
        let clock = now.clone();
        let session = session.clone();
        gpui_kit::open_window(options, cx, move |_, cx| {
            cx.new(|cx| {
                let mut bar = StatusBar::new();
                bar.set_clock(Rc::new(move || clock.get()), cx);
                bar.set_session(Some(session), cx);
                if width < crate::layout::NARROW_BELOW {
                    bar.set_width(crate::layout::WidthClass::Narrow, cx);
                }
                bar
            })
        })
        .unwrap()
    });
    cx.run_until_parked();
    Bar {
        window,
        bar,
        session,
        now,
    }
}

fn link(url: &str, interface: &str) -> Link {
    Link {
        url: url.into(),
        interface: Some(interface.into()),
        kind: LinkKind::Lan,
        loopback: false,
    }
}

/// A busy session: workers, outside access, the web page, a plan near its
/// limit, a cost and a context.
fn busy() -> Reading {
    let mut reading = Reading {
        cost: 1.2,
        context_tokens: Some(142_318),
        context_window: Some(1_000_000),
        workers_running: vec!["r1".into(), "r2".into()],
        page: Some(Page {
            addr: "0.0.0.0:8765".parse().unwrap(),
            links: vec![link("http://192.168.1.20:8765/#key=k", "wlan0")],
            note: None,
        }),
        plans: vec![PlanStatus {
            plan: "Claude plan".into(),
            windows: vec![PlanWindow {
                label: "5 h".into(),
                used_percent: 84.,
                resets_at: None,
            }],
            text: String::new(),
        }],
        ..Reading::default()
    };
    reading.settings.outside = true;
    reading.settings.handover_at = 950_000;
    reading.costs.main = CostPart {
        billed: 1.2,
        list_price: 1.2,
        calls: 4,
        ..CostPart::default()
    };
    reading
}

fn working(since: i64) -> Activity {
    Activity {
        working: true,
        since: Some(since),
        ..Activity::default()
    }
}

#[gpui_kit::test]
fn an_idle_session_with_nothing_unusual_shows_nothing(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    assert_eq!(bar.shown(cx), Shown::default());
    for id in [
        "status-turn",
        "status-workers",
        "status-outside",
        "status-web-page",
        "status-plan",
        "status-cost",
        "status-context",
    ] {
        assert!(!bar.find(cx, id), "{id} shows");
    }
}

#[gpui_kit::test]
fn a_busy_session_shows_its_turn_workers_warnings_and_figures(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    bar.show(cx, busy(), working(NOW - 83_000));
    assert_eq!(
        bar.shown(cx),
        Shown {
            turn: Some("Working 1:23".into()),
            workers: vec!["r1".into(), "r2".into()],
            outside: true,
            web_page: true,
            plan: Some("5 h limit 84%".into()),
            cost: Some("$1.20".into()),
            context: Some(("142k / 1M".into(), false)),
        }
    );
    for id in [
        "status-turn",
        "status-workers",
        "status-outside",
        "status-web-page",
        "status-plan",
        "status-cost",
        "status-context",
    ] {
        assert!(bar.find(cx, id), "{id} is missing");
    }
    // A question: the bar says so instead.
    let mut asking = working(NOW - 83_000);
    asking.question = Some(arko::remote::Question {
        id: 1,
        text: "Tabs or spaces?".into(),
    });
    bar.show(cx, busy(), asking);
    assert_eq!(
        bar.shown(cx).turn.as_deref(),
        Some("Waiting for your answer")
    );
}

#[gpui_kit::test]
fn a_narrow_bar_keeps_the_context_the_cost_and_the_warnings(cx: &mut TestAppContext) {
    let bar = bar(cx, 600.);
    bar.show(cx, busy(), working(NOW - 83_000));
    let shown = bar.shown(cx);
    assert_eq!(shown.turn, None);
    assert!(shown.workers.is_empty());
    assert_eq!(shown.plan, None);
    assert!(shown.outside && shown.web_page);
    assert!(shown.cost.is_some() && shown.context.is_some());
}

#[gpui_kit::test]
fn the_turns_clock_ticks_while_it_runs_and_stops_after(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    let drawn = Rc::new(Cell::new(0));
    cx.update(|cx| {
        let drawn = drawn.clone();
        cx.observe(&bar.bar, move |_, _| drawn.set(drawn.get() + 1))
            .detach()
    });
    bar.show(cx, Reading::default(), working(NOW - 83_000));
    assert!(bar.find(cx, "status-turn"));
    assert!(cx.read(|cx| bar.bar.read(cx).is_ticking()));
    let before = drawn.get();
    bar.advance(cx, 1_000);
    assert_eq!(bar.shown(cx).turn.as_deref(), Some("Working 1:24"));
    assert!(drawn.get() > before, "the bar didn't draw the new second");
    // Idle: the turn goes, and so does the clock.
    bar.show(cx, Reading::default(), Activity::default());
    assert!(!bar.find(cx, "status-turn"));
    bar.advance(cx, 1_000);
    assert!(!cx.read(|cx| bar.bar.read(cx).is_ticking()));
    let idle = drawn.get();
    bar.advance(cx, 5_000);
    assert_eq!(drawn.get(), idle, "an idle bar drew again");
}

#[gpui_kit::test]
fn the_workers_count_toggles_the_workers_panel(cx: &mut TestAppContext) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    let reading = Reading {
        workers_running: vec!["r1".into()],
        ..Reading::default()
    };
    session.update(cx, |session, cx| {
        session.set_status(reading, Activity::default(), cx)
    });
    let bar = cx.read(|cx| workspace.read(cx).status_bar().clone());
    bar.update(cx, |bar, cx| bar.set_session(Some(session), cx));
    cx.run_until_parked();
    assert!(!cx.read(|cx| workspace.read(cx).is_workers_open()));
    act(cx, window, |window, cx| window.click("status-workers", cx));
    assert!(cx.read(|cx| workspace.read(cx).is_workers_open()));
    act(cx, window, |window, cx| window.click("status-workers", cx));
    assert!(!cx.read(|cx| workspace.read(cx).is_workers_open()));
}

/// The command lines the session was sent.
fn sent(session: &Entity<LiveSession>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| {
        session
            .read(cx)
            .queued()
            .iter()
            .filter_map(|input| match input {
                arko::Input::Message(message) => Some(message.text.clone()),
                _ => None,
            })
            .collect()
    })
}

#[gpui_kit::test]
fn outside_access_turns_off_from_its_popover(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    bar.show(cx, busy(), Activity::default());
    act(cx, bar.window, |window, cx| {
        window.click("status-outside", cx)
    });
    assert!(bar.find(cx, "outside-details"));
    act(cx, bar.window, |window, cx| window.click("outside-off", cx));
    assert_eq!(sent(&bar.session, cx), ["/outside off"]);
    assert!(!bar.find(cx, "outside-details"), "the popover stayed open");
}

#[gpui_kit::test]
fn the_web_pages_popover_shows_its_links_and_stops_sharing(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    bar.show(cx, busy(), Activity::default());
    act(cx, bar.window, |window, cx| {
        window.click("status-web-page", cx)
    });
    assert!(bar.find(cx, "web-page-panel"));
    assert!(bar.find(cx, "web-page-qr"));
    act(cx, bar.window, |window, cx| {
        window.click("web-page-new-key", cx)
    });
    act(cx, bar.window, |window, cx| {
        window.click("web-page-stop", cx)
    });
    assert_eq!(sent(&bar.session, cx), ["/remote new-key", "/remote off"]);
}

#[gpui_kit::test]
fn the_contexts_popover_shows_the_split_and_hands_over(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    let mut reading = busy();
    reading.last_request = Some(arko::remote::LastRequest {
        at: NOW,
        model: "claude-opus-5-5".into(),
        sizes: Some(Default::default()),
        bytes_per_token: 4.,
    });
    if let Some(sizes) = reading
        .last_request
        .as_mut()
        .and_then(|request| request.sizes.as_mut())
    {
        sizes.system_prompt = 80_000;
        sizes.user_text = 400_000;
        sizes.tool_results = 100_000;
    }
    reading.cache.read = Some(140_000);
    reading.cache.written = Some(2_000);
    bar.show(cx, reading, Activity::default());
    act(cx, bar.window, |window, cx| {
        window.click("status-context", cx)
    });
    assert!(bar.find(cx, "context-details"));
    act(cx, bar.window, |window, cx| {
        window.click("hand-over-now", cx)
    });
    assert_eq!(sent(&bar.session, cx), ["/handover"]);
}

#[gpui_kit::test]
fn the_context_warns_from_90_percent_of_the_handover_size(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    let mut reading = busy();
    reading.context_tokens = Some(855_000);
    bar.show(cx, reading, Activity::default());
    assert_eq!(bar.shown(cx).context, Some(("855k / 1M".into(), true)));
}

#[gpui_kit::test]
fn the_costs_popover_shows_what_cost_says(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    bar.show(cx, busy(), Activity::default());
    act(cx, bar.window, |window, cx| window.click("status-cost", cx));
    assert!(bar.find(cx, "cost-details"));
}
