//! Settings › Worker profiles: the setups the main agent starts workers
//! with, a model, an effort and instructions each. The user's own are
//! edited here (Arko's folder holds them as files, so the terminal app sees
//! the same ones), and each recent project's and repository's are listed read
//! only. Where profiles are kept is chosen here too. Files are read and
//! written through `arko::profiles`, on a background thread.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use arko::profiles::{self, Cache, MAX_DESCRIPTION, MAX_INSTRUCTIONS, Preferences, Profile};
use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_kit::component::menu::{DropdownMenu as _, PopupMenuItem};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _, WindowExt as _,
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::accounts::effort_options;
use crate::backend::account_words::tilde;
use crate::backend::profiles::{self as store, Entry, List, Profiles};
use crate::backend::{ModelChoices, SharedBackend};
use crate::model_select::ModelSelect;
use crate::prefs;
use crate::session_view::{HomeFolder, recent_folders};
use crate::workers::curly;

/// The page's state: the profiles and the models as last read.
pub struct ProfilesPage {
    profiles: Option<Profiles>,
    models: Option<ModelChoices>,
    home: Option<PathBuf>,
    /// How many times what the page shows changed: its groups draw again
    /// only then.
    revision: u64,
    /// The editor, while one is open.
    editor: Option<WeakEntity<ProfileForm>>,
    _load: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl ProfilesPage {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![
            // Coming back to the window shows what was edited elsewhere (the
            // terminal app, an editor, the agent saving a profile).
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() {
                    this.refresh(cx);
                }
            }),
            // A key, a login or Claude Code changed: which models need
            // something changed.
            cx.observe_global::<crate::backend::AccountsSeen>(|this, cx| this.refresh(cx)),
        ];
        let mut this = Self {
            profiles: None,
            models: None,
            home: cx.global::<HomeFolder>().0.clone(),
            revision: 0,
            editor: None,
            _load: None,
            _subscriptions: subscriptions,
        };
        this.refresh(cx);
        this
    }

    /// The profiles as last read.
    pub fn profiles(&self) -> Option<&Profiles> {
        self.profiles.as_ref()
    }

    /// What the row of `entry` says (tests).
    #[cfg(test)]
    pub fn row_words(&self, entry: &Entry) -> String {
        entry_words(entry, self.models.as_ref())
    }

    /// The editor that is open, if one is.
    pub fn editor(&self) -> Option<Entity<ProfileForm>> {
        self.editor.as_ref().and_then(WeakEntity::upgrade)
    }

    /// Reads the profiles and the models again, in the background, for the
    /// folders of the sessions in Arko's folder. The page draws again only
    /// when something it shows changed.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = cx.global::<SharedBackend>().clone();
        let reading = cx.background_spawn(async move {
            let folders = backend
                .sessions()
                .map(|sessions| recent_folders(&sessions))
                .unwrap_or_default();
            let profiles = backend.profiles(&folders);
            let models = backend.models(None);
            (profiles, models)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let (profiles, models) = reading.await;
            this.update(cx, |this, cx| {
                if this.profiles.as_ref() == Some(&profiles)
                    && this.models.as_ref() == Some(&models)
                {
                    return;
                }
                this.profiles = Some(profiles);
                this.models = Some(models);
                this.revision += 1;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Opens the editor on a new profile, or on `profile` read from `file`.
    fn edit(this: &Entity<Self>, profile: Option<Entry>, window: &mut Window, cx: &mut App) {
        let page = this.read(cx);
        let names: Vec<String> = page
            .profiles
            .iter()
            .flat_map(|profiles| &profiles.user.entries)
            .map(|entry| entry.profile.name.clone())
            .collect();
        let form = ProfileForm::open(
            this.clone(),
            profile,
            names,
            page.models.clone(),
            page.home.clone(),
            window,
            cx,
        );
        this.update(cx, |page, _| page.editor = Some(form.downgrade()));
    }

    /// Asks before deleting the user's profile `name`, then deletes it.
    fn confirm_delete(this: &Entity<Self>, entry: &Entry, window: &mut Window, cx: &mut App) {
        let (name, file) = (
            entry.profile.name.clone(),
            tilde(&entry.file, this.read(cx).home.as_deref()),
        );
        let this = this.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, name) = (this.clone(), name.clone());
            alert
                .title(format!("Delete the profile {name}?"))
                .description(format!(
                    "Arko deletes {file}. Workers already started with it keep their setup, and the agent can’t start new ones with it."
                ))
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    delete(this.clone(), name.clone(), window, cx);
                    true
                })
        });
    }

    /// The page, as last read.
    pub fn page(&self, this: &Entity<Self>) -> SettingPage {
        let home = self.home.as_deref();
        let (user, projects, repositories) = match &self.profiles {
            None => (
                vec![
                    SettingItem::new("Profiles", SettingField::render(|_, _, _| div()))
                        .description("Reading…"),
                ],
                Vec::new(),
                Vec::new(),
            ),
            Some(profiles) => {
                // (A group's rows have ids of their own, so the repositories'
                // number on from the projects'.)
                let shown = |from: usize, folders: &[store::Project]| -> Vec<_> {
                    folders
                        .iter()
                        .enumerate()
                        .map(|(ix, folder)| {
                            let items =
                                project_items(from + ix, &folder.list, self.models.as_ref(), home);
                            (tilde(&folder.folder, home), items)
                        })
                        .collect()
                };
                (
                    self.user_items(this, &profiles.user),
                    shown(0, &profiles.projects),
                    shown(profiles.projects.len(), &profiles.repositories),
                )
            }
        };
        let mut groups = vec![
            SettingGroup::new()
                .key(super::group_key("Your profiles", &self.revision))
                .title("Your profiles")
                .description(
                    "For every project. The main agent can save one here too, after asking you.",
                )
                .items(user),
        ];
        for (folder, items) in projects {
            groups.push(
                SettingGroup::new()
                    .key(super::group_key(&folder, &self.revision))
                    .title(folder)
                    .description(
                        "Read only. Edit the files in the project. In a session there, a project’s profile beats yours of the same name.",
                    )
                    .items(items),
            );
        }
        for (folder, items) in repositories {
            groups.push(
                SettingGroup::new()
                    .key(super::group_key(&folder, &self.revision))
                    .title(folder)
                    .description(
                        "Read only. Edit the files in the repository. A session in one of its folders reads these after the project’s own, and before yours.",
                    )
                    .items(items),
            );
        }
        if let Some(profiles) = &self.profiles {
            groups.push(self.kept_group(this, profiles.preferences));
        }
        SettingPage::new(super::PAGES[super::PROFILES_PAGE])
            .icon(gpui_kit::assets::IconName::UserCog)
            .resettable(false)
            .description(
                "A profile is a named setup for workers, with a model, an effort and instructions. The main agent starts a worker with one by name. Profiles are files in Arko’s folder, so the terminal app uses the same ones.",
            )
            .groups(groups)
    }

    /// The choices about where profiles are kept, which Arko keeps for every
    /// project and reads each time a worker starts.
    fn kept_group(&self, this: &Entity<Self>, preferences: Preferences) -> SettingGroup {
        let switch = |id: &'static str,
                      title: &'static str,
                      description: &'static str,
                      on: bool,
                      set: fn(&mut Preferences, bool),
                      keywords: [&'static str; 3]| {
            let page = this.clone();
            SettingItem::new(
                title,
                SettingField::render(move |_, _, _| {
                    let page = page.clone();
                    Switch::new(id)
                        .checked(on)
                        .on_click(move |on, window, cx| {
                            ProfilesPage::choose(&page, |prefs| set(prefs, *on), window, cx)
                        })
                        .into_any_element()
                }),
            )
            .description(description)
            .keywords(keywords)
        };
        SettingGroup::new()
            .key(super::group_key("Where profiles are kept", &self.revision))
            .title("Where profiles are kept")
            .items([
                switch(
                    "keep-profiles-out-of-git",
                    "Keep new profile folders out of git",
                    "A project’s new .agents/profiles folder gets a .gitignore, so git ignores it. Delete that file to share the profiles.",
                    preferences.keep_out_of_git,
                    |prefs, on| prefs.keep_out_of_git = on,
                    ["worker", "profile", "gitignore"],
                ),
                switch(
                    "read-repository-profiles",
                    "Read the repository’s profiles",
                    "In a subfolder of a git repository, Arko reads the .agents/profiles in the repository’s root too, after the project’s own.",
                    preferences.repo_root,
                    |prefs, on| prefs.repo_root = on,
                    ["worker", "profile", "repository"],
                ),
            ])
    }

    /// Changes where profiles are kept: the page shows it at once, the file
    /// is written off the UI thread, and the page reads again.
    fn choose(
        this: &Entity<Self>,
        change: impl FnOnce(&mut Preferences),
        window: &mut Window,
        cx: &mut App,
    ) {
        let mut preferences = Preferences::default();
        this.update(cx, |page, cx| {
            if let Some(profiles) = page.profiles.as_mut() {
                change(&mut profiles.preferences);
                preferences = profiles.preferences;
                page.revision += 1;
                cx.notify();
            }
        });
        let backend = cx.global::<SharedBackend>().clone();
        let saving =
            cx.background_spawn(async move { backend.set_profile_preferences(preferences) });
        let page = this.clone();
        window
            .spawn(cx, async move |cx| {
                let saved = saving.await;
                cx.update(|window, cx| {
                    if let Err(why) = saved {
                        window.push_notification(Notification::error(curly(&why)), cx);
                    }
                    page.update(cx, |page, cx| page.refresh(cx));
                })
                .ok();
            })
            .detach();
    }

    /// The user's profiles as rows with Edit, then the row that adds one.
    fn user_items(&self, this: &Entity<Self>, list: &List) -> Vec<SettingItem> {
        let home = self.home.as_deref();
        let mut items: Vec<SettingItem> = list
            .entries
            .iter()
            .enumerate()
            .map(|(ix, entry)| {
                let (page, edited) = (this.clone(), entry.clone());
                let name = entry.profile.name.clone();
                SettingItem::new(
                    entry.profile.name.clone(),
                    SettingField::render(move |_, _, _| {
                        let (page, entry) = (page.clone(), edited.clone());
                        Button::new(("edit-profile", ix))
                            .outline()
                            .small()
                            .label("Edit")
                            .accessibility_label(format!("Edit the profile {}", entry.profile.name))
                            .on_click(move |_, window, cx| {
                                ProfilesPage::edit(&page, Some(entry.clone()), window, cx)
                            })
                    }),
                )
                .description(entry_words(entry, self.models.as_ref()))
                .keywords(entry_keywords(entry, &name))
            })
            .collect();
        for (ix, problem) in list.unreadable.iter().enumerate() {
            items.push(unreadable_item(ix, problem, home));
        }
        let page = this.clone();
        let add = SettingField::render(move |_, _, _| {
            let page = page.clone();
            Button::new("add-profile")
                .outline()
                .small()
                .icon(IconName::Plus)
                .label("Add")
                .accessibility_label("Add a profile")
                .on_click(move |_, window, cx| ProfilesPage::edit(&page, None, window, cx))
        });
        items.push(if list.is_empty() {
            SettingItem::new("No worker profiles yet", add)
                .description("Add one, or ask the agent to save one.")
        } else {
            SettingItem::new("Add a profile", add).description("Or ask the agent to save one.")
        });
        items
    }
}

