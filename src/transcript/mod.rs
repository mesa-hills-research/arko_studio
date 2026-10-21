//! The transcript: a session's feed as the reader sees it, in the main
//! column (the main agent's) and in the workers panel (a worker's).
//!
//! A [`TranscriptView`] subscribes to the session's [`Feed`] itself. The
//! updates are drained off the UI thread into an inbox and applied once per
//! frame, when the view draws ([`inbox`]). The rows live in a virtualized
//! list that follows the latest row while the reader is at the bottom and
//! keeps the reader's place otherwise ([`list`]). An
//! assistant's text is one Markdown document per entry, shown as one row per
//! top-level block, so a long answer only lays out the blocks on screen and
//! a finished block is never parsed again.
//!
//! Mount it with [`TranscriptView::cached`], so that what happens around it
//! (a keystroke in the composer) doesn't draw it again.

mod find;
mod fold;
mod height;
#[cfg(test)]
mod height_tests;
mod inbox;
mod list;
mod model;
mod nav;
mod row;
#[cfg(test)]
mod scrollbar_tests;
#[cfg(test)]
mod tests;
mod tool;

#[cfg(test)]
#[path = "find_theme_tests.rs"]
mod find_theme_tests;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use arko::remote::{CallStatus, Feed, Kind};
use gpui_kit::base::image_cache::LruImageCache;
use gpui_kit::base::{SelectableText, TextSelection};
use gpui_kit::component::diff_view::DiffViewState;
use gpui_kit::component::image_viewer::ImageViewerItem;
use gpui_kit::component::input;
use gpui_kit::component::text::{TextBlockContent, TextBlockId, TextViewState};
use gpui_kit::component::{ActiveTheme as _, WindowExt as _};
use gpui_kit::*;

use crate::prefs;

pub use find::FindStatus;
pub use model::AgentFilter;
use model::{Changes, Layout, Model, Row, RowKey, Waiting};
pub use nav::{FocusTranscript, Leave, SelectNext, SelectPrevious, ToggleRow};

/// The key context of the transcript, for its key bindings.
pub const CONTEXT: &str = "Transcript";

gpui_kit::actions!(
    transcript,
    [
        /// Scrolls the transcript up by most of a screen.
        ScrollPageUp,
        /// Scrolls the transcript down by most of a screen.
        ScrollPageDown,
        /// Scrolls to the start of the conversation.
        ScrollToStart,
        /// Scrolls to the latest row and follows the rows from there.
        ScrollToLatest,
    ]
);

/// How much of a screen Page Up and Page Down scroll: a few lines stay in
/// sight.
const PAGE: f32 = 0.9;

/// How long the hover actions stay after the pointer left their entry.
const HOVER_GRACE: Duration = Duration::from_millis(150);

/// Code blocks show this many lines until the reader asks for all of them.
pub const CODE_LINES: usize = 40;

/// How much decoded image data a transcript keeps (thumbnails, Markdown's
/// embedded images).
const IMAGE_BUDGET: usize = 64 * 1024 * 1024;

/// How many bytes of encoded images, read from the session's store, a
/// transcript keeps ready for the image cache: those drawn last.
const IMAGE_COPIES: usize = 32 * 1024 * 1024;

/// How many block rows keep their views, those drawn last: some screens'
/// worth, so that scrolling back a little finds them ready.
const BLOCK_VIEWS: usize = 200;

/// How many views are made between two looks for views to let go.
const RELEASE_EVERY: usize = 50;

/// The latest entries made ready to show when a transcript starts over (a
/// session opens, another conversation): the reader sees the end of it at
/// once, and the older entries are made ready a few at a time.
const FIRST_ENTRIES: usize = 200;
/// Older entries made ready per frame after the first: each is a Markdown
/// parse at most, so a frame stays well under 16 ms.
const ENTRIES_PER_FRAME: usize = 100;

/// How long, in milliseconds, the agent's text stops coming before
/// "Working…" shows under it: the model may be writing a call, which shows
/// only once it's whole.
const STALL_MS: i64 = 2000;

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
    /// Esc on the rows: the keyboard goes back to the composer.
    Leave,
}

/// The transcript's copy of the preferences it follows.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Settings {
    view: prefs::TranscriptView,
    render_markdown: bool,
    stream_fade: bool,
    expand_thinking: bool,
    /// The fonts, their sizes and the zoom, which every row's height
    /// depends on.
    fonts: u64,
}

