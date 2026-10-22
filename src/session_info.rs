//! Session info (click the title, Session › Info, or the palette): the
//! session's folder, mode and models, its handover sizes, its settings,
//! its MCP servers, skills and indexes, its ID and data folder. The
//! settings that can change while it runs change here, through the same
//! commands the terminal sends. Those fixed once the conversation started
//! say so.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use arko::details::{IndexFacts, IndexWatch, McpFacts, SessionFacts, SkillsFacts};
use arko::remote::Reading;
use arko::{ExtraRules, ModelChoice, ProjectFiles};
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
use crate::workers::{curly, models_about};
use arko::workers;

/// Why some settings can't change any more.
pub const FIXED: &str = "Fixed once the conversation started.";

/// What the model pickers and the info say of the main agent's model once
/// the conversation has started.
pub const MODEL_KEPT: &str = "The main agent keeps its model once the conversation starts.";

/// The tooltip over a setting the conversation has fixed, drawn as the
/// kit's tooltips are. The selector lets tests see it.
struct Fixed(AnyView);

impl Render for Fixed {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .debug_selector(|| "fixed-tooltip".into())
            .child(self.0.clone())
    }
}

/// How long a switch or a segment shows the user's choice before the
/// session's status does (the status comes about half a second after a
/// change, and says why not when the session refused it).
pub const HOLD: std::time::Duration = std::time::Duration::from_secs(3);

/// What a control (by its id) shows of `settings`: "on" or "off" for a
/// switch, the choice's name for a segmented group.
fn current(control: &str, settings: &arko::remote::LiveSettings) -> Option<String> {
    let switch = |on: bool| Some(if on { "on" } else { "off" }.to_string());
    match control {
        "outside-access" => switch(settings.outside),
        "text-replies" => switch(settings.text_replies),
        "cc-compact" => switch(settings.cc_compact),
        "project-workers" => switch(settings.worker_project_instructions),
        "extra-rules" => Some(settings.extra_rules.name().to_string()),
        "project-instructions" => Some(settings.project_instructions.name().to_string()),
        "worker-models" => Some(settings.worker_models.name().to_string()),
        "worker-depth" => Some(settings.worker_depth.to_string()),
        _ => None,
    }
}

