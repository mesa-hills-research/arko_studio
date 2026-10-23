//! Golden screenshots of Arko Studio's window, rendered headlessly by
//! mhr-gpui-screenshot on Mesa's software Vulkan driver, in Work Sans and
//! JetBrains Mono. The goldens live in `tests/screenshots`. On a mismatch the
//! actual image and a diff go to `tests/screenshots/failures`, and
//! `UPDATE_GOLDENS=1` rewrites the goldens.
#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::SystemTime;

use arko_studio::backend::{Backend as _, FakeBackend, SessionEntry, SessionId};
use arko_studio::prefs::{self, Motion, Prefs, ThemeMode};
use arko_studio::settings_page::{
    ACCOUNTS_PAGE, ADVANCED_PAGE, APPEARANCE_PAGE, EXTENSIONS_PAGE, INPUT_PAGE, NOTIFICATIONS_PAGE,
    PROFILES_PAGE, SESSIONS_PAGE, WEB_PAGE_PAGE,
};
use arko_studio::{Startup, assets::Assets, fonts, workspace};
use gpui_kit::{px, size};
use gpui_screenshot::{Goldens, Screenshot, ScreenshotApp, Tolerance};

/// Mesa and LLVM releases round antialiased edges a level or two apart, so
/// each channel of each pixel may differ from its golden by up to 2.
fn goldens() -> Goldens {
    gpui_screenshot::goldens!().tolerance(Tolerance::channel(2))
}

/// A window as it opens, on a new session, `width` logical pixels wide, with
/// the sample sessions in the sidebar.
fn new_window(mode: ThemeMode, width: f32) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    // The harness draws animations at their end, and so does the app then.
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        for family in [prefs::DEFAULT_UI_FONT, prefs::DEFAULT_CODE_FONT] {
            assert!(fonts::is_available(family, cx), "{family} isn't loaded");
        }
    });
    let window = app
        .open_window_with(size(px(width), px(800.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

/// A window `width` logical pixels wide on a session with a long title, in
/// a folder with a long name, that had workers (so both layout buttons
/// show). The folder's name gives way first, then the title.
fn long_title(mode: ThemeMode, width: f32) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let mut sessions = FakeBackend::sample("/arko", SystemTime::now())
        .sessions()
        .unwrap();
    let id = SessionId("20261016-091300-e1e1".into());
    sessions.push(SessionEntry {
        id: id.clone(),
        title: "A long session title that should stay readable".into(),
        workspace: Some("/home/ada/src/a-project-folder-with-a-very-long-descriptive-name".into()),
        had_workers: true,
        ..sessions[0].clone()
    });
    let backend =
        FakeBackend::new("/arko", sessions).with_default_model(Some("claude-opus-5-5".into()));
    let startup = Startup::for_tests(Arc::new(backend))
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut opened = None;
    let window = app
        .open_window_with(size(px(width), px(400.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened = Some(workspace);
                window
            })
        })
        .unwrap();
    let workspace = opened.unwrap();
    app.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| workspace.open_session(id, window, cx));
    })
    .unwrap();
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn long_title_light() {
    goldens().assert("long-title-light", &long_title(ThemeMode::Light, 600.));
}
#[test]
fn long_title_dark() {
    goldens().assert("long-title-dark", &long_title(ThemeMode::Dark, 600.));
}
#[test]
fn minimum_title_light() {
    goldens().assert("minimum-title-light", &long_title(ThemeMode::Light, 400.));
}
#[test]
fn minimum_title_dark() {
    goldens().assert("minimum-title-dark", &long_title(ThemeMode::Dark, 400.));
}

#[test]
fn wide_light() {
    goldens().assert("wide-light", &new_window(ThemeMode::Light, 1300.));
}

#[test]
fn wide_dark() {
    goldens().assert("wide-dark", &new_window(ThemeMode::Dark, 1300.));
}

#[test]
fn half_light() {
    goldens().assert("half-light", &new_window(ThemeMode::Light, 900.));
}

#[test]
fn half_dark() {
    goldens().assert("half-dark", &new_window(ThemeMode::Dark, 900.));
}

#[test]
fn narrow_light() {
    goldens().assert("narrow-light", &new_window(ThemeMode::Light, 600.));
}

#[test]
fn narrow_dark() {
    goldens().assert("narrow-dark", &new_window(ThemeMode::Dark, 600.));
}

/// Settings' page `page` in a window `width` wide, on the sample sessions
/// and accounts.
fn settings_window(mode: ThemeMode, width: f32, page: usize) -> Screenshot {
    settings_window_on(
        FakeBackend::sample("/arko", SystemTime::now()),
        mode,
        width,
        page,
    )
}

/// Settings' page `page` as [`settings_window`], on `backend`.
fn settings_window_on(
    backend: FakeBackend,
    mode: ThemeMode,
    width: f32,
    page: usize,
) -> Screenshot {
    settings_window_with(backend, mode, width, page, |_, _| {})
}

/// Settings' page `page` as [`settings_window`], on `backend`, once
/// `change` has acted on the window.
fn settings_window_with(
    backend: FakeBackend,
    mode: ThemeMode,
    width: f32,
    page: usize,
    change: impl FnOnce(&mut ScreenshotApp, gpui_kit::AnyWindowHandle),
) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let startup = Startup::for_tests(Arc::new(backend))
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let opened = std::cell::RefCell::new(None);
    let window = app
        .open_window_with(size(px(width), px(1200.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened.replace(Some(workspace));
                window
            })
        })
        .unwrap();
    let workspace = opened.take().unwrap();
    app.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(arko_studio::menus::OpenSettings), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        let settings = workspace.read(cx).settings().unwrap().clone();
        settings.update(cx, |settings, cx| settings.show_page(page, cx));
    });
    change(&mut app, window);
    app.capture(window).unwrap()
}

#[test]
fn settings_appearance_light() {
    goldens().assert(
        "settings-appearance-light",
        &settings_window(ThemeMode::Light, 1300., APPEARANCE_PAGE),
    );
}

#[test]
fn settings_appearance_dark() {
    goldens().assert(
        "settings-appearance-dark",
        &settings_window(ThemeMode::Dark, 1300., APPEARANCE_PAGE),
    );
}

#[test]
fn settings_input_light() {
    goldens().assert(
        "settings-input-light",
        &settings_window(ThemeMode::Light, 1300., INPUT_PAGE),
    );
}

#[test]
fn settings_input_dark() {
    goldens().assert(
        "settings-input-dark",
        &settings_window(ThemeMode::Dark, 1300., INPUT_PAGE),
    );
}

/// Where a choice no longer fits beside its title: Send key goes under it.
#[test]
fn settings_input_medium_light() {
    goldens().assert(
        "settings-input-medium-light",
        &settings_window(ThemeMode::Light, 980., INPUT_PAGE),
    );
}

#[test]
fn settings_input_medium_dark() {
    goldens().assert(
        "settings-input-medium-dark",
        &settings_window(ThemeMode::Dark, 980., INPUT_PAGE),
    );
}

#[test]
fn settings_accounts_light() {
    goldens().assert(
        "settings-accounts-light",
        &settings_window(ThemeMode::Light, 1300., ACCOUNTS_PAGE),
    );
}

#[test]
fn settings_accounts_dark() {
    goldens().assert(
        "settings-accounts-dark",
        &settings_window(ThemeMode::Dark, 1300., ACCOUNTS_PAGE),
    );
}

#[test]
fn settings_sessions_light() {
    goldens().assert(
        "settings-sessions-light",
        &settings_window(ThemeMode::Light, 1300., SESSIONS_PAGE),
    );
}

#[test]
fn settings_sessions_dark() {
    goldens().assert(
        "settings-sessions-dark",
        &settings_window(ThemeMode::Dark, 1300., SESSIONS_PAGE),
    );
}

#[test]
fn settings_notifications_light() {
    goldens().assert(
        "settings-notifications-light",
        &settings_window(ThemeMode::Light, 1300., NOTIFICATIONS_PAGE),
    );
}

#[test]
fn settings_notifications_dark() {
    goldens().assert(
        "settings-notifications-dark",
        &settings_window(ThemeMode::Dark, 1300., NOTIFICATIONS_PAGE),
    );
}

#[test]
fn settings_web_page_light() {
    goldens().assert(
        "settings-web-page-light",
        &settings_window(ThemeMode::Light, 1300., WEB_PAGE_PAGE),
    );
}

#[test]
fn settings_web_page_dark() {
    goldens().assert(
        "settings-web-page-dark",
        &settings_window(ThemeMode::Dark, 1300., WEB_PAGE_PAGE),
    );
}

#[test]
fn settings_extensions_light() {
    goldens().assert(
        "settings-extensions-light",
        &settings_window(ThemeMode::Light, 1300., EXTENSIONS_PAGE),
    );
}

#[test]
fn settings_extensions_dark() {
    goldens().assert(
        "settings-extensions-dark",
        &settings_window(ThemeMode::Dark, 1300., EXTENSIONS_PAGE),
    );
}

#[test]
fn settings_advanced_light() {
    goldens().assert(
        "settings-advanced-light",
        &settings_window(ThemeMode::Light, 1300., ADVANCED_PAGE),
    );
}

#[test]
fn settings_advanced_dark() {
    goldens().assert(
        "settings-advanced-dark",
        &settings_window(ThemeMode::Dark, 1300., ADVANCED_PAGE),
    );
}

/// Under 480 px, each setting's control goes under its title.
#[test]
fn settings_sessions_narrow_light() {
    goldens().assert(
        "settings-sessions-narrow-light",
        &settings_window(ThemeMode::Light, 460., SESSIONS_PAGE),
    );
}

#[test]
fn settings_sessions_narrow_dark() {
    goldens().assert(
        "settings-sessions-narrow-dark",
        &settings_window(ThemeMode::Dark, 460., SESSIONS_PAGE),
    );
}

/// The profiles Settings lists in the screenshots, over Arko's folder `/arko`.
fn profiles_sample() -> arko_studio::backend::profiles::Profiles {
    arko_studio::backend::profiles::Profiles::sample(std::path::Path::new("/arko"))
}