/// A profile as a row says it: what it is for, what it runs on and what is
/// set besides, then what is wrong with its model.
fn entry_words(entry: &Entry, models: Option<&ModelChoices>) -> String {
    let profile = &entry.profile;
    let mut words = vec![with_stop(&profile.description)];
    // The model as the terminal writes it in as few characters.
    let mut shown = profile.clone();
    shown.model = profile
        .model
        .as_deref()
        .map(arko::workers::short_model_name);
    words.push(format!("Runs on {}.", curly(&shown.runs_on())));
    if profile.fast {
        words.push("Fast mode.".into());
    }
    match profile.cache {
        Some(Cache::FiveMinutes) => words.push("Short prompt cache.".into()),
        Some(Cache::OneHour) => words.push("Long prompt cache.".into()),
        _ => {}
    }
    if profile.nested {
        words.push("Can start workers.".into());
    }
    if profile.made_by_agent() {
        words.push("Made by the main agent.".into());
    }
    if let Some(note) = &entry.note {
        words.push(curly(&with_stop(note)));
    }
    if let (Some(models), Some(model)) = (models, &profile.model)
        && let Some(needs) = needs_of(models, model)
    {
        words.push(format!("Needs {needs}, so workers can’t use it yet."));
    }
    words.join(" ")
}

