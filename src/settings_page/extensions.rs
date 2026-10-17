//! Settings › Extensions: the MCP servers sessions start (Arko's
//! `mcp.json`, the config files set under Advanced, each project's
//! `.mcp.json` with the answer remembered for it, which can be forgotten)
//! and the skill folders, with the skills found in each.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::{IconName, Sizable as _, WindowExt as _};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::SharedBackend;
use crate::backend::account_words::tilde;
use crate::backend::extensions::{
    Approval, Extensions, McpFile, McpServer, ProjectServers, SkillFolder, SkillsOf,
};
use crate::prefs;
use crate::session_view::{HomeFolder, recent_folders};

/// The page's state: what sessions get, as last read.
pub struct ExtensionsPage {
    extensions: Option<Extensions>,
    home: Option<PathBuf>,
    /// The MCP config files set in Settings, as last read.
    config: Vec<PathBuf>,
    _load: Option<Task<()>>,
    _prefs: Subscription,
}

impl ExtensionsPage {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // A file added or removed under Advanced, or by a reset: the page
        // reads them again.
        let prefs = cx.observe_global::<prefs::PrefsStore>(|this, cx| {
            if prefs::get(cx).advanced.mcp_config != this.config {
                this.refresh(cx);
            }
        });
        let mut this = Self {
            extensions: None,
            home: cx.global::<HomeFolder>().0.clone(),
            config: Vec::new(),
            _load: None,
            _prefs: prefs,
        };
        this.refresh(cx);
        this
    }

    pub fn extensions(&self) -> Option<&Extensions> {
        self.extensions.as_ref()
    }

    /// Reads the files again, in the background: for the folders of the
    /// sessions in Arko's folder.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = cx.global::<SharedBackend>().clone();
        let config = prefs::get(cx).advanced.mcp_config.clone();
        self.config = config.clone();
        let reading = cx.background_spawn(async move {
            let folders = backend
                .sessions()
                .map(|sessions| recent_folders(&sessions))
                .unwrap_or_default();
            backend.extensions(&folders, &config)
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let extensions = reading.await;
            this.update(cx, |this, cx| {
                this.extensions = Some(extensions);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Forgets the answer about the servers of the project in `folder`, so
    /// its next session asks again.
    pub fn forget(this: &Entity<Self>, folder: PathBuf, window: &mut Window, cx: &mut App) {
        let backend = cx.global::<SharedBackend>().clone();
        let home = this.read(cx).home.clone();
        let shown = tilde(&folder, home.as_deref());
        let forgetting = cx.background_spawn(async move { backend.forget_mcp_approval(&folder) });
        let this = this.clone();
        window
            .spawn(cx, async move |cx| {
                let forgotten = forgetting.await;
                cx.update(|window, cx| {
                    match forgotten {
                        Ok(()) => window.push_notification(
                            format!("The next session in {shown} asks again before starting its MCP servers."),
                            cx,
                        ),
                        Err(why) => window.push_notification(Notification::error(why), cx),
                    }
                    this.update(cx, |page, cx| page.refresh(cx));
                })
                .ok();
            })
            .detach();
    }

    pub fn page(&self, this: &Entity<Self>) -> SettingPage {
        let home = self.home.as_deref();
        let (files, projects, skills) = match &self.extensions {
            None => {
                let checking = |title: &'static str| {
                    vec![
                        SettingItem::new(title, SettingField::render(|_, _, _| div()))
                            .description("Reading…"),
                    ]
                };
                (
                    checking("Arko’s mcp.json"),
                    Vec::new(),
                    checking("Skill folders"),
                )
            }
            Some(extensions) => (
                extensions
                    .files
                    .iter()
                    .map(|file| file_item(file, home))
                    .collect(),
                extensions
                    .projects
                    .iter()
                    .enumerate()
                    .map(|(ix, project)| project_item(this, ix, project, home))
                    .collect(),
                if extensions.skills.is_empty() {
                    vec![
                        SettingItem::new("No skill folders", SettingField::render(|_, _, _| div()))
                            .description(
                                "Make one of the folders above, with a folder per skill holding its SKILL.md.",
                            )
                            .keywords(["skills"]),
                    ]
                } else {
                    extensions
                        .skills
                        .iter()
                        .map(|folder| skills_item(folder, home))
                        .collect()
                },
            ),
        };
        let mut groups = vec![
            SettingGroup::new()
                .title("MCP servers")
                .description(
                    "Programs that give the agents more tools. Agent sessions opened from now on start the servers in these files, and a server named in two is taken from the first. Servers over HTTP are skipped, because Arko talks to servers over standard input and output.",
                )
                .items(files),
        ];
        if !projects.is_empty() {
            groups.push(
                SettingGroup::new()
                    .title("Projects’ servers")
                    .description(
                        "A project’s .mcp.json would run programs as soon as a session opens in its folder, so Arko asks first, and remembers the answer until the file’s servers change.",
                    )
                    .items(projects),
            );
        }
        groups.push(
            SettingGroup::new()
                .title("Skills")
                .description(
                    "Instructions for particular kinds of work, which the agents load when a task calls for them. A skill is a folder with a SKILL.md, or a Markdown file, whose front matter gives its name and description. Arko reads a project’s .agents/skills, .claude/skills and .skills, then skills in Arko’s folder, ~/.agents/skills and ~/.claude/skills. A skill named as an earlier one gets a number (pdf-2), and the same skill twice is read once.",
                )
                .items(skills),
        );
        SettingPage::new(super::PAGES[6])
            .icon(IconName::Frame)
            .resettable(false)
            .groups(groups)
    }
}

/// The servers a file sets up, in a sentence: each one's name and the
/// command it runs, or why arko skips it.
fn servers_words(servers: &[McpServer]) -> String {
    if servers.is_empty() {
        return "It sets up no servers.".into();
    }
    let names: Vec<String> = servers
        .iter()
        .map(|server| match &server.skipped {
            Some(why) => format!("{} (skipped because {why})", server.name),
            None if server.command.is_empty() => server.name.clone(),
            None => format!("{} ({})", server.name, server.command),
        })
        .collect();
    format!("It sets up {}.", names.join(", "))
}

fn file_item(file: &McpFile, home: Option<&Path>) -> SettingItem {
    let path = tilde(&file.path, home);
    let title = if file.arko {
        "Arko’s mcp.json".to_string()
    } else {
        path.clone()
    };
    let description = match &file.servers {
        None if file.arko => {
            format!("{path} isn’t there. Servers set up in it start in every agent session.")
        }
        None => format!("{path} isn’t there. Remove it under Advanced, or make it."),
        Some(Err(why)) => why.clone(),
        Some(Ok(servers)) if file.arko => format!("{path}. {}", servers_words(servers)),
        Some(Ok(servers)) => servers_words(servers),
    };
    SettingItem::new(title, SettingField::render(|_, _, _| div()))
        .description(description)
        .keywords(["mcp", "servers", "tools", "mcp.json"])
}

fn project_item(
    this: &Entity<ExtensionsPage>,
    ix: usize,
    project: &ProjectServers,
    home: Option<&Path>,
) -> SettingItem {
    let servers = match &project.servers {
        Ok(servers) => servers_words(servers),
        Err(why) => why.clone(),
    };
    let answer = match project.approval {
        Approval::Approved => "Approved, so they start with its sessions.",
        Approval::Refused => "Refused, so they don’t start.",
        Approval::NotAsked => "No answer for this config. Its next session asks.",
        Approval::Unavailable => "",
    };
    let remembered = matches!(project.approval, Approval::Approved | Approval::Refused);
    let folder = project.folder.clone();
    let this = this.clone();
    SettingItem::new(
        tilde(&project.folder, home),
        SettingField::render(move |_, _, _| {
            let (this, folder) = (this.clone(), folder.clone());
            div().when(remembered, |div| {
                div.child(
                    Button::new(("forget-approval", ix))
                        .outline()
                        .small()
                        .label("Forget answer")
                        .on_click(move |_, window, cx| {
                            ExtensionsPage::forget(&this, folder.clone(), window, cx)
                        }),
                )
            })
        }),
    )
    .description(format!("{servers} {answer}"))
    .keywords(["mcp", ".mcp.json", "approval", "project"])
}

fn skills_item(folder: &SkillFolder, home: Option<&Path>) -> SettingItem {
    let whose = match &folder.of {
        SkillsOf::Project(project) => format!("{}’s", tilde(project, home)),
        SkillsOf::Arko => "Arko’s folder’s".to_string(),
        SkillsOf::Home => "Yours, for every project".to_string(),
    };
    let found = if folder.skills.is_empty() {
        "No skills yet.".to_string()
    } else {
        let names: Vec<String> = folder
            .skills
            .iter()
            .map(|skill| match &skill.skipped {
                Some(why) => format!("{} (skipped because {why})", skill.name),
                None if skill.auto_load => format!("{} (loaded automatically)", skill.name),
                None => skill.name.clone(),
            })
            .collect();
        format!("{}.", names.join(", "))
    };
    let mut keywords: Vec<SharedString> = vec!["skills".into(), "SKILL.md".into()];
    keywords.extend(folder.skills.iter().map(|skill| skill.name.clone().into()));
    SettingItem::new(
        tilde(&folder.path, home),
        SettingField::render(|_, _, _| div()),
    )
    .description(format!("{whose}. {found}"))
    .keywords(keywords)
}
