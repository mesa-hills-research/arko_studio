//! Settings › Extensions: what agents may add without asking, the MCP
//! servers sessions start (Arko's `mcp.json`, the config files set under
//! Advanced, each project's `.arko/mcp.json` and `.mcp.json` with the answer
//! remembered for it, which can be forgotten) and the skill folders, with
//! the skills found in each, then the skills built into Arko. A skill or a
//! server an agent added says when and why, and can be removed.

use std::path::{Path, PathBuf};

use arko::extension_prefs::{
    ASK_MCP_ABOUT, ASK_MCP_NAME, ASK_MCP_OFF, CANCEL, REVIEW_SKILLS_ABOUT, REVIEW_SKILLS_NAME,
    TURN_OFF,
};
use arko::extensions::Added;
use gpui_kit::component::button::{Button, ButtonVariant};
use gpui_kit::component::notification::Notification;
use gpui_kit::component::setting::{SettingField, SettingGroup, SettingItem, SettingPage};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::ManagedTooltipExt as _;
use gpui_kit::component::{ActiveTheme as _, IconName, Sizable as _, WindowExt as _, h_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::SharedBackend;
use crate::backend::account_words::tilde;
use crate::backend::extensions::{
    self as store, Approval, BuiltinSkill, Extensions, McpFile, McpServer, ProjectServers,
    SkillFolder, SkillsOf,
};
use crate::live::{LiveSession, LiveSessions, Phase};
use crate::prefs;
use crate::session_view::{HomeFolder, recent_folders};
use crate::workers::curly;

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

    /// Forgets the answer about the servers of the project in `folder`, of
    /// its `.arko/mcp.json` when `local`, else of its `.mcp.json`, so its next
    /// session asks again.
    pub fn forget(
        this: &Entity<Self>,
        folder: PathBuf,
        local: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let backend = cx.global::<SharedBackend>().clone();
        let home = this.read(cx).home.clone();
        let shown = tilde(&folder, home.as_deref());
        let forgetting =
            cx.background_spawn(async move { backend.forget_mcp_approval(&folder, local) });
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

    /// Turns a switch of the Approvals group on or off. The page shows it at
    /// once.
    fn approve(
        this: &Entity<Self>,
        switch: store::Switch,
        on: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        this.update(cx, |page, cx| {
            if let Some(extensions) = page.extensions.as_mut() {
                let approvals = &mut extensions.approvals;
                match switch {
                    store::Switch::AskMcp => approvals.ask_before_adding_mcp = on,
                    store::Switch::ReviewSkills => approvals.review_project_skills = on,
                    store::Switch::KeepOutOfGit => {}
                }
                cx.notify();
            }
        });
        let page = this.clone();
        set_switch(switch, on, window, cx, move |cx| {
            page.update(cx, |page, cx| page.refresh(cx))
        });
    }

    /// Asks before letting agents add MCP servers without asking, then
    /// does.
    fn confirm_ask_mcp_off(this: &Entity<Self>, window: &mut Window, cx: &mut App) {
        let this = this.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let this = this.clone();
            alert
                .description(ASK_MCP_OFF)
                .confirm()
                .ok_text(TURN_OFF)
                .ok_variant(ButtonVariant::Danger)
                .cancel_text(CANCEL)
                .on_ok(move |_, window, cx| {
                    ExtensionsPage::approve(&this, store::Switch::AskMcp, false, window, cx);
                    true
                })
        });
    }

    /// The switches for what agents may add without asking.
    fn approvals_group(&self, this: &Entity<Self>, revision: u64) -> SettingGroup {
        let approvals = self
            .extensions
            .as_ref()
            .map(|extensions| extensions.approvals)
            .unwrap_or_default();
        let ask = this.clone();
        let review = this.clone();
        SettingGroup::new()
            .key(super::group_key("Approvals", &(revision, approvals)))
            .title("Approvals")
            .description("What agents may add without asking you.")
            .items([
                SettingItem::new(
                    ASK_MCP_NAME,
                    SettingField::render(move |_, _, _| {
                        let page = ask.clone();
                        Switch::new("ask-before-adding-mcp")
                            .checked(approvals.ask_before_adding_mcp)
                            .on_click(move |on, window, cx| {
                                if *on {
                                    ExtensionsPage::approve(
                                        &page,
                                        store::Switch::AskMcp,
                                        true,
                                        window,
                                        cx,
                                    );
                                } else {
                                    ExtensionsPage::confirm_ask_mcp_off(&page, window, cx);
                                }
                            })
                            .into_any_element()
                    }),
                )
                .description(ASK_MCP_ABOUT)
                .keywords(["mcp", "servers", "approval", "agent"]),
                SettingItem::new(
                    REVIEW_SKILLS_NAME,
                    SettingField::render(move |_, _, _| {
                        let page = review.clone();
                        Switch::new("review-project-skills")
                            .checked(approvals.review_project_skills)
                            .on_click(move |on, window, cx| {
                                ExtensionsPage::approve(
                                    &page,
                                    store::Switch::ReviewSkills,
                                    *on,
                                    window,
                                    cx,
                                )
                            })
                            .into_any_element()
                    }),
                )
                .description(REVIEW_SKILLS_ABOUT)
                .keywords(["skills", "SKILL.md", "approval", "agent"]),
            ])
    }

    /// Asks before removing what an agent added, then removes it.
    fn confirm_remove(this: &Entity<Self>, added: Added, window: &mut Window, cx: &mut App) {
        let (title, description) = remove_words(&added);
        let this = this.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let (this, added) = (this.clone(), added.clone());
            alert
                .title(title.clone())
                .description(description)
                .confirm()
                .ok_text("Remove")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    ExtensionsPage::remove(&this, added.clone(), window, cx);
                    true
                })
        });
    }

    /// Removes what an agent added, off the UI thread, and says how it went.
    fn remove(this: &Entity<Self>, added: Added, window: &mut Window, cx: &mut App) {
        let backend = cx.global::<SharedBackend>().clone();
        let removing = cx.background_spawn(async move { backend.remove_added(added) });
        let this = this.clone();
        window
            .spawn(cx, async move |cx| {
                let removed = removing.await;
                cx.update(|window, cx| {
                    match removed {
                        Ok(said) => window.push_notification(curly(&said), cx),
                        Err(why) => window.push_notification(Notification::error(curly(&why)), cx),
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
        // Each Remove button has a number of its own.
        let mut removes = 0;
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
            Some(extensions) => {
                let mut files = Vec::new();
                for file in &extensions.files {
                    files.push(file_item(file, home));
                    if let Some(Ok(servers)) = &file.servers {
                        files.extend(added_servers(this, servers, &mut removes));
                    }
                }
                let mut projects = Vec::new();
                for (ix, project) in extensions.projects.iter().enumerate() {
                    projects.push(project_item(this, ix, project, home));
                    if let Ok(servers) = &project.servers {
                        projects.extend(added_servers(this, servers, &mut removes));
                    }
                }
                let mut skills = Vec::new();
                for folder in &extensions.skills {
                    skills.push(skills_item(folder, home));
                    skills.extend(added_skills(this, folder, &mut removes));
                }
                if skills.is_empty() {
                    skills.push(
                        SettingItem::new("No skill folders", SettingField::render(|_, _, _| div()))
                            .description(
                                "Make one of the folders above, with a folder per skill holding its SKILL.md.",
                            )
                            .keywords(["skills"]),
                    );
                }
                (files, projects, skills)
            }
        };
        let mut groups = vec![
            self.approvals_group(this, revision),
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
                        "A project’s .arko/mcp.json and .mcp.json would run programs as soon as a session opens in its folder, so Arko asks first, and remembers the answer until the file’s servers change.",
                    )
                    .items(projects),
            );
        }
        groups.push(
            SettingGroup::new()
                .key(super::group_key("Skills", &revision))
                .title("Skills")
                .description(
                    "Instructions for particular kinds of work, which the agents load when a task calls for them. A skill is a folder with a SKILL.md, or a Markdown file, whose front matter gives its name and description. Arko reads a project’s .agents/skills, .claude/skills, .skills and .arko/skills, then skills in Arko’s folder, ~/.agents/skills and ~/.claude/skills. A skill named as an earlier one gets a number (pdf-2), and the same skill twice is read once.",
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
    let (folder, local) = (project.folder.clone(), project.local);
    let this = this.clone();
    SettingItem::new(
        tilde(&project.file, home),
        SettingField::render(move |_, _, _| {
            let (this, folder) = (this.clone(), folder.clone());
            div().when(remembered, |div| {
                div.child(
                    Button::new(("forget-approval", ix))
                        .outline()
                        .small()
                        .label("Forget answer")
                        .on_click(move |_, window, cx| {
                            ExtensionsPage::forget(&this, folder.clone(), local, window, cx)
                        }),
                )
            })
        }),
    )
    .description(format!("{servers} {answer}"))
    .keywords(["mcp", ".mcp.json", ".arko/mcp.json", "approval", "project"])
}

/// A row with Remove for each server of `servers` an agent added, saying
/// when and why. `removes` numbers the buttons.
fn added_servers(
    this: &Entity<ExtensionsPage>,
    servers: &[McpServer],
    removes: &mut usize,
) -> Vec<SettingItem> {
    servers
        .iter()
        .filter_map(|server| {
            let record = server.added.as_ref()?;
            let added = Added::Server {
                file: record.file.clone(),
                name: server.name.clone(),
            };
            let label = format!("Remove the MCP server {}", server.name);
            let row = remove_row(this, *removes, server.name.clone(), added, label)
                .description(store::server_added_words(record))
                .keywords(["mcp", "servers", "agent", "added"]);
            *removes += 1;
            Some(row)
        })
        .collect()
}

/// A row with Remove for each skill of `folder` an agent saved, saying when
/// and why. `removes` numbers the buttons.
fn added_skills(
    this: &Entity<ExtensionsPage>,
    folder: &SkillFolder,
    removes: &mut usize,
) -> Vec<SettingItem> {
    folder
        .skills
        .iter()
        .filter_map(|skill| {
            let record = skill.added.as_ref()?;
            let added = Added::Skill {
                path: skill.path.clone(),
            };
            let label = format!("Remove the skill {}", skill.name);
            let row = remove_row(this, *removes, skill.name.clone(), added, label)
                .description(store::skill_added_words(record))
                .keywords(["skills", "SKILL.md", "agent", "added"]);
            *removes += 1;
            Some(row)
        })
        .collect()
}

/// A row titled `title` with a Remove button, the `ix`th, that asks before
/// removing `added`. `label` is what a screen reader hears of the button.
fn remove_row(
    this: &Entity<ExtensionsPage>,
    ix: usize,
    title: String,
    added: Added,
    label: String,
) -> SettingItem {
    let this = this.clone();
    SettingItem::new(
        title,
        SettingField::render(move |_, _, _| {
            let (this, added) = (this.clone(), added.clone());
            Button::new(("remove-added", ix))
                .outline()
                .small()
                .label("Remove")
                .accessibility_label(label.clone())
                .on_click(move |_, window, cx| {
                    ExtensionsPage::confirm_remove(&this, added.clone(), window, cx)
                })
        }),
    )
}

/// What Remove asks before removing what an agent added: its question and
/// what removing does.
fn remove_words(added: &Added) -> (String, &'static str) {
    match added {
        Added::Skill { path } => (
            format!(
                "Remove the skill {}?",
                path.file_name().unwrap_or_default().to_string_lossy()
            ),
            "Arko deletes its folder.",
        ),
        Added::Server { name, .. } => (
            format!("Remove the MCP server {name}?"),
            "Sessions open now keep it running until they close.",
        ),
    }
}

/// The sessions open in this app that take commands now.
fn open_sessions(cx: &App) -> Vec<Entity<LiveSession>> {
    LiveSessions::all(cx)
        .into_iter()
        .filter(|session| matches!(session.read(cx).phase(), Phase::Opening | Phase::Open))
        .collect()
}

/// Sets a switch of what agents may add, as `/extensions` does in a
/// session, off the UI thread, then `saved` runs. The git switch follows in
/// the `.arko` folders of the projects of the sessions open in this app, or
/// without one of the latest session's. The open sessions then read the
/// switches again and send them to their web pages, with nothing in their
/// transcripts.
pub(super) fn set_switch(
    switch: store::Switch,
    on: bool,
    window: &mut Window,
    cx: &mut App,
    saved: impl FnOnce(&mut App) + 'static,
) {
    let sessions = open_sessions(cx);
    let mut workspaces: Vec<PathBuf> = Vec::new();
    for session in &sessions {
        if let Some(workspace) = session.read(cx).workspace()
            && !workspaces.contains(workspace)
        {
            workspaces.push(workspace.clone());
        }
    }
    let details: Vec<_> = sessions
        .iter()
        .filter_map(|session| Some(session.read(cx).link()?.details.clone()))
        .collect();
    let backend = cx.global::<SharedBackend>().clone();
    let saving = cx.background_spawn(async move {
        if workspaces.is_empty() {
            workspaces.extend(
                backend
                    .sessions()
                    .map(|sessions| recent_folders(&sessions))
                    .unwrap_or_default()
                    .into_iter()
                    .next(),
            );
        }
        let result = backend.set_extension_switch(switch, on, &workspaces);
        for details in details {
            details.refresh_settings();
        }
        result
    });
    window
        .spawn(cx, async move |cx| {
            let result = saving.await;
            cx.update(|window, cx| {
                if let Err(why) = result {
                    window.push_notification(Notification::error(curly(&why)), cx);
                }
                saved(cx);
            })
            .ok();
        })
        .detach();
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
            .map(|skill| {
                if let Some(why) = &skill.skipped {
                    return format!("{} (skipped because {why})", skill.name);
                }
                let mut notes = store::skill_notes(None, &skill.models, skill.workers);
                if skill.auto_load {
                    notes.insert(0, "loaded automatically".into());
                }
                if notes.is_empty() {
                    skill.name.clone()
                } else {
                    format!("{} ({})", skill.name, notes.join(", "))
                }
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
                                .managed_tooltip({
                                    let replaced = replaced.clone();
                                    move |window, cx| {
                                        gpui_kit::component::tooltip::Tooltip::new(replaced.clone())
                                            .build(window, cx)
                                    }
                                })
                                .child(replaced),
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

    use super::{Added, BuiltinSkill, remove_words, replaced_words};

    #[test]
    fn remove_asks_in_a_few_words() {
        assert_eq!(
            remove_words(&Added::Skill {
                path: "/home/ada/src/arko/.arko/skills/release-notes".into()
            }),
            (
                "Remove the skill release-notes?".to_string(),
                "Arko deletes its folder."
            )
        );
        assert_eq!(
            remove_words(&Added::Server {
                file: "/home/ada/src/arko/.arko/mcp.json".into(),
                name: "github".into()
            }),
            (
                "Remove the MCP server github?".to_string(),
                "Sessions open now keep it running until they close."
            )
        );
    }

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
