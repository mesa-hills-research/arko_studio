//! Session info (click the title, Session › Info, or the palette): the
//! session's folder, mode and models, its handover sizes, its settings,
//! its MCP servers, skills and indexes, its ID and data folder. The
//! settings that can change while it runs change here, through the same
//! commands the terminal sends. Those fixed once the conversation started
//! say so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arko::details::{McpFacts, SessionFacts, SkillsFacts};
use arko::remote::Reading;
use arko::{ExtraRules, ProjectFiles};
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _, Toggle, ToggleGroup},
    h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::Mode;
use crate::live::LiveSession;
use crate::session_view::{HomeFolder, shown_folder};
use crate::status_bar::run;

/// Why some settings can't change any more.
pub const FIXED: &str = "Fixed once the conversation started.";

/// What the session's details say, read off the UI thread.
#[derive(Debug, Clone)]
pub struct Facts {
    pub session: SessionFacts,
    pub mcp: McpFacts,
    pub skills: Result<SkillsFacts, String>,
}

/// A handover size as `/handover-at` takes it: "950k".
pub fn handover_value(tokens: usize) -> String {
    format!("{}k", tokens / 1000)
}

/// After how long a running command moves to the background, as
/// `/background-after` takes it: "off", "90s", "10m", "1h".
pub fn background_value(seconds: u64) -> String {
    match seconds {
        0 => "off".into(),
        seconds if seconds % 3_600 == 0 => format!("{}h", seconds / 3_600),
        seconds if seconds % 60 == 0 => format!("{}m", seconds / 60),
        seconds => format!("{seconds}s"),
    }
}

/// The command a field's new value sends, `None` when it says nothing.
pub fn field_command(field: Field, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(match field {
        Field::HandoverMain => format!("/handover-at main {value}"),
        Field::HandoverWorkers => format!("/handover-at workers {value}"),
        Field::BackgroundAfter => format!("/background-after {value}"),
    })
}

/// The fields typed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    HandoverMain,
    HandoverWorkers,
    BackgroundAfter,
}

impl Field {
    /// The field's value in `reading`.
    fn value(self, reading: &Reading) -> String {
        let settings = &reading.settings;
        match self {
            Field::HandoverMain => Some(settings.handover_at)
                .filter(|at| *at > 0)
                .map(handover_value)
                .unwrap_or_default(),
            Field::HandoverWorkers => Some(settings.workers_handover_at)
                .filter(|at| *at > 0)
                .map(handover_value)
                .unwrap_or_default(),
            Field::BackgroundAfter => background_value(settings.background_after),
        }
    }
}

/// What the info says of a session it isn't connected to (previews and
/// screenshots).
#[derive(Debug, Clone)]
pub struct Preview {
    pub title: String,
    pub mode: Mode,
    pub workspace: Option<PathBuf>,
    pub id: String,
    pub dir: Option<PathBuf>,
    pub reading: Arc<Reading>,
    /// The banner's lines of tools: MCP servers, skills, indexes.
    pub tools: Vec<String>,
}

/// The session's facts the info shows.
struct About {
    title: String,
    mode: Mode,
    workspace: Option<PathBuf>,
    id: String,
    dir: Option<PathBuf>,
    reading: Arc<Reading>,
    indexes: Option<String>,
}

/// The info of the session a window shows, for its popover.
pub struct SessionInfo {
    session: Option<Entity<LiveSession>>,
    preview: Option<Preview>,
    facts: Option<Facts>,
    fields: Vec<(Field, Entity<InputState>)>,
    _load: Option<Task<()>>,
    _session: Option<Subscription>,
    _fields: Vec<Subscription>,
}

