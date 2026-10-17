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
    bar_of_height(cx, width, 400.)
}

/// The same, in a window `height` tall.
fn bar_of_height(cx: &mut TestAppContext, width: f32, height: f32) -> Bar {
    start(cx);
    let now = Rc::new(Cell::new(NOW));
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    let (window, bar) = cx.update(|cx| {
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds {
                origin: Point::default(),
                size: size(px(width), px(height)),
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
        "status-cache",
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
            cache: None,
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
    assert!(bar.find(cx, "web-page-allows"));
    assert!(bar.find(cx, "web-page-qr"));
    act(cx, bar.window, |window, cx| {
        window.click("web-page-new-key", cx)
    });
    act(cx, bar.window, |window, cx| {
        window.click("web-page-stop", cx)
    });
    assert_eq!(sent(&bar.session, cx), ["/remote new-key", "/remote off"]);
}

/// In a short window, the web page's popover fits: its links and QR code
/// scroll, and its buttons stay in reach.
#[gpui_kit::test]
fn the_web_pages_popover_fits_a_short_window(cx: &mut TestAppContext) {
    let bar = bar_of_height(cx, 1300., 320.);
    let mut reading = busy();
    if let Some(page) = reading.page.as_mut() {
        page.links = (0..10)
            .map(|n| {
                link(
                    &format!("http://10.0.0.{n}:8765/#key=k"),
                    &format!("veth{n}"),
                )
            })
            .collect();
    }
    bar.show(cx, reading, Activity::default());
    act(cx, bar.window, |window, cx| {
        window.click("status-web-page", cx)
    });
    let (panel, viewport) = cx
        .update_window(bar.window, |_, window, cx| {
            window.render_frame(cx);
            (
                window
                    .try_find("web-page-panel")
                    .map(|found| found.bounds()),
                window.viewport_size(),
            )
        })
        .unwrap();
    let panel = panel.expect("the popover shows");
    assert!(
        panel.origin.y >= px(0.) && panel.origin.y + panel.size.height <= viewport.height,
        "{panel:?} in {viewport:?}"
    );
    act(cx, bar.window, |window, cx| {
        window.click("web-page-stop", cx)
    });
    assert_eq!(sent(&bar.session, cx), ["/remote off"]);
}

/// A page only this computer can open has no QR code: the popover says how
/// to reach it from a phone, and opens Settings › Web page.
#[gpui_kit::test]
fn a_page_only_this_computer_opens_has_no_qr_code(cx: &mut TestAppContext) {
    let (window, workspace, session) = window_with_session(cx);
    let mut reading = busy();
    reading.page = Some(Page {
        addr: "127.0.0.1:8765".parse().unwrap(),
        links: vec![Link {
            url: "http://127.0.0.1:8765/#key=k".into(),
            interface: None,
            kind: LinkKind::Loopback,
            loopback: true,
        }],
        note: None,
    });
    session.update(cx, |session, cx| {
        session.set_status(reading, Activity::default(), cx)
    });
    cx.run_until_parked();
    act(cx, window, |window, cx| window.click("status-web-page", cx));
    let shows = |cx: &mut TestAppContext, id: &'static str| {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    };
    assert!(shows(cx, "web-page-panel"));
    assert!(!shows(cx, "web-page-qr"));
    assert!(shows(cx, "web-page-this-computer"));
    act(cx, window, |window, cx| {
        window.click("web-page-settings", cx)
    });
    let settings = cx.read(|cx| workspace.read(cx).settings().cloned());
    assert_eq!(
        settings.map(|settings| cx.read(|cx| settings.read(cx).page())),
        Some(5)
    );
    assert!(!shows(cx, "web-page-panel"), "the popover stayed open");
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

#[gpui_kit::test]
fn the_cache_timer_counts_down_while_idle_and_warns_in_its_last_minute(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    let mut reading = Reading::default();
    reading.cache.lifetime = Some(Duration::from_secs(300));
    reading.cache.refreshed_at = Some(NOW - 180_000);
    reading.cache.tokens = Some(180_000);
    reading.cache.miss_cost = Some(0.9);
    bar.show(cx, reading.clone(), Activity::default());
    assert_eq!(bar.shown(cx).cache, Some(("cache 2:00".into(), false)));
    assert!(bar.find(cx, "status-cache"));
    assert!(cx.read(|cx| bar.bar.read(cx).is_ticking()));
    bar.advance(cx, 61_000);
    assert_eq!(bar.shown(cx).cache, Some(("cache 0:59".into(), true)));
    bar.advance(cx, 60_000);
    assert_eq!(bar.shown(cx).cache, Some(("cache expired".into(), false)));
    // Expired, it no longer ticks.
    bar.advance(cx, 1_000);
    assert!(!cx.read(|cx| bar.bar.read(cx).is_ticking()));
    // While a turn runs, every request refreshes it: hidden.
    bar.show(cx, reading, working(NOW));
    assert_eq!(bar.shown(cx).cache, None);
}

/// A question waiting for the user stops the turn's requests: the cache
/// timer shows beside "Waiting for your answer".
#[gpui_kit::test]
fn the_cache_timer_counts_while_a_question_waits(cx: &mut TestAppContext) {
    let bar = bar(cx, 1300.);
    let mut reading = Reading::default();
    reading.cache.lifetime = Some(Duration::from_secs(300));
    reading.cache.refreshed_at = Some(NOW - 180_000);
    let asking = Activity {
        question: Some(arko::remote::Question {
            id: 1,
            text: "Tabs or spaces?".into(),
        }),
        ..working(NOW - 240_000)
    };
    bar.show(cx, reading, asking);
    let shown = bar.shown(cx);
    assert_eq!(shown.turn.as_deref(), Some("Waiting for your answer"));
    assert_eq!(shown.cache, Some(("cache 2:00".into(), false)));
    assert!(cx.read(|cx| bar.bar.read(cx).is_ticking()));
}

/// A window showing a session that hasn't opened (what it's sent waits in
/// its queue).
fn window_with_session(
    cx: &mut TestAppContext,
) -> (
    AnyWindowHandle,
    Entity<crate::workspace::Workspace>,
    Entity<LiveSession>,
) {
    start(cx);
    let (window, workspace) = open(cx, 1300.);
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.show_session(session.clone(), window, cx)
        })
    })
    .unwrap();
    cx.run_until_parked();
    (window, workspace, session)
}

