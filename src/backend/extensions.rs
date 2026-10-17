//! The MCP servers and skills sessions get, as Settings › Extensions shows
//! them, read from the files arko reads: Arko's `mcp.json`, the MCP config
//! files set in Settings, each project's `.mcp.json` with the answer
//! remembered for it (`mcp-approvals.json` in Arko's folder), and the skill
//! folders.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// The servers set up for every project, in Arko's folder.
pub const USER_MCP_FILE: &str = "mcp.json";
/// A project's servers, in its folder.
pub const PROJECT_MCP_FILE: &str = ".mcp.json";
/// The answers about projects' servers, in Arko's folder.
pub const APPROVALS_FILE: &str = "mcp-approvals.json";

/// A project's skill folders, in its folder, in the order arko reads them.
pub const PROJECT_SKILL_FOLDERS: [&str; 3] = [".agents/skills", ".claude/skills", ".skills"];
/// The user's skill folders, in the home folder.
pub const HOME_SKILL_FOLDERS: [&str; 2] = [".agents/skills", ".claude/skills"];

/// What sessions get besides arko's own tools.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Extensions {
    /// Arko's `mcp.json`, then the config files set in Settings, each with
    /// its servers.
    pub files: Vec<McpFile>,
    /// The projects with a `.mcp.json`, each with the answer remembered for
    /// its servers.
    pub projects: Vec<ProjectServers>,
    /// The skill folders that exist: the projects', then the user's.
    pub skills: Vec<SkillFolder>,
}

/// An MCP config file and its servers, or why it can't be read.
#[derive(Debug, Clone, PartialEq)]
pub struct McpFile {
    pub path: PathBuf,
    /// Arko's own `mcp.json` (else a file set in Settings).
    pub arko: bool,
    /// `None` when the file isn't there.
    pub servers: Option<Result<Vec<McpServer>, String>>,
}

/// A server a file sets up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpServer {
    pub name: String,
    /// Its command line, as the file writes it.
    pub command: String,
    /// Why arko skips it, when it does (an HTTP server, no command).
    pub skipped: Option<String>,
}

/// What the user answered about a project's servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Approval {
    /// They start with the project's sessions.
    Approved,
    /// They don't start.
    Refused,
    /// No one was asked yet: a session in the folder asks.
    NotAsked,
    /// The file changed since the answer: a session asks again.
    Changed,
}

/// A project's `.mcp.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectServers {
    pub folder: PathBuf,
    pub servers: Result<Vec<McpServer>, String>,
    pub approval: Approval,
}

/// Where a skill folder is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillsOf {
    /// A project's, in this folder.
    Project(PathBuf),
    /// Arko's folder's `skills`.
    Arko,
    /// The home folder's.
    Home,
}

/// A skill folder and the skills in it.
#[derive(Debug, Clone, PartialEq)]
pub struct SkillFolder {
    pub path: PathBuf,
    pub of: SkillsOf,
    pub skills: Vec<Skill>,
}

/// A skill: a folder with a `SKILL.md`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skill {
    pub name: String,
    pub description: String,
}

impl Extensions {
    /// A set-up for tests and screenshots, in `home` with Arko's folder
    /// `arko`: a server in Arko's `mcp.json`, a project whose servers were
    /// approved, and the user's skills.
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
                    .map(|name| Skill {
                        name: name.into(),
                        description: String::new(),
                    })
                    .collect(),
            }],
        }
    }
}