fn profiles_backend(profiles: arko_studio::backend::profiles::Profiles) -> FakeBackend {
    FakeBackend::sample("/arko", SystemTime::now()).with_profiles(profiles)
}

/// Worker profiles with the editor open on the profile at `row`, or on a
/// new one.
fn profile_editor(
    backend: FakeBackend,
    mode: ThemeMode,
    width: f32,
    row: Option<usize>,
) -> Screenshot {
    use gpui_kit::test::TestWindowExt as _;
    settings_window_with(backend, mode, width, PROFILES_PAGE, |app, window| {
        app.run_until_parked();
        app.update_window(window, |_, window, cx| match row {
            Some(row) => window.click(("edit-profile", row), cx),
            None => window.click("add-profile", cx),
        })
        .unwrap();
        app.run_until_parked();
    })
}

fn profiles_page(backend: FakeBackend, mode: ThemeMode, width: f32) -> Screenshot {
    settings_window_with(backend, mode, width, PROFILES_PAGE, |app, _| {
        app.run_until_parked()
    })
}

#[test]
fn settings_profiles_empty_light() {
    goldens().assert(
        "settings-profiles-empty-light",
        &profiles_page(
            profiles_backend(Default::default()),
            ThemeMode::Light,
            1300.,
        ),
    );
}

#[test]
fn settings_profiles_empty_dark() {
    goldens().assert(
        "settings-profiles-empty-dark",
        &profiles_page(profiles_backend(Default::default()), ThemeMode::Dark, 1300.),
    );
}

#[test]
fn settings_profiles_light() {
    goldens().assert(
        "settings-profiles-light",
        &profiles_page(profiles_backend(profiles_sample()), ThemeMode::Light, 1300.),
    );
}

#[test]
fn settings_profiles_dark() {
    goldens().assert(
        "settings-profiles-dark",
        &profiles_page(profiles_backend(profiles_sample()), ThemeMode::Dark, 1300.),
    );
}

#[test]
fn settings_profiles_narrow_light() {
    goldens().assert(
        "settings-profiles-narrow-light",
        &profiles_page(profiles_backend(profiles_sample()), ThemeMode::Light, 460.),
    );
}

#[test]
fn settings_profiles_narrow_dark() {
    goldens().assert(
        "settings-profiles-narrow-dark",
        &profiles_page(profiles_backend(profiles_sample()), ThemeMode::Dark, 460.),
    );
}

#[test]
fn settings_profile_editor_light() {
    goldens().assert(
        "settings-profile-editor-light",
        &profile_editor(
            profiles_backend(profiles_sample()),
            ThemeMode::Light,
            1300.,
            Some(0),
        ),
    );
}

#[test]
fn settings_profile_editor_dark() {
    goldens().assert(
        "settings-profile-editor-dark",
        &profile_editor(
            profiles_backend(profiles_sample()),
            ThemeMode::Dark,
            1300.,
            Some(0),
        ),
    );
}

#[test]
fn settings_profile_editor_narrow_light() {
    goldens().assert(
        "settings-profile-editor-narrow-light",
        &profile_editor(
            profiles_backend(profiles_sample()),
            ThemeMode::Light,
            460.,
            Some(0),
        ),
    );
}

#[test]
fn settings_profile_editor_narrow_dark() {
    goldens().assert(
        "settings-profile-editor-narrow-dark",
        &profile_editor(
            profiles_backend(profiles_sample()),
            ThemeMode::Dark,
            460.,
            Some(0),
        ),
    );
}

/// The models of the profile screenshots. Claude needs a key, OpenAI doesn't.
fn profile_models() -> arko_studio::backend::ModelChoices {
    use arko_studio::backend::{ModelChoice, ModelChoices, ModelGroup};
    let group = |name: &str, needs: Option<&str>, models: &[(&str, &str)]| ModelGroup {
        name: name.into(),
        needs: needs.map(str::to_string),
        models: models
            .iter()
            .map(|(name, alias)| ModelChoice {
                name: (*name).into(),
                alias: (*alias).into(),
            })
            .collect(),
    };
    ModelChoices {
        groups: vec![
            group(
                "Anthropic API, billed per token",
                Some("ANTHROPIC_API_KEY"),
                &[("claude-opus-5-5", "opus"), ("claude-haiku-5-5", "haiku")],
            ),
            group("OpenAI, with your API key", None, &[("gpt-6.1-sol", "sol")]),
        ],
        default: None,
        note: None,
    }
}

/// The user's profiles, a project's and a repository's, read only.
fn profiles_with_project() -> arko_studio::backend::profiles::Profiles {
    use arko::profiles::Profile;
    use arko_studio::backend::profiles::{Entry, List, Project, Unreadable};
    let mut lint = Profile::new("lint", "Runs the linter and reports what it finds");
    lint.model = Some("haiku".into());
    let mut docs = Profile::new("docs-writer", "Writes the changelog entry for a change");
    docs.model = Some("sol".into());
    docs.effort = Some(arko::Effort::Medium);
    let entry = |profile: Profile| Entry {
        file: format!("/home/ada/src/arko/.agents/profiles/{}.md", profile.name).into(),
        note: profile.model_problem(),
        profile,
    };
    let mut profiles = profiles_sample();
    profiles.projects.push(Project {
        folder: "/home/ada/src/arko".into(),
        list: List {
            entries: vec![entry(docs), entry(lint)],
            unreadable: vec![Unreadable {
                file: "/home/ada/src/arko/.agents/profiles/torn.md".into(),
                reason: "it has no description".into(),
            }],
        },
    });
    let mut triage = Profile::new("triage", "Sorts new issues by area");
    triage.model = Some("haiku".into());
    profiles.repositories.push(Project {
        folder: "/home/ada/src/mesa".into(),
        list: List {
            entries: vec![Entry {
                file: "/home/ada/src/mesa/.agents/profiles/triage.md".into(),
                note: triage.model_problem(),
                profile: triage,
            }],
            unreadable: Vec::new(),
        },
    });
    profiles
}

#[test]
fn settings_profiles_needing_a_key_light() {
    goldens().assert(
        "settings-profiles-key-light",
        &profile_editor(
            profiles_backend(profiles_sample()).with_models(profile_models()),
            ThemeMode::Light,
            1300.,
            Some(1),
        ),
    );
}

#[test]
fn settings_profiles_needing_a_key_dark() {
    goldens().assert(
        "settings-profiles-key-dark",
        &profile_editor(
            profiles_backend(profiles_sample()).with_models(profile_models()),
            ThemeMode::Dark,
            1300.,
            Some(1),
        ),
    );
}

#[test]
fn settings_profiles_project_light() {
    goldens().assert(
        "settings-profiles-project-light",
        &profiles_page(
            profiles_backend(profiles_with_project()).with_models(profile_models()),
            ThemeMode::Light,
            1300.,
        ),
    );
}

#[test]
fn settings_profiles_project_dark() {
    goldens().assert(
        "settings-profiles-project-dark",
        &profiles_page(
            profiles_backend(profiles_with_project()).with_models(profile_models()),
            ThemeMode::Dark,
            1300.,
        ),
    );
}

/// The app's icon, the image the transcript's tool call shows.
const PNG: &[u8] = include_bytes!(
    "../packaging/linux/icons/hicolor/256x256/apps/com.mesahillsresearch.ArkoStudio.png"
);

/// The transcript's feed, fed as a session feeds it.
mod feed {
    use std::collections::HashMap;

    use arko::images::ImageRef;
    use arko::remote::{CallStatus, Delivery, Feed, Kind, Level, Source, WorkerChange};
    use arko_agents::AgentEvent;
    use arko_workflows::WorkflowEvent;
    use serde_json::Value;
    use uuid::Uuid;

    fn event(event: AgentEvent) -> WorkflowEvent {
        WorkflowEvent {
            timestamp: chrono::Utc::now(),
            workflow_run_id: Uuid::nil(),
            agent_name: "main".into(),
            agent_run_id: Uuid::nil(),
            event,
        }
    }

    fn say(feed: &Feed, text: &str) {
        feed.event(
            &event(AgentEvent::AssistantResponseChunk(text.into())),
            None,
        );
    }

    fn call(feed: &Feed, tool: &str, parameters: &[(&str, Value)]) -> Uuid {
        let id = Uuid::new_v4();
        let parameters: HashMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| (key.to_string(), value.clone()))
            .collect();
        feed.event(
            &event(AgentEvent::ToolCallFormatted {
                tool_call_id: id,
                tool_name: tool.into(),
                parameters,
            }),
            None,
        );
        id
    }

    fn result(feed: &Feed, id: Uuid, tool: &str, output: &str) {
        feed.event(
            &event(AgentEvent::ToolResult {
                tool_call_id: id,
                tool_name: tool.into(),
                output: output.into(),
            }),
            None,
        );
    }

    const ANSWER: &str = "The parser now reads its input as it arrives. Each chunk goes \
through `Parser::feed`, and a finished block is never parsed again.

## What changed

- `Parser::feed` takes a chunk and returns the blocks it completed.
- The open block keeps its state between chunks.

```rust
pub fn feed(&mut self, chunk: &str) -> Vec<Block> {
    self.pending.push_str(chunk);
    let mut done = Vec::new();
    while let Some(end) = self.block_end() {
        let text: String = self.pending.drain(..end).collect();
        done.push(Block::parse(&text));
    }
    done
}
```

