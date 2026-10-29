//! Extensions as the engine lists them, grouped for Settings. Approval
//! answers are kept separately so Settings can offer Forget answer.

use std::path::{Path, PathBuf};

use arko::agent_added::{ServerRecord, SkillRecord};
use arko::extension_prefs::ExtensionPrefs;

pub const USER_MCP_FILE: &str = "mcp.json";
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
/// The project's own servers, which agents add for the project.
pub const LOCAL_MCP_FILE: &str = ".arko/mcp.json";
const PROJECT_SKILL_FOLDERS: [&str; 4] = [
    ".agents/skills",
    ".claude/skills",
    ".skills",
    ".arko/skills",
];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extensions {
    pub files: Vec<McpFile>,
    pub projects: Vec<ProjectServers>,
    pub skills: Vec<SkillFolder>,
    /// The skills built into Arko, which belong to no folder.
    pub builtin: Vec<BuiltinSkill>,
    /// What agents may add without asking, as Arko's `extensions.json`
    /// says.
    pub approvals: ExtensionPrefs,
}

#[derive(Debug, Clone, PartialEq)]
pub struct McpFile {
    pub path: PathBuf,
    pub arko: bool,
    pub servers: Option<Result<Vec<McpServer>, String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    pub command: String,
    pub skipped: Option<String>,
    /// When and why an agent added it, if one did.
    pub added: Option<ServerRecord>,
}

/// A switch of what agents may add, as `/extensions` names them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// "Ask before adding MCP servers".
    AskMcp,
    /// "Review project skills".
    ReviewSkills,
    /// "Keep agents' project files out of git".
    KeepOutOfGit,
}

/// Saves `switch` as `/extensions` does in a session: in Arko's folder
/// `root`, and for the git switch also in the `.arko` folder of each project
/// of `workspaces`, which adds or removes Arko's `.gitignore` there. The
/// sessions open then read it again with `Details::refresh_settings`.
pub fn set_switch(
    root: &Path,
    workspaces: &[PathBuf],
    switch: Switch,
    on: bool,
) -> Result<(), String> {
    use arko::extension_prefs::update;
    match switch {
        Switch::AskMcp => update(root, |prefs| prefs.ask_before_adding_mcp = on).map(|_| ()),
        Switch::ReviewSkills => update(root, |prefs| prefs.review_project_skills = on).map(|_| ()),
        Switch::KeepOutOfGit => {
            let mut prefs = arko::profiles::Preferences::load(root);
            prefs.keep_out_of_git = on;
            let (first, rest) = match workspaces.split_first() {
                Some((first, rest)) => (Some(first.as_path()), rest),
                None => (None, workspaces),
            };
            super::profiles::set_preferences(root, first, prefs)?;
            for workspace in rest {
                arko::arko_dir::apply_setting(workspace, on)?;
            }
            Ok(())
        }
    }
}

/// "Added by an agent on 28 Oct, for this project." and the agent's reason,
/// for a skill or a server an agent added. `project` says it is for one
/// project, else for every project.
pub fn added_words(added: &chrono::DateTime<chrono::Utc>, project: bool, why: &str) -> String {
    let day = added.with_timezone(&chrono::Local).format("%-d %b");
    let reach = if project {
        "this project"
    } else {
        "every project"
    };
    let why = why.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut words = format!("Added by an agent on {day}, for {reach}.");
    if !why.is_empty() {
        words.push(' ');
        words.push_str(&why);
        if !why.ends_with(['.', '!', '?']) {
            words.push('.');
        }
    }
    words
}

/// [`added_words`] for a skill.
pub fn skill_added_words(record: &SkillRecord) -> String {
    added_words(
        &record.added,
        record.scope == arko::agent_added::Scope::Project,
        &record.why,
    )
}

/// [`added_words`] for a server, which is for one project when it is in a
/// project's `.arko/mcp.json`.
pub fn server_added_words(record: &ServerRecord) -> String {
    added_words(
        &record.added,
        record.file.ends_with(LOCAL_MCP_FILE),
        &record.why,
    )
}

