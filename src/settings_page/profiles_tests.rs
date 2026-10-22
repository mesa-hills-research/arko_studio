//! Settings › Worker profiles in the app's real window, driven by clicks on
//! GPUI's test platform, on the fake backend (profiles in memory) and on the
//! real one over a temporary folder.

use std::path::Path;
use std::time::SystemTime;

use arko::profiles::Profile;
use gpui_kit::component::WindowExt as _;
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext, VisualTestContext};

use super::interaction::{settings_window, start_on};
use super::profiles::{ProfileForm, ProfilesPage};
use super::{PROFILES_PAGE, SettingsPage};
use crate::backend::profiles::{Entry, List, Profiles, Project};
use crate::backend::{FakeBackend, ModelChoice, ModelChoices, ModelGroup};
use crate::prefs;
use crate::tests::act;

use std::sync::Arc;

/// The app with three profiles of the user's, as the fake backend has them.
fn start_with_profiles(cx: &mut TestAppContext, profiles: Profiles) -> Arc<FakeBackend> {
    start_on(
        cx,
        FakeBackend::sample("/arko", SystemTime::now()).with_profiles(profiles),
        prefs::Prefs::default(),
    )
}

fn sample(cx: &mut TestAppContext) -> Arc<FakeBackend> {
    start_with_profiles(cx, Profiles::sample(Path::new("/arko")))
}

fn page(cx: &mut TestAppContext, settings: &Entity<SettingsPage>) -> Entity<ProfilesPage> {
    cx.read(|cx| settings.read(cx).profiles().clone())
}

fn editor(cx: &mut TestAppContext, page: &Entity<ProfilesPage>) -> Entity<ProfileForm> {
    cx.read(|cx| page.read(cx).editor())
        .expect("an editor is open")
}

fn names(backend: &FakeBackend) -> Vec<String> {
    backend
        .saved_profiles()
        .user
        .entries
        .iter()
        .map(|entry| entry.profile.name.clone())
        .collect()
}

fn shows(
    window: AnyWindowHandle,
    cx: &mut TestAppContext,
    id: impl Into<gpui_kit::ElementId>,
) -> bool {
    let id = id.into();
    cx.update_window(window, |_, window, cx| {
        window.render_frame(cx);
        window.try_find(id.clone()).is_some()
    })
    .unwrap()
}

/// Types `name` and `description` into the open editor.
fn fill(
    cx: &mut TestAppContext,
    window: AnyWindowHandle,
    form: &Entity<ProfileForm>,
    name: &str,
    description: &str,
) {
    let (name_field, description_field, _) = cx.read(|cx| form.read(cx).inputs());
    act(cx, window, |window, cx| {
        name_field.update(cx, |field, cx| {
            field.set_value(name.to_string(), window, cx)
        });
        description_field.update(cx, |field, cx| {
            field.set_value(description.to_string(), window, cx)
        });
    });
}

/// With no profiles the page says so and offers Add. A profile is
/// filled in with every field and saved, through the backend, and the page
/// lists it.
#[gpui_kit::test]
fn a_profile_is_added_in_the_editor(cx: &mut TestAppContext) {
    let backend = start_with_profiles(cx, Profiles::default());
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    assert!(shows(window, cx, "add-profile"));
    assert!(!shows(window, cx, ("edit-profile", 0usize)));
    // The button says "Add", and a screen reader says what it adds.
    act(cx, window, |window, _| {
        assert_eq!(window.find("add-profile").label(), Some("Add a profile"));
    });
    act(cx, window, |window, cx| window.click("add-profile", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx))
    });
    let form = editor(cx, &page);
    // A screen reader names the dropdowns with the field and its value.
    act(cx, window, |window, _| {
        assert_eq!(
            window.find("profile-model").label(),
            Some("Model, The session’s worker model")
        );
        assert_eq!(
            window.find("profile-effort").label(),
            Some("Effort, The session’s worker effort")
        );
    });
    // Nothing yet: Save says what is missing, and writes nothing.
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert!(shows(window, cx, "profile-error"));
    assert!(names(&backend).is_empty());

    fill(
        cx,
        window,
        &form,
        "quick-reader",
        "Reads files and answers in a line",
    );
    // The model and the effort come from the menus, the effort from the
    // model's own.
    act(cx, window, |window, cx| window.click("profile-model", cx));
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(2usize, cx)
    });
    act(cx, window, |window, cx| window.click("profile-effort", cx));
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(1usize, cx)
    });
    act(cx, window, |window, cx| window.click("profile-cache", cx));
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(2usize, cx)
    });
    act(cx, window, |window, cx| window.click("profile-fast", cx));
    act(cx, window, |window, cx| window.click("profile-nested", cx));
    let (_, _, instructions) = cx.read(|cx| form.read(cx).inputs());
    act(cx, window, |window, cx| {
        instructions.update(cx, |field, cx| {
            field.set_value(
                String::from("Read what you are asked to.\nAnswer in one line."),
                window,
                cx,
            )
        })
    });
    act(cx, window, |window, cx| window.click("profile-save", cx));
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx), "saved, so closed");
        assert_eq!(window.notifications(cx).len(), 1);
    });
    let saved = backend.saved_profiles().user;
    assert_eq!(saved.entries.len(), 1);
    let profile = &saved.entries[0].profile;
    assert_eq!(profile.name, "quick-reader");
    assert_eq!(profile.description, "Reads files and answers in a line");
    assert_eq!(profile.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(profile.effort, Some(arko::Effort::Low));
    assert_eq!(profile.cache, Some(arko::profiles::Cache::OneHour));
    assert!(profile.fast && profile.nested);
    assert_eq!(
        profile.instructions,
        "Read what you are asked to.\nAnswer in one line."
    );
    // The page lists it, with Edit.
    assert!(shows(window, cx, ("edit-profile", 0usize)));
}

