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
    /// The config files set in Settings, then Arko's `mcp.json`, each with
    /// its servers, in the order arko reads them.
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
    // In the order arko reads them: a name in two is taken from the first.
    let mut files: Vec<McpFile> = mcp_config
        .iter()
        .map(|path| mcp_file(path, false))
        .collect();
    files.push(mcp_file(&root.join(USER_MCP_FILE), true));
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

/// Whether arko can read the MCP config file at `path`: why not, when it
/// can't (it isn't there, it isn't JSON, it has no `mcpServers`).
pub fn check_mcp_file(path: &Path) -> Result<(), String> {
    match std::fs::read_to_string(path) {
        Ok(text) => parse(&text).map(|_| ()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Err("it isn’t there".to_string())
        }
        Err(error) => Err(error.to_string()),
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
    parse_with(text, &|name| std::env::var(name).ok())
}

/// The servers in an MCP config file's `text`, as arko starts them: those
/// arko skips say why, as it says it. `lookup` gives the values of the
/// `${VAR}`s in their commands, as the environment does for arko.
fn parse_with(
    text: &str,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<(Vec<McpServer>, Value), String> {
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
        .map(
            |(index, (name, entry))| match name.filter(|name| !name.trim().is_empty()) {
                None => McpServer {
                    name: format!("server {}", index + 1),
                    command: String::new(),
                    skipped: Some("it has no name".to_string()),
                },
                Some(name) => {
                    let (command, skipped) = match server_command(entry, lookup) {
                        Ok(command) => (command, None),
                        Err((command, why)) => (command, Some(why)),
                    };
                    McpServer {
                        name,
                        command,
                        skipped,
                    }
                }
            },
        )
        .collect();
    Ok((servers, raw))
}

/// A server's command line as its entry writes it, or that and why arko
/// skips the server (`mcp::server` in arko).
fn server_command(
    entry: &Value,
    lookup: &dyn Fn(&str) -> Option<String>,
) -> Result<String, (String, String)> {
    let Some(entry) = entry.as_object() else {
        return Err((String::new(), "it isn’t an object".to_string()));
    };
    let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
    let text = |value: &Value| match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    };
    let args: Result<Vec<String>, &str> = match entry.get("args") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(args)) => args
            .iter()
            .map(|arg| text(arg).ok_or("an argument isn’t text"))
            .collect(),
        Some(_) => Err("its args aren’t a list"),
    };
    let shown = std::iter::once(command.to_string())
        .chain(args.clone().unwrap_or_default())
        .collect::<Vec<_>>()
        .join(" ");
    let skip = |why: &str| Err((shown.clone(), why.to_string()));
    if let Some(kind) = entry.get("type").and_then(Value::as_str)
        && kind != "stdio"
    {
        return skip(&format!("it’s a {kind} server"));
    }
    if command.trim().is_empty() {
        return if entry.contains_key("url") {
            skip("it’s an HTTP server")
        } else {
            skip("it has no command")
        };
    }
    let args = match args {
        Ok(args) => args,
        Err(why) => return skip(why),
    };
    let env: Vec<String> = match entry.get("env") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Object(env)) => match env.values().map(text).collect::<Option<_>>() {
            Some(values) => values,
            None => return skip("an env value isn’t text"),
        },
        Some(_) => return skip("its env isn’t an object"),
    };
    let unset = std::iter::once(command.to_string())
        .chain(args)
        .chain(env)
        .find_map(|text| unset_variable(&text, lookup));
    match unset {
        Some(name) => skip(&format!("it uses ${{{name}}}, which isn’t set")),
        None => Ok(shown),
    }
}

/// The first `${VAR}` in `text` that isn't set and has no default
/// (`${VAR:-default}`), as arko expands them.
fn unset_variable(text: &str, lookup: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let mut rest = text;
    while let Some(start) = rest.find("${") {
        let length = rest[start..].find('}')?;
        let inner = &rest[start + 2..start + length];
        let (name, default) = match inner.split_once(":-") {
            Some((name, default)) => (name, Some(default)),
            None => (inner, None),
        };
        if default.is_none() && lookup(name).is_none() {
            return Some(name.to_string());
        }
        rest = &rest[start + length + 1..];
    }
    None
}