/// What `/skills` adds to a skill's place: that an agent added it, the
/// models it is listed for, and that only the main agent gets it, as in
/// "added by the agent 28 Oct, for haiku, main agent only". Empty for a skill
/// with none of them.
pub fn skill_notes(added: Option<&SkillRecord>, models: &[String], workers: bool) -> Vec<String> {
    let mut notes = Vec::new();
    if let Some(added) = added {
        notes.push(format!(
            "added by the agent {}",
            added.added.with_timezone(&chrono::Local).format("%-d %b")
        ));
    }
    if !models.is_empty() {
        notes.push(format!("for {}", and_list(models)));
    }
    if !workers {
        notes.push("main agent only".to_string());
    }
    notes
}

/// "a", "a and b", "a, b and c".
pub fn and_list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Refused,
    NotAsked,
    /// The engine could not read the answer. The servers carry its error.
    Unavailable,
}

/// A project's `.mcp.json`, or its own `.arko/mcp.json`, with the answer
/// Arko remembers for it.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectServers {
    pub folder: PathBuf,
    /// The file.
    pub file: PathBuf,
    /// It is the project's `.arko/mcp.json` (else its `.mcp.json`).
    pub local: bool,
    pub servers: Result<Vec<McpServer>, String>,
    pub approval: Approval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsOf {
    Project(PathBuf),
    Arko,
    Home,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SkillFolder {
    pub path: PathBuf,
    pub of: SkillsOf,
    pub skills: Vec<arko::extensions::SkillEntry>,
}

/// A skill built into Arko, with the skill folders whose skill of the same
/// name replaces it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuiltinSkill {
    pub name: String,
    pub description: String,
    pub auto_load: bool,
    /// The skills of its name, where each is (its folder or file), in the
    /// order Arko reads them. An empty list means nothing replaces it.
    pub replaced_by: Vec<PathBuf>,
}

/// A skill of a folder, for tests and screenshots.
pub fn sample_skill(folder: &Path, name: &str) -> arko::extensions::SkillEntry {
    arko::extensions::SkillEntry {
        name: name.into(),
        description: String::new(),
        path: folder.join(name),
        auto_load: false,
        skipped: None,
        builtin: false,
        added: None,
        workers: true,
        models: Vec::new(),
    }
}

impl Extensions {
    /// A setup for tests and screenshots.
    pub fn sample(home: &Path, arko: &Path) -> Self {
        let server = |name: &str, command: &str| McpServer {
            name: name.into(),
            command: command.into(),
            skipped: None,
            added: None,
        };
        let project = home.join("src/arko");
        let skills = home.join(".claude/skills");
        Self {
            files: vec![McpFile {
                path: arko.join(USER_MCP_FILE),
                arko: true,
                servers: Some(Ok(vec![server(
                    "github",
                    "npx -y @modelcontextprotocol/server-github",
                )])),
            }],
            projects: vec![ProjectServers {
                file: project.join(PROJECT_MCP_FILE),
                folder: project,
                local: false,
                servers: Ok(vec![server("playwright", "npx @playwright/mcp")]),
                approval: Approval::Approved,
            }],
            skills: vec![SkillFolder {
                skills: ["docx", "pdf", "repo-docs-style"]
                    .into_iter()
                    .map(|name| sample_skill(&skills, name))
                    .collect(),
                path: skills,
                of: SkillsOf::Home,
            }],
            builtin: vec![
                BuiltinSkill {
                    name: "user-facing-prose".into(),
                    description: "How to write any text a person reads, such as UI text, messages, docs and commit messages. Load it before writing or reviewing any of these.".into(),
                    auto_load: false,
                    replaced_by: Vec::new(),
                },
                BuiltinSkill {
                    name: "repo-docs-style".into(),
                    description: "Rules for repository documentation, such as READMEs and docs pages. Load it before creating or editing project docs.".into(),
                    auto_load: false,
                    replaced_by: vec![
                        home.join(".claude/skills/repo-docs-style"),
                        home.join("src/arko/.agents/skills/repo-docs-style"),
                    ],
                },
                BuiltinSkill {
                    name: "ponytail".into(),
                    description: "Finish the whole task with the smallest change that fully works, split the work sensibly between workers, and check it in proportion to the risk. Use it for any coding task, for running workers, and when the user asks for the simplest solution.".into(),
                    auto_load: false,
                    replaced_by: Vec::new(),
                },
            ],
            approvals: ExtensionPrefs::default(),
        }
    }