impl Settings {
    fn of(prefs: &prefs::Prefs) -> Self {
        Self {
            view: prefs.view,
            render_markdown: prefs.appearance.render_markdown,
            stream_fade: prefs.appearance.stream_fade,
            expand_thinking: prefs.appearance.expand_thinking,
            fonts: {
                use std::hash::{Hash as _, Hasher as _};
                let appearance = &prefs.appearance;
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                appearance.ui_font.family.hash(&mut hasher);
                appearance.ui_font.size().to_bits().hash(&mut hasher);
                appearance.code_font.family.hash(&mut hasher);
                appearance.code_font.size().to_bits().hash(&mut hasher);
                appearance.zoom().to_bits().hash(&mut hasher);
                hasher.finish()
            },
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
    /// The view of the block, made when its row is drawn: a long session
    /// opens without a view per block of its history, and the views of
    /// rows not drawn for long go ([`BLOCK_VIEWS`]).
    view: Option<Entity<TextViewState>>,
    /// The frame its row was last drawn in.
    drawn: usize,
    content: TextBlockContent,
    range: Option<Range<usize>>,
    /// The lines of its longest code block (one in a list item or a quote
    /// counts), for "Show all N lines".
    code_lines: usize,
    /// What its row's height is estimated from, its characters or lines
    /// shown rendered and the characters of each line of its source shown
    /// as source.
    facts: height::BlockFacts,
    lines: Vec<u32>,
}

/// What estimating the height of the block of `text` at `range` takes.
fn block_facts(
    text: &str,
    range: &Option<Range<usize>>,
    content: &TextBlockContent,
) -> (height::BlockFacts, Vec<u32>) {
    let source = range
        .clone()
        .and_then(|range| text.get(range))
        .unwrap_or_default()
        .trim_end();
    (
        height::BlockFacts::of(content, source),
        height::hard_lines(source),
    )
}

/// The lines of the longest code block of the block of `text` at `range`.
fn code_lines(text: &str, range: &Option<Range<usize>>, content: &TextBlockContent) -> usize {
    let Some(source) = range.clone().and_then(|range| text.get(range)) else {
        return 0;
    };
    match content {
        TextBlockContent::CodeBlock(_) => row::code_lines(source),
        _ => row::longest_code(source),
    }
}

/// What the reader chose for an entry, and what its rows need.
#[derive(Default)]
struct EntryUi {
    doc: Option<Doc>,
    /// A call's details or a thinking's text, opened or closed by the
    /// reader. `None` follows the defaults.
    expanded: Option<bool>,
    /// The group of calls this entry starts (Quiet), opened or closed by the
    /// reader, apart from the entry's own details.
    group_open: Option<bool>,
    /// The text shows as its Markdown source ("View Source").
    source: bool,
    /// Code blocks shown whole, by block index.
    uncapped: HashSet<usize>,
    /// An edit's diff, made when its details first open.
    diff: Option<Entity<DiffViewState>>,
    /// A user's message, as its row draws it.
    text: Option<SharedString>,
    /// The characters of each line of a message's, a notice's or an
    /// output's text, which its row's height is estimated from.
    lines: Vec<u32>,
    /// Whether its text's rows are a thinking's, in small text, and whether
    /// a line ("Question", "From …") goes over its first block, for their
    /// heights' estimates.
    small: bool,
    labeled: bool,
    /// How many images its item shows.
    images: usize,
}

/// Whether the rows of `item`'s text are small, and whether its first
/// block has a line over it ([`EntryUi::small`], [`EntryUi::labeled`]).
fn text_look(item: &arko::remote::Item) -> (bool, bool) {
    let labeled =
        item.kind == Kind::Question || (item.kind == Kind::Message && item.from.is_some());
    (item.kind == Kind::Thinking, labeled)
}

/// Whether the rows of an entry of `kind` show its text as plain text,
/// whose lines its height is estimated from ([`EntryUi::lines`]).
fn plain_text(kind: Kind) -> bool {
    matches!(kind, Kind::User | Kind::Notice | Kind::Output)
}

/// Returns the current time in Unix milliseconds.
pub type Clock = Rc<dyn Fn() -> i64>;

/// Reads an image's bytes and format by its ID in the session's store.
type ImageLookup = Rc<dyn Fn(u64) -> Option<(Arc<[u8]>, ImageFormat)>>;

/// Images read from the session's store, ready for the image cache: those
/// drawn last, up to [`IMAGE_COPIES`] bytes. The image cache takes a GPUI
/// `Image`, which owns its bytes, so an image is copied out of the store
/// as it is drawn, and the copy goes when other images take its place or
/// when the store lets the image go.
#[derive(Default)]
struct ImageCopies {
    /// The least recently drawn first.
    images: Vec<(u64, Arc<Image>)>,
    bytes: usize,
}

impl ImageCopies {
    /// The image `id`, while `lookup` (the store) keeps it.
    fn get(&mut self, id: u64, lookup: &ImageLookup) -> Option<Arc<Image>> {
        let kept = self.images.iter().position(|(of, _)| *of == id);
        let Some((bytes, format)) = lookup(id) else {
            if let Some(ix) = kept {
                self.remove(ix);
            }
            return None;
        };
        if let Some(ix) = kept {
            let entry = self.images.remove(ix);
            let image = entry.1.clone();
            self.images.push(entry);
            return Some(image);
        }
        let image = Arc::new(Image::from_bytes(format, bytes.to_vec()));
        self.bytes += image.bytes.len();
        self.images.push((id, image.clone()));
        while self.bytes > IMAGE_COPIES && self.images.len() > 1 {
            self.remove(0);
        }
        Some(image)
    }

    fn remove(&mut self, ix: usize) {
        let (_, image) = self.images.remove(ix);
        self.bytes -= image.bytes.len();
    }

    fn clear(&mut self) {
        self.images.clear();
        self.bytes = 0;
    }
}

/// A session's transcript (the main agent's, or a worker's).
pub struct TranscriptView {
    model: Model,
    inbox: inbox::Inbox,
    ui: HashMap<u64, EntryUi>,
    /// Whether the reader opened or closed an entry that isn't made ready
    /// yet, by its item.
    chosen: HashMap<u64, bool>,
    rows: Vec<Row>,
    list: list::RowList,
    focus_handle: FocusHandle,
    settings: Settings,
    /// The entry whose hover actions show.
    hovered: Option<u64>,
    /// The rows under the pointer, of entries with hover actions.
    hovered_rows: HashSet<RowKey>,
    /// Hides the hover actions a moment after the pointer left their entry.
    hover_grace: Option<Task<()>>,
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
    /// The entry whose running indicator turns this frame: the latest
    /// thing that runs, unless "Working…" shows (it turns then).
    animated: Option<u64>,
    /// The agent's entry "Working…" follows, while the agent waits on the
    /// model ([`Self::working_after`]).
    working: Option<u64>,
    /// Whether the rows show "Working…" (not where a thinking's row says
    /// it's busy).
    shows_working: bool,
    /// When the agent's text last came or grew, by [`Self::clock`].
    text_at: i64,
    /// Looks again whether the agent waits once its text stops coming.
    stall: Option<Task<()>>,
    image_cache: Entity<LruImageCache>,
    /// Where the images the items show are read: the session's store.
    images: Option<ImageLookup>,
    /// The images read from the store last, ready for the image cache.
    image_copies: Rc<RefCell<ImageCopies>>,
    /// The find going on (Mod+F).
    find: Option<find::Find>,
    /// The row the keyboard is on.
    cursor: Option<RowKey>,
    renders: usize,
    /// Block views made since the last look for views to let go.
    views_made: usize,
    /// The sizes the rows' heights are estimated with, for the theme and
    /// fonts of the last frame.
    metrics: Option<height::Metrics>,
    /// Blocks whose text grew since the last frame, by entry and index. The
    /// list counts their new height before it draws them again.
    grown: Vec<(u64, usize)>,
    /// Whether the list's scrollbar counts estimated heights for the rows
    /// it hasn't drawn (always, but for measurements).
    estimates: bool,
    /// The rows of each frame drawn, in order.
    #[cfg(test)]
    frames: Vec<usize>,
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
        let inbox = inbox::Inbox::subscribe(feed, cx);
        let settings = Settings::of(prefs::get(cx));
        let observe_prefs = cx.observe_global::<prefs::PrefsStore>(|this, cx| {
            let settings = Settings::of(prefs::get(cx));
            if settings != this.settings {
                let rows_change = settings.view != this.settings.view
                    || settings.render_markdown != this.settings.render_markdown
                    || settings.expand_thinking != this.settings.expand_thinking;
                // Rows off screen keep their heights until they're drawn,
                // which a font's change makes wrong.
                if settings.fonts != this.settings.fonts {
                    this.list.remeasure();
                }
                this.settings = settings;
                if rows_change {
                    this.refresh_rows(cx);
                    // A find covers what the rows show now.
                    this.find_changed_all(cx);
                }
                cx.notify();
            }
        });
        let this = Self {
            model: Model::new(filter),
            inbox,
            ui: HashMap::new(),
            chosen: HashMap::new(),
            rows: Vec::new(),
            list: list::RowList::new(),
            // A Tab stop, so the keyboard reaches the transcript to scroll it.
            focus_handle: cx.focus_handle().tab_stop(true),
            settings,
            hovered: None,
            hovered_rows: HashSet::new(),
            hover_grace: None,
            new_while_away: 0,
            loaded_from: None,
            scroll_to: None,
            clock: Rc::new(|| chrono::Utc::now().timestamp_millis()),
            ticker: None,
            animated: None,
            working: None,
            shows_working: false,
            text_at: 0,
            stall: None,
            image_cache: LruImageCache::new(IMAGE_BUDGET, cx),
            images: None,
            image_copies: Rc::default(),
            find: None,
            cursor: None,
            renders: 0,
            views_made: 0,
            metrics: None,
            grown: Vec::new(),
            estimates: true,
            #[cfg(test)]
            frames: Vec::new(),
            _prefs: observe_prefs,
        };
        // The history so far comes with the next frame.
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
        self.list.scroll_to_end();
        cx.notify();
    }

    /// Whether the transcript follows the latest row.
    pub fn is_following(&self, _: &App) -> bool {
        !self.list.is_scrolled_up()
    }

    /// The number of rows (for tests and diagnostics).
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// How many times the view has drawn (for tests and measurements).
    pub fn render_count(&self) -> usize {
        self.renders
    }

    /// The user's latest message that can be edited (Alt+↑ in the empty
    /// composer edits it).
    pub fn last_message(&self) -> Option<u64> {
        self.model.last_message()
    }

    /// The conversation as an export takes it: the main agent's items in
    /// order and what the user wrote to its workers, without the messages
    /// that only kept the prompt cache warm and their answers.
    pub fn export_items(&self) -> Vec<arko::remote::Item> {
        let entries = self.model.entries();
        let refreshed: Vec<Option<u64>> = entries
            .iter()
            .filter(|entry| model::is_refresh(&entry.item))
            .map(|entry| entry.item.turn)
            .collect();
        entries
            .iter()
            .map(|entry| &entry.item)
            .filter(|item| item.is_main() || item.kind == Kind::User)
            .filter(|item| {
                !(item.is_main() && item.turn.is_some() && refreshed.contains(&item.turn))
            })
            .cloned()
            .collect()
    }

    /// Whether the conversation's first messages are missing: the
    /// transcript keeps the latest part of a long conversation, and a
    /// session opened again reads back its latest messages.
    pub fn starts_partway(&self) -> bool {
        self.model.starts_partway()
    }

    /// The reply Regenerate replaces: the main agent's latest, after the
    /// user's last message.
    pub fn last_reply(&self) -> Option<u64> {
        self.model.last_reply()
    }

    /// The entry Regenerate shows on: the latest reply, or the last line
    /// of a turn that ended without one.
    pub fn regenerate_entry(&self) -> Option<u64> {
        self.last_reply()
            .filter(|reply| self.offers_regenerate(RowKey::Actions(*reply)))
            .or_else(|| self.regenerate_end().map(RowKey::entry))
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
        // An entry not made ready yet opens or closes as it is.
        if !self.ui.contains_key(&item) {
            self.chosen.insert(item, open);
            return;
        }
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
        self.images = Some(Rc::new(move |id| {
            let image = images.get(id)?;
            Some((
                image.bytes.clone(),
                ImageFormat::from_mime_type(&image.mime)?,
            ))
        }));
        self.image_copies.borrow_mut().clear();
        cx.notify();
    }

    /// Reads images by ID from `lookup` instead of a session's store: for
    /// tests and screenshots.
    #[doc(hidden)]
    pub fn set_image_lookup(
        &mut self,
        lookup: impl Fn(u64) -> Option<(Arc<[u8]>, String)> + 'static,
        cx: &mut Context<Self>,
    ) {
        self.images = Some(Rc::new(move |id| {
            let (bytes, mime) = lookup(id)?;
            Some((bytes, ImageFormat::from_mime_type(&mime)?))
        }));
        self.image_copies.borrow_mut().clear();
        cx.notify();
    }

    /// Counts rows the list hasn't drawn at the heights it last measured or
    /// at none (`false`) rather than at estimated heights, for measurements
    /// of what the estimates bring.
    #[doc(hidden)]
    pub fn set_height_estimates(&mut self, on: bool, cx: &mut Context<Self>) {
        self.estimates = on;
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
        if pending.snapshot.is_none() && pending.updates.is_empty() && pending.activity.is_none() {
            return;
        }
        let mut changes = Changes::default();
        if let Some(snapshot) = pending.snapshot {
            self.model.load(snapshot, &mut changes);
        }
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
        // (Only the agent's text: a note or a worker's news isn't output
        // that shows it's busy.)
        let text = changes.added.iter().chain(&changes.text).any(|id| {
            self.model
                .entry(*id)
                .is_some_and(|entry| matches!(entry.item.kind, Kind::Text | Kind::Message))
        });
        if changes.reset || text {
            self.text_at = (self.clock)();
        }
        if changes.reset {
            // What the reader chose to open stays open (also before the
            // history came: a row opened from elsewhere as the view opens).
            for (id, ui) in self.ui.drain() {
                if let Some(expanded) = ui.expanded {
                    self.chosen.insert(id, expanded);
                }
            }
            let model = &self.model;
            self.chosen.retain(|id, _| model.entry(*id).is_some());
            self.hovered = None;
            self.hovered_rows.clear();
            self.cursor = None;
            let ids: Vec<u64> = self.model.entries().iter().map(|e| e.item.id).collect();
            let start = ids.len().saturating_sub(FIRST_ENTRIES);
            for id in &ids[start..] {
                self.add_ui(*id, cx);
            }
            self.loaded_from = ids.get(start).copied().filter(|_| start > 0);
            self.rows.clear();
            self.list.reset(0);
            self.new_while_away = 0;
        } else {
            for id in &changes.removed {
                self.ui.remove(id);
                // An entry that went (a note the agent read comes again
                // with a new id) takes the pointer's and the keyboard's
                // place on it along.
                if self.hovered == Some(*id) {
                    self.hovered = None;
                }
                if self.cursor.is_some_and(|key| key.entry() == *id) {
                    self.cursor = None;
                }
            }
            for id in &changes.added {
                self.add_ui(*id, cx);
            }
            for id in changes.text.iter().chain(&changes.changed) {
                self.sync_text(*id, cx);
            }
        }
        self.refresh_rows(cx);
        if !changes.reset {
            for id in changes.text.iter().chain(&changes.changed) {
                self.remeasure_entry(*id);
            }
        }
        self.tick_while_running(cx);
        if changes.reset {
            self.find_changed_all(cx);
        } else {
            let changed = changes
                .added
                .iter()
                .chain(&changes.text)
                .chain(&changes.changed)
                .copied();
            self.find_changed(&changes.removed, changed, cx);
        }
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
        let mut ui = EntryUi {
            expanded: self.chosen.remove(&id),
            text: (entry.item.kind == Kind::User).then(|| entry.item.text.clone().into()),
            lines: if plain_text(entry.item.kind) {
                height::hard_lines(&entry.item.text)
            } else {
                Vec::new()
            },
            ..EntryUi::default()
        };
        (ui.small, ui.labeled) = text_look(&entry.item);
        ui.images = entry.item.images.len();
        if ui.expanded == Some(true) {
            ui.diff =
                row::edit_diff(&entry.item).map(|diff| cx.new(|cx| DiffViewState::new(diff, cx)));
        }
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
            } else {
                this.remeasure_grown();
            }
            // A parse brings the text a find searches as it renders.
            this.find_changed(&[], [id], cx);
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
        if entry.item.kind == Kind::User && ui.text.as_deref() != Some(text.as_str()) {
            ui.text = Some(text.clone().into());
        }
        if plain_text(entry.item.kind) {
            ui.lines = height::hard_lines(text);
        }
        (ui.small, ui.labeled) = text_look(&entry.item);
        ui.images = entry.item.images.len();
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
        let mut grown = Vec::new();
        for (ix, (row, block)) in doc
            .blocks
            .iter_mut()
            .zip(&blocks)
            .enumerate()
            .skip(kept.saturating_sub(1))
        {
            let range = block.source_range();
            if row.range == range && row.content == *block.content() {
                continue;
            }
            row.range = range;
            row.content = block.content().clone();
            row.code_lines = code_lines(&doc.text, &row.range, &row.content);
            (row.facts, row.lines) = block_facts(&doc.text, &row.range, &row.content);
            grown.push(ix);
        }
        let added = blocks.len() > kept;
        let text = &doc.text;
        doc.blocks.extend(blocks[kept..].iter().map(|block| {
            let range = block.source_range();
            let content = block.content().clone();
            let (facts, lines) = block_facts(text, &range, &content);
            Block {
                id: block.id(),
                view: None,
                drawn: 0,
                code_lines: code_lines(text, &range, &content),
                facts,
                lines,
                content,
                range,
            }
        }));
        self.grown
            .extend(grown.into_iter().map(|block| (id, block)));
        dropped || added
    }

