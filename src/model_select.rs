//! A model setting: a button naming the chosen model, which opens a
//! searchable list of the models by the way they're reached, the field's
//! own first choice ("Last used", "Automatic") at the top. The OpenRouter
//! group ends in "All OpenRouter models…", which opens OpenRouter's whole
//! catalog in [`crate::openrouter::browse`] on what was typed.

use std::rc::Rc;

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::popover::Popover;
use gpui_kit::component::{
    ActiveTheme as _, IconName, IndexPath, Sizable as _, button::Button, h_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Listed, ModelChoices, SharedBackend};

/// The model the field holds, as `/model` takes it, "" for its first
/// choice.
pub type Chosen = Rc<dyn Fn(&App) -> SharedString>;
/// Sets the field to a model as `/model` takes it, `None` for its first
/// choice.
pub type Choose = Rc<dyn Fn(Option<String>, &mut Window, &mut App)>;

/// What the OpenRouter group's last row says.
pub const ALL_OPENROUTER: &str = "All OpenRouter models…";

pub struct ModelSelect {
    id: SharedString,
    /// The field's name, for screen readers: "Model".
    what: SharedString,
    /// The first choice: "Last used (Opus 5.5)".
    first: SharedString,
    models: Option<ModelChoices>,
    /// OpenRouter's models known by their details: the models picked
    /// lately, those arko keeps in memory, and the one picked here.
    known: Vec<Listed>,
    _known: Option<Task<()>>,
    list: Entity<ListState<ModelList>>,
    open: bool,
    /// The button takes the width it's given.
    full_width: bool,
    chosen: Chosen,
    choose: Choose,
}

impl ModelSelect {
    /// A field found in tests by `id` (its button is `{id}-button`).
    pub fn new(
        id: impl Into<SharedString>,
        what: impl Into<SharedString>,
        chosen: Chosen,
        choose: Choose,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let select = cx.entity().downgrade();
        let list = cx.new(|cx| {
            ListState::new(
                ModelList {
                    select,
                    sections: Vec::new(),
                    shown: Vec::new(),
                    chosen: SharedString::default(),
                    query: String::new(),
                    selected: None,
                },
                window,
                cx,
            )
            .searchable(true)
        });
        let mut this = Self {
            id: id.into(),
            what: what.into(),
            first: SharedString::default(),
            models: None,
            known: Vec::new(),
            _known: None,
            list,
            open: false,
            full_width: false,
            chosen,
            choose,
        };
        this.read_known(cx);
        this
    }

    /// Makes the button as wide as its place.
    pub fn full_width(mut self) -> Self {
        self.full_width = true;
        self
    }

    /// Lists `models` after the first choice, `first`.
    pub fn set_models(
        &mut self,
        first: impl Into<SharedString>,
        models: Option<ModelChoices>,
        cx: &mut Context<Self>,
    ) {
        let first = first.into();
        if self.first != first || self.models != models {
            self.first = first;
            self.models = models;
            cx.notify();
        }
    }