/// Reads what sessions in Arko's folder `root` get: for the projects in
/// `folders`, with the MCP config files `mcp_config` set in Settings, and
/// the user's skills in `home`.
pub fn read(
    root: &Path,
    folders: &[PathBuf],
    mcp_config: &[PathBuf],
    home: Option<&Path>,
) -> Extensions {
    let mut files = vec![mcp_file(&root.join(USER_MCP_FILE), true)];
    files.extend(mcp_config.iter().map(|path| mcp_file(path, false)));
    let approvals = approvals(root);
    let mut projects: Vec<PathBuf> = folders.to_vec();
    // Projects answered about, whose sessions aren't listed any more.
    for folder in approvals.keys() {
        let folder = PathBuf::from(folder);
        if !projects.contains(&folder) {
            projects.push(folder);
        }
    }
    let projects = projects
        .into_iter()
        .filter_map(|folder| {
            let file = folder.join(PROJECT_MCP_FILE);
            if !file.is_file() {
                return None;
            }
            let parsed = parse_file(&file);
            let approval = match approvals.get(&folder.display().to_string()) {
                None => Approval::NotAsked,
                Some(answer) => match &parsed {
                    Ok((_, raw)) if answer.get("mcpServers") == Some(raw) => {
                        if answer.get("start").and_then(Value::as_bool) == Some(true) {
                            Approval::Approved
                        } else {
                            Approval::Refused
                        }
                    }
                    _ => Approval::Changed,
                },
            };
            Some(ProjectServers {
                folder,
                servers: parsed.map(|(servers, _)| servers),
                approval,
            })
        })
        .collect();
    let mut skill_folders: Vec<(PathBuf, SkillsOf)> = Vec::new();
    for folder in folders {
        for name in PROJECT_SKILL_FOLDERS {
            skill_folders.push((folder.join(name), SkillsOf::Project(folder.clone())));
        }
    }
    skill_folders.push((root.join("skills"), SkillsOf::Arko));
    if let Some(home) = home {
        for name in HOME_SKILL_FOLDERS {
            skill_folders.push((home.join(name), SkillsOf::Home));
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    let skills = skill_folders
        .into_iter()
        .filter_map(|(path, of)| {
            let real = std::fs::canonicalize(&path).ok()?;
            if !real.is_dir() || seen.contains(&real) {
                return None;
            }
            seen.push(real);
            Some(SkillFolder {
                skills: skills_in(&path),
                path,
                of,
            })
        })
        .collect();
    Extensions {
        files,
        projects,
        skills,
    }
}

/// Forgets the answer about the servers of the project in `folder`: its
/// next session asks again. Arko's folder is `root`.
pub fn forget_approval(root: &Path, folder: &Path) -> Result<(), String> {
    let path = root.join(APPROVALS_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("Could not read {}: {error}.", path.display())),
    };
    let mut value: Value = serde_json::from_str(&text)
        .map_err(|error| format!("Could not read {}: {error}.", path.display()))?;
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
        .map_err(|error| format!("Could not save {}: {error}.", path.display()))
}

/// The answers remembered, by project folder.
fn approvals(root: &Path) -> serde_json::Map<String, Value> {
    std::fs::read_to_string(root.join(APPROVALS_FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| value.get("projects").and_then(Value::as_object).cloned())
        .unwrap_or_default()
}

fn mcp_file(path: &Path, arko: bool) -> McpFile {
    McpFile {
        path: path.to_path_buf(),
        arko,
        servers: path
            .is_file()
            .then(|| parse_file(path).map(|(servers, _)| servers)),
    }
}

/// The servers in the MCP config file at `path`, in Claude Code's format
/// (servers by name) or arko's (a list of servers with names), and its
/// `mcpServers` as written.
fn parse_file(path: &Path) -> Result<(Vec<McpServer>, Value), String> {
    let text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    parse(&text)
}

fn parse(text: &str) -> Result<(Vec<McpServer>, Value), String> {
    let value: Value = serde_json::from_str(text).map_err(|error| error.to_string())?;
    let raw = value
        .get("mcpServers")
        .ok_or("it has no mcpServers")?
        .clone();
    let entries: Vec<(Option<String>, &Value)> = match &raw {
        Value::Object(servers) => servers
            .iter()
            .map(|(name, entry)| (Some(name.clone()), entry))
            .collect(),
        Value::Array(servers) => servers
            .iter()
            .map(|entry| {
                let name = entry.get("name").and_then(Value::as_str).map(String::from);
                (name, entry)
            })
            .collect(),
        _ => return Err("its mcpServers is neither an object nor a list".into()),
    };
    let servers = entries
        .into_iter()
        .enumerate()
        .filter(|(_, (_, entry))| entry.get("disabled").and_then(Value::as_bool) != Some(true))
        .map(|(index, (name, entry))| {
            let name = name
                .filter(|name| !name.trim().is_empty())
                .unwrap_or_else(|| format!("server {}", index + 1));
            let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
            let args: Vec<String> = entry
                .get("args")
                .and_then(Value::as_array)
                .map(|args| {
                    args.iter()
                        .map(|arg| match arg {
                            Value::String(text) => text.clone(),
                            other => other.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let kind = entry.get("type").and_then(Value::as_str);
            let skipped = match kind {
                Some(kind) if kind != "stdio" => Some(format!("a {kind} server")),
                _ if command.trim().is_empty() && entry.get("url").is_some() => {
                    Some("an HTTP server".to_string())
                }
                _ if command.trim().is_empty() => Some("no command".to_string()),
                _ => None,
            };
            McpServer {
                name,
                command: std::iter::once(command.to_string())
                    .chain(args)
                    .collect::<Vec<_>>()
                    .join(" "),
                skipped,
            }
        })
        .collect();
    Ok((servers, raw))
}

/// The skills in the folder at `path`: each folder in it with a
/// `SKILL.md`, by name.
fn skills_in(path: &Path) -> Vec<Skill> {
    let Ok(entries) = std::fs::read_dir(path) else {
        return Vec::new();
    };
    let mut skills: Vec<Skill> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let text = std::fs::read_to_string(entry.path().join("SKILL.md")).ok()?;
            let folder = entry.file_name().to_string_lossy().into_owned();
            let (name, description) = front_matter(&text);
            Some(Skill {
                name: name.unwrap_or(folder),
                description: description.unwrap_or_default(),
            })
        })
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
}

/// A `SKILL.md`'s name and description, from its front matter.
fn front_matter(text: &str) -> (Option<String>, Option<String>) {
    let mut lines = text.lines();
    if lines.next().map(str::trim) != Some("---") {
        return (None, None);
    }
    let (mut name, mut description) = (None, None);
    for line in lines {
        if line.trim() == "---" {
            break;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches(['"', '\'']).trim();
        if value.is_empty() {
            continue;
        }
        match key.trim() {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            _ => {}
        }
    }
    (name, description)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn servers_are_read_in_both_formats() {
        let (servers, _) = parse(
            r#"{"mcpServers": {
                "github": {"command": "npx", "args": ["-y", "@github/mcp"]},
                "docs": {"type": "http", "url": "https://docs.example"},
                "old": {"command": "x", "disabled": true}
            }}"#,
        )
        .unwrap();
        assert_eq!(servers.len(), 2, "the disabled one is left out");
        let named = |name: &str| servers.iter().find(|server| server.name == name).unwrap();
        assert_eq!(named("github").command, "npx -y @github/mcp");
        assert_eq!(named("github").skipped, None);
        assert_eq!(named("docs").skipped.as_deref(), Some("a http server"));
        let (servers, _) =
            parse(r#"{"mcpServers": [{"name": "files", "command": "mcp-files"}]}"#).unwrap();
        assert_eq!(servers[0].name, "files");
        assert!(parse("{}").is_err());
    }

    /// A project's answer holds while its servers stay as they were, and
    /// forgetting it asks again.
    #[test]
    fn approvals_are_read_and_forgotten() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let project = temp.path().join("project");
        let other = temp.path().join("other");
        let servers = r#"{"github": {"command": "npx"}}"#;
        write(
            &project.join(PROJECT_MCP_FILE),
            &format!(r#"{{"mcpServers": {servers}}}"#),
        );
        write(
            &other.join(PROJECT_MCP_FILE),
            r#"{"mcpServers": {"github": {"command": "npx", "args": ["-y"]}}}"#,
        );
        write(
            &root.join(APPROVALS_FILE),
            &format!(
                r#"{{"projects": {{
                    "{}": {{"start": true, "mcpServers": {servers}}},
                    "{}": {{"start": true, "mcpServers": {servers}}}
                }}}}"#,
                project.display(),
                other.display()
            ),
        );
        let read_now = || read(&root, std::slice::from_ref(&project), &[], None);
        let found = read_now();
        assert_eq!(found.projects.len(), 2, "answered folders are listed too");
        assert_eq!(found.projects[0].approval, Approval::Approved);
        assert_eq!(found.projects[1].approval, Approval::Changed);
        assert_eq!(found.files[0].servers, None, "no mcp.json in Arko's folder");
        forget_approval(&root, &project).unwrap();
        let found = read_now();
        assert_eq!(found.projects[0].approval, Approval::NotAsked);
        assert_eq!(found.projects[1].approval, Approval::Changed);
        // Forgetting what isn't there does nothing.
        forget_approval(&root, &project).unwrap();
        forget_approval(&temp.path().join("none"), &project).unwrap();
    }

    #[test]
    fn skill_folders_are_found_the_projects_first_each_once() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let home = temp.path().join("home");
        let project = temp.path().join("project");
        write(
            &project.join(".claude/skills/pdf/SKILL.md"),
            "---\nname: pdf\ndescription: \"Read PDFs\"\n---\nInstructions",
        );
        write(&root.join("skills/notes/SKILL.md"), "No front matter");
        std::fs::create_dir_all(home.join(".agents/skills")).unwrap();
        let found = read(&root, std::slice::from_ref(&project), &[], Some(&home));
        let of: Vec<&SkillsOf> = found.skills.iter().map(|folder| &folder.of).collect();
        assert_eq!(
            of,
            [
                &SkillsOf::Project(project.clone()),
                &SkillsOf::Arko,
                &SkillsOf::Home
            ]
        );
        assert_eq!(
            found.skills[0].skills,
            [Skill {
                name: "pdf".into(),
                description: "Read PDFs".into()
            }]
        );
        assert_eq!(found.skills[1].skills[0].name, "notes");
        assert!(found.skills[2].skills.is_empty());
    }
}
