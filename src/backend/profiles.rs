//! Worker profiles as Settings lists and changes them: the user's, in Arko's
//! folder, and each recent project's and repository's, read only, with the
//! choices about where profiles are kept. The files are Arko's
//! (`arko::profiles` reads, checks and writes them), so the terminal app and
//! the sessions see what Settings saves.

use std::path::{Path, PathBuf};

use arko::profiles::{self, Author, Place, Profile, Roots};

/// A profile that was read, with the file it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub profile: Profile,
    pub file: PathBuf,
    /// Why a worker can't start with it as it is (a model Arko doesn't
    /// know, or an effort the model doesn't take).
    pub note: Option<String>,
}

/// A profile file that couldn't be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unreadable {
    pub file: PathBuf,
    pub reason: String,
}

/// The profiles in one folder.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct List {
    pub entries: Vec<Entry>,
    pub unreadable: Vec<Unreadable>,
}

impl List {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.unreadable.is_empty()
    }

    /// The profile called `name`.
    pub fn get(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.profile.name == name)
    }
}

/// A project's own profiles, or those in a repository's root.
#[derive(Debug, Clone, PartialEq)]
pub struct Project {
    pub folder: PathBuf,
    pub list: List,
}

/// The profiles Settings shows.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Profiles {
    /// The user's, in `profiles` in Arko's folder.
    pub user: List,
    /// The projects that have some.
    pub projects: Vec<Project>,
    /// The roots of the git repositories that hold some, which the projects
    /// in their subfolders read after their own. Each shows once. None while
    /// Arko is told not to read them.
    pub repositories: Vec<Project>,
    /// Where profiles are kept, as the user chose for every project.
    pub preferences: profiles::Preferences,
}

/// Reads the user's profiles and those of the projects in `folders`, without
/// a session. It reads files: call it off the UI thread.
pub fn read(root: &Path, folders: &[PathBuf]) -> Profiles {
    let user = list_of(&profiles::list(&Roots::new(root)), Place::User);
    let mut projects: Vec<Project> = Vec::new();
    let mut repositories: Vec<Project> = Vec::new();
    for folder in folders {
        if projects.iter().any(|project| &project.folder == folder) {
            continue;
        }
        let roots = Roots::new(root).workspace(folder);
        let listed = profiles::list(&roots);
        let list = list_of(&listed, Place::Project);
        if !list.is_empty() {
            projects.push(Project {
                folder: folder.clone(),
                list,
            });
        }
        // The repository's root, two folders above its `.agents/profiles`.
        let repository = roots
            .folder(Place::Repository)
            .and_then(|profiles| profiles.ancestors().nth(2).map(Path::to_path_buf));
        if let Some(top) = repository
            && !repositories.iter().any(|known| known.folder == top)
        {
            let list = list_of(&listed, Place::Repository);
            if !list.is_empty() {
                repositories.push(Project { folder: top, list });
            }
        }
    }
    Profiles {
        user,
        projects,
        repositories,
        preferences: profiles::Preferences::load(root),
    }
}

/// Saves where worker profiles are kept, for every project. Arko reads the
/// choice each time, so the next worker started follows it.
pub fn set_preferences(root: &Path, preferences: profiles::Preferences) -> Result<(), String> {
    preferences.save(root)
}

/// What `listed` has from `place`. (A project's profile always wins over the
/// user's of the same name, so it is never among the hidden.)
fn list_of(listed: &profiles::Profiles, place: Place) -> List {
    List {
        entries: listed
            .active
            .iter()
            .filter(|found| found.place == place)
            .map(|found| Entry {
                profile: found.profile.clone(),
                file: found.file.clone(),
                note: found.note.clone(),
            })
            .collect(),
        unreadable: listed
            .unreadable
            .iter()
            .filter(|problem| problem.place == place)
            .map(|problem| Unreadable {
                file: problem.file.clone(),
                reason: problem.reason.clone(),
            })
            .collect(),
    }
}