/// The draws of each of the window's views over five seconds, after two
/// to settle, the bar reading `now`.
fn draws_in_five_seconds(
    cx: &mut TestAppContext,
    now: &Rc<Cell<i64>>,
) -> std::collections::BTreeMap<&'static str, usize> {
    let second = |cx: &mut TestAppContext| {
        now.set(now.get() + 1_000);
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
    };
    second(cx);
    second(cx);
    crate::testing::count_renders();
    for _ in 0..5 {
        second(cx);
    }
    crate::testing::renders()
}

/// Each second the cache timer counts, the bar draws again, and the title
/// bar and the sidebar beside it don't.
#[gpui_kit::test]
fn a_tick_of_the_bars_clock_leaves_the_other_panes_alone(cx: &mut TestAppContext) {
    let (window, workspace, session) = window_with_session(cx);
    let now = Rc::new(Cell::new(NOW));
    let bar = cx.read(|cx| workspace.read(cx).status_bar().clone());
    let clock = now.clone();
    bar.update(cx, |bar, cx| {
        bar.set_clock(Rc::new(move || clock.get()), cx)
    });
    // Nothing focused: the composer's caret doesn't blink.
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    act(cx, window, |window, cx| window.blur(cx));
    let mut reading = Reading::default();
    reading.cache.lifetime = Some(Duration::from_secs(3_600));
    reading.cache.refreshed_at = Some(NOW - 300_000);
    session.update(cx, |session, cx| {
        session.set_status(reading, Activity::default(), cx)
    });
    cx.run_until_parked();
    assert!(cx.read(|cx| bar.read(cx).is_ticking()));
    let draws = draws_in_five_seconds(cx, &now);
    assert_eq!(draws.get("StatusBar"), Some(&5), "{draws:?}");
    for pane in ["AppTitleBar", "Sidebar"] {
        assert_eq!(draws.get(pane), None, "{pane} drew again: {draws:?}");
    }
}