/// A choice of who picks the workers' models as one word, the one its
/// command takes with a capital, for a segment of the info's control. The
/// segment's tooltip has Arko's label.
fn short_choice(choice: ModelChoice) -> String {
    let name = choice.name();
    let mut letters = name.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// A choice the user made that the session's status doesn't show yet.
struct Held {
    control: &'static str,
    value: String,
    _expires: Task<()>,
}

/// What the session's details say, read off the UI thread.
#[derive(Debug, Clone)]
pub struct Facts {
    pub session: SessionFacts,
    pub mcp: McpFacts,
    pub skills: Result<SkillsFacts, String>,
    /// The workspace's search indexes.
    pub indexes: Vec<IndexFacts>,
}

/// A handover size as the info shows it and `/handover-at` takes it:
/// "950k" for whole thousands, else all of it ("950,500"), never rounded.
pub fn handover_value(tokens: usize) -> String {
    if tokens.is_multiple_of(1000) {
        return format!("{}k", tokens / 1000);
    }
    let digits = tokens.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

use arko::commands::{parse_background_after, parse_handover_size};
use arko::monitors::{self, Wakes, parse_wakes};

/// How long monitors' warnings wake the agent, as a number the fields
/// compare: its seconds, 0 for never, or `u64::MAX` for always.
fn wakes_number(wakes: Wakes) -> u64 {
    wakes.0.unwrap_or(u64::MAX)
}

/// The setting [`wakes_number`] made `number` of.
fn wakes_of(number: u64) -> Wakes {
    Wakes((number != u64::MAX).then_some(number))
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

/// The command that sets `field` to what was typed (`text`), `None` when
/// it says nothing or what is set already (`current`), `Err` with why the
/// text can't be taken.
pub fn field_command(field: Field, text: &str, current: u64) -> Result<Option<String>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value = match field {
        Field::HandoverMain | Field::HandoverWorkers => parse_handover_size(text)? as u64,
        Field::BackgroundAfter => parse_background_after(text)?,
        Field::MonitorWakes => wakes_number(parse_wakes(text)?),
    };
    if value == current {
        return Ok(None);
    }
    Ok(Some(match field {
        Field::HandoverMain => format!("/handover-at main {}", handover_value(value as usize)),
        Field::HandoverWorkers => {
            format!("/handover-at workers {}", handover_value(value as usize))
        }
        Field::BackgroundAfter => format!("/background-after {}", background_value(value)),
        Field::MonitorWakes => wakes_of(value).command(),
    }))
}

/// The fields typed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    HandoverMain,
    HandoverWorkers,
    BackgroundAfter,
    MonitorWakes,
}

impl Field {
    /// The setting the field changes, in `reading`: tokens or seconds.
    fn current(self, reading: &Reading) -> u64 {
        let settings = &reading.settings;
        match self {
            Field::HandoverMain => settings.handover_at as u64,
            Field::HandoverWorkers => settings.workers_handover_at as u64,
            Field::BackgroundAfter => settings.background_after,
            Field::MonitorWakes => wakes_number(settings.monitor_wakes),
        }
    }

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
            Field::MonitorWakes => settings.monitor_wakes.shown(),
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
}

/// The session's facts the info shows.
struct About {
    title: String,
    mode: Mode,
    workspace: Option<PathBuf>,
    id: String,
    dir: Option<PathBuf>,
    reading: Arc<Reading>,
}

/// The info of the session a window shows, for its popover.
pub struct SessionInfo {
    session: Option<Entity<LiveSession>>,
    preview: Option<Preview>,
    facts: Option<Facts>,
    /// The session and its opening the facts are read for.
    facts_for: Option<(EntityId, u64)>,
    /// The info shows: its facts follow the session's openings.
    open: bool,
    /// Choices shown until the session's status does.
    held: Vec<Held>,
    /// The session opening these pending choices and errors belong to.
    controls_for: Option<(EntityId, u64)>,
    /// Why what's typed in a field can't be taken.
    errors: std::collections::HashMap<Field, SharedString>,
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
            (Field::MonitorWakes, "2h or always"),
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
                        // What's typed is checked as it's typed.
                        InputEvent::Change => {
                            let value = input.read(cx).value().to_string();
                            let current = field.current(&this.reading(cx));
                            this.set_error(field, field_command(field, &value, current).err(), cx);
                        }
                        // Leaving a field unapplied shows the setting again.
                        InputEvent::Blur => {
                            this.set_error(field, None, cx);
                            this.sync_fields(window, cx)
                        }
                        _ => {}
                    }
                })
            })
            .collect();
        Self {
            session: None,
            preview: None,
            facts: None,
            facts_for: None,
            open: false,
            held: Vec::new(),
            controls_for: None,
            errors: std::collections::HashMap::new(),
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
        self.facts_for = None;
        self._load = None;
        self._session = session.as_ref().map(|session| {
            cx.observe_in(session, window, |this, _, window, cx| {
                this.follow_controls(cx);
                this.sync_fields(window, cx);
                this.follow_facts(cx);
                cx.notify();
            })
        });
        self.session = session;
        self.follow_controls(cx);
        self.sync_fields(window, cx);
        self.follow_facts(cx);
        cx.notify();
    }

    fn follow_controls(&mut self, cx: &App) {
        let opening = self
            .session
            .as_ref()
            .map(|session| (session.entity_id(), session.read(cx).opening()));
        if self.controls_for != opening {
            self.controls_for = opening;
            self.held.clear();
            self.errors.clear();
        }
    }

    /// The info shows (`true`) or no longer does. Shown, its facts are
    /// read, and read again when the session opens anew.
    pub fn set_open(&mut self, open: bool, cx: &mut Context<Self>) {
        self.open = open;
        if open {
            self.load(cx);
        }
    }

    /// While the info shows, reads the facts of an opening of the session
    /// they aren't read for: the session opened after the info, or opened
    /// again (after a switch between Chat and Agent).
    fn follow_facts(&mut self, cx: &mut Context<Self>) {
        if !self.open {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let live = session.read(cx);
        let opening = (session.entity_id(), live.opening());
        if live.link().is_some() && self.facts_for != Some(opening) {
            self.load(cx);
        }
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
        let Some((details, opening)) = self.session.as_ref().and_then(|session| {
            let live = session.read(cx);
            let details = live.link()?.details.clone();
            Some((details, (session.entity_id(), live.opening())))
        }) else {
            return;
        };
        self.facts_for = Some(opening);
        let load = cx.background_spawn(async move {
            Facts {
                session: details.session(),
                mcp: details.mcp(),
                skills: details.skills(),
                indexes: details.indexes(),
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
            });
        }
        self.preview.as_ref().map(|preview| About {
            title: preview.title.clone(),
            mode: preview.mode,
            workspace: preview.workspace.clone(),
            id: preview.id.clone(),
            dir: preview.dir.clone(),
            reading: preview.reading.clone(),
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
        let Some(session) = self.session.clone() else {
            return;
        };
        match field_command(field, value, field.current(&self.reading(cx))) {
            Ok(Some(command)) => run(&session, &command, window, cx),
            Ok(None) => {}
            Err(why) => self.set_error(field, Some(why), cx),
        }
    }

    /// Shows why what's typed in `field` can't be taken, or nothing.
    fn set_error(&mut self, field: Field, error: Option<String>, cx: &mut Context<Self>) {
        let error = error.map(SharedString::from);
        if self.errors.get(&field) != error.as_ref() {
            match error {
                Some(error) => self.errors.insert(field, error),
                None => self.errors.remove(&field),
            };
            cx.notify();
        }
    }

    /// Why what's typed in `field` can't be taken (tests).
    #[cfg(test)]
    pub fn error(&self, field: Field) -> Option<&SharedString> {
        self.errors.get(&field)
    }

    fn send(&self, command: String, window: &mut Window, cx: &mut App) {
        if let Some(session) = &self.session {
            run(session, &command, window, cx);
        }
    }

    /// Sends `command`, which sets `control` to `value`, and shows that
    /// value at once.
    fn choose(
        &mut self,
        control: &'static str,
        value: String,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.held.retain(|held| held.control != control);
        let expires = cx.spawn(async move |this, cx| {
            cx.background_executor().timer(HOLD).await;
            this.update(cx, |this, cx| {
                this.held.retain(|held| held.control != control);
                cx.notify();
            })
            .ok();
        });
        self.held.push(Held {
            control,
            value,
            _expires: expires,
        });
        self.send(command, window, cx);
        cx.notify();
    }
}

/// A label and its value, in the info's two columns, or the label above
/// its value in a narrow window.
fn row(narrow: bool, label: &'static str, value: impl IntoElement, cx: &App) -> Div {
    let label = div()
        .flex_none()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(label);
    let value = div().min_w_0().text_sm().child(value);
    if narrow {
        return v_flex().gap_0p5().pb_1().child(label).child(value);
    }
    h_flex()
        .items_start()
        .gap_3()
        .min_h(rems(1.75))
        .child(label.w(rems(8.5)).pt(rems(0.25)))
        .child(value.flex_1())
}

/// Windows narrower than this put each label above its value.
const NARROW_BELOW: Pixels = px(480.);

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
        // As wide as the window allows, with a margin.
        let viewport = window.viewport_size();
        let narrow = viewport.width < NARROW_BELOW;
        let width = (window.rem_size() * 28.).min(viewport.width - window.rem_size() * 2.);
        let Some(About {
            title,
            mode,
            workspace,
            id,
            dir,
            reading,
        }) = self.about(cx)
        else {
            return v_flex()
                .id("session-info")
                .test_support()
                .w(width)
                .text_sm()
                .child("No session is open.");
        };
        let agent = mode == Mode::Agent;
        let started = reading.models.started;
        let settings = reading.settings.clone();
        // Choices the status now shows are held no more.
        self.held
            .retain(|held| current(held.control, &settings).as_ref() != Some(&held.value));
        let held: Vec<(&'static str, String)> = self
            .held
            .iter()
            .map(|held| (held.control, held.value.clone()))
            .collect();
        let shown = move |control: &'static str| {
            held.iter()
                .find(|(held, _)| *held == control)
                .map(|(_, value)| value.clone())
                .or_else(|| current(control, &settings))
                .unwrap_or_default()
        };
        let settings = reading.settings.clone();
        let facts = self.facts.clone();
        let theme = cx.theme();
        let muted = theme.muted_foreground;
        let view = cx.entity().downgrade();
        let choose = move |control: &'static str, value: &str, command: String| {
            let (view, value) = (view.clone(), value.to_string());
            move |window: &mut Window, cx: &mut App| {
                view.update(cx, |this, cx| {
                    this.choose(control, value.clone(), command.clone(), window, cx)
                })
                .ok();
            }
        };
        let danger = theme.danger;
        let input = |field: Field, this: &Self| {
            let error = this.errors.get(&field).cloned();
            this.fields
                .iter()
                .find(|(kind, _)| *kind == field)
                .map(|(_, input)| Input::new(input).small().w(rems(6.)))
                .map(|input| {
                    v_flex()
                        .gap_0p5()
                        // Enter applies the field, and the info stays open.
                        .on_action(|_: &gpui_kit::base::actions::Confirm, _, _| {})
                        .child(input)
                        .when_some(error, |this, error| {
                            this.child(div().text_xs().text_color(danger).child(error))
                        })
                })
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

        let switch = |id: &'static str,
                      label: &'static str,
                      locked: bool,
                      on_cmd: String,
                      off_cmd: String| {
            let apply_on = choose(id, "on", on_cmd);
            let apply_off = choose(id, "off", off_cmd);
            Switch::new(id)
                .small()
                .accessibility_label(label)
                .checked(shown(id) == "on")
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
            let choose = ExtraRules::ALL.map(|rules| {
                choose(
                    "extra-rules",
                    rules.name(),
                    format!("/extra-rules {}", rules.name()),
                )
            });
            let current = ExtraRules::ALL
                .into_iter()
                .find(|rules| rules.name() == shown("extra-rules"))
                .unwrap_or(settings.extra_rules);
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
        // Locked with the others, and says so on hover.
        let extra_rules = h_flex().child(
            div()
                .id("extra-rules-fixed")
                .test_support()
                .when(started, |this| {
                    this.tooltip(|window, cx| {
                        let said =
                            gpui_kit::component::tooltip::Tooltip::new(FIXED).build(window, cx);
                        cx.new(|_| Fixed(said)).into()
                    })
                })
                .child(extra_rules),
        );
        let project = {
            let choose = ProjectFiles::ALL.map(|files| {
                choose(
                    "project-instructions",
                    files.name(),
                    format!("/project-instructions {}", files.name()),
                )
            });
            let current = ProjectFiles::ALL
                .into_iter()
                .find(|files| files.name() == shown("project-instructions"))
                .unwrap_or(settings.project_instructions);
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

        let worker_models = {
            let choose = ModelChoice::ALL
                .map(|choice| choose("worker-models", choice.name(), choice.command()));
            let current = ModelChoice::ALL
                .into_iter()
                .find(|choice| choice.name() == shown("worker-models"))
                .unwrap_or(settings.worker_models);
            ToggleGroup::new("worker-models")
                .segmented()
                .small()
                .children(ModelChoice::ALL.map(|choice| {
                    Toggle::new(SharedString::from(format!(
                        "worker-models-{}",
                        choice.name()
                    )))
                    .label(short_choice(choice))
                    .tooltip(curly(choice.label()))
                    .checked(choice == current)
                }))
                .on_click(move |checks, window, cx| {
                    if let Some(ix) =
                        clicked(checks, ModelChoice::ALL.map(|choice| choice == current))
                    {
                        choose[ix](window, cx)
                    }
                })
        };
        let worker_depth = {
            let depths: [u32; workers::MAX_DEPTH as usize] =
                std::array::from_fn(|ix| ix as u32 + 1);
            let choose = depths.map(|depth| {
                choose(
                    "worker-depth",
                    &depth.to_string(),
                    workers::depth_command(depth),
                )
            });
            let current = depths
                .into_iter()
                .find(|depth| depth.to_string() == shown("worker-depth"))
                .unwrap_or(settings.worker_depth);
            ToggleGroup::new("worker-depth")
                .segmented()
                .small()
                .children(depths.map(|depth| {
                    Toggle::new(SharedString::from(format!("worker-depth-{depth}")))
                        .label(depth.to_string())
                        .checked(depth == current)
                }))
                .on_click(move |checks, window, cx| {
                    if let Some(ix) = clicked(checks, depths.map(|depth| depth == current)) {
                        choose[ix](window, cx)
                    }
                })
        };

        let mut body = v_flex().gap_1();
        // Where it works, and on what.
        body = body
            .child(heading("Session", cx))
            .when_some(workspace.filter(|_| agent), |this, folder| {
                this.child(row(
                    narrow,
                    "Folder",
                    path_value("reveal-folder", &folder, cx),
                    cx,
                ))
            })
            .child(row(narrow, "Mode", mode.label(), cx))
            .child(row(
                narrow,
                "Model",
                v_flex().child(main_model).when(started, |this| {
                    this.child(div().text_xs().text_color(muted).child(MODEL_KEPT))
                }),
                cx,
            ))
            .when_some(workers_model.filter(|_| agent), |this, model| {
                this.child(row(narrow, "Workers’ model", model, cx))
            });
        // When the agents hand over.
        body = body
            .child(heading(if agent { "Handover" } else { "Context" }, cx))
            .child(row(
                narrow,
                if compacts {
                    "Main agent compacts at"
                } else if agent {
                    "Main agent at"
                } else {
                    "Summarizes at"
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
                    narrow,
                    "Workers at",
                    h_flex().children(input(Field::HandoverWorkers, self)),
                    cx,
                ))
            });
        if agent {
            body = body
                .child(heading("Settings", cx))
                .child(row(
                    narrow,
                    "Outside access",
                    switch(
                        "outside-access",
                        "Outside access",
                        false,
                        "/outside on".into(),
                        "/outside off".into(),
                    ),
                    cx,
                ))
                .child(row(
                    narrow,
                    "Background after",
                    h_flex().children(input(Field::BackgroundAfter, self)),
                    cx,
                ))
                .child(row(
                    narrow,
                    monitors::NAME,
                    v_flex()
                        .gap_1()
                        .child(h_flex().children(input(Field::MonitorWakes, self)))
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted)
                                .child(settings.monitor_wakes.sentence().replace('\'', "’")),
                        ),
                    cx,
                ))
                .child(row(
                    narrow,
                    "Text replies",
                    switch(
                        "text-replies",
                        "Text replies",
                        started,
                        "/text-replies on".into(),
                        "/text-replies off".into(),
                    ),
                    cx,
                ))
                .child(row(
                    narrow,
                    "Claude Code compaction",
                    switch(
                        "cc-compact",
                        "Claude Code compaction",
                        started,
                        "/cc-compact on".into(),
                        "/cc-compact off".into(),
                    ),
                    cx,
                ))
                .child(row(narrow, "Extra rules", extra_rules, cx))
                .child(row(
                    narrow,
                    "Project instructions",
                    v_flex()
                        .gap_1()
                        .child(project)
                        .child(div().text_xs().text_color(muted).child({
                            let said =
                                crate::backend::sentence(settings.project_instructions.describe());
                            // Before the first message, it holds for
                            // the conversation coming.
                            if started {
                                format!(
                                    "{}, from the next conversation.",
                                    said.trim_end_matches('.')
                                )
                            } else {
                                said
                            }
                        }))
                        .when(reading.models.has_workers, |this| {
                            this.child(
                                h_flex()
                                    .gap_2()
                                    .text_xs()
                                    .child(switch(
                                        "project-workers",
                                        "Workers get the project’s instructions too",
                                        false,
                                        "/project-instructions workers on".into(),
                                        "/project-instructions workers off".into(),
                                    ))
                                    .child("Workers get them too"),
                            )
                        }),
                    cx,
                ));
            if reading.models.has_workers {
                let chosen = ModelChoice::ALL
                    .into_iter()
                    .find(|choice| choice.name() == shown("worker-models"))
                    .unwrap_or(settings.worker_models);
                let depth = shown("worker-depth")
                    .parse()
                    .unwrap_or(settings.worker_depth);
                body = body
                    .child(row(
                        narrow,
                        workers::MODELS_NAME,
                        v_flex().gap_1().child(worker_models).child(
                            div()
                                .id("worker-models-about")
                                .test_support()
                                .text_xs()
                                .text_color(muted)
                                .child(models_about(chosen)),
                        ),
                        cx,
                    ))
                    .child(row(
                        narrow,
                        workers::DEPTH_NAME,
                        v_flex().gap_1().child(worker_depth).child(
                            div()
                                .id("worker-depth-about")
                                .test_support()
                                .text_xs()
                                .text_color(muted)
                                .child(curly(&workers::depth_effect(
                                    depth,
                                    settings.nested_workers,
                                ))),
                        ),
                        cx,
                    ));
            }
            // What the agents have besides their own tools.
            body = body.child(heading("Extensions", cx));
            body = match &facts {
                None => body.child(div().text_xs().text_color(muted).child("Reading…")),
                Some(facts) => body
                    .child(row(
                        narrow,
                        "MCP servers",
                        v_flex().children(mcp_lines(&facts.mcp).into_iter().enumerate().map(
                            |(ix, (line, tools))| {
                                div()
                                    .id(("mcp-server", ix))
                                    .text_sm()
                                    .child(line)
                                    .when_some(tools, |this, tools| {
                                        this.tooltip(move |window, cx| {
                                            gpui_kit::component::tooltip::Tooltip::new(
                                                tools.clone(),
                                            )
                                            .build(window, cx)
                                        })
                                    })
                            },
                        )),
                        cx,
                    ))
                    .child(row(narrow, "Skills", skills_line(&facts.skills), cx))
                    .child(row(
                        narrow,
                        "Indexes",
                        indexes_text(&facts.indexes).unwrap_or_else(|| "None".to_string()),
                        cx,
                    )),
            };
        }
        // Where it's kept.
        body = body
            .child(heading("Storage", cx))
            .child(row(
                narrow,
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
                            .tooltip("Copy the session ID")
                            .accessibility_label("Copy the session ID"),
                    ),
                cx,
            ))
            .when_some(dir, |this, dir| {
                this.child(row(
                    narrow,
                    "Data folder",
                    path_value("reveal-data", &dir, cx),
                    cx,
                ))
            });

        // As tall as the window allows, scrolling past that.
        let tallest = viewport.height * 0.8;
        v_flex()
            .id("session-info")
            .test_support()
            .w(width)
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