The new test streams a document one byte at a time and compares the blocks with a \
parse of the whole text.";

    /// A reply still streaming, with a note the user typed meanwhile that
    /// the agent hasn't read yet.
    pub fn note_mid_reply() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Make the parser streaming, and add a test that feeds it a byte at a time.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        say(
            &feed,
            "The parser now reads its input as it arrives. Each chunk go",
        );
        feed.note(
            "Keep `parse` as a wrapper for now.",
            Source::App,
            1,
            Vec::new(),
        );
        say(
            &feed,
            "es through `Parser::feed`, and a finished block is never parsed again.",
        );
        feed
    }

    /// A worker that started another, and the failure of that one and of
    /// another worker of the main agent.
    pub fn nested_failures() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Review the parser and count the tokens it makes.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        feed.worker("r1", "Review the parser", WorkerChange::Started);
        feed.worker_under("r2", "r1");
        feed.worker("r2", "Count the tokens", WorkerChange::Started);
        let mut info = arko::remote::WorkerInfo::new();
        info.depth = 2;
        feed.worker_info("r2", info);
        feed.worker("r3", "Profile the renderer", WorkerChange::Started);
        for id in ["r2", "r3"] {
            let error = "the model gave no reply in 60 s";
            feed.worker(id, "", WorkerChange::Failed { error });
        }
        feed
    }

    /// Calls that moved to the background: one whose job runs, three whose
    /// jobs ended (finished, stopped, failed), and a command that still
    /// runs, which can move. `now` is the time the rows are read at. Gives
    /// the running command's item too.
    pub fn background_jobs(now: i64) -> (Feed, u64) {
        use arko::remote::{JobKind, JobStart, JobState, MAIN_AGENT};

        let feed = Feed::new("main");
        feed.user(
            "Index the workspace, then build it.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        let moved = |tool: &str, parameters: &[(&str, Value)], started: i64| {
            let job = Uuid::new_v4();
            let id = call(&feed, tool, parameters);
            result(
                &feed,
                id,
                tool,
                &format!(
                    "in the background as job {job}, 95.3 s\nThe user moved this call to the background."
                ),
            );
            feed.job_started(
                JobStart::new(job, MAIN_AGENT, JobKind::Call, tool)
                    .made_by(id)
                    .started(started),
            );
            job
        };
        let minutes = |n: i64| now - n * 60_000 - 5_000;
        let finished = moved("index_create", &[("path", "docs".into())], minutes(4));
        feed.job_ended(finished, JobState::Finished, None);
        let stopped = moved(
            "mcp__github__search_code",
            &[("q", "parser".into())],
            minutes(2),
        );
        feed.job_ended(stopped, JobState::Stopped, None);
        let failed = moved("mcp__slow__lookup", &[("word", "mesa".into())], minutes(1));
        feed.job_ended(
            failed,
            JobState::Failed,
            Some("the server went away".into()),
        );
        moved("index_create", &[("path", ".".into())], minutes(3));
        call(
            &feed,
            "shell",
            &[("command", "cargo build --release".into())],
        );
        let running = feed.subscribe().0.items.last().unwrap().id;
        (feed, running)
    }

    /// Calls that started monitors: one that runs and alerted three times,
    /// one that ended after its time, one stopped, and one whose command
    /// failed. `now` is the time the rows are read at, 8 minutes after the
    /// running one started.
    pub fn monitors(now: i64) -> Feed {
        use arko::remote::{JobKind, JobStart, JobState, MAIN_AGENT};

        let feed = Feed::new("main");
        feed.user(
            "Run the dev server and the tests, and tell me when something breaks.",
            Source::App,
            Delivery::Turn,
        );
        // The feed times their ends by its own clock, so the ended ones
        // started 30 minutes before it.
        let ago = chrono::Utc::now().timestamp_millis() - 30 * 60_000 - 5_000;
        let monitor = |command: &str, pattern: &str, started: i64| {
            let job = Uuid::new_v4();
            let id = call(
                &feed,
                "monitor_start",
                &[("command", command.into()), ("pattern", pattern.into())],
            );
            feed.job_started(
                JobStart::new(job, MAIN_AGENT, JobKind::Monitor, command)
                    .made_by(id)
                    .started(started)
                    .monitoring(format!("lines that match {pattern}"), started + 30 * 60_000),
            );
            result(
                &feed,
                id,
                "monitor_start",
                &format!("Monitor {job} started. It watches `{command}`."),
            );
            job
        };
        let expired = monitor("cargo watch -x test", "FAILED", ago);
        feed.job_changed(expired, 4, ago + 30 * 60_000);
        feed.job_ended(expired, JobState::Expired, None);
        let stopped = monitor("tail -F logs/app.log", "panic", ago);
        feed.job_ended(stopped, JobState::Stopped, None);
        let failed = monitor("npm run e2e -- --watch", "fail", ago);
        feed.job_changed(failed, 1, ago + 30 * 60_000);
        feed.job_ended(failed, JobState::Failed, Some("exit 1".into()));
        let started = now - 8 * 60_000 - 10_000;
        let running = monitor("npm run dev", "error|warn", started);
        feed.job_changed(running, 3, started + 30 * 60_000);
        feed
    }

    /// Six calls in a row: four finished reads, a monitor that runs and a
    /// command that runs, which can move. `now` is the time the rows are
    /// read at. Gives the running command's item too.
    pub fn many_calls(now: i64) -> (Feed, u64) {
        use arko::remote::{JobKind, JobStart, MAIN_AGENT};

        let feed = Feed::new("main");
        feed.user(
            "Find where the parser reads its input, watch the dev server and build it.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        for path in ["src/parse.rs", "src/block.rs", "src/input.rs", "src/lib.rs"] {
            let read = call(&feed, "read_file", &[("path", path.into())]);
            result(&feed, read, "read_file", "1 | pub struct Parser {\n2 | }\n");
        }
        let (started, job) = (now - 8 * 60_000 - 10_000, Uuid::new_v4());
        let id = call(
            &feed,
            "monitor_start",
            &[
                ("command", "npm run dev".into()),
                ("pattern", "error".into()),
            ],
        );
        feed.job_started(
            JobStart::new(job, MAIN_AGENT, JobKind::Monitor, "npm run dev")
                .made_by(id)
                .started(started)
                .monitoring("lines that match error", started + 30 * 60_000),
        );
        result(
            &feed,
            id,
            "monitor_start",
            &format!("Monitor {job} started. It watches `npm run dev`."),
        );
        call(
            &feed,
            "shell",
            &[("command", "cargo build --release".into())],
        );
        let running = feed.subscribe().0.items.last().unwrap().id;
        (feed, running)
    }

    /// A turn whose agent waits on the model after a call's result.
    pub fn working_after_a_call() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Make the parser streaming, and add a test that feeds it a byte at a time.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        let read = call(
            &feed,
            "read_file",
            &[
                ("path", "src/parse.rs".into()),
                ("start_line", 1.into()),
                ("end_line", 120.into()),
            ],
        );
        result(&feed, read, "read_file", "1 | pub struct Parser {\n2 | }\n");
        feed
    }

    /// A finished turn with each kind of row, and a second one running with
    /// an image a tool returned.
    pub fn representative() -> Feed {
        let feed = Feed::new("main");
        feed.user(
            "Make the parser streaming, and add a test that feeds it a byte at a time.",
            Source::App,
            Delivery::Turn,
        );
        feed.turn_started();
        feed.event(
            &event(AgentEvent::ReasoningChunk(
                "The parser takes a whole string today.".into(),
            )),
            None,
        );
        let read = call(
            &feed,
            "read_file",
            &[
                ("path", "src/parse.rs".into()),
                ("start_line", 1.into()),
                ("end_line", 120.into()),
            ],
        );
        result(&feed, read, "read_file", "1 | pub struct Parser {\n2 | }\n");
        let edit = call(
            &feed,
            "edit_file",
            &[
                ("path", "src/parse.rs".into()),
                (
                    "old_string",
                    "pub fn parse(text: &str) -> Vec<Block> {\n".into(),
                ),
                (
                    "new_string",
                    "pub fn feed(&mut self, chunk: &str) -> Vec<Block> {\n".into(),
                ),
            ],
        );
        result(
            &feed,
            edit,
            "edit_file",
            "Edited src/parse.rs at lines 40-40",
        );
        let test = call(&feed, "shell", &[("command", "cargo test -p parse".into())]);
        result(
            &feed,
            test,
            "shell",
            "exit code 101, 3.1 s\nrunning 12 tests\ntest streams_a_byte_at_a_time ... FAILED\n\
             test result: FAILED. 11 passed; 1 failed",
        );
        let again = call(&feed, "shell", &[("command", "cargo test -p parse".into())]);
        result(
            &feed,
            again,
            "shell",
            "exit code 0, 2.9 s\nrunning 12 tests\ntest result: ok. 12 passed; 0 failed",
        );
        say(&feed, ANSWER);
        feed.reply(ANSWER);
        feed.turn_ended("48 s · $0.212 (session $1.04) · context 61k", None);

        feed.user("Now fuzz it.", Source::App, Delivery::Turn);
        feed.turn_started();
        feed.worker(
            "r1",
            "Write a fuzz target for the parser",
            WorkerChange::Started,
        );
        feed.notice(Level::Warn, "rate limited, retrying in 4 s (attempt 1)");
        // A file read that showed an image (as a resumed session has it).
        let mut read = arko::remote::Item::new(Kind::Call, "read assets/icon.png");
        read.id = 1;
        read.tool = Some("read_file".into());
        read.status = Some(CallStatus::Ok);
        read.images = vec![ImageRef {
            id: 1,
            mime: "image/png".into(),
            label: "assets/icon.png".into(),
            bytes: super::PNG.len(),
        }];
        feed.restore(vec![read]);
        call(
            &feed,
            "shell",
            &[(
                "command",
                "cargo fuzz run parser -- -max_total_time=60".into(),
            )],
        );
        feed
    }
}