/// A name Arko wouldn't take, or another profile's, shows under the field as
/// it's typed, and Save refuses with Arko's reasons.
#[gpui_kit::test]
fn a_name_and_a_description_arko_wouldnt_take_are_refused(cx: &mut TestAppContext) {
    let backend = sample(cx);
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    act(cx, window, |window, cx| window.click("add-profile", cx));
    let form = editor(cx, &page);
    let problem = |cx: &mut TestAppContext, form: &Entity<ProfileForm>| {
        cx.read(|cx| form.read(cx).name_problem(cx))
    };
    for (typed, expected) in [
        ("quick", None),
        ("", None),
        ("My Judge", Some("isn’t a profile name")),
        ("default", Some("is reserved")),
        (
            "reviewer",
            Some("There is already a profile called reviewer"),
        ),
    ] {
        fill(cx, window, &form, typed, "Does a job");
        let found = problem(cx, &form);
        match expected {
            None => assert_eq!(found, None, "{typed}"),
            Some(words) => assert!(
                found.as_deref().is_some_and(|found| found.contains(words)),
                "{typed}: {found:?}"
            ),
        }
        assert_eq!(
            shows(window, cx, "profile-name-problem"),
            expected.is_some()
        );
    }
    // Save says so too, and nothing is written.
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert!(shows(window, cx, "profile-error"));
    // A description that is too long, and Arko's message for it.
    fill(cx, window, &form, "quick", &"a".repeat(121));
    assert!(shows(window, cx, "profile-description-problem"));
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert!(shows(window, cx, "profile-error"));
    // The instructions' limit shows as a count.
    let (_, _, instructions) = cx.read(|cx| form.read(cx).inputs());
    act(cx, window, |window, cx| {
        instructions.update(cx, |field, cx| {
            field.set_value("a".repeat(arko::profiles::MAX_INSTRUCTIONS + 1), window, cx)
        })
    });
    assert!(shows(window, cx, "profile-instructions-length"));
    assert_eq!(names(&backend), ["3d-judge", "quick-reader", "reviewer"]);
}

/// An effort the model doesn't take shows Arko's reason under the field.
#[gpui_kit::test]
fn an_effort_the_model_lacks_shows_arkos_reason(cx: &mut TestAppContext) {
    let mut broken = Profile::new("broken", "Asks too much");
    broken.model = Some("gemini".into());
    broken.effort = Some(arko::Effort::Max);
    let note = broken.model_problem();
    assert!(note.is_some(), "arko says it can't");
    let profiles = Profiles {
        user: List {
            entries: vec![Entry {
                file: "/arko/profiles/broken.md".into(),
                profile: broken,
                note,
            }],
            unreadable: Vec::new(),
        },
        projects: Vec::new(),
    };
    start_with_profiles(cx, profiles);
    let (window, _, _) = settings_window(cx, PROFILES_PAGE, 1400.);
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 0usize), cx)
    });
    assert!(shows(window, cx, "profile-effort-problem"));
}