/// Which toggle a click chose: the first checked now that wasn't before. A
/// click on the one chosen already chooses nothing.
fn clicked<const N: usize>(after: &[bool], before: [bool; N]) -> Option<usize> {
    (0..N).find(|ix| after.get(*ix).copied().unwrap_or(false) && !before[*ix])
}

/// The MCP servers, a line each ("files — 4 tools", "web — didn’t start
/// (…)"), with the names of the server's tools for its tooltip.
pub fn mcp_lines(mcp: &McpFacts) -> Vec<(String, Option<String>)> {
    let mut lines: Vec<(String, Option<String>)> = mcp
        .servers
        .iter()
        .map(|server| match &server.tools {
            Ok(tools) => (
                format!(
                    "{} — {} tool{}",
                    server.name,
                    tools.len(),
                    if tools.len() == 1 { "" } else { "s" }
                ),
                (!tools.is_empty()).then(|| tools.join(", ")),
            ),
            Err(why) => (format!("{} — didn’t start ({why})", server.name), None),
        })
        .collect();
    lines.extend(mcp.notes.iter().map(|note| (note.clone(), None)));
    if lines.is_empty() {
        lines.push(("None".to_string(), None));
    }
    lines
}

/// The skills, in a line: "3 (release-notes, docs-style, triage)".
pub fn skills_line(skills: &Result<SkillsFacts, String>) -> String {
    match skills {
        Err(why) => format!("Couldn’t be read ({why})"),
        Ok(skills) if skills.skills.is_empty() => "None".to_string(),
        Ok(skills) => {
            let names: Vec<&str> = skills
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect();
            format!("{} ({})", names.len(), names.join(", "))
        }
    }
}