/// The skills in the folder at `path`, as the agents load them, read by
/// arko_agents' own loader: each folder with a `SKILL.md`, each other
/// `.md` file and each `.skill` bundle whose front matter gives a name
/// and a description. Others are skipped, as they are for the agents.
fn skills_in(path: &Path) -> Vec<Skill> {
    let config = arko_agents::SkillsConfig {
        workspace_source: None,
        global_sources: vec![arko_agents::SkillSource {
            path: path.display().to_string(),
            include: None,
            exclude: None,
        }],
        on_invalid_skill: arko_agents::InvalidSkillPolicy::Warn,
        load_skill_interception: None,
    };
    let Ok(skills) = arko_agents::SkillManager::load(&config, path) else {
        return Vec::new();
    };
    let found = skills.prompt_context();
    let mut skills: Vec<Skill> = found
        .on_demand_skills
        .iter()
        .map(|skill| Skill {
            name: skill.name.clone(),
            description: skill.description.clone(),
        })
        .chain(found.auto_loaded_skills.iter().map(|skill| Skill {
            name: skill.name.clone(),
            description: String::new(),
        }))
        .collect();
    skills.sort_by(|a, b| a.name.cmp(&b.name));
    skills
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
        assert_eq!(named("docs").skipped.as_deref(), Some("it’s a http server"));
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

    /// The files set in Settings come before Arko's mcp.json, as arko
    /// reads them: a server named in both is taken from them.
    #[test]
    fn config_files_come_before_arkos_mcp_json() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("arko");
        let config = temp.path().join("work.json");
        write(&root.join(USER_MCP_FILE), r#"{"mcpServers": {}}"#);
        write(&config, r#"{"mcpServers": {}}"#);
        let found = read(&root, &[], std::slice::from_ref(&config), None);
        let order: Vec<(&Path, bool)> = found
            .files
            .iter()
            .map(|file| (file.path.as_path(), file.arko))
            .collect();
        assert_eq!(
            order,
            [
                (config.as_path(), false),
                (root.join(USER_MCP_FILE).as_path(), true)
            ]
        );
    }

    /// Servers arko skips show as skipped, with why: no name, not an
    /// object, arguments or environment of the wrong kind, a `${VAR}` that
    /// isn't set.
    #[test]
    fn servers_arko_skips_say_why() {
        let lookup = |name: &str| (name == "TOKEN").then(|| "secret".to_string());
        let (servers, _) = parse_with(
            r#"{"mcpServers": {
                "gh": {"command": "npx", "args": ["--token", "${TOKEN}"]},
                "unset": {"command": "npx", "args": ["${SURELY_UNSET}"]},
                "fallback": {"command": "npx", "args": ["${SURELY_UNSET:-none}"]},
                "args": {"command": "x", "args": "not-a-list"},
                "env": {"command": "x", "env": {"A": ["no"]}},
                "notobj": "npx"
            }}"#,
            &lookup,
        )
        .unwrap();
        let skipped = |name: &str| {
            servers
                .iter()
                .find(|server| server.name == name)
                .unwrap()
                .skipped
                .clone()
        };
        assert_eq!(skipped("gh"), None);
        assert_eq!(
            servers[servers.iter().position(|s| s.name == "gh").unwrap()].command,
            "npx --token ${TOKEN}",
            "as the file writes it"
        );
        assert_eq!(
            skipped("unset").as_deref(),
            Some("it uses ${SURELY_UNSET}, which isn’t set")
        );
        assert_eq!(skipped("fallback"), None);
        assert_eq!(skipped("args").as_deref(), Some("its args aren’t a list"));
        assert_eq!(skipped("env").as_deref(), Some("an env value isn’t text"));
        assert_eq!(skipped("notobj").as_deref(), Some("it isn’t an object"));
        let (servers, _) =
            parse_with(r#"{"mcpServers": [{"command": "unnamed"}]}"#, &lookup).unwrap();
        assert_eq!(servers[0].skipped.as_deref(), Some("it has no name"));
    }

    /// Skills are read as the agents load them: a folder's SKILL.md or a
    /// flat Markdown file with a name and a description, folded lines
    /// included. Hidden entries and files without them are skipped.
    #[test]
    fn skills_are_listed_as_the_agents_load_them() {
        let temp = tempfile::tempdir().unwrap();
        let skills = temp.path().join("skills");
        write(
            &skills.join("no-front-matter/SKILL.md"),
            "Just instructions",
        );
        write(
            &skills.join(".hidden/SKILL.md"),
            "---\nname: hidden\ndescription: d\n---\nhidden body",
        );
        write(
            &skills.join("flat.md"),
            "---\nname: flat\ndescription: A flat one\n---\nflat body",
        );
        write(
            &skills.join("folded/SKILL.md"),
            "---\nname: folded\ndescription: >\n  Long text\n  over lines\n---\nfolded body",
        );
        let found: Vec<(String, String)> = skills_in(&skills)
            .into_iter()
            .map(|skill| (skill.name, skill.description))
            .collect();
        assert_eq!(
            found,
            [
                ("flat".to_string(), "A flat one".to_string()),
                ("folded".to_string(), "Long text over lines".to_string()),
            ]
        );
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
        assert!(found.skills[1].skills.is_empty(), "no front matter");
        assert!(found.skills[2].skills.is_empty());
    }
}