impl SessionInfo {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let fields: Vec<(Field, Entity<InputState>)> = [
            (Field::HandoverMain, "950k"),
            (Field::HandoverWorkers, "200k"),
            (Field::BackgroundAfter, "10m or off"),
        ]
        .into_iter()
        .map(|(field, placeholder)| {
            (
                field,
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder)),
            )
        })
        .collect();
        let subscriptions = fields
            .iter()
            .map(|(field, input)| {
                let field = *field;
                cx.subscribe_in(input, window, move |this, input, event, window, cx| {
                    match event {
                        InputEvent::PressEnter { .. } => {
                            let value = input.read(cx).value().to_string();
                            this.apply(field, &value, window, cx);
                        }
                        // Leaving a field unapplied shows the setting again.
                        InputEvent::Blur => this.sync_fields(window, cx),
                        _ => {}
                    }
                })
            })
            .collect();
        Self {
            session: None,
            preview: None,
            facts: None,
            fields,
            _load: None,
            _session: None,
            _fields: subscriptions,
        }
    }

    /// Shows `session`'s info.
    pub fn set_session(
        &mut self,
        session: Option<Entity<LiveSession>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.session.as_ref().map(Entity::entity_id) == session.as_ref().map(Entity::entity_id) {
            return;
        }
        self.facts = None;
        self._load = None;
        self._session = session.as_ref().map(|session| {
            cx.observe_in(session, window, |this, _, window, cx| {
                this.sync_fields(window, cx);
                cx.notify();
            })
        });
        self.session = session;
        self.sync_fields(window, cx);
        cx.notify();
    }

    pub fn session(&self) -> Option<&Entity<LiveSession>> {
        self.session.as_ref()
    }

    /// The field typed into for `field` (tests).
    #[cfg(test)]
    pub fn field(&self, field: Field) -> Entity<InputState> {
        self.fields
            .iter()
            .find(|(kind, _)| *kind == field)
            .map(|(_, input)| input.clone())
            .unwrap()
    }

    /// What the details said when the info last opened.
    pub fn facts(&self) -> Option<&Facts> {
        self.facts.as_ref()
    }

    /// Reads the session's details again, off the UI thread (as the info
    /// opens).
    pub fn load(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self
            .session
            .as_ref()
            .and_then(|session| session.read(cx).link())
            .map(|link| link.details.clone())
        else {
            return;
        };
        let load = cx.background_spawn(async move {
            Facts {
                session: details.session(),
                mcp: details.mcp(),
                skills: details.skills(),
            }
        });
        self._load = Some(cx.spawn(async move |this, cx| {
            let facts = load.await;
            this.update(cx, |this, cx| {
                this.facts = Some(facts);
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows `preview` and `facts`, with no session behind them (its
    /// controls send nothing).
    pub fn set_preview(
        &mut self,
        preview: Preview,
        facts: Facts,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.preview = Some(preview);
        self.facts = Some(facts);
        self.sync_fields(window, cx);
        cx.notify();
    }

    /// The facts shown: the session's, else the preview's.
    fn about(&self, cx: &App) -> Option<About> {
        if let Some(session) = &self.session {
            let live = session.read(cx);
            return Some(About {
                title: live.title().to_string(),
                mode: live.mode(),
                workspace: live.workspace().cloned(),
                id: live.id().map(|id| id.to_string()).unwrap_or_default(),
                dir: live.link().map(|link| link.info.dir.clone()),
                reading: live.reading().clone(),
                indexes: live
                    .link()
                    .and_then(|link| indexes_line(&link.banner.tools)),
            });
        }
        self.preview.as_ref().map(|preview| About {
            title: preview.title.clone(),
            mode: preview.mode,
            workspace: preview.workspace.clone(),
            id: preview.id.clone(),
            dir: preview.dir.clone(),
            reading: preview.reading.clone(),
            indexes: indexes_line(&preview.tools),
        })
    }

    /// The fields show the session's settings, but for the one being
    /// typed in.
    fn sync_fields(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reading = self.reading(cx);
        for (field, input) in &self.fields {
            if input.read(cx).focus_handle(cx).is_focused(window) {
                continue;
            }
            let value = field.value(&reading);
            if input.read(cx).value() != value.as_str() {
                input.update(cx, |input, cx| input.set_value(value, window, cx));
            }
        }
    }

    fn reading(&self, cx: &App) -> Arc<Reading> {
        self.about(cx)
            .map(|about| about.reading)
            .unwrap_or_default()
    }

    fn apply(&mut self, field: Field, value: &str, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(session), Some(command)) = (self.session.clone(), field_command(field, value))
        else {
            return;
        };
        run(&session, &command, window, cx);
    }

    fn send(&self, command: String, window: &mut Window, cx: &mut App) {
        if let Some(session) = &self.session {
            run(session, &command, window, cx);
        }
    }
}

/// A label and its value, in the info's two columns.
fn row(label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .items_start()
        .gap_3()
        .min_h(rems(1.75))
        .child(
            div()
                .w(rems(8.5))
                .flex_none()
                .pt(rems(0.25))
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().text_sm().child(value))
}

/// A group's heading.
fn heading(text: &'static str, cx: &App) -> Div {
    div()
        .pt_2()
        .text_xs()
        .font_medium()
        .text_color(cx.theme().muted_foreground)
        .child(text)
}

/// A path, shortened under the home folder, with a button that shows it in
/// the file manager.
fn path_value(id: &'static str, path: &Path, cx: &App) -> Div {
    let shown = shown_folder(path, cx.global::<HomeFolder>().0.as_deref());
    let reveal: PathBuf = path.to_path_buf();
    h_flex()
        .gap_1()
        .min_w_0()
        .child(div().min_w_0().truncate().child(shown))
        .child(
            Button::new(id)
                .ghost()
                .xsmall()
                .icon(IconName::FolderOpen)
                .tooltip("Show in the file manager")
                .on_click(move |_, _, cx| cx.reveal_path(&reveal)),
        )
}

impl Render for SessionInfo {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(About {
            title,
            mode,
            workspace,
            id,
            dir,
            reading,
            indexes,
        }) = self.about(cx)
        else {
            return v_flex()
                .id("session-info")
                .test_support()
                .w(rems(26.))
                .text_sm()
                .child("No session is open.");
        };
        let agent = mode == Mode::Agent;
        let started = reading.models.started;
        let settings = reading.settings.clone();
        let facts = self.facts.clone();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let view = cx.entity().downgrade();
        let send = move |command: String| {
            let view = view.clone();
            move |window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| this.send(command.clone(), window, cx))
                    .ok();
            }
        };
        let input = |field: Field, this: &Self| {
            this.fields
                .iter()
                .find(|(kind, _)| *kind == field)
                .map(|(_, input)| Input::new(input).small().w(rems(6.)))
        };

        let main_model = facts
            .as_ref()
            .map(|facts| facts.session.main.shown.clone())
            .filter(|shown| !shown.is_empty())
            .unwrap_or_else(|| reading.models.banner_model.clone());
        let workers_model = facts
            .as_ref()
            .and_then(|facts| facts.session.workers.as_ref())
            .map(|workers| workers.model.shown.clone())
            .or_else(|| {
                (reading.models.has_workers && !reading.models.banner_workers.is_empty())
                    .then(|| reading.models.banner_workers.clone())
            });
        let compacts = facts
            .as_ref()
            .is_some_and(|facts| facts.session.handover.main_compacts);

        let switch = |id: &'static str, on: bool, locked: bool, on_cmd: String, off_cmd: String| {
            let apply_on = send(on_cmd);
            let apply_off = send(off_cmd);
            Switch::new(id)
                .small()
                .checked(on)
                .disabled(locked)
                .when(locked, |this| this.tooltip(FIXED))
                .on_click(move |checked, window, cx| {
                    if *checked {
                        apply_on(window, cx)
                    } else {
                        apply_off(window, cx)
                    }
                })
        };

        let extra_rules = {
            let choose =
                ExtraRules::ALL.map(|rules| send(format!("/extra-rules {}", rules.name())));
            let current = settings.extra_rules;
            ToggleGroup::new("extra-rules")
                .segmented()
                .small()
                .disabled(started)
                .children(ExtraRules::ALL.map(|rules| {
                    Toggle::new(rules.name())
                        .label(match rules {
                            ExtraRules::Small => "Small models",
                            ExtraRules::All => "All",
                            ExtraRules::Off => "Off",
                        })
                        .checked(rules == current)
                }))
                .on_click(move |checks, window, cx| {
                    if let Some(ix) = clicked(checks, ExtraRules::ALL.map(|rules| rules == current))
                    {
                        choose[ix](window, cx)
                    }
                })
        };
        let project = {
            let choose = ProjectFiles::ALL
                .map(|files| send(format!("/project-instructions {}", files.name())));
            let current = settings.project_instructions;
            ToggleGroup::new("project-instructions")
                .segmented()
                .small()
                .children(ProjectFiles::ALL.map(|files| {
                    Toggle::new(files.name())
                        .label(match files {
                            ProjectFiles::One => "One",
                            ProjectFiles::Both => "Both",
                            ProjectFiles::Off => "Off",
                        })
                        .checked(files == current)
                }))
                .on_click(move |checks, window, cx| {
                    if let Some(ix) =
                        clicked(checks, ProjectFiles::ALL.map(|files| files == current))
                    {
                        choose[ix](window, cx)
                    }
                })
        };

        let mut body = v_flex().gap_1();
        // Where it works, and on what.
        body =
            body.child(heading("Session", cx))
                .when_some(workspace.filter(|_| agent), |this, folder| {
                    this.child(row("Folder", path_value("reveal-folder", &folder, cx), cx))
                })
                .child(row("Mode", mode.label(), cx))
                .child(row(
                    "Model",
                    v_flex().child(main_model).when(started, |this| {
                        this.child(
                            div().text_xs().text_color(muted).child(
                                "The main agent keeps its model once the conversation starts.",
                            ),
                        )
                    }),
                    cx,
                ))
                .when_some(workers_model.filter(|_| agent), |this, model| {
                    this.child(row("Workers’ model", model, cx))
                });
        // When the agents hand over.
        body = body
            .child(heading("Handover", cx))
            .child(row(
                if compacts {
                    "Main agent compacts at"
                } else {
                    "Main agent at"
                },
                h_flex()
                    .gap_2()
                    .children(input(Field::HandoverMain, self))
                    .when_some(reading.context_window, |this, window| {
                        this.child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!("of {}", crate::figures::tokens(window))),
                        )
                    }),
                cx,
            ))
            .when(agent && reading.models.has_workers, |this| {
                this.child(row(
                    "Workers at",
                    h_flex().children(input(Field::HandoverWorkers, self)),
                    cx,
                ))
            });
        if agent {
            body = body
                .child(heading("Settings", cx))
                .child(row(
                    "Outside access",
                    switch(
                        "outside-access",
                        settings.outside,
                        false,
                        "/outside on".into(),
                        "/outside off".into(),
                    ),
                    cx,
                ))
                .child(row(
                    "Background after",
                    h_flex().children(input(Field::BackgroundAfter, self)),
                    cx,
                ))
                .child(row(
                    "Text replies",
                    switch(
                        "text-replies",
                        settings.text_replies,
                        started,
                        "/text-replies on".into(),
                        "/text-replies off".into(),
                    ),
                    cx,
                ))
                .child(row(
                    "Claude Code compaction",
                    switch(
                        "cc-compact",
                        settings.cc_compact,
                        started,
                        "/cc-compact on".into(),
                        "/cc-compact off".into(),
                    ),
                    cx,
                ))
                .child(row("Extra rules", extra_rules, cx))
                .when(started, |this| {
                    this.child(
                        div()
                            .pl(rems(9.25))
                            .text_xs()
                            .text_color(muted)
                            .child("Text replies, Claude Code compaction and the extra rules are fixed once the conversation started."),
                    )
                })
                .child(row(
                    "Project instructions",
                    v_flex()
                        .gap_1()
                        .child(project)
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(format!(
                                    "{}, from the next conversation.",
                                    crate::backend::sentence(settings.project_instructions.describe())
                                        .trim_end_matches('.')
                                )),
                        )
                        .when(reading.models.has_workers, |this| {
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .child(switch(
                                        "project-workers",
                                        settings.worker_project_instructions,
                                        false,
                                        "/project-instructions workers on".into(),
                                        "/project-instructions workers off".into(),
                                    ))
                                    .child("Workers get them too"),
                            )
                        }),
                    cx,
                ));
            // What the agents have besides their own tools.
            body = body.child(heading("Extensions", cx));
            body = match &facts {
                None => body.child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .child("Reading the session’s details…"),
                ),
                Some(facts) => body
                    .child(row(
                        "MCP servers",
                        v_flex().children(
                            mcp_lines(&facts.mcp)
                                .into_iter()
                                .map(|line| div().text_sm().child(line)),
                        ),
                        cx,
                    ))
                    .child(row("Skills", skills_line(&facts.skills), cx)),
            };
            body = body.child(row(
                "Indexes",
                indexes.unwrap_or_else(|| "None".to_string()),
                cx,
            ));
        }
        // Where it's kept.
        body = body
            .child(heading("Kept as", cx))
            .child(row(
                "Session ID",
                h_flex()
                    .gap_1()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_family(theme.mono_font_family.clone())
                            .text_xs()
                            .child(id.clone()),
                    )
                    .child(
                        Clipboard::new("copy-session-id")
                            .value(id)
                            .tooltip("Copy the session ID"),
                    ),
                cx,
            ))
            .when_some(dir, |this, dir| {
                this.child(row("Data folder", path_value("reveal-data", &dir, cx), cx))
            });

        // As tall as the window allows, scrolling past that.
        let tallest = window.viewport_size().height * 0.8;
        v_flex()
            .id("session-info")
            .test_support()
            .w(rems(28.))
            .max_h(tallest)
            .overflow_y_scroll()
            .child(
                div()
                    .flex_none()
                    .text_sm()
                    .font_medium()
                    .truncate()
                    .child(title),
            )
            .child(body.flex_none())
    }
}