/// The composer's caret blinks twice a second: the window draws again
/// around it, but not its title bar, sidebar and status bar.
#[gpui_kit::test]
fn the_composers_caret_leaves_the_other_panes_alone(cx: &mut TestAppContext) {
    let (window, _, _) = window_with_session(cx);
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    let draws = draws_in_five_seconds(cx, &Rc::new(Cell::new(NOW)));
    assert!(
        draws.get("Workspace").is_some_and(|draws| *draws >= 5),
        "{draws:?}"
    );
    for pane in ["AppTitleBar", "Sidebar", "StatusBar"] {
        assert_eq!(draws.get(pane), None, "{pane} drew again: {draws:?}");
    }
}

/// Share Web Page serves the page where Settings say and shows its links
/// once it's up. Stop Sharing Web Page stops it.
#[gpui_kit::test]
fn sharing_the_web_page_serves_it_and_shows_its_links(cx: &mut TestAppContext) {
    let (window, workspace, session) = window_with_session(cx);
    cx.update(|cx| {
        crate::prefs::update(cx, |prefs| {
            prefs.web_page.address = "0.0.0.0".into();
            prefs.web_page.port = Some(9000);
        })
    });
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    assert_eq!(sent(&session, cx), ["/remote 0.0.0.0:9000"]);
    // The page is up: its links show.
    session.update(cx, |session, cx| {
        session.set_status(busy(), Activity::default(), cx)
    });
    cx.run_until_parked();
    let bar = cx.read(|cx| workspace.read(cx).status_bar().clone());
    act(cx, window, |_, _| {});
    assert!(cx.read(|cx| bar.read(cx).is_web_page_open()));
    act(cx, window, |window, _| {
        assert!(window.try_find("web-page-panel").is_some())
    });
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::StopSharingWebPage), cx)
    });
    assert_eq!(sent(&session, cx), ["/remote 0.0.0.0:9000", "/remote off"]);
}

/// Refresh Cache asks the session to keep its cache warm, and says why it
/// couldn't.
#[gpui_kit::test]
fn refresh_cache_asks_the_session(cx: &mut TestAppContext) {
    let (window, _, _) = window_with_session(cx);
    let before = cx
        .update_window(window, |_, window, cx| {
            gpui_kit::component::WindowExt::notifications(window, cx).len()
        })
        .unwrap();
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::RefreshCache), cx)
    });
    let after = cx
        .update_window(window, |_, window, cx| {
            gpui_kit::component::WindowExt::notifications(window, cx).len()
        })
        .unwrap();
    assert_eq!(after, before + 1, "it says why the cache wasn't refreshed");
}