/// What a profile is found by.
fn entry_keywords(entry: &Entry, name: &str) -> Vec<SharedString> {
    let mut keywords: Vec<SharedString> =
        vec!["worker".into(), "profile".into(), name.to_string().into()];
    keywords.extend(entry.profile.model.iter().map(|model| model.clone().into()));
    keywords.extend(
        entry
            .profile
            .description
            .split_whitespace()
            .map(|word| word.to_string().into()),
    );
    keywords
}

/// `text` with a full stop, as it is when it has one.
fn with_stop(text: &str) -> String {
    let text = text.trim();
    if text.ends_with(['.', '!', '?']) {
        text.to_string()
    } else {
        format!("{text}.")
    }
}

/// What the model `chosen` (as `/model` takes it, a name or an alias) needs
/// that isn't there, from the group of models that has it.
pub fn needs_of(models: &ModelChoices, chosen: &str) -> Option<String> {
    models
        .groups
        .iter()
        .find(|group| {
            group
                .models
                .iter()
                .any(|model| model.name == chosen || model.alias == chosen)
        })?
        .needs
        .clone()
}

/// A project's profiles as read-only rows, each with a button that shows
/// its file.
fn project_items(
    group: usize,
    list: &List,
    models: Option<&ModelChoices>,
    home: Option<&Path>,
) -> Vec<SettingItem> {
    let mut items: Vec<SettingItem> = list
        .entries
        .iter()
        .enumerate()
        .map(|(ix, entry)| {
            let file = entry.file.clone();
            SettingItem::new(
                entry.profile.name.clone(),
                SettingField::render(move |_, _, _| {
                    reveal_button(("show-profile", group * 1000 + ix), &file)
                }),
            )
            .description(entry_words(entry, models))
            .keywords(entry_keywords(entry, &entry.profile.name))
        })
        .collect();
    for (ix, problem) in list.unreadable.iter().enumerate() {
        items.push(unreadable_item(group * 1000 + 500 + ix, problem, home));
    }
    items
}