/// Why `profile` can't be saved as `replacing` (the name it had, for an
/// edit, `None` for a new one), or nothing. A profile can be saved when
/// Arko accepts it and no other profile is called the same, which `taken`
/// says of a name.
pub fn check(
    profile: &Profile,
    replacing: Option<&str>,
    taken: impl Fn(&str) -> bool,
) -> Result<(), String> {
    profile.check()?;
    if replacing != Some(profile.name.as_str()) && taken(&profile.name) {
        return Err(format!(
            "There is already a profile called {}. Choose another name.",
            profile.name
        ));
    }
    Ok(())
}

/// Saves `profile` for every project, the user's own. An edit that changed
/// the name (`replacing` is the old one) takes the old file away once the
/// new one is written. Returns the file written. It reads and writes files:
/// call it off the UI thread.
pub fn save(root: &Path, profile: &Profile, replacing: Option<&str>) -> Result<PathBuf, String> {
    let roots = Roots::new(root);
    // A file that can't be read keeps its name too.
    let current = list_of(&profiles::list(&roots), Place::User);
    let taken = |name: &str| {
        current.get(name).is_some()
            || current
                .unreadable
                .iter()
                .any(|problem| problem.file.file_stem().is_some_and(|stem| stem == name))
    };
    check(profile, replacing, taken)?;
    let saved = profiles::save(&roots, Place::User, profile, Author::User)?;
    if let Some(old) = replacing.filter(|old| *old != profile.name) {
        // The new file is there: a failure to remove the old one is
        // reported, and the profile saved stays.
        profiles::delete(&roots, Place::User, old).map_err(|why| {
            format!(
                "Saved {}, but couldn’t remove the profile {old}. {why}",
                profile.name
            )
        })?;
    }
    Ok(saved.file)
}

/// Deletes the user's profile `name`. Returns the file removed.
pub fn delete(root: &Path, name: &str) -> Result<PathBuf, String> {
    profiles::delete(&Roots::new(root), Place::User, name)
}

/// A name for a copy of the profile `name` that none of `taken` has: `name-copy`,
/// then `name-copy-2` and on, cut to fit a profile name.
pub fn copy_name(name: &str, taken: impl Fn(&str) -> bool) -> String {
    let fit = |base: &str, suffix: &str| {
        let room = profiles::MAX_NAME.saturating_sub(suffix.chars().count());
        let base: String = base.chars().take(room).collect();
        format!("{}{suffix}", base.trim_end_matches('-'))
    };
    let first = fit(name, "-copy");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|number| fit(name, &format!("-copy-{number}")))
        .find(|candidate| !taken(candidate))
        .unwrap_or(first)
}

