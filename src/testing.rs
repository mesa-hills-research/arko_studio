//! Test helpers: arko's mock model server on a runtime of its own, real
//! sessions in a temporary Arko folder, and waiting from GPUI's test
//! executor for what the engine's threads do.

use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use arko::testing::{ModelServer, Respond};
use gpui_kit::TestAppContext;

use crate::backend::{Configure, SessionId};

/// How long a test waits for the engine before it fails.
pub const WAIT: Duration = Duration::from_secs(30);

/// An answer the mock model server gives a request.
pub type Answer = Pin<Box<dyn Future<Output = String> + Send>>;

/// arko's mock model server, on a runtime of its own (the app's GPUI tests
/// have none), with every request body it got.
pub struct Mock {
    runtime: tokio::runtime::Runtime,
    server: Option<ModelServer>,
    pub bodies: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    /// A server that answers request `index` with body `body` as `respond`
    /// says (`arko::testing::reply` and friends).
    pub fn start(respond: impl Fn(usize, String) -> Answer + Send + Sync + 'static) -> Self {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let bodies: Arc<Mutex<Vec<String>>> = Arc::default();
        let respond: Arc<Respond> = {
            let bodies = bodies.clone();
            Arc::new(move |index, body: String| {
                bodies.lock().unwrap().push(body.clone());
                respond(index, body)
            })
        };
        let server = runtime.block_on(ModelServer::start(respond));
        Self {
            runtime,
            server: Some(server),
            bodies,
        }
    }

    /// Gives every session the server's client. A new one runs luna with
    /// plain-text replies.
    pub fn configure(&self) -> Configure {
        let client = Arc::new(self.server.as_ref().unwrap().client());
        Arc::new(move |options: &mut arko::Options| {
            options.client = Some(arko::testing::GivenClient(client.clone()));
            if options.session == arko::SessionChoice::New {
                options.model.get_or_insert_with(|| "luna".into());
                options.openai_auth = Some(arko::OpenAiAuth::ApiKey);
                options.cache_1h = Some(false);
                options.text_replies = Some(true);
            }
        })
    }

    /// Whether a request body contained `text`.
    pub fn was_sent(&self, text: &str) -> bool {
        self.bodies
            .lock()
            .unwrap()
            .iter()
            .any(|body| body.contains(text))
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        let _inside = self.runtime.enter();
        self.server.take();
    }
}

/// A model turn that replies `text`, after `delay`.
pub fn reply_after(id: &str, text: &str, delay: Duration) -> Answer {
    let reply = arko::testing::reply(id, text);
    Box::pin(async move {
        tokio::time::sleep(delay).await;
        reply
    })
}

/// A model turn that replies `text` at once.
pub fn reply(id: &str, text: &str) -> Answer {
    let reply = arko::testing::reply(id, text);
    Box::pin(async move { reply })
}

/// Creates a real session in Arko's folder `root`, working in `workspace`,
/// whose first message is `message`, and closes it again.
pub fn create_session(root: &Path, workspace: &Path, message: &str) -> SessionId {
    let mock = Mock::start(|_, _| reply("a1", "Done."));
    let mut options = arko::Options {
        root: Some(root.to_path_buf()),
        workspace: Some(workspace.to_path_buf()),
        ..arko::Options::default()
    };
    (mock.configure())(&mut options);
    let running = arko::spawn(options, Arc::new(arko::NoPrompter));
    mock.runtime.block_on(async {
        let seam = tokio::time::timeout(WAIT, running.opened)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        seam.input
            .send(arko::Input::Message(arko::Message {
                text: message.into(),
                images: Vec::new(),
            }))
            .unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                let answered = seam
                    .feed
                    .subscribe()
                    .0
                    .items
                    .iter()
                    .any(|item| item.text.contains("Done."));
                if answered && !seam.feed.activity().working {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        seam.input
            .send(arko::Input::Quit(Some(Vec::new())))
            .unwrap();
        tokio::time::timeout(WAIT, running.ended)
            .await
            .unwrap()
            .unwrap();
        SessionId(seam.session.id)
    })
}

/// Runs the app until `done` holds, letting the engine's threads work in
/// real time (and GPUI's clock follow), or fails after [`WAIT`].
pub fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + WAIT;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
        cx.executor().advance_clock(crate::live::FRAME);
    }
}