    /// Has the list count the new height of the blocks whose text grew,
    /// off screen too, rather than the height it measured before. (On
    /// screen, it measures them again as it draws them.)
    fn remeasure_grown(&mut self) {
        for (id, block) in std::mem::take(&mut self.grown) {
            let row = self.rows.iter().rposition(|row| {
                matches!(row.key, RowKey::Block { id: row_id, block: row_block, .. }
                    if row_id == id && row_block == block)
            });
            if let Some(ix) = row {
                self.list.remeasure_row(ix);
            }
        }
    }

    /// Has the list count the new height of the rows of the entry `id`, a
    /// call whose output came or a message whose text changed. (A text's
    /// blocks are counted as they're parsed, by [`Self::remeasure_grown`].)
    fn remeasure_entry(&mut self, id: u64) {
        let has_doc = self.ui.get(&id).is_none_or(|ui| ui.doc.is_some());
        if has_doc {
            return;
        }
        for ix in 0..self.rows.len() {
            let key = self.rows[ix].key;
            if key.entry() == id && !matches!(key, RowKey::Block { .. }) {
                self.list.remeasure_row(ix);
            }
        }
    }

    /// The view of block `block` of the entry `id`'s text, made now if its
    /// row is drawn for the first time.
    fn block_view(
        &mut self,
        id: u64,
        block: usize,
        cx: &mut Context<Self>,
    ) -> Option<Entity<TextViewState>> {
        let frame = self.renders;
        let doc = self.ui.get_mut(&id)?.doc.as_mut()?;
        let state = doc.state.clone();
        let row = doc.blocks.get_mut(block)?;
        row.drawn = frame;
        if let Some(view) = &row.view {
            return Some(view.clone());
        }
        self.views_made += 1;
        let view = cx.new(|cx| TextViewState::block_range(&state, block..block + 1, cx));
        row.view = Some(view.clone());
        // Drawn for the first time while a find goes on: its matches show.
        self.highlight_block(id, block, &view, cx);
        Some(view)
    }

