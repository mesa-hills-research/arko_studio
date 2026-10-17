//! The transcript: a session's feed as the reader sees it, in the main
//! column (the main agent's) and in the workers panel (a worker's).
//!
//! A [`TranscriptView`] subscribes to the session's [`Feed`] itself. The
//! updates are drained off the UI thread into an inbox and applied once per
//! frame, when the view draws ([`inbox`]). The rows live in GPUI Kit's
//! `MessageScroller`, a virtualized list that follows the latest row while
//! the reader is at the bottom and keeps the reader's place otherwise. An
//! assistant's text is one Markdown document per entry, shown as one row per
//! top-level block, so a long answer only lays out the blocks on screen and
//! a finished block is never parsed again.

mod inbox;
mod model;
mod row;
#[cfg(test)]
mod tests;
mod tool;

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::time::Duration;

use arko::remote::{CallStatus, Feed, Kind};
use gpui_kit::base::SelectableText;
use gpui_kit::component::diff_view::DiffViewState;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::{TextBlockContent, TextBlockId, TextViewState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs;

pub use model::AgentFilter;
use model::{Changes, Layout, Model, Row, RowKey};

/// Code blocks show this many lines until the reader asks for all of them.
pub const CODE_LINES: usize = 40;

/// What the transcript asks of the views around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// The reader opened a worker's card: show that worker in the workers
    /// panel.
    OpenWorker(SharedString),
    // TODO: Edit, Regenerate and Branch of a message (by its item), once
    // the app can edit a conversation. The hover actions of user messages
    // and replies have room for them.
}

/// The transcript's copy of the preferences it follows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    stream_fade: bool,
    expand_thinking: bool,
}

impl Settings {
    fn of(prefs: &prefs::Prefs) -> Self {
        Self {
            stream_fade: prefs.appearance.stream_fade,
            expand_thinking: prefs.appearance.expand_thinking,
        }
    }
}

/// An entry's text as Markdown: the document, and a view of each of its
/// top-level blocks, which the rows show.
struct Doc {
    state: Entity<TextViewState>,
    /// Exactly the text pushed to the document, which its blocks' source
    /// ranges index.
    text: String,
    blocks: Vec<Block>,
    /// Changes when blocks are dropped (the text was replaced), so their
    /// rows are replaced too.
    generation: u32,
    _observe: Subscription,
}

struct Block {
    id: TextBlockId,
    /// The view of the block, made when its row is first drawn: a long
    /// session opens without a view per block of its history.
    view: Option<Entity<TextViewState>>,
    content: TextBlockContent,
    range: Option<Range<usize>>,
}

/// What the reader chose for an entry, and what its rows need.
#[derive(Default)]
struct EntryUi {
    doc: Option<Doc>,
    /// A call's details or a thinking's text, opened or closed by the
    /// reader. `None` follows the defaults.
    expanded: Option<bool>,
    /// Code blocks shown whole, by block index.
    uncapped: HashSet<usize>,
    /// An edit's diff, made when its details first open.
    diff: Option<Entity<DiffViewState>>,
}

/// Returns the current time in Unix milliseconds.
pub type Clock = Rc<dyn Fn() -> i64>;

/// A session's transcript (the main agent's, or a worker's).
pub struct TranscriptView {
    model: Model,
    inbox: inbox::Inbox,
    ui: HashMap<u64, EntryUi>,
    rows: Vec<Row>,
    scroller: Entity<MessageScrollerState>,
    settings: Settings,
    /// Entries that came while the reader was scrolled up.
    new_while_away: usize,
    clock: Clock,
    /// Draws again every second while a call or a worker runs.
    ticker: Option<Task<()>>,
    renders: usize,
    _prefs: Subscription,
}

impl EventEmitter<TranscriptEvent> for TranscriptView {}

impl TranscriptView {
    /// The transcript of `feed`'s session, as `filter` says whose.
    pub fn new(feed: &Feed, filter: AgentFilter, _: &mut Window, cx: &mut Context<Self>) -> Self {
        warm_up_languages(cx);
        let (snapshot, inbox) = inbox::Inbox::subscribe(feed, cx);
        let settings = Settings::of(prefs::get(cx));
        let observe_prefs = cx.observe_global::<prefs::PrefsStore>(|this, cx| {
            let settings = Settings::of(prefs::get(cx));
            if settings != this.settings {
                let rows_change = settings.expand_thinking != this.settings.expand_thinking;
                this.settings = settings;
                if rows_change {
                    this.refresh_rows(cx);
                }
                cx.notify();
            }
        });
        let mut this = Self {
            model: Model::new(filter),
            inbox,
            ui: HashMap::new(),
            rows: Vec::new(),
            scroller: cx.new(|cx| MessageScrollerState::new(0, cx)),
            settings,
            new_while_away: 0,
            clock: Rc::new(|| chrono::Utc::now().timestamp_millis()),
            ticker: None,
            renders: 0,
            _prefs: observe_prefs,
        };
        let mut changes = Changes::default();
        this.model.load(snapshot, &mut changes);
        this.follow(changes, cx);
        this
    }