/// A note the agent hasn't read, outlined and muted below the reply that
/// goes on streaming above it.
fn pending_note_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::note_mid_reply();
    let window = app
        .open_window_with(size(px(900.), px(300.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_pending_note_light() {
    goldens().assert(
        "transcript-pending-note-light",
        &pending_note_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_pending_note_dark() {
    goldens().assert(
        "transcript-pending-note-dark",
        &pending_note_window(ThemeMode::Dark),
    );
}

/// The failures of a worker's worker and of a worker of the main agent in the
/// main transcript, the first naming the worker that started it.
fn nested_failures_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::nested_failures();
    let window = app
        .open_window_with(size(px(640.), px(240.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    // The cards show what the workers did, as they were left.
                    view.set_clock(|| WORKERS_NOW, cx);
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_nested_failures_light() {
    goldens().assert(
        "transcript-nested-failures-light",
        &nested_failures_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_nested_failures_dark() {
    goldens().assert(
        "transcript-nested-failures-dark",
        &nested_failures_window(ThemeMode::Dark),
    );
}

/// Calls in the background: how their jobs ended, one that runs with Stop,
/// and a command that runs with Move to background.
fn background_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let now = chrono::Utc::now().timestamp_millis();
    let (feed, running) = feed::background_jobs(now);
    // The command started 12 s ago.
    let shown_at = now + 12_000;
    let window = app
        .open_window_with(size(px(900.), px(260.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || shown_at, cx);
                    view.set_movable(&[running], cx);
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_background_light() {
    goldens().assert(
        "transcript-background-light",
        &background_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_background_dark() {
    goldens().assert(
        "transcript-background-dark",
        &background_window(ThemeMode::Dark),
    );
}

/// Calls that started monitors: how three ended, and one that runs with its
/// alerts, when it ends and Stop.
fn monitors_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let now = chrono::Utc::now().timestamp_millis();
    let feed = feed::monitors(now);
    let window = app
        .open_window_with(size(px(900.), px(260.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || now, cx);
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_monitors_light() {
    goldens().assert(
        "transcript-monitors-light",
        &monitors_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_monitors_dark() {
    goldens().assert(
        "transcript-monitors-dark",
        &monitors_window(ThemeMode::Dark),
    );
}

/// The Quiet view's group of calls, folded, while a monitor and a command
/// that can move run. Its header names the command and offers Move to
/// background.
fn group_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    settings.view = prefs::TranscriptView::Quiet;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let now = chrono::Utc::now().timestamp_millis();
    let (feed, running) = feed::many_calls(now);
    // The command started 12 s ago.
    let shown_at = now + 12_000;
    let window = app
        .open_window_with(size(px(520.), px(180.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || shown_at, cx);
                    view.set_movable(&[running], cx);
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_group_light() {
    goldens().assert("transcript-group-light", &group_window(ThemeMode::Light));
}

#[test]
fn transcript_group_dark() {
    goldens().assert("transcript-group-dark", &group_window(ThemeMode::Dark));
}

/// "Working…" below a call's result, while the agent waits on the model.
fn working_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::working_after_a_call();
    let window = app
        .open_window_with(size(px(900.), px(220.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_working_light() {
    goldens().assert(
        "transcript-working-light",
        &working_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_working_dark() {
    goldens().assert("transcript-working-dark", &working_window(ThemeMode::Dark));
}

/// The representative transcript alone in a window `width` wide, its
/// running times read at a fixed moment.
fn transcript_window(mode: ThemeMode, width: f32) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::representative();
    // The edit and the failed test show their details.
    let items = feed.subscribe().0.items;
    let open: Vec<u64> = items
        .iter()
        .filter(|item| {
            item.tool.as_deref() == Some("edit_file")
                || item
                    .outcome
                    .as_deref()
                    .is_some_and(|o| o.starts_with("exit"))
        })
        .map(|item| item.id)
        .collect();
    // The call and the worker started a moment ago: they show 12 s.
    let now = chrono::Utc::now().timestamp_millis() + 12_000;
    let window = app
        .open_window_with(size(px(width), px(1500.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || now, cx);
                    view.set_image_lookup(|_| Some((PNG.into(), "image/png".into())), cx);
                    for item in open {
                        view.set_expanded(item, true, cx);
                    }
                    view
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_wide_light() {
    goldens().assert(
        "transcript-wide-light",
        &transcript_window(ThemeMode::Light, 1300.),
    );
}

#[test]
fn transcript_wide_dark() {
    goldens().assert(
        "transcript-wide-dark",
        &transcript_window(ThemeMode::Dark, 1300.),
    );
}

#[test]
fn transcript_narrow_light() {
    goldens().assert(
        "transcript-narrow-light",
        &transcript_window(ThemeMode::Light, 600.),
    );
}

#[test]
fn transcript_narrow_dark() {
    goldens().assert(
        "transcript-narrow-dark",
        &transcript_window(ThemeMode::Dark, 600.),
    );
}

/// A session opened again on a turn that ended, in a window `width` wide,
/// drawn once. The turn ended with a reply, or without one, stopped.
fn ended_turn_app(
    mode: ThemeMode,
    width: f32,
    replied: bool,
) -> (ScreenshotApp, gpui_kit::AnyWindowHandle) {
    use arko::remote::{Delivery, Feed, Item, Kind, Level, Source};
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut message = Item::new(Kind::User, "Name a fruit that keeps well.");
    message.message_index = Some(0);
    message.source = Some(Source::App);
    message.delivery = Some(Delivery::Turn);
    let ending = if replied {
        Item::new(
            Kind::Reply,
            "An apple. Kept cool and dry, it lasts for months.",
        )
    } else {
        Item::new(Kind::Notice, "Stopped.").level(Level::Warn)
    };
    let footer = Item::new(Kind::Turn, "6 s · $0.004 (session $0.01) · context 3k");
    let items = [message, ending, footer]
        .into_iter()
        .zip(1..)
        .map(|(mut item, id)| {
            item.id = id;
            item.turn = Some(1);
            item
        })
        .collect();
    let feed = Feed::new("main");
    feed.restore(items);
    let window = app
        .open_window_with(size(px(width), px(300.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| TranscriptView::new(&feed, AgentFilter::Main, window, cx))
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.run_until_parked();
    let _ = app.capture(window).unwrap();
    (app, window)
}

/// A session opened again on a turn that ended, in a window `width` wide,
/// with the pointer on the row `hovered` (a row's name and its entry). The
/// turn ended with a reply, or without one, stopped.
fn ended_turn_window(
    mode: ThemeMode,
    width: f32,
    replied: bool,
    hovered: (&'static str, u64),
) -> Screenshot {
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{Modifiers, MouseMoveEvent, PlatformInput, point};

    let (mut app, window) = ended_turn_app(mode, width, replied);
    // The pointer at the row's left, in the window's margin, away from its
    // buttons.
    app.update_window(window, |_, window, cx| {
        let row = window.find(hovered).bounds();
        let position = point(row.left() + px(4.), row.bottom() - px(4.));
        window.dispatch_event(
            PlatformInput::MouseMove(MouseMoveEvent {
                position,
                pressed_button: None,
                modifiers: Modifiers::default(),
            }),
            cx,
        );
    })
    .unwrap();
    app.capture(window).unwrap()
}

/// The menu of the latest reply's "more" button, open.
fn reply_menu_window(mode: ThemeMode, width: f32) -> Screenshot {
    use gpui_kit::test::TestWindowExt as _;

    let (mut app, window) = ended_turn_app(mode, width, true);
    app.update_window(window, |_, window, cx| {
        window.click(("reply-menu", 2u64), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.capture(window).unwrap()
}

/// The latest reply, under the pointer: Regenerate is alone at the right
/// end of its actions.
#[test]
fn transcript_regenerate_light() {
    goldens().assert(
        "transcript-regenerate-light",
        &ended_turn_window(ThemeMode::Light, 1300., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_dark() {
    goldens().assert(
        "transcript-regenerate-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_narrow_light() {
    goldens().assert(
        "transcript-regenerate-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., true, ("actions", 2)),
    );
}

#[test]
fn transcript_regenerate_narrow_dark() {
    goldens().assert(
        "transcript-regenerate-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., true, ("actions", 2)),
    );
}

/// The latest reply's "more" button pressed: its menu offers View source.
#[test]
fn transcript_reply_menu_light() {
    goldens().assert(
        "transcript-reply-menu-light",
        &reply_menu_window(ThemeMode::Light, 1300.),
    );
}

#[test]
fn transcript_reply_menu_dark() {
    goldens().assert(
        "transcript-reply-menu-dark",
        &reply_menu_window(ThemeMode::Dark, 1300.),
    );
}

/// A turn stopped before it replied, its footer under the pointer:
/// Regenerate is at the line's right end, where the reply would be.
#[test]
fn transcript_stopped_light() {
    goldens().assert(
        "transcript-stopped-light",
        &ended_turn_window(ThemeMode::Light, 1300., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_dark() {
    goldens().assert(
        "transcript-stopped-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_narrow_light() {
    goldens().assert(
        "transcript-stopped-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., false, ("turn", 3)),
    );
}

#[test]
fn transcript_stopped_narrow_dark() {
    goldens().assert(
        "transcript-stopped-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., false, ("turn", 3)),
    );
}

/// A user's message under the pointer: Edit, Branch and Copy in a row under
/// the bubble, at its right edge.
#[test]
fn transcript_message_actions_light() {
    goldens().assert(
        "transcript-message-actions-light",
        &ended_turn_window(ThemeMode::Light, 1300., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_dark() {
    goldens().assert(
        "transcript-message-actions-dark",
        &ended_turn_window(ThemeMode::Dark, 1300., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_narrow_light() {
    goldens().assert(
        "transcript-message-actions-narrow-light",
        &ended_turn_window(ThemeMode::Light, 460., true, ("user", 1)),
    );
}

#[test]
fn transcript_message_actions_narrow_dark() {
    goldens().assert(
        "transcript-message-actions-narrow-dark",
        &ended_turn_window(ThemeMode::Dark, 460., true, ("user", 1)),
    );
}

/// The representative transcript with a find going on: its matches marked,
/// the current one stronger, and the keyboard's ring on a row.
fn transcript_find_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::transcript::{AgentFilter, TranscriptView};
    use gpui_kit::AppContext as _;
    use gpui_kit::test::TestWindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let feed = feed::representative();
    let now = chrono::Utc::now().timestamp_millis() + 12_000;
    let mut opened = None;
    let window = app
        .open_window_with(size(px(1300.), px(1500.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut view = TranscriptView::new(&feed, AgentFilter::Main, window, cx);
                    view.set_clock(move || now, cx);
                    view.set_image_lookup(|_| Some((PNG.into(), "image/png".into())), cx);
                    view
                })
            })
            .map(|(window, view)| {
                opened = Some(view);
                window
            })
        })
        .unwrap();
    let view = opened.unwrap();
    app.run_until_parked();
    app.update_window(window, |_, window, cx| {
        view.update(cx, |view, cx| {
            view.find("parse", cx);
            view.focus_rows(window, cx);
        });
        // From the keyboard: its ring shows.
        window.press("up", cx);
    })
    .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn transcript_find_light() {
    goldens().assert(
        "transcript-find-light",
        &transcript_find_window(ThemeMode::Light),
    );
}

#[test]
fn transcript_find_dark() {
    goldens().assert(
        "transcript-find-dark",
        &transcript_find_window(ThemeMode::Dark),
    );
}

/// The composer of an agent's new session, with the files an "@" mentions
/// listed above it.
fn mention_window(mode: ThemeMode) -> Screenshot {
    use gpui_kit::test::TestWindowExt as _;

    // A folder of a fixed name: the window shows it.
    let root = std::env::temp_dir().join("arko-mention-list");
    let _ = std::fs::remove_dir_all(&root);
    for dir in ["src/parser", "docs"] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    for file in [
        "README.md",
        "src/parse.rs",
        "src/parser/mod.rs",
        "src/main.rs",
        "docs/parsing.md",
    ] {
        std::fs::write(root.join(file), "").unwrap();
    }
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut opened = None;
    let window = app
        .open_window_with(size(px(900.), px(600.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened = Some(workspace);
                window
            })
        })
        .unwrap();
    let workspace = opened.unwrap();
    app.update_window(window, |_, window, cx| {
        let view = workspace.read(cx).session_view().clone();
        view.update(cx, |view, cx| {
            view.set_draft_folder(root.clone(), cx);
            view.focus_composer(window, cx);
        });
        window.input("Look at the @pars", cx);
    })
    .unwrap();
    // The folder is listed and ranked off the UI thread.
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn mention_list_light() {
    goldens().assert("mention-list-light", &mention_window(ThemeMode::Light));
}

#[test]
fn mention_list_dark() {
    goldens().assert("mention-list-dark", &mention_window(ThemeMode::Dark));
}

/// The workers panel with workers in each state: two running (one past an
/// hour), one waiting, one that finished and one that failed a moment ago,
/// and one finished long ago in the collapsed "Finished" group.
fn workers_window(mode: ThemeMode) -> Screenshot {
    use arko::remote::{AgentStatus, WorkerState, WorkerSummary};
    use arko_studio::workers_panel::{PanelSource, WorkersPanel};
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    const NOW: i64 = 1_760_000_000_000;
    const MINUTE: i64 = 60_000;
    let worker = |id: &str, title: &str, state, calls, started, ended, last: Option<&str>| {
        let mut summary = WorkerSummary::new(id);
        summary.title = title.into();
        summary.state = state;
        summary.tool_calls = calls;
        summary.started_at = started;
        summary.ended_at = ended;
        summary.ran_ms = started.zip(ended).map_or(0, |(s, e)| (e - s).max(0) as u64);
        summary.running_since = started.filter(|_| state == WorkerState::Running);
        summary.last = last.map(Into::into);
        summary
    };
    let mut finished = worker(
        "r4",
        "Update the changelog",
        WorkerState::Finished,
        7,
        Some(NOW - 6 * MINUTE),
        Some(NOW - 2 * MINUTE),
        None,
    );
    finished.report = Some("Wrote the changelog for 0.4.".into());
    let workers = vec![
        worker(
            "r1",
            "Fix the flaky parser test",
            WorkerState::Running,
            18,
            Some(NOW - 4 * MINUTE - 12_000),
            None,
            Some("$ cargo test -p parse"),
        ),
        worker(
            "r2",
            "Profile the renderer under load",
            WorkerState::Running,
            142,
            Some(NOW - 65 * MINUTE),
            None,
            Some("read src/render/scene.rs:120-180"),
        ),
        worker(
            "r3",
            "Write the migration guide",
            WorkerState::Queued,
            0,
            None,
            None,
            None,
        ),
        finished,
        worker(
            "r5",
            "Bisect the slowdown",
            WorkerState::Failed,
            3,
            Some(NOW - 3 * MINUTE),
            Some(NOW - MINUTE),
            Some("the model failed: overloaded"),
        ),
        worker(
            "r6",
            "Rename the config keys",
            WorkerState::Finished,
            11,
            Some(NOW - 40 * MINUTE),
            Some(NOW - 20 * MINUTE),
            None,
        ),
    ];
    let agent = |id: &str, tokens, window| AgentStatus {
        id: id.into(),
        context_tokens: Some(tokens),
        context_window: Some(window),
        handover_at: None,
        compacts: false,
    };
    let source = PanelSource {
        workers,
        agents: vec![
            agent("r1", 88_000, 200_000),
            agent("r2", 152_000, 1_000_000),
        ],
        feed: None,
        images: None,
        movable: Vec::new(),
        key: None,
    };
    let window = app
        .open_window_with(size(px(352.), px(420.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut panel = WorkersPanel::new(window, cx);
                    panel.set_source(source, cx);
                    panel.set_clock(|| NOW, cx);
                    panel
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn workers_light() {
    goldens().assert("workers-light", &workers_window(ThemeMode::Light));
}

#[test]
fn workers_dark() {
    goldens().assert("workers-dark", &workers_window(ThemeMode::Dark));
}

/// The clock of the workers' screenshots, in Unix milliseconds.
const WORKERS_NOW: i64 = 1_760_000_000_000;

/// A running worker with workers under it, three levels deep, one of them
/// finished a moment ago, each row with its model, profile and cost, the
/// workers that started others with what their family cost, and a finished
/// family in the collapsed group. The session lists them as they started, and
/// the panel puts a worker's workers under it.
fn workers_family_source() -> arko_studio::workers_panel::PanelSource {
    use arko::remote::{AgentStatus, WorkerState, WorkerSummary};
    use arko_studio::workers_panel::PanelSource;

    const MINUTE: i64 = 60_000;
    let worker = |id: &str, parent: Option<&str>, title: &str, state, started: i64, ended| {
        let mut summary = WorkerSummary::new(id);
        summary.title = title.into();
        summary.state = state;
        summary.parent = parent.map(str::to_string);
        // r7 is the one at the third level.
        summary.depth = match (parent, id) {
            (None, _) => 1,
            (Some(_), "r7") => 3,
            (Some(_), _) => 2,
        };
        summary.started_at = Some(WORKERS_NOW - started * MINUTE);
        summary.ended_at = ended;
        summary.ran_ms = ended.map_or(0, |ended| (ended - (WORKERS_NOW - started * MINUTE)) as u64);
        summary.running_since =
            (state == WorkerState::Running).then(|| WORKERS_NOW - started * MINUTE);
        summary
    };
    let mut parser = worker(
        "r1",
        None,
        "Split the parser into modules",
        WorkerState::Running,
        12,
        None,
    );
    parser.model = Some("claude-opus-5-5".into());
    parser.effort = Some("high".into());
    parser.cost_billed = 0.84;
    parser.cost_list = 0.84;
    parser.tool_calls = 31;
    parser.last = Some("edit src/parser/mod.rs".into());
    let mut renderer = worker(
        "r2",
        None,
        "Profile the renderer under load",
        WorkerState::Running,
        65,
        None,
    );
    renderer.model = Some("cc:claude-opus-5-5".into());
    renderer.effort = Some("xhigh".into());
    renderer.cost_list = 3.4;
    renderer.tool_calls = 142;
    renderer.last = Some("read src/render/scene.rs:120-180".into());
    let mut tests = worker(
        "r3",
        Some("r1"),
        "Fix the tokenizer tests",
        WorkerState::Running,
        4,
        None,
    );
    tests.model = Some("claude-haiku-5-5".into());
    tests.profile = Some("quick-reader".into());
    tests.cost_billed = 0.02;
    tests.cost_list = 0.02;
    tests.tool_calls = 9;
    tests.last = Some("$ cargo test -p parse".into());
    let mut keys = worker(
        "r4",
        Some("r1"),
        "Rename the config keys",
        WorkerState::Finished,
        6,
        Some(WORKERS_NOW - MINUTE),
    );
    keys.model = Some("claude-haiku-5-5".into());
    keys.profile = Some("quick-reader".into());
    keys.cost_billed = 0.01;
    keys.cost_list = 0.01;
    keys.tool_calls = 11;
    keys.report = Some("Renamed 14 keys and updated the docs.".into());
    let mut changelog = worker(
        "r5",
        None,
        "Update the changelog",
        WorkerState::Finished,
        50,
        Some(WORKERS_NOW - 30 * MINUTE),
    );
    changelog.model = Some("gpt-6.1-sol".into());
    changelog.profile = Some("3d-judge".into());
    changelog.cost_billed = 0.4;
    changelog.cost_list = 0.4;
    let mut notes = worker(
        "r6",
        Some("r5"),
        "Check the links",
        WorkerState::Finished,
        45,
        Some(WORKERS_NOW - 35 * MINUTE),
    );
    notes.model = Some("claude-haiku-5-5".into());
    let mut lexer = worker(
        "r7",
        Some("r3"),
        "Check the lexer edge cases",
        WorkerState::Running,
        2,
        None,
    );
    lexer.model = Some("claude-haiku-5-5".into());
    lexer.cost_billed = 0.03;
    lexer.cost_list = 0.03;
    lexer.tool_calls = 5;
    lexer.last = Some("read src/parser/lexer.rs:40-90".into());
    // What each worker that started others cost with them, as the session
    // adds it up. The others have their own.
    let families = [("r1", 0.9), ("r3", 0.05), ("r5", 0.4)];
    let mut all = vec![parser, renderer, tests, keys, changelog, notes, lexer];
    for worker in &mut all {
        let family = families.iter().find(|(id, _)| *id == worker.id);
        worker.family_cost_billed = family.map_or(worker.cost_billed, |(_, cost)| *cost);
        worker.family_cost_list = family.map_or(worker.cost_list, |(_, cost)| *cost);
    }
    let agent = |id: &str, tokens, window| AgentStatus {
        id: id.into(),
        context_tokens: Some(tokens),
        context_window: Some(window),
        handover_at: None,
        compacts: false,
    };
    PanelSource {
        workers: all,
        agents: vec![
            agent("r1", 88_000, 200_000),
            agent("r2", 152_000, 1_000_000),
            agent("r3", 31_000, 200_000),
        ],
        feed: None,
        images: None,
        movable: Vec::new(),
        key: None,
    }
}

/// The workers panel docked, over the family of workers.
fn workers_family_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::workers_panel::WorkersPanel;
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let source = workers_family_source();
    let window = app
        .open_window_with(size(px(352.), px(480.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| {
                    let mut panel = WorkersPanel::new(window, cx);
                    panel.set_source(source, cx);
                    panel.set_clock(|| WORKERS_NOW, cx);
                    panel
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn workers_family_light() {
    goldens().assert(
        "workers-family-light",
        &workers_family_window(ThemeMode::Light),
    );
}

#[test]
fn workers_family_dark() {
    goldens().assert(
        "workers-family-dark",
        &workers_family_window(ThemeMode::Dark),
    );
}

/// The same workers in the panel as a sheet from the right, in a window
/// too narrow to dock it.
fn workers_sheet_window(mode: ThemeMode) -> Screenshot {
    workers_in_window(mode, size(px(640.), px(640.)))
}

/// The same workers in the panel docked beside the conversation, in a wide
/// window.
fn workers_docked_window(mode: ThemeMode) -> Screenshot {
    workers_in_window(mode, size(px(1300.), px(700.)))
}

/// The workers of the family in the panel of a window of `window_size`.
fn workers_in_window(mode: ThemeMode, window_size: gpui_kit::Size<gpui_kit::Pixels>) -> Screenshot {
    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let opened = std::cell::RefCell::new(None);
    let window = app
        .open_window_with(window_size, 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, workspace)| {
                opened.replace(Some(workspace));
                window
            })
        })
        .unwrap();
    let workspace = opened.take().unwrap();
    app.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(arko_studio::menus::ToggleWorkersPanel), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.update(|cx| {
        let panel = workspace.read(cx).workers_panel().clone();
        panel.update(cx, |panel, cx| {
            panel.set_source(workers_family_source(), cx);
            panel.set_clock(|| WORKERS_NOW, cx);
        });
    });
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn workers_sheet_light() {
    goldens().assert(
        "workers-sheet-light",
        &workers_sheet_window(ThemeMode::Light),
    );
}

#[test]
fn workers_sheet_dark() {
    goldens().assert("workers-sheet-dark", &workers_sheet_window(ThemeMode::Dark));
}

#[test]
fn workers_docked_light() {
    goldens().assert(
        "workers-docked-light",
        &workers_docked_window(ThemeMode::Light),
    );
}

#[test]
fn workers_docked_dark() {
    goldens().assert(
        "workers-docked-dark",
        &workers_docked_window(ThemeMode::Dark),
    );
}

/// The dialog that asks before an edit in an agent session removes the
/// messages after the message, over a new window.
fn edit_dialog_window(mode: ThemeMode) -> Screenshot {
    use arko_studio::editing::{self, Change};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(900.), px(560.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.update_window(window, |_, window, cx| {
        editing::confirm(Change::Edit, 3, true, window, cx, |_, _, _| {})
    })
    .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn edit_dialog_light() {
    goldens().assert("edit-dialog-light", &edit_dialog_window(ThemeMode::Light));
}

#[test]
fn edit_dialog_dark() {
    goldens().assert("edit-dialog-dark", &edit_dialog_window(ThemeMode::Dark));
}

/// About Arko Studio over a window `width` × `height` logical pixels, with
/// the built-in fonts, so all three credits show. `scroll` turns the mouse
/// wheel that many pixels down over the dialog.
fn about_window(mode: ThemeMode, width: f32, height: f32, scroll: f32) -> Screenshot {
    use gpui_kit::{InputEvent as _, ScrollDelta, ScrollWheelEvent, point};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(width), px(height)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.update_window(window, |_, window, cx| arko_studio::about::open(window, cx))
        .unwrap();
    if scroll > 0. {
        // A frame first, so the wheel finds the dialog's body.
        app.capture(window).unwrap();
        app.update_window(window, |_, window, cx| {
            window.dispatch_event(
                ScrollWheelEvent {
                    position: point(px(width / 2.), px(height / 2.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-scroll))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        app.run_until_parked();
    }
    app.capture(window).unwrap()
}

#[test]
fn about_light() {
    goldens().assert(
        "about-light",
        &about_window(ThemeMode::Light, 900., 560., 0.),
    );
}

#[test]
fn about_dark() {
    goldens().assert("about-dark", &about_window(ThemeMode::Dark, 900., 560., 0.));
}

/// At the window's smallest size the dialog narrows with it, and the
/// credits wrap inside its padding.
#[test]
fn about_narrow_light() {
    goldens().assert(
        "about-narrow-light",
        &about_window(ThemeMode::Light, 400., 300., 0.),
    );
}

#[test]
fn about_narrow_dark() {
    goldens().assert(
        "about-narrow-dark",
        &about_window(ThemeMode::Dark, 400., 300., 0.),
    );
}

/// The dialog's body scrolls to the rest of the credits and the links when
/// the window is too short for it.
#[test]
fn about_narrow_scrolled_light() {
    goldens().assert(
        "about-narrow-scrolled-light",
        &about_window(ThemeMode::Light, 400., 300., 400.),
    );
}

#[test]
fn about_narrow_scrolled_dark() {
    goldens().assert(
        "about-narrow-scrolled-dark",
        &about_window(ThemeMode::Dark, 400., 300., 400.),
    );
}

/// The status bar's figures, at a fixed moment.
mod status {
    use std::time::Duration;

    use arko::remote::{
        Activity, CostPart, LastRequest, Link, LinkKind, Page, PlanStatus, PlanWindow, Reading,
    };

    /// The moment the bars are drawn at, in Unix milliseconds.
    pub const NOW: i64 = 1_800_000_000_000;

    fn lan(url: &str, interface: &str) -> Link {
        Link {
            url: url.into(),
            interface: Some(interface.into()),
            kind: LinkKind::Lan,
            loopback: false,
        }
    }

    /// A page shared on every interface.
    pub fn page() -> Page {
        Page {
            addr: "0.0.0.0:8765".parse().unwrap(),
            links: vec![
                lan(
                    "http://192.168.1.20:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf",
                    "wlan0",
                ),
                Link {
                    url: "http://100.101.7.12:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf".into(),
                    interface: Some("tailscale0".into()),
                    kind: LinkKind::Tailscale,
                    loopback: false,
                },
                Link {
                    url: "http://127.0.0.1:8765/#key=Xq3v9TzK1bR7mWc2LpYd8sNf".into(),
                    interface: None,
                    kind: LinkKind::Loopback,
                    loopback: true,
                },
            ],
            note: None,
        }
    }

    /// A session at work: two workers, outside access, the web page, a plan
    /// near its 5-hour limit, a cost and a context of 142k.
    pub fn busy() -> Reading {
        let mut reading = Reading {
            cost: 1.2,
            context_tokens: Some(142_318),
            context_window: Some(1_000_000),
            workers_running: vec![("r1".into(), None), ("r2".into(), None)],
            page: Some(page()),
            plans: vec![PlanStatus {
                plan: "Claude plan".into(),
                windows: vec![
                    PlanWindow {
                        label: "5 h".into(),
                        used_percent: 84.,
                        resets_at: None,
                    },
                    PlanWindow {
                        label: "7 d".into(),
                        used_percent: 41.,
                        resets_at: None,
                    },
                ],
                credits: None,
                set_aside: 0,
                extra_usage: None,
                text: String::new(),
            }],
            last_request: Some(LastRequest {
                at: NOW,
                model: "claude-opus-5-5".into(),
                sizes: Some(Default::default()),
                bytes_per_token: 4.,
            }),
            ..Reading::default()
        };
        reading.settings.outside = true;
        reading.settings.handover_at = 950_000;
        reading.costs.main = CostPart {
            billed: 1.0,
            list_price: 1.0,
            input_tokens: 420_000,
            output_tokens: 18_000,
            calls: 14,
        };
        reading.costs.workers = CostPart {
            billed: 0.2,
            list_price: 0.2,
            input_tokens: 90_000,
            output_tokens: 4_000,
            calls: 6,
        };
        if let Some(sizes) = reading
            .last_request
            .as_mut()
            .and_then(|request| request.sizes.as_mut())
        {
            sizes.system_prompt = 30_000;
            sizes.tool_definitions = 42_000;
            sizes.user_text = 60_000;
            sizes.assistant_text = 120_000;
            sizes.tool_calls = 40_000;
            sizes.tool_results = 270_000;
            sizes.image_tokens = 3_000;
        }
        reading.cache.read = Some(140_100);
        reading.cache.written = Some(2_218);
        reading
    }

    /// A turn running for 1:23.
    pub fn working() -> Activity {
        Activity {
            working: true,
            since: Some(NOW - 83_000),
            ..Activity::default()
        }
    }

    /// An idle session on a plan near its handover size, its 5-minute cache
    /// in its last minute.
    pub fn idle() -> Reading {
        let mut reading = Reading {
            api_price: Some(3.4),
            context_tokens: Some(871_204),
            context_window: Some(1_000_000),
            ..Reading::default()
        };
        reading.settings.handover_at = 950_000;
        reading.costs.main = CostPart {
            billed: 0.,
            list_price: 3.4,
            input_tokens: 2_400_000,
            output_tokens: 31_000,
            calls: 22,
        };
        reading.cache.lifetime = Some(Duration::from_secs(300));
        reading.cache.refreshed_at = Some(NOW - 258_000);
        reading.cache.tokens = Some(871_204);
        reading.cache.on_plan = Some("your Claude plan");
        reading
    }
}

/// A status bar alone in a window `width` wide.
fn status_window(
    mode: ThemeMode,
    width: f32,
    reading: arko::remote::Reading,
    activity: arko::remote::Activity,
) -> Screenshot {
    use arko_studio::layout::{NARROW_BELOW, WidthClass};
    use arko_studio::status_bar::StatusBar;
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(width), px(28.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| {
                cx.new(|cx| {
                    let mut bar =
                        StatusBar::fixed(reading, activity, std::rc::Rc::new(|| status::NOW));
                    if width < NARROW_BELOW {
                        bar.set_width(WidthClass::Narrow, cx);
                    }
                    bar
                })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn status_busy_light() {
    goldens().assert(
        "status-busy-light",
        &status_window(ThemeMode::Light, 1300., status::busy(), status::working()),
    );
}

#[test]
fn status_busy_dark() {
    goldens().assert(
        "status-busy-dark",
        &status_window(ThemeMode::Dark, 1300., status::busy(), status::working()),
    );
}

#[test]
fn status_idle_light() {
    goldens().assert(
        "status-idle-light",
        &status_window(ThemeMode::Light, 1300., status::idle(), Default::default()),
    );
}

#[test]
fn status_idle_dark() {
    goldens().assert(
        "status-idle-dark",
        &status_window(ThemeMode::Dark, 1300., status::idle(), Default::default()),
    );
}

#[test]
fn status_narrow_light() {
    goldens().assert(
        "status-narrow-light",
        &status_window(ThemeMode::Light, 600., status::busy(), status::working()),
    );
}

#[test]
fn status_narrow_dark() {
    goldens().assert(
        "status-narrow-dark",
        &status_window(ThemeMode::Dark, 600., status::busy(), status::working()),
    );
}

/// The status bar's popovers side by side: the context's, the cost's and
/// the web page's.
struct StatusPopovers {
    reading: Arc<arko::remote::Reading>,
}

impl gpui_kit::Render for StatusPopovers {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use arko::details::{CostReport, CostShare};
        use arko_studio::context_ring::ContextDetails;
        use arko_studio::status_bar::CostDetails;
        use arko_studio::web_page::WebPagePanel;
        use gpui_kit::component::{ActiveTheme as _, h_flex};
        use gpui_kit::{IntoElement as _, ParentElement as _, Styled as _, div};

        let theme = cx.theme();
        let card = |content: gpui_kit::AnyElement| {
            div()
                .p_3()
                .bg(theme.popover)
                .text_color(theme.popover_foreground)
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius_lg)
                .child(content)
        };
        let share = |billed: f64, input: usize, output: usize| CostShare {
            billed,
            api: billed,
            input,
            output,
            plan: None,
        };
        let cost = CostReport {
            main: share(1.0, 420_000, 18_000),
            workers: share(0.2, 90_000, 4_000),
            plans: Vec::new(),
            embeddings: Some((120_000, 0.01)),
        };
        h_flex()
            .size_full()
            .items_start()
            .gap_4()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(card(
                ContextDetails::new(&self.reading, status::NOW).into_any_element(),
            ))
            .child(card(CostDetails::new(Some(cost)).into_any_element()))
            .child(card(
                WebPagePanel::new(&status::page())
                    .explained()
                    .into_any_element(),
            ))
    }
}

fn status_popovers(mode: ThemeMode) -> Screenshot {
    use gpui_kit::AppContext as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let reading = Arc::new(status::busy());
    let window = app
        .open_window_with(size(px(1100.), px(640.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| StatusPopovers { reading }))
                .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn status_popovers_light() {
    goldens().assert("status-popovers-light", &status_popovers(ThemeMode::Light));
}

#[test]
fn status_popovers_dark() {
    goldens().assert("status-popovers-dark", &status_popovers(ThemeMode::Dark));
}

/// The last cache reminder, at 0:42 before the cache lapses, over an empty
/// window.
fn cache_reminder(mode: ThemeMode) -> Screenshot {
    use arko_studio::cache_reminders::{Countdown, ReminderClock, last_reminder};
    use arko_studio::notify::SessionLink;
    use gpui_kit::AppContext as _;
    use gpui_kit::component::WindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        cx.set_global(ReminderClock(std::rc::Rc::new(|| status::NOW)));
    });
    let window = app
        .open_window_with(size(px(560.), px(240.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| gpui_kit::Empty))
                .map(|(window, _)| window)
        })
        .unwrap();
    app.update(|cx| {
        window
            .update(cx, |_, window, cx| {
                let countdown = cx.new(|cx| {
                    Countdown::new(
                        SessionLink::new(
                            1,
                            "Split the engine out of main.rs",
                            std::rc::Rc::new(|_, _| {}),
                        ),
                        "Refresh ≈ $0.04 · Letting it expire re-caches ≈ $0.90".into(),
                        status::NOW + 42_000,
                        cx,
                    )
                });
                window.push_notification(last_reminder(1, countdown, |_, _| {}), cx);
            })
            .unwrap();
    });
    app.capture(window).unwrap()
}

#[test]
fn cache_reminder_light() {
    goldens().assert("cache-reminder-light", &cache_reminder(ThemeMode::Light));
}

#[test]
fn cache_reminder_dark() {
    goldens().assert("cache-reminder-dark", &cache_reminder(ThemeMode::Dark));
}

/// The first cache reminder, or (`long`) a question's card whose session's
/// title is too long for it, each with the title as a link.
fn notification_card(mode: ThemeMode, long: bool) -> Screenshot {
    use arko_studio::cache_reminders::first_reminder;
    use arko_studio::notify::{SessionLink, card};
    use gpui_kit::AppContext as _;
    use gpui_kit::component::WindowExt as _;
    use gpui_kit::component::notification::Notification;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let window = app
        .open_window_with(size(px(560.), px(200.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| gpui_kit::Empty))
                .map(|(window, _)| window)
        })
        .unwrap();
    app.update(|cx| {
        window
            .update(cx, |_, window, cx| {
                let opens: arko_studio::notify::Opens = std::rc::Rc::new(|_, _| {});
                let note = if long {
                    let link = SessionLink::new(
                        2,
                        "A session whose title runs on and on, well past the width of a card",
                        opens,
                    );
                    Notification::new()
                        .id1::<u8>(gpui_kit::ElementId::Integer(2))
                        .content(move |_, _, cx| card(&link, "Arko asks “Tabs or spaces?”", cx))
                } else {
                    let link = SessionLink::new(1, "Split the engine out of main.rs", opens);
                    first_reminder(
                        1,
                        link,
                        "The prompt cache expires in 5 minutes".into(),
                        "Letting it expire re-caches ≈ $0.90".into(),
                    )
                };
                window.push_notification(note, cx);
            })
            .unwrap();
    });
    app.capture(window).unwrap()
}

#[test]
fn notification_card_light() {
    goldens().assert(
        "notification-card-light",
        &notification_card(ThemeMode::Light, false),
    );
}

#[test]
fn notification_card_dark() {
    goldens().assert(
        "notification-card-dark",
        &notification_card(ThemeMode::Dark, false),
    );
}

#[test]
fn notification_card_long_title_light() {
    goldens().assert(
        "notification-card-long-title-light",
        &notification_card(ThemeMode::Light, true),
    );
}

#[test]
fn notification_card_long_title_dark() {
    goldens().assert(
        "notification-card-long-title-dark",
        &notification_card(ThemeMode::Dark, true),
    );
}

/// The clock of the notification screenshots, in Unix milliseconds.
const NOTIFICATIONS_NOW: i64 = 1_760_000_000_000;

/// Four notifications, two of them unread, none older than an hour (older
/// ones show the time of day, which depends on the time zone).
fn notification_entries() -> Vec<arko_studio::notifications::Entry> {
    use arko_studio::notifications::Entry;
    const MINUTE: i64 = 60_000;
    let entry = |id: u64, title: &str, message: &str, ago: i64, read: bool| Entry {
        id,
        session: Some(format!("20261016-0912{id:02}-a1b2")),
        title: title.into(),
        message: message.into(),
        at: NOTIFICATIONS_NOW - ago,
        read,
    };
    vec![
        entry(
            1,
            "Naming a desktop app",
            "Arko finished its turn.",
            41 * MINUTE,
            true,
        ),
        entry(
            2,
            "Golden screenshots on Linux",
            "The prompt cache expires in a minute\nRefresh ≈ $0.04 · Letting it expire re-caches ≈ $0.90",
            14 * MINUTE,
            true,
        ),
        entry(
            3,
            "Variable fonts and the wght axis",
            "Arko asks “Tabs or spaces?”",
            3 * MINUTE,
            false,
        ),
        entry(
            4,
            "Split the engine out of main.rs",
            "Arko finished its turn.",
            12_000,
            false,
        ),
    ]
}

/// A window of Arko Studio, `width` logical pixels wide, with the
/// notifications `entries` in the history and the clock at
/// [`NOTIFICATIONS_NOW`], and the notifications panel opened.
fn notifications_window(
    mode: ThemeMode,
    width: f32,
    entries: Vec<arko_studio::notifications::Entry>,
) -> Screenshot {
    use arko_studio::cache_reminders::ReminderClock;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_notifications(entries)
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        cx.set_global(ReminderClock(std::rc::Rc::new(|| NOTIFICATIONS_NOW)));
    });
    let window = app
        .open_window_with(size(px(width), px(800.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.update_window(window, |_, window, cx| {
        window.dispatch_action(Box::new(arko_studio::menus::ToggleNotifications), cx)
    })
    .unwrap();
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn notifications_light() {
    goldens().assert(
        "notifications-light",
        &notifications_window(ThemeMode::Light, 1300., notification_entries()),
    );
}

#[test]
fn notifications_dark() {
    goldens().assert(
        "notifications-dark",
        &notifications_window(ThemeMode::Dark, 1300., notification_entries()),
    );
}

#[test]
fn notifications_empty_light() {
    goldens().assert(
        "notifications-empty-light",
        &notifications_window(ThemeMode::Light, 1300., Vec::new()),
    );
}

#[test]
fn notifications_empty_dark() {
    goldens().assert(
        "notifications-empty-dark",
        &notifications_window(ThemeMode::Dark, 1300., Vec::new()),
    );
}

#[test]
fn notifications_sheet_light() {
    goldens().assert(
        "notifications-sheet-light",
        &notifications_window(ThemeMode::Light, 640., notification_entries()),
    );
}

#[test]
fn notifications_sheet_dark() {
    goldens().assert(
        "notifications-sheet-dark",
        &notifications_window(ThemeMode::Dark, 640., notification_entries()),
    );
}

/// The sidebar with its marks together: a session that works and has news,
/// one that asks a question and has news, a chat with news, a closed folder
/// whose session has news, and one with no mark. `width` is the sidebar's,
/// in rem.
fn sidebar_marks(mode: ThemeMode, width: f32) -> Screenshot {
    use arko_studio::live::{Indicator, LiveSessions};

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    settings.sidebar.width = width;
    settings.sidebar.collapsed = vec!["/home/ada/src/gpui_kit".into()];
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_news(&[
            "20261016-091200-a1b2",
            "20261015-171000-c3d4",
            "20261016-080000-a7b8",
            "20261014-110500-e5f6",
        ])
        .with_fonts(fonts::bundled());
    app.update(|cx| {
        arko_studio::init(startup, cx);
        let mark = |id: &str, working: bool, question: bool, cx: &mut gpui_kit::App| {
            LiveSessions::set_indicator(&SessionId(id.into()), Indicator { working, question }, cx)
        };
        mark("20261016-091200-a1b2", true, false, cx);
        mark("20261015-171000-c3d4", false, true, cx);
    });
    let window = app
        .open_window_with(size(px(1300.), px(560.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, _)| window)
        })
        .unwrap();
    app.run_until_parked();
    app.capture(window).unwrap()
}

#[test]
fn sidebar_marks_light() {
    goldens().assert("sidebar-marks-light", &sidebar_marks(ThemeMode::Light, 16.));
}

#[test]
fn sidebar_marks_dark() {
    goldens().assert("sidebar-marks-dark", &sidebar_marks(ThemeMode::Dark, 16.));
}

#[test]
fn sidebar_marks_narrow_light() {
    goldens().assert(
        "sidebar-marks-narrow-light",
        &sidebar_marks(ThemeMode::Light, 13.),
    );
}

#[test]
fn sidebar_marks_narrow_dark() {
    goldens().assert(
        "sidebar-marks-narrow-dark",
        &sidebar_marks(ThemeMode::Dark, 13.),
    );
}

/// A session's info, as its popover shows it.
struct InfoCard {
    info: gpui_kit::Entity<arko_studio::session_info::SessionInfo>,
}

impl gpui_kit::Render for InfoCard {
    fn render(
        &mut self,
        _: &mut gpui_kit::Window,
        cx: &mut gpui_kit::Context<Self>,
    ) -> impl gpui_kit::IntoElement {
        use gpui_kit::component::ActiveTheme as _;
        use gpui_kit::{ParentElement as _, Styled as _, div};

        let theme = cx.theme();
        div()
            .size_full()
            .p_4()
            .bg(theme.background)
            .text_color(theme.foreground)
            .font_family(theme.font_family.clone())
            .child(
                div()
                    .p_3()
                    .bg(theme.popover)
                    .text_color(theme.popover_foreground)
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
                    .child(self.info.clone()),
            )
    }
}

fn session_info(mode: ThemeMode) -> Screenshot {
    use arko::details::{
        HandoverFacts, IndexEmbeddings, IndexFacts, IndexKind, IndexWatch, McpFacts,
        McpServerFacts, ModelFacts, SessionFacts, SkillFacts, SkillsFacts, WorkersFacts,
    };
    use arko::kept_notes::{Audience, Note};
    use arko_studio::backend::Mode;
    use arko_studio::session_info::{Facts, Preview, SessionInfo};
    use gpui_kit::AppContext as _;
    use std::path::PathBuf;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));

    let mut reading = arko::remote::Reading {
        context_window: Some(1_000_000),
        ..Default::default()
    };
    reading.settings.outside = true;
    reading.settings.handover_at = 950_000;
    reading.settings.workers_handover_at = 200_000;
    reading.settings.background_after = 600;
    reading.settings.text_replies = true;
    reading.settings.extra_rules = arko::ExtraRules::Small;
    reading.settings.worker_project_instructions = true;
    reading.settings.message_copies = true;
    reading.models.started = true;
    reading.models.has_workers = true;
    let preview = Preview {
        title: "Split the engine out of main.rs".into(),
        mode: Mode::Agent,
        workspace: Some(PathBuf::from("/home/ada/src/arko")),
        id: "20261016-091200-a1b2".into(),
        dir: Some(PathBuf::from(
            "/home/ada/.local/share/arko/sessions/20261016-091200-a1b2",
        )),
        reading: Arc::new(reading),
    };
    let model = |shown: &str| ModelFacts {
        model: "claude-opus-5-5".into(),
        shown: shown.into(),
        effort: arko::Effort::High,
        fast: false,
        cache_1h: false,
    };
    let facts = Facts {
        session: SessionFacts {
            id: preview.id.clone(),
            dir: preview.dir.clone().unwrap(),
            workspace: PathBuf::from("/home/ada/src/arko"),
            title: Some(preview.title.clone()),
            created: chrono::DateTime::from_timestamp(1_800_000_000, 0).unwrap(),
            main: model("claude-opus-5-5 (high effort) on the Anthropic API"),
            workers: Some(WorkersFacts {
                model: model("claude-sonnet-5-5 (high effort) on the Anthropic API"),
                max_running: None,
            }),
            handover: HandoverFacts {
                main_at: 950_000,
                main_compacts: false,
                workers_at: 200_000,
                workers_compact: false,
            },
            text_replies: true,
            extra_rules: arko::ExtraRules::Small,
            cc_compact: false,
            project_files: arko::ProjectFiles::One,
            project_workers: Some(true),
            outside: true,
            background_after: 600,
            show_thinking: false,
            hide_thinking: false,
        },
        mcp: McpFacts {
            servers: vec![
                McpServerFacts {
                    name: "files".into(),
                    shown: "files-server".into(),
                    file: PathBuf::from("/home/ada/src/arko/.mcp.json"),
                    tools: Ok(vec!["read".into(), "write".into(), "list".into()]),
                },
                McpServerFacts {
                    name: "browser".into(),
                    shown: "browser-server".into(),
                    file: PathBuf::from("/home/ada/src/arko/.mcp.json"),
                    tools: Err("no such program: browser-server".into()),
                },
            ],
            ..McpFacts::default()
        },
        skills: Ok(SkillsFacts {
            skills: [
                (
                    "release-notes",
                    "Write the release notes of a version from its commits.",
                    false,
                ),
                ("docs-style", "Rules for the project docs.", false),
                (
                    "user-facing-prose",
                    "How to write any text a person reads, such as UI text, messages, docs and commit messages.",
                    true,
                ),
            ]
            .into_iter()
            .map(|(name, description, builtin)| SkillFacts {
                name: name.into(),
                description: description.into(),
                always: false,
                builtin,
                replaces_builtin: None,
            })
            .collect(),
            folders: Vec::new(),
            own_folder: PathBuf::new(),
            builtin_off: vec!["repo-docs-style".into()],
        }),
        indexes: [
            ("arko", IndexKind::Code, "voyage-code-4", IndexWatch::Here),
            ("docs", IndexKind::Docs, "voyage-4", IndexWatch::Off),
        ]
        .into_iter()
        .map(|(name, kind, model, watch)| {
            let root = PathBuf::from("/home/ada/src/arko").join(name);
            let mut index = IndexFacts::new(name, kind, root);
            index.watch = watch;
            index.embeddings = Some(IndexEmbeddings::new(model));
            index
        })
        .collect(),
        notes: Ok([
            (
                "Tear the build box down when the last build is done.",
                Audience::Lead,
            ),
            ("Run the tests with --locked.", Audience::AllWorkers),
            (
                "Leave src/engine.rs alone.",
                Audience::Workers(vec!["r1".into(), "r2".into()]),
            ),
        ]
        .into_iter()
        .zip(1..)
        .map(|((text, audience), id)| Note {
            id,
            text: text.into(),
            audience,
            created: chrono::DateTime::UNIX_EPOCH,
        })
        .collect()),
    };
    let window = app
        .open_window_with(size(px(560.), px(1800.)), 1., |options, cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                let info = cx.new(|cx| {
                    let mut info = SessionInfo::new(window, cx);
                    info.set_preview(preview, facts, window, cx);
                    info
                });
                cx.new(|_| InfoCard { info })
            })
            .map(|(window, _)| window)
        })
        .unwrap();
    app.capture(window).unwrap()
}

#[test]
fn session_info_light() {
    goldens().assert("session-info-light", &session_info(ThemeMode::Light));
}

#[test]
fn session_info_dark() {
    goldens().assert("session-info-dark", &session_info(ThemeMode::Dark));
}

/// The open session's name being edited in the sidebar. The field alone shows
/// focus, and the row keeps its selected look without a ring of its own.
fn renaming(mode: ThemeMode) -> Screenshot {
    use arko_studio::backend::SessionId;
    use arko_studio::menus;
    use gpui_kit::test::TestWindowExt as _;

    let mut app = ScreenshotApp::with_assets(Assets).unwrap();
    let mut settings = Prefs::default();
    settings.appearance.theme_mode = mode;
    settings.appearance.motion = Motion::Off;
    let backend = Arc::new(FakeBackend::sample("/arko", SystemTime::now()));
    let startup = Startup::for_tests(backend)
        .with_prefs(settings)
        .with_fonts(fonts::bundled());
    app.update(|cx| arko_studio::init(startup, cx));
    let mut workspace = None;
    let window = app
        .open_window_with(size(px(900.), px(360.)), 1., |options, cx| {
            workspace::open_window(options, cx).map(|(window, opened)| {
                workspace = Some(opened);
                window
            })
        })
        .unwrap();
    let workspace = workspace.unwrap();
    app.update_window(window, |_, window, cx| {
        window.activate_window();
        let fonts = SessionId("20261016-080000-a7b8".into());
        workspace.update(cx, |workspace, cx| {
            workspace.open_session(fonts, window, cx)
        });
    })
    .unwrap();
    app.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.dispatch_action(Box::new(menus::FocusSessionFilter), cx);
    })
    .unwrap();
    for key in ["down", "f2"] {
        app.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.press(key, cx);
        })
        .unwrap();
    }
    app.capture(window).unwrap()
}

#[test]
fn renaming_light() {
    goldens().assert("renaming-light", &renaming(ThemeMode::Light));
}

#[test]
fn renaming_dark() {
    goldens().assert("renaming-dark", &renaming(ThemeMode::Dark));
}
