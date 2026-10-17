//! Editing, regenerating and branching on arko's real engine, its model
//! answered by the mock server.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use arko::remote::Kind;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};

use crate::Startup;
use crate::backend::ArkoBackend;
use crate::live::LiveSessions;
use crate::session_view::SessionView;
use crate::testing::{Mock, reply, wait_until};
use crate::tests::{act, open};
use crate::workspace::Workspace;

struct App {
    _temp: tempfile::TempDir,
    root: PathBuf,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    view: Entity<SessionView>,
}

fn start(cx: &mut TestAppContext, mock: &Mock) -> App {
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
    App {
        _temp: temp,
        root,
        window,
        workspace,
        view,
    }
}

impl App {
    fn send(&self, cx: &mut TestAppContext, text: &str) {
        act(cx, self.window, |window, cx| window.input(text, cx));
        act(cx, self.window, |window, cx| window.press("enter", cx));
    }

    /// The shown session's feed: each item's kind and text.
    fn feed(&self, cx: &mut TestAppContext) -> Vec<(Kind, String)> {
        cx.read(|cx| {
            self.view
                .read(cx)
                .live_session()
                .and_then(|session| session.read(cx).feed().cloned())
                .map(|feed| {
                    feed.subscribe()
                        .0
                        .items
                        .into_iter()
                        .map(|item| (item.kind, item.text))
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn says(&self, cx: &mut TestAppContext, text: &str) -> bool {
        self.feed(cx).iter().any(|(_, said)| said.contains(text))
    }

    fn wait_for(&self, cx: &mut TestAppContext, text: &str) {
        wait_until(cx, text, |cx| self.says(cx, text));
    }

    /// Waits until the turn ended and the transcript caught up.
    fn wait_idle(&self, cx: &mut TestAppContext) {
        wait_until(cx, "the turn to end", |cx| {
            cx.update_window(self.window, |_, window, cx| window.render_frame(cx))
                .unwrap();
            cx.read(|cx| {
                let view = self.view.read(cx);
                let idle = view
                    .live_session()
                    .is_some_and(|session| !session.read(cx).is_working());
                idle && view
                    .transcript(cx)
                    .is_some_and(|transcript| transcript.read(cx).busy_reason().is_none())
            })
        });
    }

    /// The id of the user's message `text` in the shown session's feed.
    fn message(&self, cx: &mut TestAppContext, text: &str) -> u64 {
        cx.read(|cx| {
            let feed = self
                .view
                .read(cx)
                .live_session()
                .and_then(|session| session.read(cx).feed().cloned())
                .unwrap();
            feed.subscribe()
                .0
                .items
                .into_iter()
                .find(|item| item.kind == Kind::User && item.text == text)
                .map(|item| item.id)
                .unwrap()
        })
    }

    fn composer_text(&self, cx: &mut TestAppContext) -> String {
        cx.read(|cx| self.view.read(cx).composer().read(cx).text(cx))
    }

    fn set_composer(&self, cx: &mut TestAppContext, text: &str) {
        let composer = cx.read(|cx| self.view.read(cx).composer().clone());
        act(cx, self.window, |window, cx| {
            composer.update(cx, |composer, cx| composer.set_text(text, window, cx))
        });
    }

    fn close_all(&self, cx: &mut TestAppContext) {
        act(cx, self.window, |window, cx| {
            let sessions = LiveSessions::all(cx);
            crate::quit::close(sessions, false, window, cx);
        });
        wait_until(cx, "the sessions to end", |cx| {
            cx.read(|cx| LiveSessions::all(cx).is_empty())
        });
    }
}

/// The mock model answers each question it is asked last.
fn answers() -> Mock {
    Mock::start(|_, body| {
        let text = if body.contains("Count the apples.") {
            "Five apples."
        } else if body.contains("And the plums?") {
            "Three plums."
        } else {
            "Seven pears."
        };
        reply("a1", text)
    })
}

#[gpui_kit::test]
fn editing_a_message_cuts_the_conversation_back_after_asking(cx: &mut TestAppContext) {
    let mock = answers();
    let app = start(cx, &mock);
    app.send(cx, "Count the pears.");
    app.wait_for(cx, "Seven pears.");
    app.wait_idle(cx);
    app.send(cx, "And the plums?");
    app.wait_for(cx, "Three plums.");
    app.wait_idle(cx);
    // A draft waits in the composer while the first message is edited.
    app.set_composer(cx, "a draft");
    let first = app.message(cx, "Count the pears.");
    // (The hover actions take clicks while hidden.)
    act(cx, app.window, |window, cx| {
        window.click(("edit-message", first), cx)
    });
    assert_eq!(app.composer_text(cx), "Count the pears.");
    act(cx, app.window, |window, _| {
        assert!(window.try_find("editing-strip").is_some());
    });
    app.set_composer(cx, "Count the apples.");
    act(cx, app.window, |window, cx| window.press("enter", cx));
    // It asks first, saying what goes.
    act(cx, app.window, |window, _| {
        assert!(window.try_find("edit-dialog").is_some());
    });
    act(cx, app.window, |window, cx| {
        window.click("dont-ask-edit", cx)
    });
    act(cx, app.window, |window, cx| {
        window.click("edit-confirm", cx)
    });
    app.wait_for(cx, "Five apples.");
    app.wait_idle(cx);
    let said: Vec<String> = app.feed(cx).into_iter().map(|(_, text)| text).collect();
    assert!(
        !said.iter().any(|text| text.contains("Seven pears.")),
        "{said:?}"
    );
    assert!(
        !said.iter().any(|text| text.contains("And the plums?")),
        "{said:?}"
    );
    // The draft is back, and Edit chosen with "Don't ask again" is kept.
    assert_eq!(app.composer_text(cx), "a draft");
    assert!(!cx.read(|cx| crate::prefs::get(cx).sessions.ask_before_edit));
    let last = mock.bodies.lock().unwrap().last().cloned().unwrap();
    assert!(!last.contains("Seven pears."), "the cut turns don't go");
    app.close_all(cx);
}

#[gpui_kit::test]
fn up_in_the_empty_composer_edits_the_last_message_and_esc_gives_it_back(cx: &mut TestAppContext) {
    let mock = answers();
    let app = start(cx, &mock);
    app.send(cx, "Count the pears.");
    app.wait_for(cx, "Seven pears.");
    app.wait_idle(cx);
    act(cx, app.window, |window, cx| window.press("up", cx));
    assert_eq!(app.composer_text(cx), "Count the pears.");
    assert!(cx.read(|cx| app.view.read(cx).is_editing()));
    act(cx, app.window, |window, cx| window.press("escape", cx));
    assert_eq!(app.composer_text(cx), "");
    assert!(!cx.read(|cx| app.view.read(cx).is_editing()));
    app.close_all(cx);
}

#[gpui_kit::test]
fn regenerate_sends_the_last_message_again_for_a_new_reply(cx: &mut TestAppContext) {
    let replies = Arc::new(AtomicUsize::new(0));
    let mock = Mock::start({
        let replies = replies.clone();
        move |_, _| {
            let n = replies.fetch_add(1, Ordering::SeqCst);
            reply("a1", if n == 0 { "First try." } else { "Second try." })
        }
    });
    let app = start(cx, &mock);
    app.send(cx, "Name a fruit.");
    app.wait_for(cx, "First try.");
    app.wait_idle(cx);
    let reply = cx.read(|cx| {
        app.view
            .read(cx)
            .transcript(cx)
            .and_then(|transcript| transcript.read(cx).last_reply())
    });
    let reply = reply.expect("the reply can be regenerated");
    act(cx, app.window, |window, cx| {
        window.click(("regenerate", reply), cx)
    });
    act(cx, app.window, |window, cx| {
        window.click("edit-confirm", cx)
    });
    app.wait_for(cx, "Second try.");
    app.wait_idle(cx);
    assert!(!app.says(cx, "First try."));
    let messages = app
        .feed(cx)
        .into_iter()
        .filter(|(kind, text)| *kind == Kind::User && text == "Name a fruit.")
        .count();
    assert_eq!(messages, 1, "the message once, sent again");
    app.close_all(cx);
}

#[gpui_kit::test]
fn branching_opens_a_new_session_with_the_conversation_before(cx: &mut TestAppContext) {
    let mock = answers();
    let app = start(cx, &mock);
    app.send(cx, "Count the pears.");
    app.wait_for(cx, "Seven pears.");
    app.wait_idle(cx);
    app.send(cx, "And the plums?");
    app.wait_for(cx, "Three plums.");
    app.wait_idle(cx);
    let original = cx.read(|cx| app.view.read(cx).live_session().cloned().unwrap());
    let second = app.message(cx, "And the plums?");
    act(cx, app.window, |window, cx| {
        window.click(("branch-message", second), cx)
    });
    // The branch opens here, named after the session, with the message
    // in its composer and the conversation before it.
    wait_until(cx, "the branch to open", |cx| {
        cx.read(|cx| {
            app.view.read(cx).live_session().is_some_and(|session| {
                session.entity_id() != original.entity_id()
                    && session.read(cx).title().starts_with("Branch of")
            })
        })
    });
    app.wait_for(cx, "Seven pears.");
    assert!(!app.says(cx, "Three plums."));
    assert_eq!(app.composer_text(cx), "And the plums?");
    // The original keeps its conversation, and both are listed.
    wait_until(cx, "both sessions listed", |cx| {
        cx.read(|cx| app.workspace.read(cx).sidebar().read(cx).sessions().len() == 2)
    });
    assert_eq!(arko::sessions::list(&app.root).len(), 2);
    app.close_all(cx);
}

#[gpui_kit::test]
fn branch_instead_goes_on_in_a_new_session_and_leaves_this_one(cx: &mut TestAppContext) {
    let mock = answers();
    let app = start(cx, &mock);
    app.send(cx, "Count the pears.");
    app.wait_for(cx, "Seven pears.");
    app.wait_idle(cx);
    app.send(cx, "And the plums?");
    app.wait_for(cx, "Three plums.");
    app.wait_idle(cx);
    let original = cx.read(|cx| app.view.read(cx).live_session().cloned().unwrap());
    act(cx, app.window, |window, cx| window.press("up", cx));
    app.set_composer(cx, "Count the apples.");
    act(cx, app.window, |window, cx| window.press("enter", cx));
    act(cx, app.window, |window, cx| window.click("edit-branch", cx));
    // The branch has the first exchange and the edited message's reply.
    wait_until(cx, "the branch's reply", |cx| {
        cx.read(|cx| {
            app.view
                .read(cx)
                .live_session()
                .is_some_and(|session| session.entity_id() != original.entity_id())
        }) && app.says(cx, "Five apples.")
    });
    assert!(app.says(cx, "Seven pears."));
    assert!(!app.says(cx, "Three plums."));
    // The session it came from is as it was, "Don't ask again" untouched.
    let kept: Vec<String> = cx.read(|cx| {
        original
            .read(cx)
            .feed()
            .unwrap()
            .subscribe()
            .0
            .items
            .into_iter()
            .map(|item| item.text)
            .collect()
    });
    assert!(kept.iter().any(|text| text.contains("Three plums.")));
    assert!(cx.read(|cx| crate::prefs::get(cx).sessions.ask_before_edit));
    app.close_all(cx);
}
