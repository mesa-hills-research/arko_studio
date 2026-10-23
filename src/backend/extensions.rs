//! Extensions as the engine lists them, grouped for Settings. Approval
//! answers are kept separately so Settings can offer Forget answer.

use std::path::{Path, PathBuf};

pub const USER_MCP_FILE: &str = "mcp.json";
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
const PROJECT_SKILL_FOLDERS: [&str; 3] = [".agents/skills", ".claude/skills", ".skills"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extensions {
    pub files: Vec<McpFile>,
    pub projects: Vec<ProjectServers>,
    pub skills: Vec<SkillFolder>,
    /// The skills built into Arko, which belong to no folder.
    pub builtin: Vec<BuiltinSkill>,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Refused,
    NotAsked,
    /// The engine could not read the answer. The servers carry its error.
    Unavailable,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ProjectServers {
    pub folder: PathBuf,
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
    /// The folders holding a skill of its name, in the order Arko reads them.
    /// An empty list means nothing replaces it.
    pub replaced_by: Vec<PathBuf>,
}

impl Extensions {
    /// A setup for tests and screenshots.
    pub fn sample(home: &Path, arko: &Path) -> Self {
        let server = |name: &str, command: &str| McpServer {
            name: name.into(),
            command: command.into(),
            skipped: None,
        };
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
                folder: home.join("src/arko"),
                servers: Ok(vec![server("playwright", "npx @playwright/mcp")]),
                approval: Approval::Approved,
            }],
            skills: vec![SkillFolder {
                path: home.join(".claude/skills"),
                of: SkillsOf::Home,
                skills: ["docx", "pdf", "repo-docs-style"]
                    .into_iter()
                    .map(|name| arko::extensions::SkillEntry {
                        name: name.into(),
                        description: String::new(),
                        path: home.join(".claude/skills").join(name),
                        auto_load: false,
                        skipped: None,
                        builtin: false,
                    })
                    .collect(),
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
                    replaced_by: vec![home.join(".claude/skills")],
                },
                BuiltinSkill {
                    name: "ponytail".into(),
                    description: "Finish the whole task with the smallest change that fully works, split the work sensibly between workers, and check it in proportion to the risk. Use it for any coding task, for running workers, and when the user asks for the simplest solution.".into(),
                    auto_load: false,
                    replaced_by: Vec::new(),
                },
            ],
        }
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
        if let Some(workspace) = workspace
            && let project = workspace.join(PROJECT_MCP_FILE)
            && project.is_file()
        {
            let mut servers = servers_in(&listed, &project);
            let approval = match arko::extensions::project_mcp_approval(root, workspace) {
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
                    .and_then(|other| other.path.parent())
            } else {
                None
            };
            match found
                .builtin
                .iter_mut()
                .find(|seen| seen.name == skill.name)
            {
                Some(seen) => {
                    if let Some(folder) = replacing
                        && !seen.replaced_by.iter().any(|known| known == folder)
                    {
                        seen.replaced_by.push(folder.to_path_buf());
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
        assert!(!found.builtin[0].replaced_by.contains(&ours));
        assert!(found.builtin[1].replaced_by.contains(&ours));
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
            .forget_mcp_approval(&alias)
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
