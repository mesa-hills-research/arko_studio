//! Sharing requests keep their replies and respect later Stop and Close actions.

use std::sync::Arc;
use std::time::SystemTime;

use arko::remote::{Activity, Reading};
use gpui_kit::component::WindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::live::LiveSession;
use crate::tests::{act, open};

fn setup(
    cx: &mut TestAppContext,
) -> (
    gpui_kit::AnyWindowHandle,
    gpui_kit::Entity<crate::workspace::Workspace>,
    gpui_kit::Entity<LiveSession>,
) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
    act(cx, window, |window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.show_session(session.clone(), window, cx)
        })
    });
    (window, workspace, session)
}

#[gpui_kit::test]
fn share_failures_survive_switching_sessions_and_repeated_requests(cx: &mut TestAppContext) {
    let (window, workspace, session) = setup(cx);
    for _ in 0..2 {
        act(cx, window, |window, cx| {
            window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
        });
    }
    let requests = session.update(cx, |session, _| session.take_requests());
    let bar = cx.read(|cx| workspace.read(cx).status_bar().clone());
    bar.update(cx, |bar, cx| bar.set_session(None, cx));
    cx.run_until_parked();
    for (index, request) in requests.into_iter().enumerate() {
        let arko::Request::ShareWebPage { reply, .. } = request else {
            panic!("share");
        };
        assert!(
            reply
                .send(Err(format!("Port {} is in use.", 9000 + index)))
                .is_ok()
        );
    }
    cx.run_until_parked();
    act(cx, window, |window, cx| {
        assert_eq!(window.notifications(cx).len(), 2)
    });
}

#[gpui_kit::test]
fn stop_cancels_a_share_queued_while_opening(cx: &mut TestAppContext) {
    let (window, _, session) = setup(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::StopSharingWebPage), cx)
    });
    let requests = session.update(cx, |session, _| session.take_requests());
    assert!(matches!(
        requests.as_slice(),
        [arko::Request::StopWebPage { .. }]
    ));
    act(cx, window, |window, cx| {
        assert!(window.notifications(cx).is_empty())
    });
}

#[gpui_kit::test]
fn closing_before_opening_cancels_requests_and_inputs(cx: &mut TestAppContext) {
    let (window, _, session) = setup(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    session.update(cx, |session, cx| session.close(session.running_work(), cx));
    assert!(
        session
            .update(cx, |session, _| session.take_requests())
            .is_empty()
    );
    assert!(cx.read(|cx| matches!(session.read(cx).queued(), [arko::Input::Quit(_)])));
}

#[gpui_kit::test]
fn stop_after_a_share_reply_prevents_its_delayed_popover(cx: &mut TestAppContext) {
    let (window, workspace, session) = setup(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    let request = session
        .update(cx, |session, _| session.take_requests())
        .pop()
        .unwrap();
    let arko::Request::ShareWebPage { reply, .. } = request else {
        panic!("share");
    };
    let page = arko::remote::Page {
        addr: "127.0.0.1:9000".parse().unwrap(),
        links: Vec::new(),
        note: None,
    };
    assert!(reply.send(Ok(page.clone())).is_ok());
    cx.run_until_parked();
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::StopSharingWebPage), cx)
    });
    session.update(cx, |session, cx| {
        session.set_status(
            Reading {
                page: Some(page),
                ..Reading::default()
            },
            Activity::default(),
            cx,
        )
    });
    act(cx, window, |_, _| {});
    assert!(!cx.read(|cx| workspace.read(cx).status_bar().read(cx).is_web_page_open()));
}

#[gpui_kit::test]
fn stop_wins_over_an_in_flight_share_and_its_late_status(cx: &mut TestAppContext) {
    let (window, workspace, session) = setup(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ShareWebPage), cx)
    });
    let request = session
        .update(cx, |session, _| session.take_requests())
        .pop()
        .unwrap();
    let arko::Request::ShareWebPage { reply, .. } = request else {
        panic!("share");
    };
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::StopSharingWebPage), cx)
    });
    let requests = session.update(cx, |session, _| session.take_requests());
    assert!(matches!(
        requests.as_slice(),
        [arko::Request::StopWebPage { .. }]
    ));
    let page = arko::remote::Page {
        addr: "127.0.0.1:9000".parse().unwrap(),
        links: Vec::new(),
        note: None,
    };
    assert!(reply.send(Ok(page.clone())).is_ok());
    session.update(cx, |session, cx| {
        session.set_status(
            Reading {
                page: Some(page),
                ..Reading::default()
            },
            Activity::default(),
            cx,
        )
    });
    act(cx, window, |_, _| {});
    assert!(!cx.read(|cx| workspace.read(cx).status_bar().read(cx).is_web_page_open()));
}