/// On arko's engine: after a turn the bar shows the context and the cache
/// timer, the session's info reads what /session says, and Refresh Cache
/// asks the model to keep the cache warm.
#[gpui_kit::test]
fn the_bar_and_the_info_follow_a_session_on_the_engine(cx: &mut TestAppContext) {
    use crate::testing::{Mock, reply, wait_until};

    let mock = Mock::start(|index, _| reply(&format!("a{index}"), "Done."));
    cx.executor().allow_parking();
    let temp = tempfile::tempdir().unwrap();
    let (root, work) = (temp.path().join("arko"), temp.path().join("work"));
    std::fs::create_dir_all(&work).unwrap();
    let backend =
        Arc::new(crate::backend::ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    let bar = cx.read(|cx| workspace.read(cx).status_bar().clone());
    let shown = {
        let workspace = workspace.clone();
        move |cx: &mut TestAppContext| cx.read(|cx| workspace.read(cx).live_session().cloned())
    };
    wait_until(cx, "the turn's end in the bar", |cx| {
        let idle = shown(cx).is_some_and(|session| {
            cx.read(|cx| {
                let session = session.read(cx);
                session.reading().context_tokens.is_some() && !session.is_working()
            })
        });
        idle && cx.read(|cx| bar.read(cx).shown(cx).context.is_some())
    });
    let figures = cx.read(|cx| bar.read(cx).shown(cx));
    assert!(
        figures.context.as_ref().unwrap().0.starts_with("10 / "),
        "{figures:?}"
    );
    // The model says how long its cache lasts: the timer counts.
    assert!(figures.cache.is_some(), "{figures:?}");

    // The info reads the session's facts as it opens.
    let title_bar = cx.read(|cx| workspace.read(cx).title_bar().clone());
    title_bar.update(cx, |bar, cx| bar.toggle_info(cx));
    let info = cx.read(|cx| title_bar.read(cx).info().clone());
    wait_until(cx, "the session's facts", |cx| {
        cx.read(|cx| info.read(cx).facts().is_some())
    });
    let facts = cx.read(|cx| info.read(cx).facts().cloned().unwrap());
    assert_eq!(facts.session.workspace, work);

    // Refresh Cache: the model is asked again.
    let asked = mock.bodies.lock().unwrap().len();
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::RefreshCache), cx)
    });
    wait_until(cx, "the refresh's request", |_| {
        mock.bodies.lock().unwrap().len() > asked
    });
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// A window on arko's engine, its model arko's mock, sessions working in a
/// temporary folder.
struct OnEngine {
    _temp: tempfile::TempDir,
    _mock: crate::testing::Mock,
    window: AnyWindowHandle,
    workspace: Entity<crate::workspace::Workspace>,
}

impl OnEngine {
    fn start(cx: &mut TestAppContext, width: f32) -> Self {
        use crate::testing::{Mock, reply};

        let mock = Mock::start(|index, _| reply(&format!("a{index}"), "Done."));
        cx.executor().allow_parking();
        let temp = tempfile::tempdir().unwrap();
        let (root, work) = (temp.path().join("arko"), temp.path().join("work"));
        std::fs::create_dir_all(&work).unwrap();
        let backend =
            Arc::new(crate::backend::ArkoBackend::new(&root).with_configure(mock.configure()));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
        let (window, workspace) = open(cx, width);
        let view = cx.read(|cx| workspace.read(cx).session_view().clone());
        cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
        Self {
            _temp: temp,
            _mock: mock,
            window,
            workspace,
        }
    }

    fn shown(&self, cx: &mut TestAppContext) -> Option<Entity<LiveSession>> {
        cx.read(|cx| self.workspace.read(cx).live_session().cloned())
    }

    /// Sends a first message and waits for its turn to end.
    fn first_turn(&self, cx: &mut TestAppContext) -> Entity<LiveSession> {
        act(cx, self.window, |window, cx| {
            window.input("Count the pears.", cx)
        });
        act(cx, self.window, |window, cx| window.press("enter", cx));
        crate::testing::wait_until(cx, "the turn's end", |cx| {
            self.shown(cx).is_some_and(|session| {
                cx.read(|cx| {
                    let session = session.read(cx);
                    session.reading().context_tokens.is_some() && !session.is_working()
                })
            })
        });
        self.shown(cx).unwrap()
    }

    fn command(&self, cx: &mut TestAppContext, session: &Entity<LiveSession>, text: &str) {
        act(cx, self.window, |window, cx| {
            crate::status_bar::run(session, text, window, cx)
        });
    }