/// The workspace's indexes in a line, as Arko's banner lists them ("arko
/// (code, monitored), docs (docs)"), or `None` without any.
pub fn indexes_text(indexes: &[IndexFacts]) -> Option<String> {
    if indexes.is_empty() {
        return None;
    }
    let mut text = indexes
        .iter()
        .map(index_text)
        .collect::<Vec<_>>()
        .join(", ");
    let needs_key = indexes.iter().any(|index| {
        index
            .embeddings
            .as_ref()
            .is_some_and(|embeddings| embeddings.needs_key)
    });
    if needs_key {
        text.push_str(" · semantic search needs VOYAGE_API_KEY");
    }
    Some(text)
}

/// An index as the banner names it: its name, its kind, "lexical" when it's
/// searched by keywords only, and whether an Arko keeps it current.
fn index_text(index: &IndexFacts) -> String {
    let lexical = if index.embeddings.is_some() {
        ""
    } else {
        ", lexical"
    };
    let monitored = match index.watch {
        IndexWatch::Here => ", monitored",
        IndexWatch::Elsewhere => ", monitored by another Arko",
        _ => "",
    };
    format!(
        "{} ({}{lexical}{monitored})",
        index.name,
        index.kind.as_str()
    )
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{
        Field, MODEL_KEPT, background_value, clicked, field_command, handover_value, index_text,
        indexes_text, mcp_lines, skills_line,
    };
    use arko::details::{
        IndexEmbeddings, IndexFacts, IndexKind, IndexWatch, McpFacts, McpServerFacts, SkillFacts,
        SkillsFacts,
    };
    use std::path::PathBuf;

    /// The info and the mode popover say it with the same words.
    #[test]
    fn the_models_sentence_is_one_wording() {
        assert_eq!(
            MODEL_KEPT,
            "The main agent keeps its model once the conversation starts."
        );
    }

    #[test]
    fn fields_send_the_terminals_commands() {
        assert_eq!(handover_value(950_000), "950k");
        assert_eq!(handover_value(1_020_000), "1020k");
        assert_eq!(background_value(0), "off");
        assert_eq!(background_value(600), "10m");
        assert_eq!(background_value(3_600), "1h");
        assert_eq!(background_value(90), "90s");
        // Sizes arko capped to what the model leaves room for show whole.
        assert_eq!(handover_value(950_500), "950,500");
        assert_eq!(handover_value(1_234_567), "1,234,567");
        let command = |field, text: &str, current| field_command(field, text, current);
        assert_eq!(
            command(Field::HandoverMain, " 300k ", 950_000),
            Ok(Some("/handover-at main 300k".into()))
        );
        assert_eq!(
            command(Field::HandoverWorkers, "1.2M", 200_000),
            Ok(Some("/handover-at workers 1200k".into()))
        );
        assert_eq!(
            command(Field::BackgroundAfter, "off", 600),
            Ok(Some("/background-after off".into()))
        );
        assert_eq!(command(Field::BackgroundAfter, "  ", 600), Ok(None));
        // What is set already sends nothing, however it's written.
        assert_eq!(command(Field::HandoverMain, "950,500", 950_500), Ok(None));
        assert_eq!(command(Field::BackgroundAfter, "600s", 600), Ok(None));
        // What the session would refuse says why at once.
        assert!(command(Field::HandoverMain, "300", 950_000).is_err());
        assert!(command(Field::HandoverMain, "1.2345k", 950_000).is_err());
        assert!(command(Field::BackgroundAfter, "10 minutes", 600).is_err());
        assert!(command(Field::BackgroundAfter, "2h", 600).is_err());
        // Monitor wakes takes a time up to a day, always, or off.
        assert_eq!(
            command(Field::MonitorWakes, "always", 7_200),
            Ok(Some("/monitor-wakes always".into()))
        );
        assert_eq!(
            command(Field::MonitorWakes, "never", 7_200),
            Ok(Some("/monitor-wakes off".into()))
        );
        assert_eq!(
            command(Field::MonitorWakes, "90m", u64::MAX),
            Ok(Some("/monitor-wakes 90m".into()))
        );
        assert_eq!(command(Field::MonitorWakes, "120m", 7_200), Ok(None));
        assert_eq!(command(Field::MonitorWakes, "Always", u64::MAX), Ok(None));
        assert!(command(Field::MonitorWakes, "25h", 7_200).is_err());
        assert!(command(Field::MonitorWakes, "soon", 7_200).is_err());
        for command in [
            "/handover-at main 300k",
            "/handover-at main 950,500",
            "/background-after off",
            "/monitor-wakes always",
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
        // The chosen one clicked again: nothing changes.
        assert_eq!(clicked(&[false, false, false], [false, true, false]), None);
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
            [
                ("files — 2 tools".into(), Some("read, write".into())),
                ("web — didn’t start (no such program)".into(), None)
            ]
        );
        assert_eq!(mcp_lines(&McpFacts::default()), [("None".into(), None)]);
        let skills = SkillsFacts {
            skills: vec![SkillFacts {
                name: "docs-style".into(),
                description: String::new(),
                always: false,
            }],
            folders: Vec::new(),
            own_folder: PathBuf::new(),
        };
        assert_eq!(skills_line(&Ok(skills)), "1 (docs-style)");
        assert_eq!(indexes_text(&[]), None);
    }

    /// The index `name` of `kind`, with embeddings or not, kept current as
    /// `watch` says.
    fn index(name: &str, kind: IndexKind, embedded: bool, watch: IndexWatch) -> IndexFacts {
        let mut index = IndexFacts::new(name, kind, PathBuf::from("/work").join(name));
        index.watch = watch;
        index.embeddings = embedded.then(|| IndexEmbeddings::new("voyage-code-4"));
        index
    }

    #[test]
    fn an_index_reads_as_the_banner_names_it() {
        assert_eq!(
            index_text(&index("arko", IndexKind::Code, true, IndexWatch::Here)),
            "arko (code, monitored)"
        );
        assert_eq!(
            index_text(&index("docs", IndexKind::Docs, false, IndexWatch::Off)),
            "docs (docs, lexical)"
        );
        assert_eq!(
            index_text(&index("arko", IndexKind::Code, true, IndexWatch::Elsewhere)),
            "arko (code, monitored by another Arko)"
        );
        assert_eq!(
            index_text(&index(
                "arko",
                IndexKind::Code,
                true,
                IndexWatch::Failed("no watcher".into())
            )),
            "arko (code)"
        );
    }

    /// The indexes read in a line, which says when semantic search needs a
    /// key.
    #[test]
    fn the_indexes_read_in_a_line() {
        let arko = index("arko", IndexKind::Code, true, IndexWatch::Here);
        let docs = index("docs", IndexKind::Docs, false, IndexWatch::Off);
        assert_eq!(
            indexes_text(&[arko.clone(), docs.clone()]).as_deref(),
            Some("arko (code, monitored), docs (docs, lexical)")
        );
        let mut keyless = arko;
        if let Some(embeddings) = &mut keyless.embeddings {
            embeddings.needs_key = true;
        }
        assert_eq!(
            indexes_text(&[keyless, docs]).as_deref(),
            Some(
                "arko (code, monitored), docs (docs, lexical) · semantic search needs VOYAGE_API_KEY"
            )
        );
    }
}