/// Profiles for tests and screenshots.
impl Profiles {
    /// A user's three profiles and none in the projects.
    pub fn sample(arko: &Path) -> Self {
        let folder = arko.join("profiles");
        let entry = |name: &str,
                     description: &str,
                     model: Option<&str>,
                     effort: Option<arko::Effort>,
                     fast: bool,
                     instructions: &str| {
            let mut profile = Profile::new(name, description);
            profile.model = model.map(str::to_string);
            profile.effort = effort;
            profile.fast = fast;
            profile.instructions = instructions.to_string();
            let note = profile.model_problem();
            Entry {
                file: folder.join(Profile::file_name(name)),
                profile,
                note,
            }
        };
        Profiles {
            user: List {
                entries: vec![
                    entry(
                        "3d-judge",
                        "Judges 3D results against a reference image",
                        Some("sol"),
                        Some(arko::Effort::High),
                        false,
                        "You judge 3D models. Compare the render with the reference image on shape, proportions and materials. Say what is wrong, in order of how much it matters.",
                    ),
                    entry(
                        "quick-reader",
                        "Reads files and answers in a line",
                        Some("haiku"),
                        None,
                        true,
                        "Read what you are asked to and answer in one line.",
                    ),
                    entry("reviewer", "Reviews a diff for bugs", None, None, false, ""),
                ],
                unreadable: Vec::new(),
            },
            projects: Vec::new(),
            repositories: Vec::new(),
            preferences: profiles::Preferences::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str, description: &str) -> Profile {
        let mut profile = Profile::new(name, description);
        profile.model = Some("sol".into());
        profile.effort = Some(arko::Effort::High);
        profile.instructions = "Judge the render.\nBe brief.".into();
        profile
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    /// A profile saved in Settings is a file Arko's own reader finds, with
    /// every field, and the user's list and a project's list are each
    /// read from their own folder.
    #[test]
    fn a_saved_profile_is_the_file_arko_reads() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        let mut judge = profile("3d-judge", "Judges 3D results");
        judge.fast = true;
        judge.cache = Some(profiles::Cache::OneHour);
        let file = save(&root, &judge, None).unwrap();
        assert_eq!(file, root.join("profiles/3d-judge.md"));
        let listed = read(&root, std::slice::from_ref(&project));
        assert_eq!(listed.user.entries.len(), 1);
        let entry = &listed.user.entries[0];
        assert_eq!(entry.profile, judge);
        assert_eq!(entry.file, file);
        assert_eq!(entry.note, None);
        assert!(listed.projects.is_empty());
        // A project's profiles list apart, and none of them is the user's.
        write(
            &project.join(".agents/profiles/lint.md"),
            "---\ndescription: Runs the linter\nmodel: haiku\n---\nRun it.",
        );
        write(
            &project.join(".agents/profiles/broken.md"),
            "no front matter",
        );
        let listed = read(&root, &[project.clone(), project.clone()]);
        assert_eq!(listed.user.entries.len(), 1);
        assert_eq!(listed.projects.len(), 1, "a folder listed twice shows once");
        let theirs = &listed.projects[0];
        assert_eq!(theirs.folder, project);
        assert_eq!(theirs.list.entries[0].profile.name, "lint");
        assert_eq!(theirs.list.unreadable.len(), 1);
        assert!(theirs.list.unreadable[0].file.ends_with("broken.md"));
    }

    /// The profiles in a repository's root list once, however many of its
    /// folders are recent projects, and not while Arko is told not to read
    /// them. The choices read back as saved.
    #[test]
    fn a_repositorys_profiles_list_once_and_follow_the_choice() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let repository = temp.path().join("repo");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        write(
            &repository.join(".agents/profiles/lint.md"),
            "---\ndescription: Runs the linter\n---\nRun it.",
        );
        let (app, docs) = (repository.join("app"), repository.join("docs"));
        let listed = read(&root, &[app.clone(), docs]);
        assert!(listed.projects.is_empty());
        assert_eq!(listed.repositories.len(), 1);
        assert_eq!(listed.repositories[0].folder, repository);
        assert_eq!(listed.repositories[0].list.entries[0].profile.name, "lint");
        assert_eq!(listed.preferences, profiles::Preferences::default());
        let off = profiles::Preferences {
            keep_out_of_git: false,
            repo_root: false,
        };
        set_preferences(&root, off).unwrap();
        let listed = read(&root, &[app]);
        assert!(listed.repositories.is_empty());
        assert_eq!(listed.preferences, off);
    }