/// Editing keeps the profile's file name unless the name changes, and a rename
/// moves the profile. Cancel changes nothing.
#[gpui_kit::test]
fn a_profile_is_edited_and_renamed(cx: &mut TestAppContext) {
    let backend = sample(cx);
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    // Cancel leaves it alone.
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 0usize), cx)
    });
    let form = editor(cx, &page);
    fill(cx, window, &form, "3d-judge", "Changed my mind");
    act(cx, window, |window, cx| window.click("profile-cancel", cx));
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx))
    });
    assert_eq!(
        backend.saved_profiles().user.entries[0].profile.description,
        "Judges 3D results against a reference image"
    );
    // The same name saves over the file.
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 0usize), cx)
    });
    let form = editor(cx, &page);
    fill(cx, window, &form, "3d-judge", "Judges renders");
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert_eq!(names(&backend), ["3d-judge", "quick-reader", "reviewer"]);
    assert_eq!(
        backend.saved_profiles().user.entries[0].profile.description,
        "Judges renders"
    );
    // Another name moves it, unless that name is taken.
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 0usize), cx)
    });
    let form = editor(cx, &page);
    fill(cx, window, &form, "reviewer", "Judges renders");
    act(cx, window, |window, cx| window.click("profile-save", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx))
    });
    fill(cx, window, &form, "render-judge", "Judges renders");
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert_eq!(
        names(&backend),
        ["quick-reader", "render-judge", "reviewer"]
    );
}

/// Duplicate opens a new profile that starts as a copy, with a name no
/// profile has, and the original stays.
#[gpui_kit::test]
fn a_profile_is_duplicated(cx: &mut TestAppContext) {
    let backend = sample(cx);
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 0usize), cx)
    });
    act(cx, window, |window, cx| {
        window.click("profile-duplicate", cx)
    });
    let copy = editor(cx, &page);
    let (name, description, instructions) = cx.read(|cx| copy.read(cx).inputs());
    assert_eq!(
        cx.read(|cx| name.read(cx).value().to_string()),
        "3d-judge-copy"
    );
    assert_eq!(
        cx.read(|cx| description.read(cx).value().to_string()),
        "Judges 3D results against a reference image"
    );
    assert!(cx.read(|cx| {
        instructions
            .read(cx)
            .value()
            .starts_with("You judge 3D models.")
    }));
    let draft = cx.read(|cx| copy.read(cx).draft(cx));
    assert_eq!(draft.model.as_deref(), Some("sol"));
    assert_eq!(draft.effort, Some(arko::Effort::High));
    // It is a new profile: no Delete, and Save adds it.
    assert!(!shows(window, cx, "profile-delete"));
    act(cx, window, |window, cx| window.click("profile-save", cx));
    assert_eq!(
        names(&backend),
        ["3d-judge", "3d-judge-copy", "quick-reader", "reviewer"]
    );
}

/// Delete asks first, names the file, and deletes only when confirmed.
#[gpui_kit::test]
fn a_profile_is_deleted_after_asking(cx: &mut TestAppContext) {
    let backend = sample(cx);
    let (window, _, _) = settings_window(cx, PROFILES_PAGE, 1400.);
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 1usize), cx)
    });
    act(cx, window, |window, cx| window.click("profile-delete", cx));
    act(cx, window, |window, cx| {
        assert!(window.has_active_dialog(cx))
    });
    // Asks first.
    assert_eq!(names(&backend).len(), 3);
    act(cx, window, |window, cx| {
        window.within("dialog").click("cancel", cx)
    });
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx))
    });
    assert_eq!(names(&backend).len(), 3, "cancelled");
    act(cx, window, |window, cx| {
        window.click(("edit-profile", 1usize), cx)
    });
    act(cx, window, |window, cx| window.click("profile-delete", cx));
    act(cx, window, |window, cx| {
        window.within("dialog").click("ok", cx)
    });
    assert_eq!(names(&backend), ["3d-judge", "reviewer"]);
    act(cx, window, |window, cx| {
        assert!(!window.has_active_dialog(cx));
        assert_eq!(window.notifications(cx).len(), 1);
    });
    assert!(!shows(window, cx, ("edit-profile", 2usize)));
}

/// A file changed elsewhere shows once the window is in front again.
#[gpui_kit::test]
fn a_file_edited_elsewhere_shows_after_the_window_regains_focus(cx: &mut TestAppContext) {
    let backend = sample(cx);
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    let count = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            page.read(cx)
                .profiles()
                .map(|profiles| profiles.user.entries.len())
        })
    };
    assert_eq!(count(cx), Some(3));
    let mut changed = backend.saved_profiles();
    changed.user.entries.pop();
    backend.set_profiles(changed);
    // Nothing happens while the window stays in front.
    cx.run_until_parked();
    assert_eq!(count(cx), Some(3));
    VisualTestContext::from_window(window, cx).deactivate_window();
    cx.update_window(window, |_, window, _| window.activate_window())
        .unwrap();
    cx.run_until_parked();
    act(cx, window, |_, _| {});
    assert_eq!(count(cx), Some(2));
    assert!(!shows(window, cx, ("edit-profile", 2usize)));
}

