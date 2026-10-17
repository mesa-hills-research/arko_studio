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
//!
//! Mount it with [`TranscriptView::cached`], so that what happens around it
//! (a keystroke in the composer) doesn't draw it again.

mod find;
mod inbox;
mod model;
mod row;
#[cfg(test)]
mod tests;
mod tool;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use arko::remote::{CallStatus, Feed, Kind};
use gpui_kit::base::image_cache::LruImageCache;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::WindowExt as _;
use gpui_kit::component::diff_view::DiffViewState;
use gpui_kit::component::image_viewer::ImageViewerItem;
use gpui_kit::component::input;
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::text::{TextBlockContent, TextBlockId, TextViewState};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::prefs;

pub use find::FindStatus;
pub use model::AgentFilter;
use model::{Changes, Layout, Model, Row, RowKey};

/// The key context of the transcript, for its key bindings.
pub const CONTEXT: &str = "Transcript";

/// Code blocks show this many lines until the reader asks for all of them.
pub const CODE_LINES: usize = 40;

/// How much decoded image data a transcript keeps (thumbnails, Markdown's
/// embedded images).
const IMAGE_BUDGET: usize = 64 * 1024 * 1024;

/// The latest entries made ready to show when a transcript starts over (a
/// session opens, another conversation): the reader sees the end of it at
/// once, and the older entries are made ready a few at a time.
const FIRST_ENTRIES: usize = 200;
/// Older entries made ready per frame after the first: each is a Markdown
/// parse at most, so a frame stays well under 16 ms.
const ENTRIES_PER_FRAME: usize = 100;

/// What the transcript asks of the views around it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscriptEvent {
    /// The reader opened a worker's card: show that worker in the workers
    /// panel.
    OpenWorker(SharedString),
    /// Edit the user's message, by its item.
    Edit(u64),
    /// Send the user's last message again for a new reply.
    Regenerate,
    /// Branch: a new session with the conversation before the user's
    /// message, by its item.
    Branch(u64),
    /// Where the find is now: the find bar shows it.
    FindStatus(FindStatus),
}

/// The transcript's copy of the preferences it follows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    view: prefs::TranscriptView,
    render_markdown: bool,
    stream_fade: bool,
    expand_thinking: bool,
}