#[cfg(test)]
mod view_tests {
    #[gpui_kit::test]
    fn optimistic_controls_belong_to_the_shown_session(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let a = cx.new(|cx| LiveSession::for_tests("a", cx));
        let b = cx.new(|cx| LiveSession::for_tests("b", cx));
        for session in [&a, &b] {
            session.update(cx, |session, cx| {
                session.set_status(
                    Reading {
                        settings: LiveSettings {
                            outside: true,
                            ..LiveSettings::default()
                        },
                        ..Reading::default()
                    },
                    Activity::default(),
                    cx,
                )
            });
        }
        let title = cx.read(|cx| workspace.read(cx).title_bar().clone());
        act(cx, window, |window, cx| {
            title.update(cx, |title, cx| {
                title.set_live_session(Some(a.clone()), window, cx);
                title.toggle_info(cx);
            })
        });
        act(cx, window, |window, cx| window.click("outside-access", cx));
        act(cx, window, |window, cx| window.click("both", cx));
        act(cx, window, |window, cx| {
            title.update(cx, |title, cx| {
                title.set_live_session(Some(b.clone()), window, cx);
                if !title.is_info_open() {
                    title.toggle_info(cx);
                }
            })
        });
        act(cx, window, |window, _| {
            assert_eq!(window.find("outside-access").checked(), Some(true));
            assert_eq!(window.find("one").checked(), Some(true));
            assert_eq!(window.find("both").checked(), Some(false));
        });
        assert!(cx.read(|cx| b.read(cx).queued().is_empty()));
    }

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

