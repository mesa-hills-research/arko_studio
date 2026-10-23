//! Settings › Extensions: the MCP servers sessions start (Arko's
//! `mcp.json`, the config files set under Advanced, each project's
//! `.mcp.json` with the answer remembered for it, which can be forgotten)
//! and the skill folders, with the skills found in each, then the skills
//! built into Arko.

use std::path::{Path, PathBuf};

use gpui_kit::component::button::Button;
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::SharedBackend;
use crate::backend::account_words::tilde;
use crate::backend::extensions::{
    Approval, BuiltinSkill, Extensions, McpFile, McpServer, ProjectServers, SkillFolder, SkillsOf,
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

    /// The page, as last read. `revision` counts the reads, and the groups
    /// draw again only after one.
    pub fn page(&self, this: &Entity<Self>, revision: u64) -> SettingPage {
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
                .key(super::group_key("MCP servers", &revision))
                .title("MCP servers")
                .description(
                    "Programs that give the agents more tools. Agent sessions opened from now on start the servers in these files, and a server named in two is taken from the first. Servers over HTTP are skipped, because Arko talks to servers over standard input and output.",
                )
                .items(files),
        ];
        if !projects.is_empty() {
            groups.push(
                SettingGroup::new()
                    .key(super::group_key("Projects’ servers", &revision))
                    .title("Projects’ servers")
                    .description(
                        "A project’s .mcp.json would run programs as soon as a session opens in its folder, so Arko asks first, and remembers the answer until the file’s servers change.",
                    )
                    .items(projects),
            );
        }
        groups.push(
            SettingGroup::new()
                .key(super::group_key("Skills", &revision))
                .title("Skills")
                .description(
                    "Instructions for particular kinds of work, which the agents load when a task calls for them. A skill is a folder with a SKILL.md, or a Markdown file, whose front matter gives its name and description. Arko reads a project’s .agents/skills, .claude/skills and .skills, then skills in Arko’s folder, ~/.agents/skills and ~/.claude/skills. A skill named as an earlier one gets a number (pdf-2), and the same skill twice is read once.",
                )
                .items(skills),
        );
        if let Some(extensions) = &self.extensions {
            let mut items = vec![builtin_switch(
                "builtin-skills",
                "Built-in skills",
                "Off leaves out all of them, whatever the rows below say.".into(),
                |prefs| prefs.extensions.builtin_skills_on(),
                |prefs, on| prefs.extensions.builtin_skills = Some(on),
            )];
            items.extend(
                extensions
                    .builtin
                    .iter()
                    .map(|skill| builtin_row(skill, home)),
            );
            groups.push(
                SettingGroup::new()
                    .key(super::group_key("Built in", &revision))
                    .title("Built in")
                    .description(
                        "Skills that come with Arko. A skill of the same name in a folder above replaces one. Sessions opened or resumed from now on get the choice, and one already open keeps its skills.",
                    )
                    .items(items),
            );
        }
        SettingPage::new(super::PAGES[super::EXTENSIONS_PAGE])
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

/// Where the skills of a built-in skill's name that replace it are, if any
/// do ("~/.claude/skills/pdf and ~/src/app/.skills/pdf").
fn replaced_words(skill: &BuiltinSkill, home: Option<&Path>) -> Option<String> {
    let skills: Vec<String> = skill
        .replaced_by
        .iter()
        .map(|path| tilde(path, home))
        .collect();
    (!skills.is_empty()).then(|| skills.join(" and "))
}

/// A built-in skill's row, one line as Arko's /skills lists it: its name,
/// its description cut to fit, in muted text what replaces it if a folder's
/// skill does, and its switch.
fn builtin_row(skill: &BuiltinSkill, home: Option<&Path>) -> SettingItem {
    let name = SharedString::from(skill.name.clone());
    let description = SharedString::from(
        skill
            .description
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" "),
    );
    let replaced = replaced_words(skill, home);
    let id = SharedString::from(format!("builtin-skill-{name}"));
    let keywords: Vec<SharedString> = vec![
        "skills".into(),
        "built in".into(),
        "SKILL.md".into(),
        name.clone(),
        description.clone(),
    ];
    SettingItem::render(move |_, _, cx| {
        let muted = cx.theme().muted_foreground;
        let (on, set) = (name.clone(), name.clone());
        h_flex()
            .w_full()
            .gap_3()
            .child(div().flex_none().text_sm().child(name.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .text_color(muted)
                    .truncate()
                    .child(description.clone()),
            )
            .when_some(replaced.clone(), |this, replaced| {
                // "Replaced by" stays whole, and a long path loses its
                // leading folders first.
                this.child(
                    h_flex()
                        .flex_none()
                        .min_w_0()
                        .max_w(relative(0.4))
                        .gap_1()
                        .text_xs()
                        .text_color(muted)
                        .child(div().flex_none().child("Replaced by"))
                        .child(
                            div()
                                .id("replaced-by")
                                .min_w_0()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_ellipsis_start()
                                .child(replaced.clone())
                                .tooltip(move |window, cx| {
                                    gpui_kit::component::tooltip::Tooltip::new(replaced.clone())
                                        .build(window, cx)
                                }),
                        ),
                )
            })
            .child(
                Switch::new(id.clone())
                    .accessibility_label(name.clone())
                    .checked(prefs::get(cx).extensions.builtin_skill_on(&on))
                    .on_click(move |checked, _, cx| {
                        let (name, on) = (set.clone(), *checked);
                        prefs::update(cx, move |prefs| {
                            prefs.extensions.set_builtin_skill(&name, on)
                        })
                    }),
            )
            .into_any_element()
    })
    .keywords(keywords)
}

/// A row with a switch bound to a preference, with `id` for tests.
fn builtin_switch(
    id: impl Into<SharedString>,
    title: impl Into<SharedString>,
    description: String,
    get: impl Fn(&prefs::Prefs) -> bool + Clone + 'static,
    set: impl Fn(&mut prefs::Prefs, bool) + Clone + 'static,
) -> SettingItem {
    let id: SharedString = id.into();
    SettingItem::new(
        title,
        SettingField::render(move |_, _, cx| {
            let (get, set) = (get.clone(), set.clone());
            Switch::new(id.clone())
                .checked(get(prefs::get(cx)))
                .on_click(move |on, _, cx| {
                    let set = set.clone();
                    let on = *on;
                    prefs::update(cx, move |prefs| set(prefs, on))
                })
                .into_any_element()
        }),
    )
    .description(description)
    .keywords(["skills", "built in", "SKILL.md"])
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{BuiltinSkill, replaced_words};

    #[test]
    fn a_built_in_skill_says_which_skills_replace_it() {
        let home = Path::new("/home/ada");
        let mut skill = BuiltinSkill {
            name: "repo-docs-style".into(),
            description: "Rules for docs.".into(),
            auto_load: false,
            replaced_by: Vec::new(),
        };
        assert_eq!(replaced_words(&skill, Some(home)), None);
        skill.replaced_by = vec![home.join(".claude/skills/repo-docs-style")];
        assert_eq!(
            replaced_words(&skill, Some(home)).as_deref(),
            Some("~/.claude/skills/repo-docs-style")
        );
        skill
            .replaced_by
            .push(home.join("src/arko/.skills/docs.md"));
        assert_eq!(
            replaced_words(&skill, Some(home)).as_deref(),
            Some("~/.claude/skills/repo-docs-style and ~/src/arko/.skills/docs.md")
        );
    }
}