/// Which toggle a click changed: the first whose state differs from
/// `before`.
fn clicked<const N: usize>(after: &[bool], before: [bool; N]) -> Option<usize> {
    (0..N).find(|ix| after.get(*ix).copied().unwrap_or(false) != before[*ix])
}

/// The MCP servers, a line each: "files — 4 tools", "web — didn't start:
/// …".
pub fn mcp_lines(mcp: &McpFacts) -> Vec<String> {
    let mut lines: Vec<String> = mcp
        .servers
        .iter()
        .map(|server| match &server.tools {
            Ok(tools) => format!(
                "{} — {} tool{}",
                server.name,
                tools.len(),
                if tools.len() == 1 { "" } else { "s" }
            ),
            Err(why) => format!("{} — didn’t start: {why}", server.name),
        })
        .collect();
    lines.extend(mcp.notes.iter().cloned());
    if lines.is_empty() {
        lines.push("None".to_string());
    }
    lines
}

/// The skills, in a line: "3: release-notes, docs-style, triage".
pub fn skills_line(skills: &Result<SkillsFacts, String>) -> String {
    match skills {
        Err(why) => format!("Couldn’t be read: {why}"),
        Ok(skills) if skills.skills.is_empty() => "None".to_string(),
        Ok(skills) => {
            let names: Vec<&str> = skills
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect();
            format!("{}: {}", names.len(), names.join(", "))
        }
    }
}