    /// [`sample`](Self::sample) with what agents added on 28 Oct 2026: the
    /// skill release-notes and the server sentry for the project, and the
    /// server figma for every project. The project's skills also hold one
    /// for haiku and one for the main agent only.
    pub fn sample_added(home: &Path, arko: &Path) -> Self {
        use arko::agent_added::Scope;
        let mut sample = Self::sample(home, arko);
        let project = home.join("src/arko");
        let added = chrono::DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
            .unwrap()
            .to_utc();
        let server_record = |file: &Path, name: &str, why: &str| ServerRecord {
            file: file.to_path_buf(),
            name: name.into(),
            session: "2026-10-28-a1b2".into(),
            session_title: Some("Release prep".into()),
            added,
            why: why.into(),
        };
        let user_file = arko.join(USER_MCP_FILE);
        if let Some(Some(Ok(servers))) = sample.files.first_mut().map(|file| file.servers.as_mut())
        {
            servers.push(McpServer {
                name: "figma".into(),
                command: "npx -y figma-developer-mcp".into(),
                skipped: None,
                added: Some(server_record(
                    &user_file,
                    "figma",
                    "Read the design files the issues link to",
                )),
            });
        }
        let local = project.join(LOCAL_MCP_FILE);
        sample.projects.insert(
            0,
            ProjectServers {
                folder: project.clone(),
                file: local.clone(),
                local: true,
                servers: Ok(vec![McpServer {
                    name: "sentry".into(),
                    command: "npx -y @sentry/mcp-server".into(),
                    skipped: None,
                    added: Some(server_record(
                        &local,
                        "sentry",
                        "Read this project’s crash reports",
                    )),
                }]),
                approval: Approval::Approved,
            },
        );
        let folder = project.join(".arko/skills");
        let mut notes = sample_skill(&folder, "release-notes");
        notes.added = Some(SkillRecord {
            path: notes.path.clone(),
            scope: Scope::Project,
            session: "2026-10-28-a1b2".into(),
            session_title: Some("Release prep".into()),
            added,
            why: "The release checklist changed, and later conversations need the new steps."
                .into(),
            source: None,
        });
        let mut quirks = sample_skill(&folder, "haiku-quirks");
        quirks.models = vec!["haiku".into()];
        let mut deploy = sample_skill(&folder, "deploy");
        deploy.workers = false;
        sample.skills.insert(
            0,
            SkillFolder {
                path: folder,
                of: SkillsOf::Project(project),
                skills: vec![notes, quirks, deploy],
            },
        );
        sample
    }
}