    pub fn filter(&self) -> &AgentFilter {
        self.model.filter()
    }

    /// Scrolls to the latest row and follows the rows from there.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
    }

    /// Whether the transcript follows the latest row.
    pub fn is_following(&self, cx: &App) -> bool {
        !self.scroller.read(cx).is_scrolled_up()
    }

    /// The number of rows (for tests and diagnostics).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// How many times the view has drawn (for tests and measurements).
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// Opens or closes the details of the feed item `item`: a call's
    /// parameters and output, a thinking's text. (Find opens the row it
    /// lands on.)
    pub fn set_expanded(&mut self, item: u64, open: bool, cx: &mut Context<Self>) {
        let current = self.ui.get(&item).and_then(|ui| ui.expanded);
        let default = self
            .model
            .entry(item)
            .is_some_and(|entry| entry.item.kind == Kind::Thinking)
            && self.thinking_open_by_default();
        if current.unwrap_or(default) != open {
            self.toggle(item, default, cx);
        }
    }

    /// Reads the time from `clock` instead of the system's: for tests and
    /// screenshots, whose elapsed times must not change.
    #[doc(hidden)]
    pub fn set_clock(&mut self, clock: impl Fn() -> i64 + 'static, cx: &mut Context<Self>) {
        self.clock = Rc::new(clock);
        cx.notify();
    }

    /// Applies what came from the feed since the last frame.
    pub fn apply_pending(&mut self, cx: &mut Context<Self>) {
        let pending = self.inbox.take();
        if pending.updates.is_empty() && pending.activity.is_none() {
            return;
        }
        let mut changes = Changes::default();
        for update in pending.updates {
            self.model.apply(update, &mut changes);
        }
        if let Some(activity) = pending.activity {
            self.model.set_activity(activity, &mut changes);
        }
        self.follow(changes, cx);
    }

    /// Brings the drawn side up to `changes` of the model.
    fn follow(&mut self, changes: Changes, cx: &mut Context<Self>) {
        if !changes.any() {
            return;
        }
        if changes.reset {
            self.ui.clear();
            let ids: Vec<u64> = self.model.entries().iter().map(|e| e.item.id).collect();
            for id in ids {
                self.add_ui(id, cx);
            }
            self.rows.clear();
            self.scroller
                .update(cx, |scroller, cx| scroller.reset(0, cx));
            self.new_while_away = 0;
        } else {
            for id in &changes.removed {
                self.ui.remove(id);
            }
            for id in &changes.added {
                self.add_ui(*id, cx);
            }
            for id in changes.text.iter().chain(&changes.changed) {
                self.sync_text(*id, cx);
            }
            if self.scroller.read(cx).is_scrolled_up() {
                self.new_while_away += changes
                    .added
                    .iter()
                    .filter(|id| {
                        self.model
                            .entry(**id)
                            .is_some_and(|e| e.item.kind != Kind::Thinking)
                    })
                    .count();
            }
        }
        self.refresh_rows(cx);
        self.tick_while_running(cx);
        cx.notify();
    }

    /// Makes what an entry's rows need: a Markdown document for text.
    fn add_ui(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.model.entry(id) else {
            return;
        };
        let has_doc = matches!(
            entry.item.kind,
            Kind::Text | Kind::Reply | Kind::Message | Kind::Question | Kind::Thinking
        );
        let text = entry.item.text.clone();
        let mut ui = EntryUi::default();
        if has_doc {
            ui.doc = Some(self.new_doc(id, text, cx));
        }
        self.ui.insert(id, ui);
        if has_doc {
            self.sync_blocks(id, cx);
        }
    }

    fn new_doc(&mut self, id: u64, text: String, cx: &mut Context<Self>) -> Doc {
        let state = cx.new(|cx| TextViewState::markdown(&text, cx));
        let observe = cx.observe(&state, move |this, _, cx| {
            if this.sync_blocks(id, cx) {
                this.refresh_rows(cx);
                cx.notify();
            }
        });
        Doc {
            state,
            text,
            blocks: Vec::new(),
            generation: 0,
            _observe: observe,
        }
    }

    /// Brings an entry's document up to its item's text.
    fn sync_text(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(entry) = self.model.entry(id) else {
            return;
        };
        let text = &entry.item.text;
        let needs_doc = matches!(
            entry.item.kind,
            Kind::Text | Kind::Reply | Kind::Message | Kind::Question | Kind::Thinking
        );
        let Some(ui) = self.ui.get_mut(&id) else {
            return;
        };
        let Some(doc) = ui.doc.as_mut() else {
            if needs_doc {
                let text = text.clone();
                let doc = self.new_doc(id, text, cx);
                if let Some(ui) = self.ui.get_mut(&id) {
                    ui.doc = Some(doc);
                }
                self.sync_blocks(id, cx);
            }
            return;
        };
        if *text == doc.text {
            return;
        }
        if let Some(delta) = text.strip_prefix(doc.text.as_str()) {
            let delta = delta.to_string();
            doc.text.push_str(&delta);
            doc.state.update(cx, |state, cx| state.push_str(&delta, cx));
        } else if text.trim() == doc.text.trim() {
            // The feed ends a streamed reply with its text trimmed: the
            // document keeps its blocks (and the reader's selection).
        } else {
            doc.text = text.clone();
            let text = text.clone();
            doc.state.update(cx, |state, cx| state.set_text(&text, cx));
        }
    }

    /// Brings an entry's block views up to its document's latest parse.
    /// Returns whether its rows changed.
    fn sync_blocks(&mut self, id: u64, cx: &mut Context<Self>) -> bool {
        let Some(doc) = self.ui.get_mut(&id).and_then(|ui| ui.doc.as_mut()) else {
            return false;
        };
        let blocks = doc.state.read(cx).blocks();
        let kept = doc
            .blocks
            .iter()
            .zip(&blocks)
            .take_while(|(row, block)| row.id == block.id())
            .count();
        let dropped = doc.blocks.len() > kept;
        if dropped {
            doc.blocks.truncate(kept);
            doc.generation += 1;
        }
        // The last block grows while text streams in.
        for (row, block) in doc
            .blocks
            .iter_mut()
            .zip(&blocks)
            .skip(kept.saturating_sub(1))
        {
            row.range = block.source_range();
            row.content = block.content().clone();
        }
        let added = blocks.len() > kept;
        doc.blocks.extend(blocks[kept..].iter().map(|block| Block {
            id: block.id(),
            view: None,
            content: block.content().clone(),
            range: block.source_range(),
        }));
        dropped || added
    }

    /// The view of block `block` of the entry `id`'s text, made now if its
    /// row is drawn for the first time.
    fn block_view(
        &mut self,
        id: u64,
        block: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TextViewState>> {
        let doc = self.ui.get_mut(&id)?.doc.as_mut()?;
        let state = doc.state.clone();
        let row = doc.blocks.get_mut(block)?;
        Some(
            row.view
                .get_or_insert_with(|| {
                    cx.new(|cx| TextViewState::block_range(&state, block..block + 1, cx))
                })
                .clone(),
        )
    }

    /// Rebuilds the rows from the model, and tells the list what changed:
    /// rows kept at the start and the end keep their place and height.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let ui = &self.ui;
        let blocks = |id: u64| {
            ui.get(&id)
                .and_then(|ui| ui.doc.as_ref())
                .map_or((0, 0), |doc| (doc.blocks.len(), doc.generation))
        };
        let expanded = |id: u64| ui.get(&id).and_then(|ui| ui.expanded);
        let rows = model::rows(
            &self.model,
            &Layout {
                expand_thinking: self.settings.expand_thinking,
                blocks: &blocks,
                expanded: &expanded,
            },
        );
        let old = &self.rows;
        let prefix = old
            .iter()
            .zip(&rows)
            .take_while(|(old, new)| old.key == new.key)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(rows[prefix..].iter().rev())
            .take_while(|(old, new)| old.key == new.key)
            .count();
        let removed = old.len() - prefix - suffix;
        let added = rows.len() - prefix - suffix;
        // Rows kept whose space above changed.
        let regapped: Vec<usize> = old
            .iter()
            .zip(&rows)
            .enumerate()
            .take(prefix)
            .filter(|(_, (old, new))| old.gap != new.gap)
            .map(|(ix, _)| ix)
            .collect();
        self.rows = rows;
        self.scroller.update(cx, |scroller, cx| {
            if removed > 0 || added > 0 {
                scroller.splice(prefix..prefix + removed, added, cx);
            }
            for ix in regapped {
                scroller.remeasure_items(ix..ix + 1, cx);
            }
        });
    }

    /// Draws every second while something shows a running time.
    fn tick_while_running(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() {
            self.ticker = None;
            return;
        }
        if self.ticker.is_some() {
            return;
        }
        self.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                let running = this
                    .update(cx, |this, cx| {
                        let running = this.is_running();
                        if running {
                            cx.notify();
                        } else {
                            this.ticker = None;
                        }
                        running
                    })
                    .unwrap_or(false);
                if !running {
                    break;
                }
            }
        }));
    }

    fn is_running(&self) -> bool {
        self.model
            .entries()
            .iter()
            .any(|entry| entry.item.status == Some(CallStatus::Running))
            || self
                .model
                .workers()
                .iter()
                .any(|worker| worker.state.is_active())
    }

    // ----- what the reader does -----

    /// Opens or closes an entry's disclosure: a call's details, a thinking's
    /// text.
    fn toggle(&mut self, id: u64, open_by_default: bool, cx: &mut Context<Self>) {
        let diff = self
            .ui
            .get(&id)
            .is_none_or(|ui| ui.diff.is_none())
            .then(|| {
                self.model
                    .entry(id)
                    .and_then(|entry| row::edit_diff(&entry.item))
            })
            .flatten()
            .map(|diff| cx.new(|cx| DiffViewState::new(diff, cx)));
        let ui = self.ui.entry(id).or_default();
        ui.expanded = Some(!ui.expanded.unwrap_or(open_by_default));
        if diff.is_some() {
            ui.diff = diff;
        }
        if let Some(ix) = self.row_of(id) {
            self.scroller
                .update(cx, |scroller, cx| scroller.remeasure_items(ix..ix + 1, cx));
        }
        self.refresh_rows(cx);
        cx.notify();
    }

    fn toggle_code(&mut self, id: u64, block: usize, cx: &mut Context<Self>) {
        let ui = self.ui.entry(id).or_default();
        if !ui.uncapped.remove(&block) {
            ui.uncapped.insert(block);
        }
        if let Some(ix) = self.rows.iter().position(|row| {
            matches!(row.key, RowKey::Block { id: row_id, block: row_block, .. }
                if row_id == id && row_block == block)
        }) {
            self.scroller
                .update(cx, |scroller, cx| scroller.remeasure_items(ix..ix + 1, cx));
        }
        cx.notify();
    }

    fn open_worker(&mut self, worker: SharedString, cx: &mut Context<Self>) {
        cx.emit(TranscriptEvent::OpenWorker(worker));
    }

    /// The row an entry's first row is at.
    fn row_of(&self, id: u64) -> Option<usize> {
        self.rows.iter().position(|row| match row.key {
            RowKey::Separator(row)
            | RowKey::User(row)
            | RowKey::Thinking(row)
            | RowKey::Call(row)
            | RowKey::Notice(row)
            | RowKey::Output(row)
            | RowKey::Turn(row)
            | RowKey::Worker(row) => row == id,
            RowKey::Block { id: row, .. } => row == id,
        })
    }
}