/// A profile file that can't be read, as a row with its reason.
fn unreadable_item(ix: usize, problem: &store::Unreadable, home: Option<&Path>) -> SettingItem {
    let file = problem.file.clone();
    SettingItem::new(
        problem
            .file
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        SettingField::render(move |_, _, _| reveal_button(("show-profile", ix + 100_000), &file)),
    )
    .description(format!(
        "{} can’t be used ({}).",
        tilde(&problem.file, home),
        curly(&problem.reason)
    ))
    .keywords(["worker", "profile", "unreadable"])
}

/// A button that shows `file` in the file manager.
fn reveal_button(id: (&'static str, usize), file: &Path) -> Button {
    let file = file.to_path_buf();
    Button::new(id)
        .ghost()
        .small()
        .icon(IconName::FolderOpen)
        .tooltip("Show in the file manager")
        .accessibility_label("Show the file in the file manager")
        .on_click(move |_, _, cx| cx.reveal_path(&file))
}

/// Deletes the user's profile `name`, off the UI thread, and says how it
/// went.
fn delete(page: Entity<ProfilesPage>, name: String, window: &mut Window, cx: &mut App) {
    let backend = cx.global::<SharedBackend>().clone();
    let deleting = {
        let name = name.clone();
        cx.background_spawn(async move { backend.delete_profile(&name) })
    };
    window
        .spawn(cx, async move |cx| {
            let deleted = deleting.await;
            cx.update(|window, cx| {
                match deleted {
                    Ok(_) => window.push_notification(format!("Deleted the profile {name}."), cx),
                    Err(why) => window.push_notification(Notification::error(curly(&why)), cx),
                }
                page.update(cx, |page, cx| page.refresh(cx));
            })
            .ok();
        })
        .detach();
}

/// `number` with commas between the thousands.
fn grouped(number: usize) -> String {
    let digits = number.to_string();
    let mut out = String::new();
    for (ix, digit) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// The editor's form, in a dialog: what a profile is, a few fields, and
/// Save, Duplicate and Delete.
pub struct ProfileForm {
    page: Entity<ProfilesPage>,
    /// The profile as it was opened (a new one has no file), so what the
    /// form doesn't show is kept and a changed name moves the file.
    original: Option<Entry>,
    /// The fields the inputs don't hold.
    profile: Profile,
    name: Entity<InputState>,
    description: Entity<InputState>,
    instructions: Entity<TextareaState>,
    /// The names the user's other profiles have.
    taken: Vec<String>,
    models: Option<ModelChoices>,
    /// The Model field.
    model_select: Entity<ModelSelect>,
    home: Option<PathBuf>,
    /// Why what was typed isn't a profile, from Save.
    error: Option<SharedString>,
    busy: bool,
    _subscriptions: Vec<Subscription>,
}

impl ProfileForm {
    /// Opens the editor in a dialog, on `entry` or on a new profile.
    #[allow(clippy::too_many_arguments)]
    fn open(
        page: Entity<ProfilesPage>,
        entry: Option<Entry>,
        taken: Vec<String>,
        models: Option<ModelChoices>,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let title = match &entry {
            Some(entry) => format!("Edit the profile {}", entry.profile.name),
            None => "Add a worker profile".to_string(),
        };
        let form = cx.new(|cx| ProfileForm::new(page, entry, taken, models, home, window, cx));
        let name = form.read(cx).name.clone();
        let (content, saving) = (form.clone(), form.clone());
        window.open_dialog(cx, move |dialog, _, _| {
            let form = content.clone();
            let saving = saving.clone();
            dialog
                .w(px(560.))
                // Enter in a field saves, as Save does. Instructions take
                // Enter as a new line.
                .on_ok(move |_, window, cx| {
                    saving.update(cx, |form, cx| form.save(window, cx));
                    false
                })
                .title(
                    h_flex()
                        .gap_2()
                        .child(gpui_kit::component::Icon::new(
                            gpui_kit::assets::IconName::UserCog,
                        ))
                        .child(title.clone()),
                )
                .content(move |content, _, _| content.child(form.clone()))
        });
        name.update(cx, |name, cx| name.focus(window, cx));
        form
    }

    #[allow(clippy::too_many_arguments)]
    fn new(
        page: Entity<ProfilesPage>,
        entry: Option<Entry>,
        taken: Vec<String>,
        models: Option<ModelChoices>,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let profile = entry
            .as_ref()
            .map(|entry| entry.profile.clone())
            .unwrap_or_else(|| Profile::new("", ""));
        let input = prefs::get(cx).input.clone();
        let name = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("3d-judge")
                .default_value(profile.name.clone())
        });
        let description = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Judges 3D results against a reference image")
                .default_value(profile.description.clone())
        });
        let instructions = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(5, 12)
                .placeholder("What every worker started with this profile is told.")
                .keymap(crate::composer::text_keymap(input.keymap))
                .default_value(profile.instructions.clone())
        });
        let subscriptions = vec![
            cx.subscribe(&name, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&description, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
            cx.subscribe(&instructions, |_, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }),
        ];
        let form = cx.entity().downgrade();
        let choosing = form.clone();
        let model_select = cx.new(|cx| {
            let mut select = ModelSelect::new(
                "profile-model",
                "Model",
                Rc::new(move |cx: &App| {
                    form.upgrade()
                        .map(|form| {
                            let form = form.read(cx);
                            form.model_value(form.profile.model.as_deref())
                        })
                        .unwrap_or_default()
                }),
                Rc::new(move |model, _, cx| {
                    choosing
                        .update(cx, |form, cx| {
                            form.profile.model = model;
                            cx.notify();
                        })
                        .ok();
                }),
                window,
                cx,
            );
            select.set_models("The session’s worker model", models.clone(), cx);
            select.full_width()
        });
        Self {
            page,
            original: entry,
            profile,
            name,
            description,
            instructions,
            taken,
            models,
            model_select,
            home,
            error: None,
            busy: false,
            _subscriptions: subscriptions,
        }
    }

    /// The fields typed into: name, description and instructions (tests).
    #[cfg(test)]
    pub fn inputs(
        &self,
    ) -> (
        Entity<InputState>,
        Entity<InputState>,
        Entity<TextareaState>,
    ) {
        (
            self.name.clone(),
            self.description.clone(),
            self.instructions.clone(),
        )
    }

    /// The profile as the form shows it now.
    pub fn draft(&self, cx: &App) -> Profile {
        let mut profile = self.profile.clone();
        profile.name = self.name.read(cx).value().trim().to_string();
        profile.description = self.description.read(cx).value().trim().to_string();
        profile.instructions = self.instructions.read(cx).value().to_string();
        profile
    }

    /// The name the profile had when the editor opened, `None` for a new
    /// one.
    fn replacing(&self) -> Option<&str> {
        self.original
            .as_ref()
            .map(|entry| entry.profile.name.as_str())
    }

    /// Why what is typed as the name can't be one, if it can't.
    pub fn name_problem(&self, cx: &App) -> Option<String> {
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return None;
        }
        if let Err(why) = profiles::check_name(&name) {
            return Some(curly(&why));
        }
        (self.replacing() != Some(name.as_str()) && self.taken.contains(&name))
            .then(|| format!("There is already a profile called {name}. Choose another name."))
    }

    /// Saves the profile, off the UI thread, and closes the editor when it
    /// is saved. Arko's reason shows when it isn't.
    pub fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // Saving already (Enter pressed again).
        if self.busy {
            return;
        }
        let profile = self.draft(cx);
        let replacing = self.replacing().map(str::to_string);
        let taken = self.taken.clone();
        if let Err(why) = store::check(&profile, replacing.as_deref(), |name| {
            taken.iter().any(|taken| taken == name)
        }) {
            self.error = Some(curly(&why).into());
            cx.notify();
            return;
        }
        self.busy = true;
        self.error = None;
        cx.notify();
        let backend = cx.global::<SharedBackend>().clone();
        let name = profile.name.clone();
        let saving = cx
            .background_spawn(async move { backend.save_profile(&profile, replacing.as_deref()) });
        cx.spawn_in(window, async move |this, cx| {
            let saved = saving.await;
            this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match saved {
                    Ok(_) => {
                        window.close_dialog(cx);
                        window.push_notification(format!("Saved the profile {name}."), cx);
                        this.page.update(cx, |page, cx| page.refresh(cx));
                    }
                    Err(why) => {
                        this.error = Some(curly(&why).into());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    /// Closes the editor and opens another on a copy of the profile as
    /// shown, with a name no profile has.
    fn duplicate(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut copy = self.draft(cx);
        let taken = self.taken.clone();
        copy.name = store::copy_name(&copy.name, |name| taken.iter().any(|taken| taken == name));
        copy.made_by = None;
        let entry = Entry {
            file: PathBuf::new(),
            note: None,
            profile: copy,
        };
        let page = self.page.clone();
        window.close_dialog(cx);
        // A new profile that starts as the copy: no file, no old name.
        let (names, models, home) = (self.taken.clone(), self.models.clone(), self.home.clone());
        let form = ProfileForm::open_copy(page.clone(), entry, names, models, home, window, cx);
        page.update(cx, |page, _| page.editor = Some(form.downgrade()));
    }

    /// Opens an editor for a new profile that starts as `copy`.
    #[allow(clippy::too_many_arguments)]
    fn open_copy(
        page: Entity<ProfilesPage>,
        copy: Entry,
        taken: Vec<String>,
        models: Option<ModelChoices>,
        home: Option<PathBuf>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let form = Self::open(page, None, taken, models, home, window, cx);
        let profile = copy.profile;
        form.update(cx, |form, cx| {
            form.profile = profile.clone();
            form.name.update(cx, |name, cx| {
                name.set_value(profile.name.clone(), window, cx)
            });
            form.description.update(cx, |description, cx| {
                description.set_value(profile.description.clone(), window, cx)
            });
            form.instructions.update(cx, |instructions, cx| {
                instructions.set_value(profile.instructions.clone(), window, cx)
            });
            cx.notify();
        });
        form
    }

    /// Asks before deleting the profile being edited.
    fn delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.original.clone() else {
            return;
        };
        let page = self.page.clone();
        window.close_dialog(cx);
        ProfilesPage::confirm_delete(&page, &entry, window, cx);
    }

    /// A dropdown over `options` (value, label), showing the label of
    /// `current`, that calls `pick` with the value chosen.
    fn dropdown(
        id: &'static str,
        what: &'static str,
        options: Vec<(SharedString, SharedString)>,
        current: SharedString,
        pick: impl Fn(&SharedString, &mut Window, &mut App) + 'static,
    ) -> impl IntoElement {
        let label = options
            .iter()
            .find(|(value, _)| *value == current)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| current.clone());
        let pick = std::rc::Rc::new(pick);
        Button::new(id)
            .outline()
            .small()
            .w_full()
            .label(label.clone())
            .accessibility_label(format!("{what}, {label}"))
            .dropdown_caret(true)
            .dropdown_menu(move |menu, _, _| {
                let menu = options.iter().fold(menu, |menu, (value, label)| {
                    let (value, pick) = (value.clone(), pick.clone());
                    menu.item(
                        PopupMenuItem::new(label.clone())
                            .checked(value == current)
                            .on_click(move |_, window, cx| pick(&value, window, cx)),
                    )
                });
                menu.scrollable(true)
            })
    }

    /// The model the profile names, as the list names it: a model written
    /// as its alias (`sol`) is the list's `gpt-6.1-sol`. A model the list
    /// doesn't have stays as written.
    fn model_value(&self, model: Option<&str>) -> SharedString {
        let Some(model) = model else {
            return SharedString::default();
        };
        self.models
            .iter()
            .flat_map(|models| &models.groups)
            .flat_map(|group| &group.models)
            .find(|choice| choice.name == model || choice.alias == model)
            .map(|choice| SharedString::from(choice.name.clone()))
            .unwrap_or_else(|| SharedString::from(model.to_string()))
    }
}

/// A field's caption.
fn caption(text: &'static str, cx: &App) -> Div {
    div()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A line of muted help or a problem under a field.
fn note(id: &'static str, text: impl Into<SharedString>, color: Hsla) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
        .text_xs()
        .text_color(color)
        .child(text.into())
}

/// What the editor says under its fields.
const FAST_ABOUT: &str = "For models that have it. On a model other than the session’s worker model, a worker is fast only when this is on.";
const CACHE_ABOUT: &str = "Long keeps it an hour on Claude models and 12 hours on OpenAI models with an API key, instead of 5 or 30 minutes. Other models keep their own.";
const NESTED_ABOUT: &str = "Down to the session’s worker depth.";
const INSTRUCTIONS_ABOUT: &str =
    "Every worker started with this profile gets these, after the project’s instructions.";

impl ProfileForm {
    /// A dropdown over a prompt cache lifetime.
    fn cache_dropdown(&self, cache: Option<Cache>, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        Self::dropdown(
            "profile-cache",
            "Prompt cache",
            vec![
                ("".into(), "The session’s choice".into()),
                (Cache::FiveMinutes.as_str().into(), "Short".into()),
                (Cache::OneHour.as_str().into(), "Long".into()),
            ],
            SharedString::from(cache.map(Cache::as_str).unwrap_or_default()),
            move |value, _, cx| {
                let value = value.clone();
                view.update(cx, |form, cx| {
                    form.profile.cache = value.parse().ok();
                    cx.notify();
                })
                .ok();
            },
        )
    }

    /// The effort's dropdown, over the efforts the model takes.
    fn effort_dropdown(&self, draft: &Profile, cx: &mut Context<Self>) -> impl IntoElement {
        let view = cx.entity().downgrade();
        let chosen = draft.effort.map(|effort| effort.to_string());
        Self::dropdown(
            "profile-effort",
            "Effort",
            effort_options(
                "The session’s worker effort",
                draft.model.as_deref(),
                chosen.as_deref(),
            ),
            SharedString::from(chosen.unwrap_or_default()),
            move |value, _, cx| {
                let value = value.clone();
                view.update(cx, |form, cx| {
                    form.profile.effort = value.parse().ok();
                    cx.notify();
                })
                .ok();
            },
        )
    }

    /// A switch for a flag of the profile.
    fn flag(
        &self,
        id: &'static str,
        label: &'static str,
        on: bool,
        set: fn(&mut Profile, bool),
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().downgrade();
        Switch::new(id)
            .small()
            .label(label)
            .checked(on)
            .on_click(move |checked, _, cx| {
                let checked = *checked;
                view.update(cx, |form, cx| {
                    set(&mut form.profile, checked);
                    cx.notify();
                })
                .ok();
            })
    }

    /// What the model chosen needs that isn't there, as the editor says it.
    fn needs_words(&self, model: Option<&str>) -> Option<String> {
        let model = model?;
        let needs = needs_of(self.models.as_ref()?, model)?;
        let name = arko::models::display_name(model);
        let page = super::PAGES[super::ACCOUNTS_PAGE];
        Some(format!("{name} needs {needs}. Add it under {page}."))
    }

    /// The fields, which scroll when the window is too short for them.
    fn fields(&self, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let draft = self.draft(cx);
        let length = draft.instructions.trim().chars().count();
        let counter_color = if length > MAX_INSTRUCTIONS {
            danger
        } else {
            muted
        };
        let counter = format!("{} of {}", grouped(length), grouped(MAX_INSTRUCTIONS));
        let description_length = draft.description.chars().count();
        let file = self
            .original
            .as_ref()
            .map(|entry| tilde(&entry.file, self.home.as_deref()));
        let name = v_flex()
            .gap_1()
            .child(caption("Name", cx))
            .child(Input::new(&self.name).small().aria_label("Name"))
            .when_some(self.name_problem(cx), |this, problem| {
                this.child(note("profile-name-problem", problem, danger))
            });
        let description = v_flex()
            .gap_1()
            .child(caption("Description", cx))
            .child(
                Input::new(&self.description)
                    .small()
                    .aria_label("Description"),
            )
            .when(description_length > MAX_DESCRIPTION, |this| {
                let words =
                    format!("{description_length} characters, and the limit is {MAX_DESCRIPTION}.");
                this.child(note("profile-description-problem", words, danger))
            });
        let model = v_flex()
            .gap_1()
            .child(caption("Model", cx))
            .child(self.model_select.clone())
            .when_some(self.needs_words(draft.model.as_deref()), |this, words| {
                this.child(note("profile-model-needs", words, muted))
            });
        let effort = v_flex()
            .gap_1()
            .child(caption("Effort", cx))
            .child(self.effort_dropdown(&draft, cx))
            .when_some(draft.model_problem(), |this, problem| {
                this.child(note("profile-effort-problem", curly(&problem), danger))
            });
        let fast = v_flex()
            .gap_1()
            .child(self.flag(
                "profile-fast",
                "Fast mode",
                draft.fast,
                |p, on| p.fast = on,
                cx,
            ))
            .child(note("profile-fast-about", FAST_ABOUT, muted));
        let cache = v_flex()
            .gap_1()
            .child(caption("Prompt cache", cx))
            .child(self.cache_dropdown(draft.cache, cx))
            .child(note("profile-cache-about", CACHE_ABOUT, muted));
        let nested = v_flex()
            .gap_1()
            .child(self.flag(
                "profile-nested",
                "Can start workers",
                draft.nested,
                |p, on| p.nested = on,
                cx,
            ))
            .child(note("profile-nested-about", NESTED_ABOUT, muted));
        let instructions = v_flex()
            .gap_1()
            .child(caption("Instructions", cx))
            .child(
                div()
                    .id("profile-instructions")
                    .test_support()
                    .w_full()
                    .child(Textarea::new(&self.instructions).aria_label("Instructions")),
            )
            .child(
                h_flex()
                    .items_start()
                    .justify_between()
                    .gap_2()
                    .child(div().flex_1().min_w_0().child(note(
                        "profile-instructions-about",
                        INSTRUCTIONS_ABOUT,
                        muted,
                    )))
                    .child(
                        div()
                            .id("profile-instructions-length")
                            .test_support()
                            .flex_none()
                            .text_xs()
                            .text_color(counter_color)
                            .child(counter),
                    ),
            );
        v_flex()
            .id("profile-fields")
            .test_support()
            .gap_3()
            .max_h(window.viewport_size().height * 0.7)
            .overflow_y_scroll()
            .pr_1()
            .when_some(file, |this, file| {
                this.child(note("profile-file", format!("Saved in {file}."), muted))
            })
            .child(name)
            .child(description)
            .child(model)
            .child(effort)
            .child(fast)
            .child(cache)
            .child(nested)
            .child(instructions)
    }
}

impl Render for ProfileForm {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let danger = cx.theme().danger;
        let existing = self.original.is_some();
        let delete = Button::new("profile-delete")
            .ghost()
            .small()
            .danger()
            .icon(gpui_kit::assets::IconName::Trash)
            .label("Delete")
            .on_click(cx.listener(|this, _, window, cx| this.delete(window, cx)));
        let duplicate = Button::new("profile-duplicate")
            .ghost()
            .small()
            .icon(IconName::Copy)
            .label("Duplicate")
            .disabled(!existing)
            .on_click(cx.listener(|this, _, window, cx| this.duplicate(window, cx)));
        let cancel = Button::new("profile-cancel")
            .ghost()
            .small()
            .label("Cancel")
            .on_click(|_, window, cx| window.close_dialog(cx));
        let save = Button::new("profile-save")
            .primary()
            .small()
            .label("Save")
            .loading(self.busy)
            .on_click(cx.listener(|this, _, window, cx| this.save(window, cx)));
        v_flex()
            .id("profile-form")
            .test_support()
            .gap_3()
            .child(self.fields(window, cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(note("profile-error", error, danger))
            })
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(h_flex().gap_2().when(existing, |this| this.child(delete)))
                    .child(h_flex().gap_2().child(duplicate).child(cancel).child(save)),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{grouped, with_stop};

    #[test]
    fn a_number_reads_with_commas() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(812), "812");
        assert_eq!(grouped(4000), "4,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn a_description_gets_one_full_stop() {
        assert_eq!(with_stop("Judges 3D results"), "Judges 3D results.");
        assert_eq!(with_stop("  Reviews a diff.  "), "Reviews a diff.");
        assert_eq!(with_stop("Why?"), "Why?");
    }
}