/// Reads extensions off the UI thread. The engine owns MCP and skill
/// parsing, precedence, skipped entries and the folders it searches.
pub fn read(root: &Path, folders: &[PathBuf], mcp_config: &[PathBuf]) -> Extensions {
    let mut projects = Vec::new();
    for folder in folders {
        if !projects.contains(folder) {
            projects.push(folder.clone());
        }
    }
    let workspaces = if projects.is_empty() {
        vec![None]
    } else {
        projects.iter().map(|path| Some(path.as_path())).collect()
    };
    let mut found = Extensions::default();
    for (index, workspace) in workspaces.into_iter().enumerate() {
        // Every built-in skill is listed, whatever the sessions get, since
        // the page has a switch for each.
        let listed =
            arko::extensions::list_with(root, workspace, mcp_config, Some(true), Some(&[]));
        if index == 0 {
            let mut files = mcp_config.to_vec();
            let user = root.join(USER_MCP_FILE);
            if !files.contains(&user) {
                files.push(user.clone());
            }
            for path in files {
                if found.files.iter().any(|file| file.path == path) {
                    continue;
                }
                found.files.push(McpFile {
                    servers: (path.is_file() || file_note(&listed, &path).is_some())
                        .then(|| servers_in(&listed, &path)),
                    arko: path == user,
                    path,
                });
            }
        }
        // The project's own servers come first, as sessions read them.
        for (name, local) in [(LOCAL_MCP_FILE, true), (PROJECT_MCP_FILE, false)] {
            let Some(workspace) = workspace else { break };
            let file = workspace.join(name);
            if !file.is_file() {
                continue;
            }
            let mut servers = servers_in(&listed, &file);
            let answer = if local {
                arko::extensions::local_mcp_approval(root, workspace)
            } else {
                arko::extensions::project_mcp_approval(root, workspace)
            };
            let approval = match answer {
                Ok(Some(true)) => Approval::Approved,
                Ok(Some(false)) => Approval::Refused,
                Ok(None) => Approval::NotAsked,
                Err(why) => {
                    servers = Err(why);
                    Approval::Unavailable
                }
            };
            found.projects.push(ProjectServers {
                folder: workspace.to_path_buf(),
                file,
                local,
                servers,
                approval,
            });
        }
        for skill in &listed.skills {
            if !skill.builtin {
                continue;
            }
            // A folder's skill of its name replaces it, in this project or
            // for every project.
            let replacing = if skill.skipped.is_some() {
                listed
                    .skills
                    .iter()
                    .find(|other| {
                        !other.builtin && other.skipped.is_none() && other.name == skill.name
                    })
                    .map(|other| other.path.as_path())
            } else {
                None
            };
            match found
                .builtin
                .iter_mut()
                .find(|seen| seen.name == skill.name)
            {
                Some(seen) => {
                    if let Some(path) = replacing
                        && !seen.replaced_by.iter().any(|known| known == path)
                    {
                        seen.replaced_by.push(path.to_path_buf());
                    }
                }
                None => found.builtin.push(BuiltinSkill {
                    name: skill.name.clone(),
                    description: skill.description.clone(),
                    auto_load: skill.auto_load,
                    replaced_by: replacing.map(Path::to_path_buf).into_iter().collect(),
                }),
            }
        }
        for skill in listed.skills {
            if skill.builtin
                || found
                    .skills
                    .iter()
                    .any(|folder| folder.skills.iter().any(|seen| seen.path == skill.path))
            {
                continue;
            }
            let path = skill.path.parent().unwrap_or(&skill.path).to_path_buf();
            let of = projects
                .iter()
                .find(|project| {
                    PROJECT_SKILL_FOLDERS
                        .iter()
                        .any(|name| path.starts_with(project.join(name)))
                })
                .map(|project| SkillsOf::Project(project.clone()))
                .unwrap_or_else(|| {
                    if path.starts_with(root.join("skills")) {
                        SkillsOf::Arko
                    } else {
                        SkillsOf::Home
                    }
                });
            if let Some(folder) = found.skills.iter_mut().find(|folder| folder.path == path) {
                folder.skills.push(skill);
            } else {
                found.skills.push(SkillFolder {
                    path,
                    of,
                    skills: vec![skill],
                });
            }
        }
    }
    found.approvals = arko::extension_prefs::load(root);
    found
}

fn normal_path(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normal = PathBuf::new();
    for component in absolute.components() {
        if component == std::path::Component::ParentDir {
            normal.pop();
        } else {
            normal.push(component.as_os_str());
        }
    }
    normal
}

fn file_note<'a>(listed: &'a arko::extensions::Extensions, path: &Path) -> Option<&'a str> {
    let path = normal_path(path);
    listed
        .notes
        .iter()
        .find(|note| note.file == path)
        .map(|note| note.message.as_str())
}

fn servers_in(
    listed: &arko::extensions::Extensions,
    path: &Path,
) -> Result<Vec<McpServer>, String> {
    if let Some(note) = file_note(listed, path) {
        return Err(note.to_string());
    }
    let absolute = normal_path(path);
    Ok(listed
        .servers
        .iter()
        .filter(|server| server.file == absolute)
        .map(|server| McpServer {
            name: server.name.clone(),
            command: server.command.clone(),
            skipped: server.skipped.clone(),
            added: server.added.clone(),
        })
        .collect())
}