    /// At the narrowest window, the info stays inside it, each label above
    /// its value.
    #[gpui_kit::test]
    fn the_info_fits_a_narrow_window(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 400.);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        let title_bar = cx.read(|cx| workspace.read(cx).title_bar().clone());
        cx.update_window(window, |_, window, cx| {
            title_bar.update(cx, |bar, cx| {
                bar.set_live_session(Some(session.clone()), window, cx);
                bar.toggle_info(cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        let (bounds, viewport) = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                (
                    window.try_find("session-info").map(|found| found.bounds()),
                    window.viewport_size(),
                )
            })
            .unwrap();
        let bounds = bounds.expect("the info shows");
        assert!(
            bounds.origin.x >= gpui_kit::px(0.)
                && bounds.origin.x + bounds.size.width <= viewport.width,
            "{bounds:?} in {viewport:?}"
        );
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

    /// A switch shows a click at once, before the session's status says
    /// so, and for a while after if it never does. A click on the segment
    /// chosen already sends nothing.
    #[gpui_kit::test]
    fn a_click_shows_at_once(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        let reading = Reading {
            models: Models {
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
        let checked = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find("outside-access")
                    .and_then(|found| found.checked())
            })
            .unwrap()
        };
        assert_eq!(checked(cx), Some(false));
        // A screen reader names the switch as its row does.
        let label = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                window
                    .try_find("outside-access")
                    .and_then(|found| found.label().map(str::to_string))
            })
            .unwrap();
        assert_eq!(label.as_deref(), Some("Outside access"));
        act(cx, window, |window, cx| window.click("outside-access", cx));
        assert_eq!(checked(cx), Some(true));
        // Clicked again before the status came: off again.
        act(cx, window, |window, cx| window.click("outside-access", cx));
        assert_eq!(checked(cx), Some(false));
        act(cx, window, |window, cx| window.click("outside-access", cx));
        // One, as it is.
        act(cx, window, |window, cx| window.click("one", cx));
        assert_eq!(
            sent(&session, cx),
            ["/outside on", "/outside off", "/outside on"]
        );
        // The session never said so: its status shows again.
        cx.executor().advance_clock(super::HOLD);
        cx.run_until_parked();
        assert_eq!(checked(cx), Some(false));
    }