    fn close_all(&self, cx: &mut TestAppContext) {
        act(cx, self.window, |window, cx| {
            let sessions = crate::live::LiveSessions::all(cx);
            crate::quit::close(sessions, false, window, cx);
        });
        crate::testing::wait_until(cx, "the sessions to end", |cx| {
            cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
        });
    }
}

/// Where the session's page is served.
fn page_addr(
    session: &Entity<LiveSession>,
    cx: &mut TestAppContext,
) -> Option<std::net::SocketAddr> {
    cx.read(|cx| {
        session
            .read(cx)
            .reading()
            .page
            .as_ref()
            .map(|page| page.addr)
    })
}

/// Share Web Page serves where Settings say, this computer only by default,
/// even in a session that served on every network before.
#[gpui_kit::test]
fn sharing_serves_where_settings_say_not_where_the_page_was(cx: &mut TestAppContext) {
    use crate::testing::wait_until;

    let app = OnEngine::start(cx, 1300.);
    let session = app.first_turn(cx);
    app.command(cx, &session, "/remote 0.0.0.0");
    wait_until(cx, "the page on every network", |cx| {
        page_addr(&session, cx).is_some_and(|addr| addr.ip().is_unspecified())
    });
    app.command(cx, &session, "/remote off");
    wait_until(cx, "the page off", |cx| page_addr(&session, cx).is_none());
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    wait_until(cx, "the page again", |cx| page_addr(&session, cx).is_some());
    let addr = page_addr(&session, cx).unwrap();
    app.close_all(cx);
    assert!(addr.ip().is_loopback(), "served on {addr}");
}

/// A page that doesn't start (its port is taken) says so where the user
/// asked for it, and its popover doesn't open by itself when a page
/// starts later.
#[gpui_kit::test]
fn a_page_that_doesnt_start_says_so(cx: &mut TestAppContext) {
    use crate::testing::wait_until;
    use gpui_kit::component::WindowExt as _;

    let app = OnEngine::start(cx, 1300.);
    let session = app.first_turn(cx);
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = taken.local_addr().unwrap().port();
    cx.update(|cx| crate::prefs::update(cx, |prefs| prefs.web_page.port = Some(port)));
    let notes = |cx: &mut TestAppContext| {
        cx.update_window(app.window, |_, window, cx| window.notifications(cx).len())
            .unwrap()
    };
    let before = notes(cx);
    act(cx, app.window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    wait_until(cx, "a notification", |cx| notes(cx) > before);
    // The user starts the page at the composer.
    drop(taken);
    app.command(cx, &session, "/remote on");
    wait_until(cx, "the page", |cx| page_addr(&session, cx).is_some());
    let bar = cx.read(|cx| app.workspace.read(cx).status_bar().clone());
    act(cx, app.window, |_, _| {});
    let opened = cx.read(|cx| bar.read(cx).is_web_page_open());
    app.close_all(cx);
    assert!(!opened, "the popover opened by itself");
}

/// The info opened while its session still opens reads the session's
/// details once it has opened.
#[gpui_kit::test]
fn the_info_opened_as_the_session_opens_reads_its_details(cx: &mut TestAppContext) {
    use crate::testing::wait_until;

    let app = OnEngine::start(cx, 1300.);
    act(cx, app.window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, app.window, |window, cx| window.press("enter", cx));
    let session = app.shown(cx).expect("the session shows at once");
    let title_bar = cx.read(|cx| app.workspace.read(cx).title_bar().clone());
    title_bar.update(cx, |bar, cx| bar.toggle_info(cx));
    assert!(
        cx.read(|cx| session.read(cx).link().is_none()),
        "the session opened before the info"
    );
    let info = cx.read(|cx| title_bar.read(cx).info().clone());
    wait_until(cx, "the session's details", |cx| {
        cx.read(|cx| info.read(cx).facts().is_some())
    });
    app.close_all(cx);
}