/// A model that needs a key says so in its row and in the editor, and a
/// project's profiles are listed read only under its folder, with the
/// files that can't be read.
#[gpui_kit::test]
fn the_page_says_what_a_model_needs_and_lists_projects_read_only(cx: &mut TestAppContext) {
    let mut profiles = Profiles::sample(Path::new("/arko"));
    let mut lint = Profile::new("lint", "Runs the linter");
    lint.model = Some("haiku".into());
    let project = Project {
        folder: "/home/ada/src/arko".into(),
        list: List {
            entries: vec![Entry {
                file: "/home/ada/src/arko/.agents/profiles/lint.md".into(),
                profile: lint,
                note: None,
            }],
            unreadable: vec![crate::backend::profiles::Unreadable {
                file: "/home/ada/src/arko/.agents/profiles/torn.md".into(),
                reason: "it has no description".into(),
            }],
        },
    };
    profiles.projects.push(project);
    let models = ModelChoices {
        groups: vec![
            ModelGroup {
                name: "Anthropic API, billed per token".into(),
                needs: Some("ANTHROPIC_API_KEY".into()),
                models: vec![ModelChoice {
                    name: "claude-haiku-5-5".into(),
                    alias: "haiku".into(),
                }],
            },
            ModelGroup {
                name: "OpenAI, with your API key".into(),
                needs: None,
                models: vec![ModelChoice {
                    name: "gpt-6.1-sol".into(),
                    alias: "sol".into(),
                }],
            },
        ],
        default: None,
        note: None,
    };
    let backend = FakeBackend::sample("/arko", SystemTime::now())
        .with_profiles(profiles)
        .with_models(models);
    start_on(cx, backend, prefs::Prefs::default());
    let (window, _, settings) = settings_window(cx, PROFILES_PAGE, 1400.);
    let page = page(cx, &settings);
    // The user's three have Edit, the project's none.
    assert!(shows(window, cx, ("edit-profile", 2usize)));
    assert!(!shows(window, cx, ("edit-profile", 3usize)));
    assert!(shows(window, cx, ("show-profile", 0usize)));
    // The project's lint profile runs on haiku, which needs a key.
    let words = cx.read(|cx| {
        let page = page.read(cx);
        let profiles = page.profiles().unwrap();
        page.row_words(&profiles.projects[0].list.entries[0])
    });
    assert_eq!(
        words,
        "Runs the linter. Runs on haiku. Needs ANTHROPIC_API_KEY, so workers can’t use it yet."
    );
    // The editor says it under the model.
    act(cx, window, |window, cx| window.click("add-profile", cx));
    let form = editor(cx, &page);
    act(cx, window, |window, cx| window.click("profile-model", cx));
    // The session's worker model, then the two models, in the order of the
    // groups. Haiku is the first.
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(1usize, cx)
    });
    assert!(shows(window, cx, "profile-model-needs"));
    assert_eq!(
        cx.read(|cx| form.read(cx).draft(cx).model),
        Some("claude-haiku-5-5".to_string())
    );
    act(cx, window, |window, cx| window.click("profile-model", cx));
    act(cx, window, |window, cx| {
        window.within("popup-menu").click(2usize, cx)
    });
    assert!(!shows(window, cx, "profile-model-needs"));
}

/// The profiles of a real folder: what the page's backend saves is the file
/// Arko reads, and what is on disk lists.
#[gpui_kit::test]
fn the_real_backend_saves_files_arko_reads(cx: &mut TestAppContext) {
    use crate::backend::Backend as _;
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("arko");
    std::fs::create_dir_all(root.join("profiles")).unwrap();
    std::fs::write(
        root.join("profiles/scout.md"),
        "---\ndescription: Looks around\nmodel: haiku\n---\nLook around.",
    )
    .unwrap();
    let backend = crate::backend::ArkoBackend::new(&root);
    let listed = backend.profiles(&[]);
    assert_eq!(listed.user.entries[0].profile.name, "scout");
    let mut profile = listed.user.entries[0].profile.clone();
    profile.description = "Looks around quickly".into();
    let file = backend.save_profile(&profile, Some("scout")).unwrap();
    assert_eq!(file, root.join("profiles/scout.md"));
    assert!(
        std::fs::read_to_string(&file)
            .unwrap()
            .contains("description: Looks around quickly")
    );
    backend.delete_profile("scout").unwrap();
    assert!(backend.profiles(&[]).user.is_empty());
    let _ = cx;
}