impl Settings {
    fn of(prefs: &prefs::Prefs) -> Self {
        Self {
            view: prefs.view,
            render_markdown: prefs.appearance.render_markdown,
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
    /// A call's details, a thinking's text or a group of calls, opened or
    /// closed by the reader. `None` follows the defaults.
    expanded: Option<bool>,
    /// The text shows as its Markdown source ("View Source").
    source: bool,
    /// Code blocks shown whole, by block index.
    uncapped: HashSet<usize>,
    /// An edit's diff, made when its details first open.
    diff: Option<Entity<DiffViewState>>,
}

/// Returns the current time in Unix milliseconds.
pub type Clock = Rc<dyn Fn() -> i64>;

/// Reads an image's bytes and MIME type by its ID in the session's store.
type ImageLookup = Rc<dyn Fn(u64) -> Option<(Arc<[u8]>, String)>>;

/// A session's transcript (the main agent's, or a worker's).
pub struct TranscriptView {
    model: Model,
    inbox: inbox::Inbox,
    ui: HashMap<u64, EntryUi>,
    rows: Vec<Row>,
    scroller: Entity<MessageScrollerState>,
    focus_handle: FocusHandle,
    settings: Settings,
    /// The reply whose hover actions show.
    hovered: Option<u64>,
    /// Entries that came while the reader was scrolled up.
    new_while_away: usize,
    /// The oldest entry ready to show, while older ones wait to be made
    /// ready ([`FIRST_ENTRIES`]).
    loaded_from: Option<u64>,
    /// Where to scroll once the rows are built (the Messages view's
    /// counts open the Full view at their calls).
    scroll_to: Option<RowKey>,
    clock: Clock,
    /// Draws again every second while a call or a worker runs.
    ticker: Option<Task<()>>,
    image_cache: Entity<LruImageCache>,
    /// Where the images the items show are read: the session's store.
    images: Option<ImageLookup>,
    /// The images read so far, by ID, ready for the image cache.
    image_sources: RefCell<HashMap<u64, Arc<Image>>>,
    /// The find going on (Mod+F).
    find: Option<find::Find>,
    renders: usize,
    _prefs: Subscription,
}

impl EventEmitter<TranscriptEvent> for TranscriptView {}

impl Focusable for TranscriptView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TranscriptView {
    /// The transcript of `feed`'s session, as `filter` says whose.
    pub fn new(feed: &Feed, filter: AgentFilter, _: &mut Window, cx: &mut Context<Self>) -> Self {
        warm_up_languages(cx);
        let (snapshot, inbox) = inbox::Inbox::subscribe(feed, cx);
        let settings = Settings::of(prefs::get(cx));
        let observe_prefs = cx.observe_global::<prefs::PrefsStore>(|this, cx| {
            let settings = Settings::of(prefs::get(cx));
            if settings != this.settings {
                let rows_change = settings.view != this.settings.view
                    || settings.render_markdown != this.settings.render_markdown
                    || settings.expand_thinking != this.settings.expand_thinking;
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
            focus_handle: cx.focus_handle(),
            settings,
            hovered: None,
            new_while_away: 0,
            loaded_from: None,
            scroll_to: None,
            clock: Rc::new(|| chrono::Utc::now().timestamp_millis()),
            ticker: None,
            image_cache: LruImageCache::new(IMAGE_BUDGET, cx),
            images: None,
            image_sources: RefCell::default(),
            find: None,
            renders: 0,
            _prefs: observe_prefs,
        };
        let mut changes = Changes::default();
        this.model.load(snapshot, &mut changes);
        this.follow(changes, cx);
        this
    }

    /// The view to put in the window: cached, so that it draws again only
    /// when something in it changed, not whenever the view around it does.
    pub fn cached(this: &Entity<Self>) -> AnyElement {
        this.clone()
            .cached(StyleRefinement::default().size_full())
            .into_any_element()
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

    /// The user's latest message that can be edited (↑ in the empty
    /// composer edits it).
    pub fn last_message(&self) -> Option<u64> {
        self.model.last_message()
    }

    /// The reply Regenerate replaces: the main agent's latest, after the
    /// user's last message.
    pub fn last_reply(&self) -> Option<u64> {
        self.model.last_reply()
    }

    /// Where the user's message `id` is in the conversation.
    pub fn message_index(&self, id: u64) -> Option<usize> {
        self.model.entry(id)?.item.message_index
    }

    /// The text and the images of the user's message `id`.
    pub fn message(&self, id: u64) -> Option<(String, Vec<arko::images::ImageRef>)> {
        let item = &self.model.entry(id)?.item;
        Some((item.text.clone(), item.images.clone()))
    }

    /// How many messages after the user's message `id` an edit of it
    /// removes: the user's, and a reply per turn.
    pub fn messages_after(&self, id: u64) -> usize {
        self.model.messages_after(id)
    }

    /// Why the conversation can't be edited, sent again or branched now, if
    /// it can't.
    pub fn busy_reason(&self) -> Option<&'static str> {
        self.model.busy_reason()
    }

    /// Whether the user's message `id` can be edited (or branched from).
    pub fn is_editable(&self, id: u64) -> bool {
        self.model
            .entry(id)
            .is_some_and(|entry| Model::is_changeable(&entry.item))
            && matches!(self.model.filter(), AgentFilter::Main)
    }

    /// Opens or closes the details of the feed item `item`: a call's
    /// parameters and output, a thinking's text, a group of calls. (Find
    /// opens the row it lands on.)
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

    /// Shows the images the items show (the user's attachments, what tools
    /// returned) from `images`, the session's store.
    pub fn set_images(&mut self, images: arko::images::Images, cx: &mut Context<Self>) {
        self.set_image_lookup(
            move |id| {
                images
                    .get(id)
                    .map(|image| (image.bytes.clone(), image.mime.clone()))
            },
            cx,
        );
    }

    /// Reads images by ID from `lookup` instead of a session's store: for
    /// tests and screenshots.
    #[doc(hidden)]
    pub fn set_image_lookup(
        &mut self,
        lookup: impl Fn(u64) -> Option<(Arc<[u8]>, String)> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.images = Some(Rc::new(lookup));
        self.image_sources.borrow_mut().clear();
        cx.notify();
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
            self.hovered = None;
            let ids: Vec<u64> = self.model.entries().iter().map(|e| e.item.id).collect();
            let start = ids.len().saturating_sub(FIRST_ENTRIES);
            for id in &ids[start..] {
                self.add_ui(*id, cx);
            }
            self.loaded_from = ids.get(start).copied().filter(|_| start > 0);
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
        self.find_changed(cx);
        cx.notify();
    }

    /// Makes the next older entries ready to show, if some wait. Returns
    /// whether more wait still.
    fn load_older(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(from) = self.loaded_from else {
            return false;
        };
        let end = self.model.position(from).unwrap_or_else(|ix| ix);
        let start = end.saturating_sub(ENTRIES_PER_FRAME);
        let entries = self.model.entries();
        let ids: Vec<u64> = entries[start..end].iter().map(|e| e.item.id).collect();
        self.loaded_from = entries.get(start).map(|e| e.item.id).filter(|_| start > 0);
        for id in ids {
            self.add_ui(id, cx);
        }
        self.refresh_rows(cx);
        self.loaded_from.is_some()
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
                this.find_changed(cx);
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
        if let Some(view) = &row.view {
            return Some(view.clone());
        }
        let view = cx.new(|cx| TextViewState::block_range(&state, block..block + 1, cx));
        row.view = Some(view.clone());
        // Drawn for the first time while a find goes on: its matches show.
        self.highlight_block(id, block, &view, cx);
        Some(view)
    }

    /// Rebuilds the rows from the model, and tells the list what changed:
    /// rows kept at the start and the end keep their place and height.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        let render_markdown = self.settings.render_markdown;
        let ui = &self.ui;
        let blocks = |id: u64| {
            ui.get(&id)
                .and_then(|ui| ui.doc.as_ref())
                .map_or((0, 0), |doc| (doc.blocks.len(), doc.generation))
        };
        let expanded = |id: u64| ui.get(&id).and_then(|ui| ui.expanded);
        let source = |id: u64| !render_markdown || ui.get(&id).is_some_and(|ui| ui.source);
        let rows = model::rows(
            &self.model,
            &Layout {
                view: self.settings.view,
                expand_thinking: self.settings.expand_thinking,
                blocks: &blocks,
                expanded: &expanded,
                source: &source,
                first: self.loaded_from,
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
        if let Some(key) = self.scroll_to
            && let Some(ix) = self.rows.iter().position(|row| row.key == key)
        {
            self.scroll_to = None;
            self.scroller
                .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
        }
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
    /// text, a group of calls.
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

    fn toggle_source(&mut self, id: u64, cx: &mut Context<Self>) {
        let ui = self.ui.entry(id).or_default();
        ui.source = !ui.source;
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

    fn set_hovered(&mut self, id: u64, hovered: bool, cx: &mut Context<Self>) {
        let before = self.hovered;
        if hovered {
            self.hovered = Some(id);
        } else if self.hovered == Some(id) {
            self.hovered = None;
        }
        if self.hovered != before {
            cx.notify();
        }
    }

    /// The Messages view's count of a run: opens the Full view there.
    fn open_run(&mut self, first: u64, cx: &mut Context<Self>) {
        self.scroll_to = Some(RowKey::Call(first));
        prefs::update(cx, |prefs| prefs.view = prefs::TranscriptView::Full);
    }

    fn open_worker(&mut self, worker: SharedString, cx: &mut Context<Self>) {
        cx.emit(TranscriptEvent::OpenWorker(worker));
    }

    /// Asks to edit, send again or branch (a hover action).
    fn change(&mut self, event: TranscriptEvent, cx: &mut Context<Self>) {
        cx.emit(event);
    }

    /// The row an entry's first row is at.
    fn row_of(&self, id: u64) -> Option<usize> {
        self.rows.iter().position(|row| match row.key {
            RowKey::Separator(row)
            | RowKey::User(row)
            | RowKey::Refresh(row)
            | RowKey::Actions(row)
            | RowKey::Thinking(row)
            | RowKey::Call(row)
            | RowKey::Group(row)
            | RowKey::Count(row)
            | RowKey::Notice(row)
            | RowKey::Output(row)
            | RowKey::Turn(row)
            | RowKey::Worker(row) => row == id,
            RowKey::Block { id: row, .. } => row == id,
        })
    }

    /// Copies what is selected in the transcript (Mod+C).
    fn copy(&mut self, _: &input::Copy, window: &mut Window, cx: &mut Context<Self>) {
        let text = TextSelection::selected_text(window, cx);
        if text.trim().is_empty() {
            cx.propagate();
            return;
        }
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    /// The text of an entry: as Markdown, or as the reader sees it.
    fn entry_text(&self, id: u64, markdown: bool, cx: &App) -> Option<String> {
        let entry = self.model.entry(id)?;
        if markdown {
            return Some(entry.item.text.trim().to_string());
        }
        let doc = self.ui.get(&id)?.doc.as_ref()?;
        Some(
            doc.state
                .read(cx)
                .rendered_text()
                .as_str()
                .trim()
                .to_string(),
        )
    }

    /// The image `id` of the session's store, ready to draw, while the
    /// store keeps it.
    fn image(&self, id: u64) -> Option<Arc<Image>> {
        if let Some(image) = self.image_sources.borrow().get(&id) {
            return Some(image.clone());
        }
        let (bytes, mime) = (self.images.as_ref()?)(id)?;
        let format = ImageFormat::from_mime_type(&mime)?;
        let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
        self.image_sources.borrow_mut().insert(id, image.clone());
        Some(image)
    }

    /// Opens the image viewer on the image `image` of the entry `entry`,
    /// with every image of the conversation to browse, in order.
    fn open_image(&mut self, entry: u64, image: u64, window: &mut Window, cx: &mut Context<Self>) {
        let mut items = Vec::new();
        let mut selected = 0;
        for shown in self.model.entries() {
            for image_ref in &shown.item.images {
                let Some(source) = self.image(image_ref.id) else {
                    continue;
                };
                if shown.item.id == entry && image_ref.id == image {
                    selected = items.len();
                }
                let caption = image_caption(&image_ref.label, image_ref.bytes);
                items.push(
                    ImageViewerItem::new(LruImageCache::source(&self.image_cache, source, None))
                        .with_caption(caption),
                );
            }
        }
        if !items.is_empty() {
            window.open_image_viewer(items, selected, cx);
        }
    }

    fn copy_entry(&mut self, id: u64, markdown: bool, cx: &mut Context<Self>) {
        if let Some(text) = self.entry_text(id, markdown, cx) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.renders += 1;
        self.apply_pending(cx);
        if self.load_older(cx) {
            // The rows of older entries go above the reader: nothing moves.
            window.request_animation_frame();
        }
        let scrolled_up = self.scroller.read(cx).is_scrolled_up();
        if !scrolled_up {
            self.new_while_away = 0;
        }
        let new = self.new_while_away;
        let view = cx.entity().downgrade();
        image_cache(self.image_cache.clone()).size_full().child(
            div()
                .id("transcript")
                .key_context(CONTEXT)
                .track_focus(&self.focus_handle)
                .size_full()
                .on_action(cx.listener(Self::copy))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !this.focus_handle.contains_focused(window, cx) {
                            this.focus_handle.focus(window, cx);
                        }
                    }),
                )
                .child(
                    MessageScroller::new(
                        "transcript-rows",
                        self.scroller.clone(),
                        move |ix, window, cx| row::render(&view, ix, window, cx),
                    )
                    .with_row_style(StyleRefinement::default().px_0().pb_0())
                    .with_jump_button_label("Jump to latest")
                    .when(new > 0, |scroller| {
                        scroller.with_jump_button_renderer(move |button| {
                            button.label(format!("{new} new"))
                        })
                    })
                    .size_full(),
                ),
        )
    }
}

/// Plain text that takes part in the window's selection, in a row.
fn selectable(id: impl Into<ElementId>, text: impl Into<SharedString>) -> SelectableText {
    SelectableText::new(id, text)
}

/// "screenshot.png · 240 KB": an image's label and size.
fn image_caption(label: &str, bytes: usize) -> String {
    let size = match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", bytes.div_ceil(1024)),
        _ => format!("{:.1} MB", bytes as f64 / 1_048_576.),
    };
    if label.is_empty() {
        size
    } else {
        format!("{label} · {size}")
    }
}
