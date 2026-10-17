//! Exporting a conversation of arko's real engine, its model answered by
//! the mock server.

use std::sync::Arc;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use crate::Startup;
use crate::backend::ArkoBackend;
use crate::testing::{Mock, reply, wait_until};
use crate::tests::{act, open};

#[gpui_kit::test]
fn export_writes_the_conversation_where_the_user_chooses(cx: &mut TestAppContext) {
    let mock = Mock::start(|_, _| reply("a1", "Seven **pears**."));
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    let work = temp.path().join("work");
    std::fs::create_dir_all(&work).unwrap();
    cx.executor().allow_parking();
    let backend = Arc::new(ArkoBackend::new(&root).with_configure(mock.configure()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    // Before a conversation there is nothing to export.
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    assert!(!cx.did_prompt_for_new_path());
    cx.update(|cx| view.update(cx, |view, cx| view.set_draft_folder(work.clone(), cx)));
    act(cx, window, |window, cx| {
        window.input("Count the pears.", cx)
    });
    act(cx, window, |window, cx| window.press("enter", cx));
    wait_until(cx, "the reply in the transcript", |cx| {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        cx.read(|cx| {
            view.read(cx).transcript(cx).is_some_and(|transcript| {
                transcript
                    .read(cx)
                    .export_items()
                    .iter()
                    .any(|item| item.text.contains("Seven"))
                    && transcript.read(cx).busy_reason().is_none()
            })
        })
    });
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    // The save dialog opens in the session's folder, named for the session.
    let file = temp.path().join("pears.md");
    cx.simulate_new_path_selection(|folder| {
        assert_eq!(folder, work.as_path());
        Some(file.clone())
    });
    wait_until(cx, "the file", |_| file.exists());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# Count the pears.\n"), "{text}");
    assert!(text.contains("\n## You\n\nCount the pears.\n"), "{text}");
    assert!(text.contains("\n## Arko\n\nSeven **pears**.\n"), "{text}");
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}