    /// Reads the details of OpenRouter's models picked lately and of those
    /// arko keeps in memory, in the background, to name them.
    fn read_known(&mut self, cx: &mut Context<Self>) {
        let backend = cx.global::<SharedBackend>().clone();
        let reading = cx.background_spawn(async move {
            let mut known = backend.openrouter().recent();
            known.extend(backend.openrouter_models_kept().models);
            known
        });
        self._known = Some(cx.spawn(async move |this, cx| {
            let mut known = reading.await;
            this.update(cx, |this, cx| {
                // The one picked here, if arko doesn't know it yet.
                for model in &this.known {
                    if !known.iter().any(|found| found.name == model.name) {
                        known.push(model.clone());
                    }
                }
                if this.known != known {
                    this.known = known;
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The button's text: the chosen model with the way it's reached.
    pub fn label(&self, cx: &App) -> SharedString {
        let chosen = (self.chosen)(cx);
        if chosen.is_empty() {
            return self.first.clone();
        }
        label_of(&chosen, self.models.as_ref(), &self.known).into()
    }

    /// Opens the list on the chosen model, or closes it.
    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open == self.open {
            return;
        }
        self.open = open;
        if open {
            self.read_known(cx);
            let chosen = (self.chosen)(cx);
            let sections = sections(&self.first, self.models.as_ref(), &self.known, &chosen);
            self.list.update(cx, |list, cx| {
                let typed = !list.delegate().query.is_empty();
                let delegate = list.delegate_mut();
                delegate.sections = sections;
                delegate.chosen = chosen.clone();
                delegate.show("");
                if typed {
                    list.set_query("", window, cx);
                }
                let at = list.delegate().position(&chosen);
                list.set_selected_index(at, window, cx);
                if let Some(at) = at {
                    list.scroll_to_item(at, ScrollStrategy::Center, window, cx);
                }
                list.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn pick(&mut self, row: Row, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.set_open(false, window, cx);
        match row {
            Row::Model { name, .. } => {
                let name = Some(name.to_string()).filter(|name| !name.is_empty());
                (self.choose)(name, window, cx);
            }
            Row::Browse { needs } => {
                let current = Some((self.chosen)(cx).to_string()).filter(|name| !name.is_empty());
                let (select, choose) = (cx.entity().downgrade(), self.choose.clone());
                crate::openrouter::browse(
                    crate::openrouter::Browse {
                        query,
                        current,
                        needs,
                        refuse_unusable: false,
                    },
                    Rc::new(move |model: Listed, window, cx| {
                        let name = model.name.clone();
                        select
                            .update(cx, |select, cx| {
                                select.known.retain(|known| known.name != name);
                                select.known.push(model);
                                cx.notify();
                            })
                            .ok();
                        choose(Some(name), window, cx)
                    }),
                    window,
                    cx,
                );
            }
        }
        cx.notify();
    }
}

impl Render for ModelSelect {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let select = cx.entity().downgrade();
        let label = self.label(cx);
        Popover::new(self.id.clone())
            .anchor(Anchor::TopRight)
            .open(self.open)
            .on_open_change(move |open, window, cx| {
                select
                    .update(cx, |select, cx| select.set_open(*open, window, cx))
                    .ok();
            })
            .track_focus(&self.list.read(cx).focus_handle(cx))
            .when(self.full_width, |this| this.w_full())
            .trigger(
                Button::new(SharedString::from(format!("{}-button", self.id)))
                    .outline()
                    .small()
                    .when(self.full_width, |this| this.w_full())
                    .label(label.clone())
                    .accessibility_label(format!("{}, {label}", self.what))
                    .dropdown_caret(true),
            )
            .child(
                div().w(rems(20.)).h(rems(22.)).child(
                    List::new(&self.list)
                        .small()
                        .search_placeholder("Find a model"),
                ),
            )
    }
}

/// A row of the list.
#[derive(Debug, Clone, PartialEq)]
enum Row {
    /// A model as `/model` takes it ("" for the first choice), and its
    /// name in the list.
    Model {
        name: SharedString,
        label: SharedString,
        /// What a screen reader says for it: its name, its way and
        /// whether it can be used.
        spoken: SharedString,
        /// What else it's found by: its alias and the way it's reached.
        found_by: String,
    },
    /// OpenRouter's whole catalog, and what its models need that isn't
    /// there.
    Browse { needs: Option<String> },
}

/// A group of rows, with its heading (none for the first choice).
#[derive(Debug, Clone, PartialEq)]
struct Section {
    heading: Option<SharedString>,
    rows: Vec<Row>,
}

/// The rows: the first choice, then each group of `models`. A chosen
/// model none of them lists joins the group of its way, else ends the list
/// as written, so the field still shows it.
fn sections(
    first: &SharedString,
    models: Option<&ModelChoices>,
    known: &[Listed],
    chosen: &str,
) -> Vec<Section> {
    let mut sections = vec![Section {
        heading: None,
        rows: vec![Row::Model {
            name: SharedString::default(),
            label: first.clone(),
            spoken: first.clone(),
            found_by: String::new(),
        }],
    }];
    let groups = models.map(|models| models.groups.as_slice()).unwrap_or(&[]);
    let listed = groups
        .iter()
        .flat_map(|group| &group.models)
        .any(|model| model.name == chosen);
    let mut placed = chosen.is_empty() || listed;
    for group in groups {
        let openrouter = group.name.starts_with("OpenRouter");
        let heading = match group.needs {
            Some(_) => format!("{} (can’t be used yet)", group.name),
            None => group.name.clone(),
        };
        let way = way(&group.name);
        let spoken = |label: &str| match group.needs {
            Some(_) => format!("{label}, {way}, can’t be used yet"),
            None => format!("{label}, {way}"),
        };
        let mut rows: Vec<Row> = group
            .models
            .iter()
            .map(|model| {
                let label = arko::models::display_name(&model.name);
                Row::Model {
                    name: model.name.clone().into(),
                    spoken: spoken(&label).into(),
                    label: label.into(),
                    found_by: format!("{} {} {way}", model.name, model.alias).to_lowercase(),
                }
            })
            .collect();
        if openrouter {
            if !placed && crate::openrouter::slug(chosen).is_some() {
                let label = openrouter_name(chosen, known);
                rows.push(Row::Model {
                    name: chosen.to_string().into(),
                    spoken: spoken(&label).into(),
                    label: label.into(),
                    found_by: format!("{chosen} {way}").to_lowercase(),
                });
                placed = true;
            }
            rows.push(Row::Browse {
                needs: group.needs.clone(),
            });
        }
        sections.push(Section {
            heading: Some(heading.into()),
            rows,
        });
    }
    if !placed {
        sections[0].rows.push(Row::Model {
            name: chosen.to_string().into(),
            label: chosen.to_string().into(),
            spoken: chosen.to_string().into(),
            found_by: chosen.to_lowercase(),
        });
    }
    sections
}

/// The way a group of models is reached, as a label names it: "Anthropic
/// API" for "Anthropic API, billed per token".
fn way(group: &str) -> &str {
    group.split(',').next().unwrap_or(group).trim()
}

/// The model `name` as the button names it: "Opus 5.5 · Anthropic API",
/// "(can't be used yet)" when its group needs something. One no group
/// lists is OpenRouter's by its slug, else shown as written.
pub fn label_of(name: &str, models: Option<&ModelChoices>, known: &[Listed]) -> String {
    let group = models.and_then(|models| {
        models
            .groups
            .iter()
            .find(|group| group.models.iter().any(|model| model.name == name))
    });
    match group {
        Some(group) => {
            let mut label = format!(
                "{} · {}",
                arko::models::display_name(name),
                way(&group.name)
            );
            if group.needs.is_some() {
                label.push_str(" (can’t be used yet)");
            }
            label
        }
        None if crate::openrouter::slug(name).is_some() => {
            format!("{} · OpenRouter", openrouter_name(name, known))
        }
        None => name.to_string(),
    }
}

/// The name of OpenRouter's model `name`, as `known` has it, else as
/// arko makes it from the slug.
fn openrouter_name(name: &str, known: &[Listed]) -> String {
    known
        .iter()
        .find(|model| model.name == name)
        .map(|model| model.label.clone())
        .unwrap_or_else(|| arko::models::display_name(name))
}

/// The list's rows, filtered by the search.
pub struct ModelList {
    select: WeakEntity<ModelSelect>,
    sections: Vec<Section>,
    /// The sections that match the search, by index into `sections`, each
    /// with its rows that do.
    shown: Vec<(usize, Vec<usize>)>,
    chosen: SharedString,
    query: String,
    selected: Option<IndexPath>,
}

impl ModelList {
    /// Shows the rows that match `query`. "All OpenRouter models…" always
    /// shows with its group, to look further.
    fn show(&mut self, query: &str) {
        self.query = query.to_string();
        let query = query.trim().to_lowercase();
        self.shown = self
            .sections
            .iter()
            .enumerate()
            .filter_map(|(at, section)| {
                let rows: Vec<usize> = section
                    .rows
                    .iter()
                    .enumerate()
                    .filter(|(_, row)| match row {
                        Row::Model {
                            label, found_by, ..
                        } => {
                            query.is_empty()
                                || label.to_lowercase().contains(&query)
                                || found_by.contains(&query)
                        }
                        Row::Browse { .. } => true,
                    })
                    .map(|(ix, _)| ix)
                    .collect();
                (!rows.is_empty()).then_some((at, rows))
            })
            .collect();
    }

    fn row(&self, ix: IndexPath) -> Option<&Row> {
        let (section, rows) = self.shown.get(ix.section)?;
        self.sections.get(*section)?.rows.get(*rows.get(ix.row)?)
    }

    fn position(&self, name: &str) -> Option<IndexPath> {
        self.shown
            .iter()
            .enumerate()
            .flat_map(|(section, (_, rows))| {
                (0..rows.len()).map(move |row| IndexPath::default().section(section).row(row))
            })
            .find(|ix| matches!(self.row(*ix), Some(Row::Model { name: n, .. }) if n == name))
    }
}

impl ListDelegate for ModelList {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.show(query);
        Task::ready(())
    }

    fn sections_count(&self, _: &App) -> usize {
        self.shown.len().max(1)
    }

    fn items_count(&self, section: usize, _: &App) -> usize {
        self.shown.get(section).map_or(0, |(_, rows)| rows.len())
    }

    fn render_section_header(
        &mut self,
        section: usize,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let (at, _) = self.shown.get(section)?;
        let heading = self.sections.get(*at)?.heading.clone()?;
        Some(
            div()
                .px_2()
                .pt_2()
                .pb_0p5()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(heading),
        )
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let id = SharedString::from(format!("model-row-{}-{}", ix.section, ix.row));
        let item = match self.row(ix)? {
            Row::Model {
                name,
                label,
                spoken,
                ..
            } => ListItem::new(id)
                .child(label.clone())
                .accessibility_label(spoken.clone())
                .confirmed(*name == self.chosen)
                .check_icon(IconName::Check),
            Row::Browse { needs } => ListItem::new(id)
                .accessibility_label(match needs {
                    Some(_) => "All OpenRouter models, can’t be used yet",
                    None => "All OpenRouter models",
                })
                .child(
                    h_flex()
                        .text_color(cx.theme().muted_foreground)
                        .child(ALL_OPENROUTER),
                ),
        };
        Some(item)
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let Some(row) = self.selected.and_then(|ix| self.row(ix)).cloned() else {
            return;
        };
        let (select, query) = (self.select.clone(), self.query.trim().to_string());
        window.defer(cx, move |window, cx| {
            select
                .update(cx, |select, cx| select.pick(row, query, window, cx))
                .ok();
        });
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        let select = self.select.clone();
        window.defer(cx, move |window, cx| {
            select
                .update(cx, |select, cx| select.set_open(false, window, cx))
                .ok();
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{Row, label_of, sections};
    use crate::backend::{ModelChoice, ModelChoices, ModelGroup};

    pub(crate) fn models() -> ModelChoices {
        let group = |name: &str, needs: Option<&str>, models: &[(&str, &str)]| ModelGroup {
            name: name.into(),
            needs: needs.map(str::to_string),
            models: models
                .iter()
                .map(|(name, alias)| ModelChoice {
                    name: (*name).into(),
                    alias: (*alias).into(),
                })
                .collect(),
        };
        ModelChoices {
            groups: vec![
                group(
                    "Anthropic API, billed per token",
                    None,
                    &[("claude-opus-5-5", "opus")],
                ),
                group(
                    "OpenRouter, billed per token",
                    Some("an OpenRouter key"),
                    &[("openrouter:moonshotai/kimi-k3", "")],
                ),
            ],
            default: None,
            note: None,
        }
    }

    #[test]
    fn the_button_names_the_model_and_its_way() {
        let models = models();
        let known = crate::backend::sample_catalog(std::time::SystemTime::now()).models;
        assert_eq!(
            label_of("claude-opus-5-5", Some(&models), &known),
            "Opus 5.5 · Anthropic API"
        );
        assert_eq!(
            label_of("openrouter:moonshotai/kimi-k3", Some(&models), &known),
            "Kimi K3 · OpenRouter (can’t be used yet)"
        );
        assert_eq!(
            label_of("openrouter:z-ai/glm-5", Some(&models), &known),
            "GLM 5 · OpenRouter"
        );
        // Without its details, arko names it from its slug.
        assert_eq!(
            label_of("openrouter:z-ai/glm-5", Some(&models), &[]),
            "Glm 5 · OpenRouter"
        );
        assert_eq!(label_of("gpt-9", Some(&models), &known), "gpt-9");
    }

    /// A model from OpenRouter's catalog joins the OpenRouter group, before
    /// the row that opens the catalog. Any other unlisted model ends the
    /// list as written.
    #[test]
    fn an_unlisted_choice_still_shows() {
        let models = models();
        let rows = sections(
            &"Last used".into(),
            Some(&models),
            &[],
            "openrouter:z-ai/glm-5",
        );
        let openrouter = &rows[2].rows;
        assert_eq!(openrouter.len(), 3);
        assert!(
            matches!(&openrouter[1], Row::Model { name, .. } if name == "openrouter:z-ai/glm-5")
        );
        assert!(matches!(openrouter[2], Row::Browse { .. }));
        let rows = sections(&"Last used".into(), Some(&models), &[], "gpt-9");
        assert_eq!(rows[0].rows.len(), 2);
    }
}