/// The indexes, from the banner's line ("indexes arko (code, monitored),
/// docs (docs)"): what follows the word.
pub fn indexes_line(tools: &[String]) -> Option<String> {
    tools.iter().find_map(|line| {
        line.strip_prefix("indexes ")
            .or_else(|| line.strip_prefix("index "))
            .map(str::to_string)
    })
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        Field, background_value, clicked, field_command, handover_value, indexes_line, mcp_lines,
        skills_line,
    };
    use arko::details::{McpFacts, McpServerFacts, SkillFacts, SkillsFacts};
    use std::path::PathBuf;

    #[test]
    fn fields_send_the_terminals_commands() {
        assert_eq!(handover_value(950_000), "950k");
        assert_eq!(handover_value(1_020_000), "1020k");
        assert_eq!(background_value(0), "off");
        assert_eq!(background_value(600), "10m");
        assert_eq!(background_value(3_600), "1h");
        assert_eq!(background_value(90), "90s");
        assert_eq!(
            field_command(Field::HandoverMain, " 300k ").as_deref(),
            Some("/handover-at main 300k")
        );
        assert_eq!(
            field_command(Field::HandoverWorkers, "200k").as_deref(),
            Some("/handover-at workers 200k")
        );
        assert_eq!(
            field_command(Field::BackgroundAfter, "off").as_deref(),
            Some("/background-after off")
        );
        assert_eq!(field_command(Field::BackgroundAfter, "  "), None);
        for command in [
            "/handover-at main 300k",
            "/background-after off",
            "/outside on",
            "/text-replies off",
            "/cc-compact on",
            "/extra-rules small",
            "/project-instructions both",
            "/project-instructions workers on",
        ] {
            assert!(arko::commands::find(command).is_some(), "{command}");
        }
    }

    #[test]
    fn a_click_in_a_group_is_the_toggle_that_changed() {
        assert_eq!(clicked(&[true, true, false], [false, true, false]), Some(0));
        assert_eq!(
            clicked(&[false, false, false], [false, true, false]),
            Some(1)
        );
        assert_eq!(clicked(&[false, true, false], [false, true, false]), None);
    }

    #[test]
    fn extensions_read_in_a_line_each() {
        let mcp = McpFacts {
            servers: vec![
                McpServerFacts {
                    name: "files".into(),
                    shown: "files-server".into(),
                    file: PathBuf::from(".mcp.json"),
                    tools: Ok(vec!["read".into(), "write".into()]),
                },
                McpServerFacts {
                    name: "web".into(),
                    shown: "web-server".into(),
                    file: PathBuf::from(".mcp.json"),
                    tools: Err("no such program".into()),
                },
            ],
            ..McpFacts::default()
        };
        assert_eq!(
            mcp_lines(&mcp),
            ["files — 2 tools", "web — didn’t start: no such program"]
        );
        assert_eq!(mcp_lines(&McpFacts::default()), ["None"]);
        let skills = SkillsFacts {
            skills: vec![SkillFacts {
                name: "docs-style".into(),
                description: String::new(),
                always: false,
            }],
            folders: Vec::new(),
            own_folder: PathBuf::new(),
        };
        assert_eq!(skills_line(&Ok(skills)), "1: docs-style");
        assert_eq!(
            indexes_line(&["2 skills".into(), "indexes arko (code), docs (docs)".into()])
                .as_deref(),
            Some("arko (code), docs (docs)")
        );
        assert_eq!(
            indexes_line(&["index arko (code)".into()]).as_deref(),
            Some("arko (code)")
        );
        assert_eq!(indexes_line(&[]), None);
    }
}

