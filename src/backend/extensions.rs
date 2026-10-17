//! Extensions as the engine lists them, grouped for Settings. Approval
//! answers are kept separately so Settings can offer Forget answer.

use std::path::{Path, PathBuf};

use serde_json::Value;

pub const USER_MCP_FILE: &str = "mcp.json";
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
pub const APPROVALS_FILE: &str = "mcp-approvals.json";
const PROJECT_SKILL_FOLDERS: [&str; 3] = [".agents/skills", ".claude/skills", ".skills"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extensions {
    pub files: Vec<McpFile>,
    pub projects: Vec<ProjectServers>,
    pub skills: Vec<SkillFolder>,
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
    Changed,
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
                skills: ["docx", "pdf"]
                    .into_iter()
                    .map(|name| arko::extensions::SkillEntry {
                        name: name.into(),
                        description: String::new(),
                        path: home.join(".claude/skills").join(name),
                        auto_load: false,
                        skipped: None,
                    })
                    .collect(),
            }],
        }
    }
}

/// Reads extensions off the UI thread. The engine owns MCP and skill
/// parsing, precedence, skipped entries and the folders it searches.
pub fn read(root: &Path, folders: &[PathBuf], mcp_config: &[PathBuf]) -> Extensions {
    let approvals = approvals(root);
    let mut projects = folders.to_vec();
    for folder in approvals.keys().map(PathBuf::from) {
        if !projects.contains(&folder) {
            projects.push(folder);
        }
    }
    let workspaces = if projects.is_empty() {
        vec![None]
    } else {
        projects.iter().map(|path| Some(path.as_path())).collect()
    };
    let mut found = Extensions::default();
    for (index, workspace) in workspaces.into_iter().enumerate() {
        let listed = arko::extensions::list(root, workspace, mcp_config);
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
            let raw = mcp_servers(&project);
            let approval = match approvals.get(&workspace.display().to_string()) {
                None => Approval::NotAsked,
                Some(answer) if raw.as_ref().ok() == answer.get("mcpServers") => {
                    if answer.get("start").and_then(Value::as_bool) == Some(true) {
                        Approval::Approved
                    } else {
                        Approval::Refused
                    }
                }
                Some(_) => Approval::Changed,
            };
            found.projects.push(ProjectServers {
                folder: workspace.to_path_buf(),
                servers: servers_in(&listed, &project),
                approval,
            });
        }
        for skill in listed.skills {
            if found
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

/// A shallow file check for Add file and approval comparison. Entries are
/// interpreted only by the engine.
fn mcp_servers(path: &Path) -> Result<Value, String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    match value.get("mcpServers") {
        Some(servers @ (Value::Object(_) | Value::Array(_))) => Ok(servers.clone()),
        Some(_) => Err("its mcpServers is neither an object nor a list".into()),
        None => Err("it has no mcpServers".into()),
    }
}

pub fn check_mcp_file(path: &Path) -> Result<(), String> {
    mcp_servers(path).map(|_| ())
}

/// Forgets the answer about the project's servers, so its next session asks.
pub fn forget_approval(root: &Path, folder: &Path) -> Result<(), String> {
    let path = root.join(APPROVALS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not read {} ({error}).", path.display())),
    };
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Could not read {} ({error}).", path.display()))?;
    let removed = value
        .get_mut("projects")
        .and_then(Value::as_object_mut)
        .and_then(|projects| projects.remove(&folder.display().to_string()));
    if removed.is_none() {
        return Ok(());
    }
    let text = serde_json::to_string_pretty(&value).map_err(|error| error.to_string())?;
    let temporary = root.join(format!("{APPROVALS_FILE}.tmp"));
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, &path))
        .map_err(|error| format!("Could not save {} ({error}).", path.display()))
}

fn approvals(root: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(root.join(APPROVALS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("projects").and_then(Value::as_object).cloned())
        .unwrap_or_default()
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
        write(&root.join(APPROVALS_FILE), &serde_json::json!({"projects": {project.display().to_string(): {"start": true, "mcpServers": servers}}}).to_string());
        let found = read(&root, &[], &[]);
        assert_eq!(found.projects[0].approval, Approval::Approved);
        forget_approval(&root, &project).unwrap();
        let found = read(&root, std::slice::from_ref(&project), &[]);
        assert_eq!(found.projects[0].approval, Approval::NotAsked);
        assert!(
            found.projects[0].servers.as_ref().unwrap()[0]
                .skipped
                .is_some()
        );
    }
}
