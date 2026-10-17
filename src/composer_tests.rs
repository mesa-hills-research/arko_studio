//! Interaction tests of the composer and the session's dialogs, on the fake
//! backend's sample sessions (which show, but don't run).

use std::sync::Arc;
use std::time::SystemTime;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, Entity, TestAppContext};

use crate::Startup;
use crate::backend::FakeBackend;
use crate::live::{PromptAnswer, PromptKind};
use crate::tests::{act, open, row_of};
use crate::workspace::Workspace;

fn start(cx: &mut TestAppContext) -> (Arc<FakeBackend>, AnyWindowHandle, Entity<Workspace>) {
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend.clone()), cx));
    let (window, workspace) = open(cx, 1300.);
    (backend, window, workspace)
}

fn open_row(
    window: AnyWindowHandle,
    workspace: &Entity<Workspace>,
    title: &str,
    cx: &mut TestAppContext,
) {
    let row = row_of(workspace, title, cx);
    act(cx, window, |window, cx| window.click(("row", row), cx));
}

#[gpui_kit::test]
fn missing_credentials_are_entered_in_a_dialog(cx: &mut TestAppContext) {
    let (backend, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    let session = cx.read(|cx| workspace.read(cx).live_session().cloned().unwrap());
    let need = || {
        PromptKind::Credentials(arko::Need::AnthropicKey {
            model: "claude-opus-5-5".into(),
        })
    };
    let mut answer = cx.update(|cx| session.update(cx, |session, cx| session.ask(need(), cx)));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        assert!(window.try_find("credentials-form").is_some());
    });
    // Saving with nothing typed says so and keeps asking.
    act(cx, window, |window, cx| {
        window.click("credentials-save", cx)
    });
    act(cx, window, |window, _| {
        assert!(window.try_find("credentials-error").is_some())
    });
    act(cx, window, |window, cx| {
        window.input("sk-ant-test-0123456789", cx)
    });
    act(cx, window, |window, cx| {
        window.click("credentials-save", cx)
    });
    assert_eq!(
        backend.added_keys(),
        [(
            "anthropic".to_string(),
            "sk-ant-test-0123456789".to_string()
        )]
    );
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Again));
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx))
    });

    // Closed without an answer, it waits: the strip brings it back; Cancel
    // tells the engine.
    let mut answer = cx.update(|cx| session.update(cx, |session, cx| session.ask(need(), cx)));
    act(cx, window, |window, cx| window.close_dialog(cx));
    act(cx, window, |window, cx| window.click("prompt-show", cx));
    act(cx, window, |window, cx| {
        window.click("credentials-cancel", cx)
    });
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Cancel));
    act(cx, window, |window, _| {
        assert!(window.try_find("prompt-strip").is_none())
    });
}

#[gpui_kit::test]
fn claude_code_is_checked_again_from_its_dialog(cx: &mut TestAppContext) {
    let (_, window, workspace) = start(cx);
    open_row(window, &workspace, "Split the engine out of main.rs", cx);
    let session = cx.read(|cx| workspace.read(cx).live_session().cloned().unwrap());
    let mut answer = cx.update(|cx| {
        session.update(cx, |session, cx| {
            session.ask(
                PromptKind::Credentials(arko::Need::ClaudeCode {
                    model: "cc:claude-opus-5-5".into(),
                    problem: arko::accounts::Problem::Missing,
                }),
                cx,
            )
        })
    });
    act(cx, window, |window, cx| {
        assert!(
            window.try_find("credentials-save").is_none(),
            "no key to enter"
        );
        window.click("credentials-again", cx);
    });
    assert_eq!(answer.try_recv(), Ok(PromptAnswer::Again));
}