#[cfg(test)]
mod view_tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::Field;
    use crate::live::LiveSession;
    use crate::tests::{act, open, row_of};
    use crate::{Startup, menus};
    use arko::remote::{Activity, LiveSettings, Models, Reading};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
    use std::sync::Arc;
    use std::time::SystemTime;

    fn start(cx: &mut TestAppContext) {
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    }

    fn shows(window: AnyWindowHandle, cx: &mut TestAppContext, id: &'static str) -> bool {
        cx.update_window(window, |_, window, cx| {
            window.render_frame(cx);
            window.try_find(id).is_some()
        })
        .unwrap()
    }

    /// The title of an open session opens its info, and Session › Info
    /// opens and closes it too.
    #[gpui_kit::test]
    fn the_title_and_the_menu_open_the_session_info(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        assert!(!shows(window, cx, "session-title-button"), "no session yet");
        let row = row_of(&workspace, "Split the engine out of main.rs", cx);
        act(cx, window, |window, cx| window.click(("row", row), cx));
        act(cx, window, |window, cx| {
            window.click("session-title-button", cx)
        });
        assert!(shows(window, cx, "session-info"));
        act(cx, window, |window, cx| {
            window.dispatch_action(Box::new(menus::SessionInfo), cx)
        });
        assert!(!shows(window, cx, "session-info"));
        act(cx, window, |window, cx| {
            window.dispatch_action(Box::new(menus::SessionInfo), cx)
        });
        assert!(shows(window, cx, "session-info"));
    }

    /// The command lines the session was sent.
    fn sent(session: &Entity<LiveSession>, cx: &mut TestAppContext) -> Vec<String> {
        cx.read(|cx| {
            session
                .read(cx)
                .queued()
                .iter()
                .filter_map(|input| match input {
                    arko::Input::Message(message) => Some(message.text.clone()),
                    _ => None,
                })
                .collect()
        })
    }

    /// Live settings change through the terminal's commands. Those fixed
    /// once the conversation started don't.
    #[gpui_kit::test]
    fn live_settings_send_the_terminals_commands(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        let reading = Reading {
            settings: LiveSettings {
                handover_at: 950_000,
                workers_handover_at: 200_000,
                background_after: 600,
                ..LiveSettings::default()
            },
            models: Models {
                started: true,
                has_workers: true,
                ..Models::default()
            },
            ..Reading::default()
        };
        session.update(cx, |session, cx| {
            session.set_status(reading, Activity::default(), cx)
        });
        let title_bar = cx.read(|cx| workspace.read(cx).title_bar().clone());
        cx.update_window(window, |_, window, cx| {
            title_bar.update(cx, |bar, cx| {
                bar.set_live_session(Some(session.clone()), window, cx);
                bar.toggle_info(cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        assert!(shows(window, cx, "session-info"));
        let info = cx.read(|cx| title_bar.read(cx).info().clone());
        // The fields show the settings.
        let main = cx.read(|cx| info.read(cx).field(Field::HandoverMain));
        assert_eq!(cx.read(|cx| main.read(cx).value().to_string()), "950k");
        let after = cx.read(|cx| info.read(cx).field(Field::BackgroundAfter));
        assert_eq!(cx.read(|cx| after.read(cx).value().to_string()), "10m");

        act(cx, window, |window, cx| window.click("outside-access", cx));
        // Fixed: a click does nothing.
        act(cx, window, |window, cx| window.click("text-replies", cx));
        act(cx, window, |window, cx| {
            main.update(cx, |input, cx| {
                input.set_value("300k", window, cx);
                input.focus(window, cx);
            });
        });
        act(cx, window, |window, cx| window.press("enter", cx));
        assert_eq!(
            sent(&session, cx),
            ["/outside on", "/handover-at main 300k"]
        );
    }
}