/// Prepares the code languages answers use most, off the UI thread, once
/// per app: a code block in a language not ready yet shows plain until it
/// is.
fn warm_up_languages(cx: &mut App) {
    struct Warm;
    impl Global for Warm {}
    if cx.has_global::<Warm>() {
        return;
    }
    cx.set_global(Warm);
    gpui_kit::component::highlighter::LanguageRegistry::singleton().warm_up(
        [
            "rust",
            "python",
            "typescript",
            "javascript",
            "json",
            "bash",
            "toml",
            "yaml",
            "markdown",
            "diff",
        ],
        cx,
    );
}

impl Render for TranscriptView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.apply_pending(cx);
        let scrolled_up = self.scroller.read(cx).is_scrolled_up();
        if !scrolled_up {
            self.new_while_away = 0;
        }
        let new = self.new_while_away;
        let view = cx.entity().downgrade();
        div().id("transcript").size_full().child(
            MessageScroller::new(
                "transcript-rows",
                self.scroller.clone(),
                move |ix, window, cx| row::render(&view, ix, window, cx),
            )
            .with_row_style(StyleRefinement::default().px_0().pb_0())
            .with_jump_button_label("Jump to latest")
            .when(new > 0, |scroller| {
                scroller.with_jump_button_renderer(move |button| button.label(format!("{new} new")))
            })
            .size_full(),
        )
    }
}

/// Plain text that takes part in the window's selection, in a row.
fn selectable(id: impl Into<ElementId>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id, text)
}