    /// Opens the info of a session that has (`started`) or hasn't started
    /// its conversation, and hovers `id` until a tooltip would show.
    /// Whether one did.
    fn tooltip_on(cx: &mut TestAppContext, started: bool, id: &'static str) -> bool {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let reading = Reading {
            models: Models {
                started,
                ..Models::default()
            },
            ..Reading::default()
        };
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
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
        // The rows are past the first screenful of the info.
        for _ in 0..40 {
            let seen = cx
                .update_window(window, |_, window, cx| {
                    window.render_frame(cx);
                    let info = window.find("session-info").bounds();
                    window
                        .try_find(id)
                        .is_some_and(|found| info.contains(&found.bounds().center()))
                })
                .unwrap();
            if seen {
                break;
            }
            act(cx, window, |window, cx| {
                window.scroll(
                    "session-info",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(-120.),
                    )),
                    cx,
                )
            });
        }
        let center = cx
            .update_window(window, |_, window, cx| {
                window.render_frame(cx);
                let bounds = window.find(id).bounds();
                gpui_kit::point(bounds.origin.x + gpui_kit::px(10.), bounds.center().y)
            })
            .unwrap();
        let cx = &mut gpui_kit::VisualTestContext::from_window(window, cx);
        cx.simulate_mouse_move(center, None, gpui_kit::Modifiers::default());
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.debug_bounds("tooltip").is_some() || cx.debug_bounds("fixed-tooltip").is_some()
    }

    /// Once the conversation has started, the settings it fixes say so
    /// when hovered, the Extra rules too. Before, they have no tooltip.
    #[gpui_kit::test]
    fn the_settings_a_conversation_fixes_say_so_on_hover(cx: &mut TestAppContext) {
        assert!(tooltip_on(cx, true, "text-replies"));
    }

    #[gpui_kit::test]
    fn the_extra_rules_say_they_are_fixed_on_hover(cx: &mut TestAppContext) {
        assert!(tooltip_on(cx, true, "extra-rules-fixed"));
    }

    #[gpui_kit::test]
    fn the_extra_rules_say_nothing_before_the_conversation_starts(cx: &mut TestAppContext) {
        assert!(!tooltip_on(cx, false, "extra-rules-fixed"));
    }

    /// Who chooses the workers' models and how deep workers go send the
    /// commands of the terminal, and show the choice at once. A session
    /// with no workers lists neither.
    #[gpui_kit::test]
    fn the_worker_settings_send_the_terminals_commands(cx: &mut TestAppContext) {
        use arko::{ModelChoice, workers};

        start(cx);
        let (window, workspace) = open(cx, 1300.);
        let reading = |has_workers| Reading {
            settings: LiveSettings {
                worker_models: ModelChoice::Agent,
                worker_depth: 2,
                ..LiveSettings::default()
            },
            models: Models {
                started: true,
                has_workers,
                ..Models::default()
            },
            ..Reading::default()
        };
        let alone = cx.new(|cx| LiveSession::for_tests("alone", cx));
        alone.update(cx, |session, cx| {
            session.set_status(reading(false), Activity::default(), cx)
        });
        let session = cx.new(|cx| LiveSession::for_tests("s1", cx));
        session.update(cx, |session, cx| {
            session.set_status(reading(true), Activity::default(), cx)
        });
        let title_bar = cx.read(|cx| workspace.read(cx).title_bar().clone());
        let show = |cx: &mut TestAppContext, shown: &Entity<LiveSession>| {
            cx.update_window(window, |_, window, cx| {
                title_bar.update(cx, |bar, cx| {
                    bar.set_live_session(Some(shown.clone()), window, cx);
                    if !bar.is_info_open() {
                        bar.toggle_info(cx);
                    }
                })
            })
            .unwrap();
            cx.run_until_parked();
        };
        show(cx, &alone);
        assert!(!shows(window, cx, "worker-models"));
        assert!(!shows(window, cx, "worker-depth"));
        show(cx, &session);
        // The rows are past the first screenful of the info.
        for _ in 0..40 {
            let seen = cx
                .update_window(window, |_, window, cx| {
                    window.render_frame(cx);
                    window
                        .try_find("worker-depth-2")
                        .is_some_and(|found| found.visible())
                })
                .unwrap();
            if seen {
                break;
            }
            act(cx, window, |window, cx| {
                window.scroll(
                    "session-info",
                    gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                        gpui_kit::px(0.),
                        gpui_kit::px(-120.),
                    )),
                    cx,
                )
            });
        }
        let checked = |cx: &mut TestAppContext, id: &'static str| {
            cx.update_window(window, |_, window, cx| {
                window.render_frame(cx);
                window.try_find(id).and_then(|found| found.checked())
            })
            .unwrap()
        };
        assert_eq!(checked(cx, "worker-models-agent"), Some(true));
        assert_eq!(checked(cx, "worker-models-profiles"), Some(false));
        assert_eq!(checked(cx, "worker-depth-2"), Some(true));
        act(cx, window, |window, cx| {
            window.click("worker-models-profiles", cx)
        });
        act(cx, window, |window, cx| window.click("worker-depth-3", cx));
        // The choice shows at once, and the one chosen already sends nothing.
        assert_eq!(checked(cx, "worker-models-profiles"), Some(true));
        assert_eq!(checked(cx, "worker-models-agent"), Some(false));
        assert_eq!(checked(cx, "worker-depth-3"), Some(true));
        act(cx, window, |window, cx| window.click("worker-depth-3", cx));
        act(cx, window, |window, cx| window.click("worker-depth-1", cx));
        assert_eq!(
            sent(&session, cx),
            [
                "/worker-models profiles",
                workers::depth_command(3).as_str(),
                "/worker-depth 1"
            ]
        );
        for command in sent(&session, cx) {
            assert!(arko::commands::find(&command).is_some(), "{command}");
        }
        // The session's own status shows once it comes.
        session.update(cx, |session, cx| {
            let mut next = reading(true);
            next.settings.worker_models = ModelChoice::Session;
            next.settings.worker_depth = 4;
            session.set_status(next, Activity::default(), cx)
        });
        cx.executor().advance_clock(super::HOLD);
        cx.run_until_parked();
        assert_eq!(checked(cx, "worker-models-session"), Some(true));
        assert_eq!(checked(cx, "worker-depth-4"), Some(true));
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
        let wakes = cx.read(|cx| info.read(cx).field(Field::MonitorWakes));
        assert_eq!(cx.read(|cx| wakes.read(cx).value().to_string()), "2h");

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
        // Enter on the size set already sends nothing. A size the session
        // would refuse says why under the field, and sends nothing either.
        for typed in ["950k", "300"] {
            act(cx, window, |window, cx| {
                main.update(cx, |input, cx| {
                    input.set_value(typed, window, cx);
                    input.focus(window, cx);
                })
            });
            act(cx, window, |window, cx| window.press("enter", cx));
        }
        assert_eq!(
            sent(&session, cx),
            ["/outside on", "/handover-at main 300k"]
        );
        assert!(
            cx.read(|cx| info.read(cx).error(Field::HandoverMain).is_some()),
            "no reason under the field"
        );
        assert!(shows(window, cx, "session-info"), "Enter closed the info");
        // Monitor wakes sends /monitor-wakes, and says why it can't take a
        // time longer than a day.
        for typed in ["always", "25h"] {
            act(cx, window, |window, cx| {
                wakes.update(cx, |input, cx| {
                    input.set_value(typed, window, cx);
                    input.focus(window, cx);
                })
            });
            act(cx, window, |window, cx| window.press("enter", cx));
        }
        assert_eq!(
            sent(&session, cx),
            [
                "/outside on",
                "/handover-at main 300k",
                "/monitor-wakes always"
            ]
        );
        assert!(cx.read(|cx| info.read(cx).error(Field::MonitorWakes).is_some()));
    }
}