    #[test]
    fn a_profile_arko_wouldnt_take_is_refused_with_its_reason() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        let bad = profile("Bad Name", "x");
        let why = save(root, &bad, None).unwrap_err();
        assert!(why.contains("isn't a profile name"), "{why}");
        let mut long = profile("long", "x");
        long.instructions = "a".repeat(profiles::MAX_INSTRUCTIONS + 1);
        let why = save(root, &long, None).unwrap_err();
        assert!(why.contains("the limit is 4,000"), "{why}");
        let mut effort = profile("effort", "x");
        effort.model = Some("gemini".into());
        effort.effort = Some(arko::Effort::Max);
        assert!(save(root, &effort, None).unwrap_err().contains("takes"));
        assert!(!root.join("profiles").exists(), "nothing was written");
    }

    /// A new profile never takes the place of one with its name, and an edit
    /// that renames one moves its file.
    #[test]
    fn names_are_kept_apart_and_a_rename_moves_the_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        save(root, &profile("one", "First"), None).unwrap();
        save(root, &profile("two", "Second"), None).unwrap();
        let why = save(root, &profile("one", "Again"), None).unwrap_err();
        assert!(why.contains("already a profile called one"), "{why}");
        // Editing one keeps its name.
        save(root, &profile("one", "First, edited"), Some("one")).unwrap();
        // A rename onto another's name is refused, and onto a free one moves.
        let why = save(root, &profile("two", "Renamed"), Some("one")).unwrap_err();
        assert!(why.contains("already a profile called two"), "{why}");
        save(root, &profile("three", "First, renamed"), Some("one")).unwrap();
        let listed = read(root, &[]);
        let names: Vec<&str> = listed
            .user
            .entries
            .iter()
            .map(|entry| entry.profile.name.as_str())
            .collect();
        assert_eq!(names, ["three", "two"]);
        assert!(!root.join("profiles/one.md").exists());
        // A file that can't be read keeps its name too.
        write(&root.join("profiles/torn.md"), "no front matter");
        let why = save(root, &profile("torn", "Mine"), None).unwrap_err();
        assert!(why.contains("already a profile called torn"), "{why}");
        assert_eq!(
            std::fs::read_to_string(root.join("profiles/torn.md")).unwrap(),
            "no front matter"
        );
    }

    /// Saving as the user takes the agent's mark off, and keeps a key this
    /// version doesn't know.
    #[test]
    fn editing_a_profile_keeps_what_the_form_doesnt_show() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write(
            &root.join("profiles/scout.md"),
            "---\ndescription: Looks around\nmade_by: agent\nisolated: true\nnested: true\n---\nLook.",
        );
        let listed = read(root, &[]);
        let mut scout = listed.user.entries[0].profile.clone();
        assert!(scout.made_by_agent());
        scout.description = "Looks around quickly".into();
        save(root, &scout, Some("scout")).unwrap();
        let text = std::fs::read_to_string(root.join("profiles/scout.md")).unwrap();
        assert!(text.contains("description: Looks around quickly"), "{text}");
        assert!(text.contains("isolated: true"), "{text}");
        assert!(text.contains("nested: true"), "{text}");
        assert!(!text.contains("made_by"), "{text}");
    }

    #[test]
    fn a_profile_is_deleted_with_its_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        save(root, &profile("one", "First"), None).unwrap();
        let file = delete(root, "one").unwrap();
        assert_eq!(file, root.join("profiles/one.md"));
        assert!(read(root, &[]).user.is_empty());
        let why = delete(root, "one").unwrap_err();
        assert!(why.contains("There is no profile `one`"), "{why}");
    }

    #[test]
    fn a_copy_gets_a_name_no_profile_has() {
        let taken = |names: &'static [&'static str]| move |name: &str| names.contains(&name);
        assert_eq!(copy_name("reviewer", taken(&[])), "reviewer-copy");
        assert_eq!(
            copy_name("reviewer", taken(&["reviewer-copy"])),
            "reviewer-copy-2"
        );
        assert_eq!(
            copy_name("reviewer", taken(&["reviewer-copy", "reviewer-copy-2"])),
            "reviewer-copy-3"
        );
        let long = "a".repeat(40);
        let copy = copy_name(&long, taken(&[]));
        assert!(copy.chars().count() <= profiles::MAX_NAME, "{copy}");
        assert!(copy.ends_with("-copy"));
        assert!(profiles::check_name(&copy).is_ok());
    }
}
