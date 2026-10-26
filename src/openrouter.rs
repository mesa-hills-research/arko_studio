//! OpenRouter's models in the app: telling a model of OpenRouter's by its
//! name, and the browser of OpenRouter's whole catalog, which the
//! composer's model list and the model settings open.

use std::rc::Rc;

use gpui_kit::component::list::{List, ListDelegate, ListItem, ListState};
use gpui_kit::component::{ActiveTheme as _, IconName, IndexPath, Sizable as _, WindowExt as _};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{Catalog, Listed, SharedBackend};

/// How `/model` names a model of OpenRouter's: `openrouter:vendor/slug`.
pub const PREFIX: &str = "openrouter:";

/// The slug of `name` when it names one of OpenRouter's models
/// (`openrouter:moonshotai/kimi-k3`), as arko takes it: any slug that
/// isn't empty and has no spaces.
pub fn slug(name: &str) -> Option<&str> {
    name.strip_prefix(PREFIX)
        .filter(|slug| !slug.is_empty() && !slug.contains(char::is_whitespace))
}

/// Sets a place's model to one of OpenRouter's.
pub type OnChoose = Rc<dyn Fn(Listed, &mut Window, &mut App)>;

/// How the browser opens.
#[derive(Debug, Clone, Default)]
pub struct Browse {
    /// The search it opens on.
    pub query: String,
    /// The model the place has now, checked in the list.
    pub current: Option<String>,
    /// What OpenRouter's models need that isn't there: "an OpenRouter key".
    pub needs: Option<String>,
    /// Its models can't be chosen while they need something (a session
    /// about to start). Settings can still name one.
    pub refuse_unusable: bool,
}

/// Opens OpenRouter's catalog in a dialog, on `browse.query`, once what
/// opens it is done (a menu closing gives the focus back). Choosing a
/// model closes it and calls `choose`. The catalog is fetched again first
/// when the one arko keeps is stale.
pub fn browse(browse: Browse, choose: OnChoose, window: &mut Window, cx: &mut App) {
    window.defer(cx, move |window, cx| open(browse, choose, window, cx));
}

fn open(browse: Browse, choose: OnChoose, window: &mut Window, cx: &mut App) {
    let browser = cx.new(|cx| Browser::new(browse, choose, window, cx));
    let content = browser.clone();
    window.open_dialog(cx, move |dialog, _, _| {
        let browser = content.clone();
        dialog
            .w(px(620.))
            .title("OpenRouter models")
            .content(move |content, _, _| content.child(browser.clone()))
    });
    browser.update(cx, |browser, cx| browser.focus(window, cx));
}

/// The browser's content: the search, the models and what is known of
/// them.
pub struct Browser {
    list: Entity<ListState<CatalogList>>,
    needs: Option<String>,
    /// The catalog, `None` while it is fetched.
    catalog: Option<Catalog>,
    _fetch: Task<()>,
}

impl Browser {
    fn new(browse: Browse, choose: OnChoose, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let query = browse.query.clone();
        let list = cx.new(|cx| {
            ListState::new(
                CatalogList {
                    models: Vec::new(),
                    shown: Vec::new(),
                    current: browse.current.clone(),
                    query: String::new(),
                    locked: browse.refuse_unusable && browse.needs.is_some(),
                    selected: None,
                    choose,
                },
                window,
                cx,
            )
            .searchable(true)
        });
        if !query.is_empty() {
            list.update(cx, |list, cx| list.set_query(&query, window, cx));
        }
        let backend = cx.global::<SharedBackend>().clone();
        let fetching = cx.background_spawn(async move { backend.openrouter_models(false) });
        let fetch = cx.spawn_in(window, async move |this, cx| {
            let catalog = fetching.await;
            this.update_in(cx, |this, window, cx| this.show(catalog, window, cx))
                .ok();
        });
        Self {
            list,
            needs: browse.needs,
            catalog: None,
            _fetch: fetch,
        }
    }

    fn focus(&self, window: &mut Window, cx: &mut App) {
        self.list.update(cx, |list, cx| list.focus(window, cx));
    }

    /// Lists `catalog`, searched as typed so far.
    fn show(&mut self, catalog: Catalog, window: &mut Window, cx: &mut Context<Self>) {
        let models = catalog.models.clone();
        self.catalog = Some(catalog);
        self.list.update(cx, |list, cx| {
            list.delegate_mut().models = models;
            let query = list.delegate().query.clone();
            list.set_query(&query, window, cx);
            // On the current model, else the first found. (The search
            // just made found nothing, the list being empty before.)
            match list.delegate().current_position() {
                Some(at) => {
                    list.set_selected_index(Some(at), window, cx);
                    list.scroll_to_item(at, ScrollStrategy::Center, window, cx);
                }
                None if !list.delegate().shown.is_empty() => {
                    list.set_selected_index(Some(IndexPath::default()), window, cx)
                }
                None => {}
            }
        });
        cx.notify();
    }

    /// The catalog's models, `None` while they're fetched (tests).
    pub fn models(&self) -> Option<&[Listed]> {
        self.catalog
            .as_ref()
            .map(|catalog| catalog.models.as_slice())
    }
}

