//! Exporting a conversation of arko's real engine, its model answered by
//! the mock server.

use std::sync::Arc;
use std::time::Duration;

use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AppContext as _, TestAppContext};

use crate::Startup;
use crate::backend::ArkoBackend;
use crate::testing::{Mock, reply, wait_until};
use crate::tests::{act, open};

#[gpui_kit::test]
fn export_writes_the_conversation_where_the_user_chooses(cx: &mut TestAppContext) {
    // The reply to the plums takes a moment, long enough to try an export
    // while it runs.
    let mock = Mock::start(|_, body| {
        if body.contains("And the plums?") && !body.contains("And the figs?") {
            crate::testing::reply_after("a1", "Seven **pears**.", Duration::from_millis(1500))
        } else {
            reply("a1", "Seven **pears**.")
        }
    });
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
    use gpui_kit::component::WindowExt as _;
    let notes = |cx: &mut TestAppContext| {
        cx.update_window(window, |_, window, cx| window.notifications(cx).len())
            .unwrap()
    };
    let session = cx.read(|cx| view.read(cx).live_session().unwrap().clone());
    act(cx, window, |window, cx| window.input("And the plums?", cx));
    act(cx, window, |window, cx| window.press("enter", cx));
    // While the reply comes, the session says to export after it, and no
    // file is chosen or written.
    wait_until(cx, "the turn", |cx| {
        cx.read(|cx| session.read(cx).is_working())
    });
    let before = notes(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    wait_until(cx, "the refusal", |cx| notes(cx) > before);
    assert!(!cx.did_prompt_for_new_path());
    wait_until(cx, "the second reply", |cx| {
        cx.read(|cx| {
            let session = view.read(cx).live_session().unwrap().read(cx);
            !session.is_working()
                && session
                    .feed()
                    .unwrap()
                    .subscribe()
                    .0
                    .items
                    .iter()
                    .filter(|item| item.kind == arko::remote::Kind::Reply)
                    .count()
                    == 2
        })
    });
    // A message sent as the export starts: the session refuses it, or what
    // it read is out of date. Either way no file is chosen.
    let before = notes(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx);
        session.update(cx, |session, cx| {
            session
                .send(
                    arko::Input::Message(arko::Message {
                        text: "And the figs?".into(),
                        images: Vec::new(),
                    }),
                    cx,
                )
                .unwrap()
        });
    });
    wait_until(cx, "the refusal of a changing conversation", |cx| {
        notes(cx) > before
    });
    assert!(!cx.did_prompt_for_new_path());
    wait_until(cx, "the third reply", |cx| {
        cx.read(|cx| {
            let session = session.read(cx);
            !session.is_working()
                && session
                    .feed()
                    .unwrap()
                    .subscribe()
                    .0
                    .items
                    .iter()
                    .filter(|item| item.kind == arko::remote::Kind::Reply)
                    .count()
                    == 3
        })
    });
    // The UI has only a late part, as it does when a long feed was trimmed.
    let feed = cx.read(|cx| {
        view.read(cx)
            .live_session()
            .unwrap()
            .read(cx)
            .feed()
            .unwrap()
            .clone()
    });
    let items = feed.subscribe().0.items;
    let start = items
        .iter()
        .position(|item| item.text == "And the plums?")
        .unwrap();
    let tail = items[start..].to_vec();
    feed.conversation("The retained part");
    feed.restore(tail);
    wait_until(cx, "the trimmed transcript", |cx| {
        cx.read(|cx| {
            view.read(cx)
                .transcript(cx)
                .unwrap()
                .read(cx)
                .starts_partway()
        })
    });
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    // The save dialog opens only after the saved history was read.
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let file = temp.path().join("pears.md");
    cx.simulate_new_path_selection(|folder| {
        assert_eq!(folder, work.as_path());
        Some(file.clone())
    });
    wait_until(cx, "the file", |_| file.exists());
    let text = std::fs::read_to_string(&file).unwrap();
    assert!(text.starts_with("# Count the pears.\n"), "{text}");
    assert!(text.contains("\n## You\n\nCount the pears.\n"), "{text}");
    assert!(text.contains("And the plums?"), "{text}");
    assert!(text.contains("And the figs?"), "{text}");
    assert!(!text.contains(crate::export::STARTS_PARTWAY), "{text}");
    assert!(text.contains("\n## Arko\n\nSeven **pears**.\n"), "{text}");
    let id = cx.read(|cx| session.read(cx).id().unwrap().0.clone());
    let marker = root
        .join("sessions")
        .join(&id)
        .join("run/latest-checkpoint");
    let checkpoint = std::fs::read(&marker).unwrap();
    let before = notes(cx);
    std::fs::write(&marker, "missing-checkpoint.json").unwrap();
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    wait_until(cx, "the missing checkpoint error", |cx| notes(cx) > before);
    assert!(!cx.did_prompt_for_new_path());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    std::fs::write(&marker, checkpoint).unwrap();
    session.update(cx, |session, cx| {
        session
            .send(
                arko::Input::Message(arko::Message {
                    text: "/new".into(),
                    images: Vec::new(),
                }),
                cx,
            )
            .unwrap()
    });
    wait_until(cx, "the empty new conversation", |_| {
        arko::sessions::conversation(&root, &id, None)
            .err()
            .is_some_and(|why| why.contains("no messages yet"))
    });
    let before = notes(cx);
    act(cx, window, |window, cx| {
        window.dispatch_action(Box::new(crate::menus::ExportMarkdown), cx)
    });
    wait_until(cx, "the empty conversation notice", |cx| notes(cx) > before);
    assert!(!cx.did_prompt_for_new_path());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    act(cx, window, |window, cx| {
        let sessions = crate::live::LiveSessions::all(cx);
        crate::quit::close(sessions, false, window, cx);
    });
    wait_until(cx, "the session to end", |cx| {
        cx.read(|cx| crate::live::LiveSessions::all(cx).is_empty())
    });
}

/// The save dialog can't open (Linux without a desktop portal): Arko offers
/// to save the conversation in the folder the dialog would have opened.
#[gpui_kit::test]
fn when_the_save_dialog_cant_open_the_export_is_offered_a_place(cx: &mut TestAppContext) {
    use crate::backend::FakeBackend;
    use gpui_kit::component::WindowExt as _;

    let temp = tempfile::tempdir().unwrap();
    let backend = Arc::new(FakeBackend::sample("/arko", std::time::SystemTime::now()));
    cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    let (window, workspace) = open(cx, 1300.);
    let view = cx.read(|cx| workspace.read(cx).session_view().clone());
    let path = crate::export::free_path(temp.path(), "Pears.md");
    act(cx, window, |window, cx| {
        view.update(cx, |view, cx| {
            view.offer_export_to(
                crate::session_view::Export {
                    text: "# Pears\n".into(),
                    starts_partway: false,
                },
                path.clone(),
                "no portal".into(),
                window,
                cx,
            )
        })
    });
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx));
        window.within("dialog").click("ok", cx)
    });
    wait_until(cx, "the file", |_| path.exists());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# Pears\n");
}