    /// Lets go of the views of the blocks drawn longest ago, past
    /// [`BLOCK_VIEWS`], and of the layouts they hold. A row drawn again
    /// gets a new view, made from its document's parse.
    fn release_block_views(&mut self) {
        self.views_made = 0;
        let blocks = || {
            self.ui
                .values()
                .filter_map(|ui| ui.doc.as_ref())
                .flat_map(|doc| &doc.blocks)
                .filter(|block| block.view.is_some())
        };
        let mut frames: Vec<usize> = blocks().map(|block| block.drawn).collect();
        if frames.len() <= BLOCK_VIEWS {
            return;
        }
        frames.sort_unstable();
        // Those of the last frames stay, whatever their number.
        let kept_from = frames[frames.len() - BLOCK_VIEWS].min(self.renders.saturating_sub(1));
        for block in self
            .ui
            .values_mut()
            .filter_map(|ui| ui.doc.as_mut())
            .flat_map(|doc| &mut doc.blocks)
        {
            if block.drawn < kept_from {
                block.view = None;
            }
        }
    }

    /// Rebuilds the rows from the model, and tells the list what changed.
    /// Rows in both keep their place and measured height, so the reader's
    /// row stays where it is. Where it went (folded into a group, shown as
    /// source), the row that took its entry's place does.
    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        self.working = self.working_after(cx);
        let render_markdown = self.settings.render_markdown;
        let ui = &self.ui;
        let blocks = |id: u64| {
            ui.get(&id)
                .and_then(|ui| ui.doc.as_ref())
                .map_or((0, 0), |doc| (doc.blocks.len(), doc.generation))
        };
        let expanded = |id: u64| ui.get(&id).and_then(|ui| ui.expanded);
        let group_open = |id: u64| ui.get(&id).and_then(|ui| ui.group_open);
        let source = |id: u64| !render_markdown || ui.get(&id).is_some_and(|ui| ui.source);
        let rows = model::rows(
            &self.model,
            &Layout {
                view: self.settings.view,
                expand_thinking: self.settings.expand_thinking,
                blocks: &blocks,
                expanded: &expanded,
                group_open: &group_open,
                source: &source,
                first: self.loaded_from,
                working: self.working,
            },
        );
        // The reader's place, unless they follow the latest row.
        let scrolled_up = self.list.is_scrolled_up();
        let anchor = (!self.list.is_following())
            .then(|| {
                let top = self.list.top();
                self.rows
                    .get(top.item_ix)
                    .map(|row| (row.key, top.offset_in_item))
            })
            .flatten();
        self.shows_working = model::shows_working(&rows);
        let old = std::mem::take(&mut self.rows);
        let (changes, kept) = model::row_changes(&old, &rows);
        for (range, count) in changes.iter().rev() {
            self.list.splice(range.clone(), *count);
        }
        // Rows kept whose space above changed.
        for &(old_ix, new_ix) in &kept {
            if old[old_ix].gap != rows[new_ix].gap {
                self.list.remeasure_row(new_ix);
            }
        }
        // What came after the last row while the reader was away ("Working…"
        // gives way to it).
        let working = |row: &Row| matches!(row.key, RowKey::Working(_));
        if scrolled_up
            && let Some((range, count)) = changes.last()
            && range.end == old.len()
            && old[range.clone()].iter().all(working)
            && old.len() > range.len()
        {
            let mut entries: Vec<u64> = rows[rows.len() - count..]
                .iter()
                .filter(|row| !working(row))
                .map(|row| row.key.entry())
                .collect();
            entries.dedup();
            self.new_while_away += entries.len();
        }
        self.rows = rows;
        // A row gone from under the pointer never says that the pointer
        // left it.
        if !self.hovered_rows.is_empty() {
            let rows = &self.rows;
            self.hovered_rows
                .retain(|key| rows.iter().any(|row| row.key == *key));
        }
        if let Some((key, offset)) = anchor
            && let Some((ix, offset)) = self.place_of(key, offset)
        {
            self.list.scroll_to(ix, offset);
        }
        if let Some(key) = self.scroll_to
            && let Some(ix) = self.rows.iter().position(|row| row.key == key)
        {
            self.scroll_to = None;
            self.list.scroll_to(ix, px(0.));
        }
        self.remeasure_grown();
    }

    /// Where the row `key` is now, `offset` into it: the row itself, the
    /// same block of its text shown another way, or else the row that
    /// stands for its entry now (a group of calls), from its top.
    fn place_of(&self, key: RowKey, offset: Pixels) -> Option<(usize, Pixels)> {
        if let Some(ix) = self.rows.iter().position(|row| row.key == key) {
            return Some((ix, offset));
        }
        if let RowKey::Block { id, block, .. } = key
            && let Some(ix) = self.rows.iter().position(|row| {
                matches!(row.key, RowKey::Block { id: row_id, block: row_block, .. }
                    if row_id == id && row_block == block)
            })
        {
            return Some((ix, offset));
        }
        let entry = key.entry();
        let ix = self
            .rows
            .iter()
            .rposition(|row| row.key.entry() <= entry)
            .unwrap_or(0);
        (!self.rows.is_empty()).then_some((ix, px(0.)))
    }

    /// The agent's entry "Working…" follows, while it shows: the agent shown
    /// waits on the model, and no other row says it's busy. Text that
    /// stopped coming [`STALL_MS`] ago counts as waiting, and while text
    /// comes, a timer looks again when it would have stopped that long.
    fn working_after(&mut self, cx: &mut Context<Self>) -> Option<u64> {
        let after = match self.model.waiting() {
            Waiting::No => return None,
            Waiting::Yes(after) => return Some(after),
            Waiting::AfterText(after) => after,
        };
        let quiet = (self.clock)() - self.text_at;
        if quiet >= STALL_MS {
            return Some(after);
        }
        if self.stall.is_none() {
            let wait = Duration::from_millis((STALL_MS - quiet).max(1) as u64);
            self.stall = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(wait).await;
                this.update(cx, |this, cx| {
                    this.stall = None;
                    if this.working_after(cx) != this.working {
                        this.refresh_rows(cx);
                        cx.notify();
                    }
                })
                .ok();
            }));
        }
        None
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

    /// The latest entry that runs: a call, a thinking that streams, a
    /// worker's card.
    fn latest_running(&self) -> Option<u64> {
        let entries = self.model.entries();
        entries.iter().enumerate().rev().find_map(|(ix, entry)| {
            let item = &entry.item;
            let running = match item.kind {
                Kind::Call => item.status == Some(CallStatus::Running),
                Kind::Thinking => self.model.is_streaming(ix),
                Kind::WorkerLine => item
                    .worker
                    .as_deref()
                    .and_then(|worker| self.model.worker(worker))
                    .is_some_and(|worker| worker.state == arko::remote::WorkerState::Running),
                _ => false,
            };
            running.then_some(item.id)
        })
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
        let row = self.rows.iter().position(
            |row| matches!(row.key, RowKey::Call(entry) | RowKey::Thinking(entry) if entry == id),
        );
        if let Some(ix) = row {
            self.list.hold_place(ix);
        }
        let ui = self.ui.entry(id).or_default();
        ui.expanded = Some(!ui.expanded.unwrap_or(open_by_default));
        if diff.is_some() {
            ui.diff = diff;
        }
        if let Some(ix) = row {
            self.list.remeasure_row(ix);
        }
        self.refresh_rows(cx);
        cx.notify();
    }

    /// Whether the Quiet group of calls that starts with the entry `first`
    /// shows its calls: as the reader chose, else when the reader opened one
    /// of them (a run that folds as it grows keeps that call in sight).
    fn group_open(&self, first: u64) -> bool {
        if let Some(open) = self.ui.get(&first).and_then(|ui| ui.group_open) {
            return open;
        }
        let entries = self.model.entries();
        model::run_of(&self.model, first, self.settings.view)
            .iter()
            .any(|ix| {
                self.ui
                    .get(&entries[*ix].item.id)
                    .and_then(|ui| ui.expanded)
                    == Some(true)
            })
    }

    /// Opens or closes a Quiet group of calls.
    fn toggle_group(&mut self, first: u64, cx: &mut Context<Self>) {
        let open = self.group_open(first);
        if let Some(ix) = self
            .rows
            .iter()
            .position(|row| row.key == RowKey::Group(first))
        {
            self.list.hold_place(ix);
        }
        self.ui.entry(first).or_default().group_open = Some(!open);
        self.refresh_rows(cx);
        cx.notify();
    }

    fn toggle_source(&mut self, id: u64, cx: &mut Context<Self>) {
        if let Some(ix) = self.row_of(id) {
            self.list.hold_place(ix);
        }
        let ui = self.ui.entry(id).or_default();
        ui.source = !ui.source;
        self.refresh_rows(cx);
        self.find_changed(&[], [id], cx);
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
            self.list.hold_place(ix);
            self.list.remeasure_row(ix);
        }
        cx.notify();
    }

    /// The pointer came over the row `key` (of an entry with hover
    /// actions), or left it. The actions show for the entry under the
    /// pointer, and stay a moment after it leaves, so that crossing to
    /// them never hides them.
    fn row_hovered(&mut self, key: RowKey, hovered: bool, cx: &mut Context<Self>) {
        if hovered {
            self.hovered_rows.insert(key);
        } else {
            self.hovered_rows.remove(&key);
        }
        let entry = if hovered {
            Some(key.entry())
        } else {
            self.hovered_rows.iter().next().map(|row| row.entry())
        };
        match entry {
            Some(entry) => {
                self.hover_grace = None;
                if self.hovered != Some(entry) {
                    self.hovered = Some(entry);
                    cx.notify();
                }
            }
            None => {
                self.hover_grace = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(HOVER_GRACE).await;
                    this.update(cx, |this, cx| {
                        if this.hovered_rows.is_empty() && this.hovered.take().is_some() {
                            cx.notify();
                        }
                    })
                    .ok();
                }));
            }
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
        self.rows.iter().position(|row| row.key.entry() == id)
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
        let lookup = self.images.as_ref()?;
        self.image_copies.borrow_mut().get(id, lookup)
    }

    /// Opens the image viewer on the image `image` of the entry `entry`,
    /// with every image of the conversation to browse, in order. Each is
    /// read from the store as the viewer shows it.
    fn open_image(&mut self, entry: u64, image: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(lookup) = self.images.clone() else {
            return;
        };
        let mut items = Vec::new();
        let mut selected = 0;
        for shown in self.model.entries() {
            for image_ref in &shown.item.images {
                if lookup(image_ref.id).is_none() {
                    continue;
                }
                if shown.item.id == entry && image_ref.id == image {
                    selected = items.len();
                }
                let caption = image_caption(&image_ref.label, image_ref.bytes);
                let (id, copies, lookup, cache) = (
                    image_ref.id,
                    self.image_copies.clone(),
                    lookup.clone(),
                    self.image_cache.clone(),
                );
                #[expect(
                    clippy::arc_with_non_send_sync,
                    reason = "GPUI's image sources are Arcs, and GPUI calls them on the UI thread only"
                )]
                let source = ImageSource::Custom(Arc::new(move |window, cx| {
                    let Some(image) = copies.borrow_mut().get(id, &lookup) else {
                        // The store let it go since the viewer opened.
                        return Some(Err(ImageCacheError::Asset("Image not kept".into())));
                    };
                    match LruImageCache::source(&cache, image, None) {
                        ImageSource::Custom(load) => load(window, cx),
                        _ => None,
                    }
                }));
                items.push(ImageViewerItem::new(source).with_caption(caption));
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
        #[cfg(test)]
        self.frames.push(self.rows.len());
        if self.views_made >= RELEASE_EVERY {
            self.release_block_views();
        }
        if !self.list.is_scrolled_up() {
            self.new_while_away = 0;
        }
        // "Working…" is the latest thing that runs, when it shows.
        self.animated = if self.shows_working {
            None
        } else {
            self.latest_running()
        };
        let new = self.new_while_away;
        // Reached by Tab, the transcript shows that it has the focus, with a
        // ring laid over its edge (a border would move the rows). A row the
        // keyboard is on shows its own ring instead.
        let ring = (self.focus_handle.is_focused(window)
            && window.last_input_was_keyboard()
            && self.cursor_index().is_none())
        .then(|| cx.theme().ring);
        // The sizes the heights of rows not drawn yet are estimated with.
        self.metrics = Some(height::Metrics::current(self.metrics.take(), window, cx));
        // (The list holds the estimates for this frame only.)
        let estimate_view = cx.entity();
        let view = estimate_view.downgrade();
        let rows = list::render(
            "transcript-rows",
            &self.list,
            new,
            move |ix, window, cx| row::render(&view, ix, window, cx),
            self.estimates
                .then_some(move |ix, width, window: &mut Window, cx: &mut App| {
                    row::estimate(&estimate_view, ix, width, window, cx)
                }),
            window,
            cx,
        );
        let root = div()
            .id("transcript")
            .test_support()
            .role(Role::List)
            .aria_label("Conversation")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle);
        let root = Self::on_row_keys(root, cx);
        image_cache(self.image_cache.clone()).size_full().child(
            root.relative()
                .size_full()
                .on_action(cx.listener(Self::copy))
                .on_action(cx.listener(|this, _: &ScrollPageUp, _, cx| {
                    this.list.scroll_by(-this.list.viewport_height() * PAGE);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &ScrollPageDown, _, cx| {
                    this.list.scroll_by(this.list.viewport_height() * PAGE);
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &ScrollToStart, _, cx| {
                    this.list.scroll_to(0, px(0.));
                    cx.notify();
                }))
                .on_action(cx.listener(|this, _: &ScrollToLatest, _, cx| {
                    this.scroll_to_end(cx);
                }))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, window, cx| {
                        if !this.focus_handle.contains_focused(window, cx) {
                            this.focus_handle.focus(window, cx);
                        }
                    }),
                )
                .child(rows)
                .children(
                    ring.map(|ring| div().absolute().inset_0().border_1().border_color(ring)),
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