impl Render for Browser {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let status = match &self.catalog {
            None => Some(("Loading…".to_string(), muted)),
            Some(catalog) => match &catalog.problem {
                Some(problem) => Some((crate::backend::sentence(problem), danger)),
                None if catalog.models.is_empty() => Some((
                    "OpenRouter offers no models with tool use under these routing settings."
                        .to_string(),
                    muted,
                )),
                None => None,
            },
        };
        v_flex()
            .id("openrouter-browser")
            .test_support()
            .gap_2()
            .when_some(self.needs.clone(), |this, needs| {
                this.child(
                    div()
                        .id("openrouter-browser-needs")
                        .test_support()
                        .text_sm()
                        .text_color(muted)
                        .child(format!(
                            "These models need {needs}. Add it in Settings › Models & accounts."
                        )),
                )
            })
            .child(
                div().h(rems(26.)).child(
                    List::new(&self.list)
                        .small()
                        .search_placeholder("Find a model"),
                ),
            )
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .text_xs()
                    .text_color(muted)
                    .child(
                        div()
                            .id("openrouter-browser-status")
                            .test_support()
                            .when_some(status, |this, (text, color)| {
                                this.text_color(color).child(text)
                            }),
                    )
                    .child("Context, and the price per million tokens in and out"),
            )
    }
}

/// The indexes of the models of `models` that match the search `query`,
/// those that start with it first, as [`Catalog::search`] orders them.
pub fn search(models: &[Listed], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut found: Vec<usize> = (0..models.len())
        .filter(|at| models[*at].matches(&query))
        .collect();
    if !query.is_empty() {
        found.sort_by_key(|at| {
            let model = &models[*at];
            !(model.slug().to_lowercase().starts_with(&query)
                || model.label.to_lowercase().starts_with(&query))
        });
    }
    found
}

/// The catalog's models that match the search.
pub struct CatalogList {
    models: Vec<Listed>,
    /// Indexes into `models`.
    shown: Vec<usize>,
    current: Option<String>,
    /// The search, as typed.
    query: String,
    /// Its models can't be chosen.
    locked: bool,
    selected: Option<IndexPath>,
    choose: OnChoose,
}

impl CatalogList {
    fn model(&self, ix: IndexPath) -> Option<&Listed> {
        self.models.get(*self.shown.get(ix.row)?)
    }

    fn current_position(&self) -> Option<IndexPath> {
        let current = self.current.as_deref()?;
        self.shown
            .iter()
            .position(|at| self.models[*at].name == current)
            .map(IndexPath::new)
    }
}

impl ListDelegate for CatalogList {
    type Item = ListItem;

    fn perform_search(
        &mut self,
        query: &str,
        _: &mut Window,
        _: &mut Context<ListState<Self>>,
    ) -> Task<()> {
        self.query = query.to_string();
        self.shown = search(&self.models, query);
        Task::ready(())
    }

    fn items_count(&self, _: usize, _: &App) -> usize {
        self.shown.len()
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<Self::Item> {
        let model = self.model(ix)?;
        let muted = cx.theme().muted_foreground;
        let checked = self.current.as_deref() == Some(model.name.as_str());
        let slug = model.slug().to_string();
        Some(
            ListItem::new(ix.row)
                .disabled(self.locked)
                .confirmed(checked)
                .check_icon(IconName::Check)
                .accessibility_label(format!("{}, {}", model.label, model.details()))
                .child(
                    h_flex()
                        .w_full()
                        .gap_3()
                        .justify_between()
                        .child(
                            h_flex()
                                .min_w_0()
                                .gap_2()
                                .child(div().flex_none().child(model.label.clone()))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(slug),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(muted)
                                .child(model.short()),
                        ),
                ),
        )
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
        if self.locked {
            return;
        }
        let Some(model) = self.selected.and_then(|ix| self.model(ix)).cloned() else {
            return;
        };
        let choose = self.choose.clone();
        window.defer(cx, move |window, cx| {
            window.close_dialog(cx);
            choose(model, window, cx);
        });
    }

    fn cancel(&mut self, window: &mut Window, cx: &mut Context<ListState<Self>>) {
        window.defer(cx, |window, cx| window.close_dialog(cx));
    }
}

#[cfg(test)]
mod tests {
    use super::{search, slug};

    #[test]
    fn a_slug_is_what_follows_the_prefix() {
        assert_eq!(slug("openrouter:z-ai/glm-5"), Some("z-ai/glm-5"));
        assert_eq!(slug("openrouter:"), None);
        assert_eq!(slug("claude-opus-5-5"), None);
        assert_eq!(slug("openrouter:a b"), None);
    }

    /// The search finds a model by its name or its slug, those that start
    /// with what's typed first.
    #[test]
    fn the_search_puts_models_that_start_with_it_first() {
        let catalog = crate::backend::sample_catalog(std::time::SystemTime::now());
        let names = |query: &str| -> Vec<String> {
            search(&catalog.models, query)
                .into_iter()
                .map(|at| catalog.models[at].label.clone())
                .collect()
        };
        assert_eq!(names("kimi"), ["Kimi K3"]);
        assert_eq!(names("moonshot"), ["Kimi K3"]);
        // DeepSeek starts with "de", and Claude only has it inside.
        assert_eq!(names("de")[0], "DeepSeek V4");
        assert_eq!(names("").len(), 6);
    }
}