/// Checks a file with the engine before Add file keeps it in Settings.
pub fn check_mcp_file(path: &Path) -> Result<(), String> {
    let root = path.parent().unwrap_or(Path::new("."));
    let listed = arko::extensions::list(root, None, &[path.to_path_buf()]);
    match file_note(&listed, path) {
        Some(why) => Err(why.to_string()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_engines_skipped_servers_skills_and_notes_are_kept() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        let config = temp.path().join("servers.json");
        write(
            &config,
            r#"{"mcpServers": {"off": {"command": "x", "disabled": true}, "same": {"command": "first"}}}"#,
        );
        write(
            &root.join(USER_MCP_FILE),
            r#"{"mcpServers": {"same": {"command": "second"}}}"#,
        );
        write(&project.join(".skills/bad/SKILL.md"), "No front matter");
        write(
            &project.join(".skills/flat.md"),
            "---\nname: flat\ndescription: A skill\n---\nInstructions",
        );
        let missing = temp.path().join("missing.json");
        let found = read(&root, std::slice::from_ref(&project), &[config, missing]);
        let servers = found.files[0].servers.as_ref().unwrap().as_ref().unwrap();
        assert!(servers.iter().any(|server| {
            server.name == "off"
                && server
                    .skipped
                    .as_deref()
                    .is_some_and(|why| why.contains("disabled"))
        }));
        let user = found
            .files
            .last()
            .unwrap()
            .servers
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap();
        assert!(user[0].skipped.as_deref().unwrap().contains("servers.json"));
        assert!(
            found.files[1]
                .servers
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap_err()
                .contains("missing.json")
        );
        let ours = found
            .skills
            .iter()
            .find(|folder| folder.path == project.join(".skills"))
            .unwrap();
        assert!(
            ours.skills
                .iter()
                .any(|skill| skill.name == "bad" && skill.skipped.is_some())
        );
        assert!(
            ours.skills
                .iter()
                .any(|skill| skill.name == "flat" && skill.skipped.is_none())
        );
    }

    #[test]
    fn built_in_skills_stay_out_of_the_folders_and_name_the_folder_that_replaces_one() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        write(
            &project.join(".skills/repo-docs-style/SKILL.md"),
            "---\nname: repo-docs-style\ndescription: Our docs rules\n---\nWrite plainly.",
        );
        let found = read(&root, std::slice::from_ref(&project), &[]);
        let names: Vec<&str> = found
            .builtin
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, ["user-facing-prose", "repo-docs-style", "ponytail"]);
        assert!(!found.builtin[0].description.is_empty());
        // The user's own skill folders (a home folder may hold skills of these names) can
        // replace a built-in too, so the test looks only at the project's folder.
        let ours = project.join(".skills");
        let replacing = ours.join("repo-docs-style");
        assert!(!found.builtin[0].replaced_by.contains(&replacing));
        assert!(found.builtin[1].replaced_by.contains(&replacing));
        // The folders hold only the folders' own skills, with a path.
        assert!(
            found
                .skills
                .iter()
                .all(|folder| folder.skills.iter().all(|skill| !skill.builtin))
        );
        let folder = found
            .skills
            .iter()
            .find(|folder| folder.path == ours)
            .unwrap();
        assert_eq!(folder.skills.len(), 1);
        assert_eq!(folder.skills[0].name, "repo-docs-style");
    }

    /// Every built-in skill is listed, the ones Arko has.
    #[test]
    fn the_listed_built_in_skills_are_arkos() {
        let temp = tempfile::tempdir().unwrap();
        let found = read(&temp.path().join("arko"), &[], &[]);
        let names: Vec<&str> = found
            .builtin
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        assert_eq!(names, arko::builtin_skills::names());
    }

    #[test]
    fn shared_extensions_need_no_synthetic_workspace_and_notes_stay_with_their_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        write(
            &root.join("studio-no-project/.mcp.json"),
            r#"{"mcpServers":{"unexpected":{"command":"x"}}}"#,
        );
        let bad = root.join("bad.json");
        let gone = root.join("studio-no-project/../gone.json");
        write(&bad, "invalid JSON");
        write(&root.join(USER_MCP_FILE), "{}");
        let listed = arko::extensions::list(&root, None, &[bad.clone(), gone.clone()]);
        let found = read(&root, &[], &[bad.clone(), gone.clone()]);
        assert!(found.projects.is_empty());
        for path in [&bad, &gone, &root.join(USER_MCP_FILE)] {
            let file = found.files.iter().find(|file| &file.path == path).unwrap();
            assert_eq!(
                file.servers.as_ref().unwrap().as_ref().unwrap_err(),
                file_note(&listed, path).unwrap()
            );
        }
    }

    /// A project's own .arko/mcp.json lists before its .mcp.json, each with
    /// its answer, which is forgotten apart. A server or a skill an agent
    /// added says so, and the skills of .arko/skills are the project's.
    #[test]
    fn a_projects_own_servers_and_what_agents_added_are_listed() {
        use arko::extensions::Added;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        let servers = serde_json::json!({"sentry": {"command": "npx"}});
        let local = project.join(LOCAL_MCP_FILE);
        write(
            &local,
            &serde_json::json!({"mcpServers": servers}).to_string(),
        );
        write(
            &project.join(PROJECT_MCP_FILE),
            r#"{"mcpServers":{"github":{"command":"npx"}}}"#,
        );
        write(
            &root.join("mcp-approvals.json"),
            &serde_json::json!({"local": {project.display().to_string(): {"start": true, "mcpServers": servers}}})
                .to_string(),
        );
        let added = chrono::Utc::now();
        arko::agent_added::record_server(
            &root,
            ServerRecord {
                file: local.clone(),
                name: "sentry".into(),
                session: "s1".into(),
                session_title: None,
                added,
                why: "Read the crash reports".into(),
            },
        )
        .unwrap();
        let skill = project.join(".arko/skills/release-notes");
        write(
            &skill.join("SKILL.md"),
            "---\nname: release-notes\ndescription: How to write them\nworkers: false\nmodels: [haiku]\n---\nWrite them.",
        );
        arko::agent_added::record_skill(
            &root,
            SkillRecord {
                path: skill.clone(),
                scope: arko::agent_added::Scope::Project,
                session: "s1".into(),
                session_title: None,
                added,
                why: "The checklist changed.".into(),
                source: None,
            },
        )
        .unwrap();
        let found = read(&root, std::slice::from_ref(&project), &[]);
        let files: Vec<(&Path, bool, Approval)> = found
            .projects
            .iter()
            .map(|listed| (listed.file.as_path(), listed.local, listed.approval))
            .collect();
        assert_eq!(
            files,
            [
                (local.as_path(), true, Approval::Approved),
                (
                    project.join(PROJECT_MCP_FILE).as_path(),
                    false,
                    Approval::NotAsked
                )
            ]
        );
        let sentry = &found.projects[0].servers.as_ref().unwrap()[0];
        assert_eq!(sentry.added.as_ref().unwrap().why, "Read the crash reports");
        assert_eq!(found.projects[1].servers.as_ref().unwrap()[0].added, None);
        let folder = found
            .skills
            .iter()
            .find(|folder| folder.path == project.join(".arko/skills"))
            .unwrap();
        assert_eq!(folder.of, SkillsOf::Project(project.clone()));
        let notes = &folder.skills[0];
        assert_eq!(notes.added.as_ref().unwrap().why, "The checklist changed.");
        assert!(!notes.workers);
        assert_eq!(notes.models, ["haiku"]);
        // The .mcp.json's answer stays when the .arko/mcp.json's is
        // forgotten.
        use crate::backend::Backend as _;
        crate::backend::ArkoBackend::new(&root)
            .forget_mcp_approval(&project, true)
            .unwrap();
        let found = read(&root, std::slice::from_ref(&project), &[]);
        assert_eq!(found.projects[0].approval, Approval::NotAsked);
        // Remove takes away what an agent added, and nothing else.
        let backend = crate::backend::ArkoBackend::new(&root);
        let said = backend
            .remove_added(Added::Skill {
                path: skill.clone(),
            })
            .unwrap();
        assert_eq!(said, "Removed the skill release-notes.");
        assert!(!skill.exists());
        let said = backend
            .remove_added(Added::Server {
                file: local.clone(),
                name: "sentry".into(),
            })
            .unwrap();
        assert_eq!(said, "Removed the MCP server sentry.");
        assert!(!std::fs::read_to_string(&local).unwrap().contains("sentry"));
        let why = backend
            .remove_added(Added::Server {
                file: project.join(PROJECT_MCP_FILE),
                name: "github".into(),
            })
            .unwrap_err();
        assert!(why.contains("wasn't added by an agent"), "{why}");
    }

    /// The switches of what agents may add are saved as `/extensions` saves
    /// them, and the git switch takes Arko's .gitignore out of the project's
    /// .arko folder and puts it back.
    #[test]
    fn the_switches_are_saved_as_the_command_saves_them() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        assert_eq!(read(&root, &[], &[]).approvals, ExtensionPrefs::default());
        set_switch(&root, &[], Switch::AskMcp, false).unwrap();
        set_switch(&root, &[], Switch::ReviewSkills, true).unwrap();
        let approvals = read(&root, &[], &[]).approvals;
        assert!(!approvals.ask_before_adding_mcp);
        assert!(approvals.review_project_skills);
        // Each project of the sessions open follows it.
        let other = temp.path().join("other");
        let ignores = [&project, &other].map(|project| {
            arko::arko_dir::ensure(project, &root)
                .unwrap()
                .join(".gitignore")
        });
        assert!(ignores.iter().all(|ignore| ignore.is_file()));
        let both = [project.clone(), other.clone()];
        set_switch(&root, &both, Switch::KeepOutOfGit, false).unwrap();
        assert!(!arko::profiles::Preferences::load(&root).keep_out_of_git);
        assert!(ignores.iter().all(|ignore| !ignore.exists()));
        set_switch(&root, &both, Switch::KeepOutOfGit, true).unwrap();
        assert!(arko::profiles::Preferences::load(&root).keep_out_of_git);
        assert!(ignores.iter().all(|ignore| ignore.is_file()));
    }

    #[test]
    fn what_an_agent_added_says_when_and_why() {
        let day = chrono::DateTime::parse_from_rfc3339("2026-10-28T12:00:00Z")
            .unwrap()
            .to_utc();
        assert_eq!(
            added_words(&day, true, "The release checklist changed"),
            "Added by an agent on 28 Oct, for this project. The release checklist changed."
        );
        assert_eq!(
            added_words(&day, false, "Read the designs.\n"),
            "Added by an agent on 28 Oct, for every project. Read the designs."
        );
        assert_eq!(
            added_words(&day, false, ""),
            "Added by an agent on 28 Oct, for every project."
        );
        assert_eq!(skill_notes(None, &[], true), Vec::<String>::new());
        assert_eq!(
            skill_notes(None, &["haiku".into(), "cc:haiku".into()], false),
            ["for haiku and cc:haiku", "main agent only"]
        );
        assert_eq!(
            and_list(&["a".into(), "b".into(), "c".into()]),
            "a, b and c"
        );
    }

    #[test]
    fn approvals_are_read_and_forgotten() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        let servers = serde_json::json!({"github": {"command": "npx"}});
        write(
            &project.join(PROJECT_MCP_FILE),
            &serde_json::json!({"mcpServers": servers}).to_string(),
        );
        write(&root.join("mcp-approvals.json"), &serde_json::json!({"projects": {project.display().to_string(): {"start": true, "mcpServers": servers}}}).to_string());
        let alias = project.join(".");
        let found = read(&root, std::slice::from_ref(&alias), &[]);
        assert_eq!(found.projects[0].approval, Approval::Approved);
        write(
            &project.join(PROJECT_MCP_FILE),
            r#"{"mcpServers":{"github":{"command":"changed"}}}"#,
        );
        assert_eq!(
            read(&root, std::slice::from_ref(&alias), &[]).projects[0].approval,
            Approval::NotAsked
        );
        use crate::backend::Backend as _;
        crate::backend::ArkoBackend::new(&root)
            .forget_mcp_approval(&alias, false)
            .unwrap();
        assert!(!arko::extensions::forget_project_mcp_approval(&root, &project).unwrap());
        let found = read(&root, std::slice::from_ref(&project), &[]);
        assert_eq!(found.projects[0].approval, Approval::NotAsked);
        assert!(
            found.projects[0].servers.as_ref().unwrap()[0]
                .skipped
                .is_some()
        );
    }
}
