//! What a transcript shows of a session's feed, apart from how it's drawn:
//! one agent's items (the main agent's, or a worker's), kept from the feed's
//! snapshot and updates, and the rows a view makes of them.
//!
//! The app shows one conversation at a time: a conversation marker (`/new`,
//! `/resume`) starts the transcript over. A call's result block goes under
//! its call rather than in a row of its own.

use arko::remote::{Activity, Delivery, Item, Kind, Level, Snapshot, Update, WorkerSummary};
use gpui_kit::SharedString;

use crate::prefs::TranscriptView as View;

/// Whose transcript a view shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentFilter {
    /// The main agent's, with each worker as a card where it started.
    Main,
    /// One worker's own transcript, by its id (`r1`).
    Worker(SharedString),
}

impl AgentFilter {
    /// Whether the transcript keeps `item` at all (a view may still hide
    /// it). The main transcript keeps what the user wrote to a worker, the
    /// workers' calls and what went wrong for them, as the terminal does.
    fn keeps(&self, item: &Item) -> bool {
        match self {
            AgentFilter::Main => {
                item.is_main()
                    || matches!(
                        item.kind,
                        Kind::User | Kind::Call | Kind::Result | Kind::Notice
                    )
            }
            AgentFilter::Worker(id) => item.agent == id.as_ref() && item.kind != Kind::WorkerLine,
        }
    }
}

/// The bytes of whole call outputs a transcript keeps, the latest first: as
/// much again as the feed keeps per lane (it keeps them for the latest calls
/// only, and the transcript keeps what came in its updates).
pub(crate) const KEPT_OUTPUTS: usize = 8 * 1024 * 1024;

/// An item of the transcript, and the result block under it for a call.
#[derive(Debug, Clone)]
pub(crate) struct Entry {
    pub item: Item,
    pub result: Option<Item>,
}

/// What updates changed, for the drawn side to follow.
#[derive(Debug, Default)]
pub(crate) struct Changes {
    /// Everything was replaced: a snapshot, or another conversation.
    pub reset: bool,
    pub added: Vec<u64>,
    /// Entries whose text changed: appended to, or replaced.
    pub text: Vec<u64>,
    /// Entries whose other fields changed: a call's status or result, a text
    /// that became the reply.
    pub changed: Vec<u64>,
    pub removed: Vec<u64>,
    pub workers: bool,
    pub activity: bool,
}

impl Changes {
    /// Whether the rows may have to change.
    pub fn any(&self) -> bool {
        self.reset
            || !self.added.is_empty()
            || !self.text.is_empty()
            || !self.changed.is_empty()
            || !self.removed.is_empty()
            || self.workers
            || self.activity
    }
}

/// The items of one transcript, in the feed's order (by id).
pub(crate) struct Model {
    filter: AgentFilter,
    entries: Vec<Entry>,
    workers: Vec<WorkerSummary>,
    activity: Activity,
    /// The latest conversation marker: items before it are another
    /// conversation's.
    cut: Option<u64>,
}

impl Model {
    pub fn new(filter: AgentFilter) -> Self {
        Self {
            filter,
            entries: Vec::new(),
            workers: Vec::new(),
            activity: Activity::default(),
            cut: None,
        }
    }

    pub fn filter(&self) -> &AgentFilter {
        &self.filter
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn workers(&self) -> &[WorkerSummary] {
        &self.workers
    }

    pub fn worker(&self, id: &str) -> Option<&WorkerSummary> {
        self.workers.iter().find(|worker| worker.id == id)
    }

    /// Where the entry `id` is, or would go.
    pub fn position(&self, id: u64) -> Result<usize, usize> {
        self.entries
            .binary_search_by_key(&id, |entry| entry.item.id)
    }

    pub fn entry(&self, id: u64) -> Option<&Entry> {
        self.position(id).ok().map(|ix| &self.entries[ix])
    }

    /// Starts over from `snapshot`.
    pub fn load(&mut self, snapshot: Snapshot, changes: &mut Changes) {
        self.entries.clear();
        self.cut = None;
        for item in snapshot.items {
            self.add(item, changes);
        }
        self.workers = snapshot.workers;
        self.activity = snapshot.activity;
        *changes = Changes {
            reset: true,
            ..Changes::default()
        };
    }

    pub fn set_activity(&mut self, activity: Activity, changes: &mut Changes) {
        if self.activity != activity {
            self.activity = activity;
            changes.activity = true;
        }
    }

    pub fn apply(&mut self, update: Update, changes: &mut Changes) {
        match update {
            Update::Item(item) => self.add(item, changes),
            Update::Append { id, text } => {
                if let Ok(ix) = self.position(id) {
                    self.entries[ix].item.text.push_str(&text);
                    changes.text.push(id);
                }
            }
            Update::Change(item) => {
                let id = item.id;
                match self.position(id) {
                    Ok(ix) => {
                        let entry = &mut self.entries[ix];
                        if entry.item.text != item.text {
                            changes.text.push(id);
                        }
                        let mut item = item;
                        // The feed keeps a call's tool, parameters and output
                        // for its latest calls only: a change of an older
                        // one keeps what this transcript had.
                        let old = &mut entry.item;
                        item.tool = item.tool.or_else(|| old.tool.take());
                        item.parameters = item.parameters.or_else(|| old.parameters.take());
                        item.output = item.output.or_else(|| old.output.take());
                        item.whole = item.whole.or_else(|| old.whole.take());
                        let output = item.output.is_some();
                        entry.item = item;
                        changes.changed.push(id);
                        if output {
                            self.keep_outputs_within_budget();
                        }
                    }
                    // A change of an item never seen: one from before this
                    // client, or one the filter left out.
                    Err(_) => self.add(item, changes),
                }
            }
            Update::Worker(summary) => {
                match self.workers.iter_mut().find(|w| w.id == summary.id) {
                    Some(worker) => *worker = summary,
                    None => self.workers.push(summary),
                }
                changes.workers = true;
            }
            // The inbox reads the activity with the update (it carries none).
            Update::Activity => {}
            Update::Truncate { from } => {
                let main = matches!(self.filter, AgentFilter::Main);
                self.entries.retain(|entry| {
                    let cut = main && entry.item.is_main() && entry.item.id >= from;
                    if cut {
                        changes.removed.push(entry.item.id);
                    }
                    !cut
                });
            }
            // Text a failed response streamed, which its retry streams again.
            Update::Remove { id } => {
                if let Ok(ix) = self.position(id) {
                    self.entries.remove(ix);
                    changes.removed.push(id);
                }
            }
        }
    }

    fn add(&mut self, item: Item, changes: &mut Changes) {
        let main = matches!(self.filter, AgentFilter::Main);
        if item.kind == Kind::Conversation {
            if main && item.is_main() {
                // Another conversation: what came before was the last one's.
                self.cut = Some(item.id);
                self.entries.clear();
                self.entries.push(Entry { item, result: None });
                changes.reset = true;
            }
            return;
        }
        if self.cut.is_some_and(|cut| item.id < cut) || !self.filter.keeps(&item) {
            return;
        }
        if item.kind == Kind::Result {
            if let Some(call) = item.call
                && let Ok(ix) = self.position(call)
            {
                let whole = item.whole.is_some();
                self.entries[ix].result = Some(item);
                changes.changed.push(call);
                if whole {
                    self.keep_outputs_within_budget();
                }
            }
            return;
        }
        let id = item.id;
        match self.position(id) {
            Ok(ix) => {
                self.entries[ix].item = item;
                changes.text.push(id);
                changes.changed.push(id);
            }
            Err(ix) => {
                self.entries.insert(ix, Entry { item, result: None });
                changes.added.push(id);
            }
        }
    }

    /// Lets go of the oldest calls' whole outputs past [`KEPT_OUTPUTS`]
    /// bytes: the latest stay, and the older calls keep their lines.
    fn keep_outputs_within_budget(&mut self) {
        let size = |entry: &Entry| {
            let len = |text: &Option<std::sync::Arc<str>>| text.as_ref().map_or(0, |t| t.len());
            len(&entry.item.output)
                + len(&entry.item.whole)
                + entry.result.as_ref().map_or(0, |result| len(&result.whole))
        };
        let mut total: usize = self.entries.iter().map(size).sum();
        for entry in &mut self.entries {
            if total <= KEPT_OUTPUTS {
                break;
            }
            total -= size(entry);
            entry.item.output = None;
            entry.item.whole = None;
            if let Some(result) = &mut entry.result {
                result.whole = None;
            }
        }
    }

    /// Whether the agent of `entry` still writes it: the latest of that
    /// agent's entries while the agent works. For thinking, which shows
    /// "Thinking…" until then.
    pub fn is_streaming(&self, ix: usize) -> bool {
        let entry = &self.entries[ix];
        let agent = &entry.item.agent;
        let later = self.entries[ix + 1..]
            .iter()
            .any(|later| &later.item.agent == agent);
        if later {
            return false;
        }
        if entry.item.is_main() {
            self.activity.working
        } else {
            self.worker(agent)
                .is_some_and(|worker| worker.state.is_active())
        }
    }

    /// How long the thinking at `ix` took, in milliseconds: until its
    /// agent's next item. `None` while it goes on, under a second, or when
    /// unknown (items restored from a saved session all carry the time they
    /// were restored).
    pub fn thought_for(&self, ix: usize) -> Option<i64> {
        let entry = &self.entries[ix];
        let next = self.entries[ix + 1..]
            .iter()
            .find(|later| later.item.agent == entry.item.agent)?;
        let took = next.item.at - entry.item.at;
        (took >= 1000).then_some(took)
    }
}

/// How a view shows an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Show {
    Hidden,
    Item,
    /// Counted in its run's line of tool activity (the Messages view).
    Count,
}

fn show(filter: &AgentFilter, view: View, item: &Item) -> Show {
    if let AgentFilter::Worker(_) = filter {
        return match item.kind {
            Kind::Result | Kind::Turn => Show::Hidden,
            _ => Show::Item,
        };
    }
    if item.kind == Kind::WorkerLine {
        return Show::Item;
    }
    let warning = item.level.is_some_and(|level| level != Level::Info);
    if !item.is_main() {
        // A worker's calls and what went wrong for it show in Full, and what
        // the user wrote to it in every view. Its text and report are its
        // own transcript's.
        return match (view, item.kind) {
            (_, Kind::User) => Show::Item,
            (View::Full, Kind::Call) => Show::Item,
            (View::Full, Kind::Notice) if warning => Show::Item,
            _ => Show::Hidden,
        };
    }
    match view {
        View::Full | View::Quiet => match item.kind {
            Kind::Result => Show::Hidden,
            _ => Show::Item,
        },
        View::Messages => match item.kind {
            Kind::User
            | Kind::Text
            | Kind::Message
            | Kind::Question
            | Kind::Reply
            | Kind::Output
            | Kind::Conversation
            | Kind::WorkerLine => Show::Item,
            Kind::Notice if warning => Show::Item,
            Kind::Call => Show::Count,
            Kind::Result | Kind::Notice | Kind::Turn | Kind::Thinking => Show::Hidden,
        },
    }
}

/// A row of the transcript's list. Rows keep their key while their entry
/// changes, so the list keeps their place and measured height. A key that
/// changes replaces the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum RowKey {
    /// Where the conversation began (`/new`, `/resume`).
    Separator(u64),
    User(u64),
    /// A message that only kept the prompt cache warm, and its reply, as
    /// one quiet line.
    Refresh(u64),
    /// One top-level Markdown block of an entry's text: `generation` changes
    /// when the text is replaced rather than appended to, `source` when it
    /// shows its source.
    Block {
        id: u64,
        generation: u32,
        block: usize,
        source: bool,
    },
    /// The hover actions under a reply.
    Actions(u64),
    Thinking(u64),
    Call(u64),
    /// Consecutive calls folded into one row (Quiet), by the first entry.
    Group(u64),
    /// A run of calls counted in one line (Messages), by its first call.
    Count(u64),
    Notice(u64),
    Output(u64),
    Turn(u64),
    Worker(u64),
}

/// The space above a row, by what it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gap {
    None,
    /// Between calls of a run.
    Tight,
    /// Under a disclosure, before the turn's footer.
    Small,
    /// Between paragraphs, and between prose and activity.
    Section,
    /// Before a message of the user's: a new turn.
    Turn,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub key: RowKey,
    pub gap: Gap,
}

/// What kind of row a row is, for the spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Separator,
    User,
    Prose,
    /// The text of an expanded thinking.
    Thought,
    Activity,
    Actions,
    Footer,
}

fn gap(previous: Option<Class>, next: Class) -> Gap {
    let Some(previous) = previous else {
        return Gap::None;
    };
    match (previous, next) {
        (_, Class::Actions) => Gap::None,
        (_, Class::Footer) => Gap::Small,
        (Class::Separator, Class::User) => Gap::Section,
        (_, Class::User) => Gap::Turn,
        (Class::Activity, Class::Activity) => Gap::Tight,
        (Class::Activity, Class::Thought) | (Class::Thought, Class::Activity) => Gap::Small,
        (Class::Actions, _) => Gap::Small,
        _ => Gap::Section,
    }
}

/// What the rows depend on besides the entries.
pub(crate) struct Layout<'a> {
    pub view: View,
    /// Thinking shows expanded unless the reader folds it (Full only).
    pub expand_thinking: bool,
    /// The number of Markdown blocks of an entry's text so far, and the
    /// generation of its document.
    pub blocks: &'a dyn Fn(u64) -> (usize, u32),
    /// The reader's choice for an entry's thinking or a group, if any.
    pub expanded: &'a dyn Fn(u64) -> Option<bool>,
    /// Whether an entry's text shows as its source.
    pub source: &'a dyn Fn(u64) -> bool,
    /// The oldest entry ready to show, when older ones are still being made
    /// ready: the rows start there.
    pub first: Option<u64>,
}

struct Rows<'a> {
    out: Vec<Row>,
    previous: Option<Class>,
    layout: &'a Layout<'a>,
}

impl Rows<'_> {
    fn push(&mut self, key: RowKey, class: Class) {
        self.out.push(Row {
            key,
            gap: gap(self.previous, class),
        });
        self.previous = Some(class);
    }

    fn blocks(&mut self, id: u64, class: Class) {
        let (count, generation) = (self.layout.blocks)(id);
        let source = (self.layout.source)(id);
        for block in 0..count {
            self.push(
                RowKey::Block {
                    id,
                    generation,
                    block,
                    source,
                },
                class,
            );
        }
    }

    /// The rows of one entry.
    fn entry(&mut self, entry: &Entry, view: View, worker_view: bool) {
        let item = &entry.item;
        let id = item.id;
        match item.kind {
            Kind::Conversation => self.push(RowKey::Separator(id), Class::Separator),
            Kind::User => self.push(RowKey::User(id), Class::User),
            Kind::Text | Kind::Message | Kind::Question => self.blocks(id, Class::Prose),
            Kind::Reply => {
                self.blocks(id, Class::Prose);
                if (self.layout.blocks)(id).0 > 0 {
                    self.push(RowKey::Actions(id), Class::Actions);
                }
            }
            Kind::Thinking => {
                self.push(RowKey::Thinking(id), Class::Activity);
                let default = self.layout.expand_thinking && (view == View::Full || worker_view);
                if (self.layout.expanded)(id).unwrap_or(default) {
                    self.blocks(id, Class::Thought);
                }
            }
            Kind::Call => self.push(RowKey::Call(id), Class::Activity),
            Kind::Notice => self.push(RowKey::Notice(id), Class::Activity),
            Kind::Output => self.push(RowKey::Output(id), Class::Prose),
            Kind::Turn => self.push(RowKey::Turn(id), Class::Footer),
            Kind::WorkerLine => self.push(RowKey::Worker(id), Class::Activity),
            Kind::Result => {}
        }
    }
}

/// Calls in a row past this many fold into one row in the Quiet view.
pub(crate) const GROUP_AFTER: usize = 3;

/// Whether `item` belongs in a run of the main agent's tool activity: its
/// calls and its thinking between them.
fn in_run(item: &Item) -> bool {
    item.is_main() && matches!(item.kind, Kind::Call | Kind::Thinking)
}

/// The entries of the run of activity starting at `start`, as indexes, and
/// the index after it. Entries the view hides don't break a run.
fn run(
    entries: &[Entry],
    start: usize,
    filter: &AgentFilter,
    view: View,
    member: impl Fn(&Item) -> bool,
) -> (Vec<usize>, usize) {
    let mut members = Vec::new();
    let mut ix = start;
    while ix < entries.len() {
        let item = &entries[ix].item;
        if member(item) {
            members.push(ix);
        } else if show(filter, view, item) != Show::Hidden {
            break;
        }
        ix += 1;
    }
    (members, ix)
}

/// Whether `item` is a message that only kept the prompt cache warm.
fn is_refresh(item: &Item) -> bool {
    item.is_main() && item.kind == Kind::User && item.delivery == Some(Delivery::Refresh)
}

/// Where the transcript goes on after the cache refresh at `ix`: past the
/// agent's answer to it ("Acknowledged") and its turn's footer.
fn after_refresh(entries: &[Entry], ix: usize) -> usize {
    let mut next = ix + 1;
    while let Some(entry) = entries.get(next) {
        let item = &entry.item;
        let answer = matches!(
            item.kind,
            Kind::Text | Kind::Reply | Kind::Thinking | Kind::Message | Kind::Turn
        );
        if !(item.is_main() && answer) {
            break;
        }
        next += 1;
        if item.kind == Kind::Turn {
            break;
        }
    }
    next
}

/// The rows `view` shows of `model`.
pub(crate) fn rows(model: &Model, layout: &Layout) -> Vec<Row> {
    let filter = model.filter();
    let worker_view = matches!(filter, AgentFilter::Worker(_));
    let view = if worker_view { View::Full } else { layout.view };
    let entries = model.entries();
    let mut rows = Rows {
        out: Vec::with_capacity(entries.len()),
        previous: None,
        layout,
    };
    let mut ix = layout
        .first
        .map_or(0, |first| model.position(first).unwrap_or_else(|ix| ix));
    while ix < entries.len() {
        let entry = &entries[ix];
        if is_refresh(&entry.item) {
            rows.push(RowKey::Refresh(entry.item.id), Class::Activity);
            ix = after_refresh(entries, ix);
            continue;
        }
        match show(filter, view, &entry.item) {
            Show::Hidden => ix += 1,
            Show::Count => {
                let (members, next) = run(entries, ix, filter, view, |item| {
                    item.is_main() && item.kind == Kind::Call
                });
                let first = members.first().map_or(ix, |first| *first);
                rows.push(RowKey::Count(entries[first].item.id), Class::Activity);
                ix = next.max(ix + 1);
            }
            Show::Item if view == View::Quiet && in_run(&entry.item) => {
                let (members, next) = run(entries, ix, filter, view, in_run);
                let calls = members
                    .iter()
                    .filter(|member| entries[**member].item.kind == Kind::Call)
                    .count();
                let id = entry.item.id;
                let folded = calls > GROUP_AFTER;
                if folded {
                    rows.push(RowKey::Group(id), Class::Activity);
                }
                if !folded || (layout.expanded)(id) == Some(true) {
                    for member in members {
                        rows.entry(&entries[member], view, worker_view);
                    }
                }
                ix = next.max(ix + 1);
            }
            Show::Item => {
                rows.entry(entry, view, worker_view);
                ix += 1;
            }
        }
    }
    rows.out
}

/// The entries a group or count row stands for: the run starting at the
/// entry `first`.
pub(crate) fn run_of(model: &Model, first: u64, view: View) -> Vec<usize> {
    let Ok(start) = model.position(first) else {
        return Vec::new();
    };
    let entries = model.entries();
    let filter = model.filter();
    match view {
        View::Messages => {
            run(entries, start, filter, view, |item| {
                item.is_main() && item.kind == Kind::Call
            })
            .0
        }
        _ => run(entries, start, filter, view, in_run).0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arko::remote::{CallStatus, Delivery, Source};

    fn item(id: u64, kind: Kind, agent: &str, text: &str) -> Item {
        let mut item = Item::new(kind, text).agent(agent);
        item.id = id;
        item
    }

    fn model(items: Vec<Item>) -> Model {
        let mut model = Model::new(AgentFilter::Main);
        let mut changes = Changes::default();
        model.load(
            Snapshot {
                items,
                ..Snapshot::default()
            },
            &mut changes,
        );
        model
    }

    fn ids(model: &Model) -> Vec<u64> {
        model.entries().iter().map(|entry| entry.item.id).collect()
    }

    /// Every text has one block, and nothing is expanded or in source.
    fn layout(view: View) -> Layout<'static> {
        Layout {
            view,
            expand_thinking: false,
            blocks: &|_| (1, 0),
            expanded: &|_| None,
            source: &|_| false,
            first: None,
        }
    }

    fn keys(model: &Model, view: View) -> Vec<RowKey> {
        rows(model, &layout(view))
            .into_iter()
            .map(|row| row.key)
            .collect()
    }

    fn block(id: u64) -> RowKey {
        RowKey::Block {
            id,
            generation: 0,
            block: 0,
            source: false,
        }
    }

    #[test]
    fn a_conversation_marker_starts_the_transcript_over() {
        let mut model = model(vec![
            item(1, Kind::User, "main", "first"),
            item(2, Kind::Text, "main", "an answer"),
            item(3, Kind::Call, "r1", "read a.rs"),
        ]);
        assert_eq!(ids(&model), [1, 2, 3]);
        let mut changes = Changes::default();
        model.apply(
            Update::Item(item(4, Kind::Conversation, "main", "New conversation")),
            &mut changes,
        );
        assert!(changes.reset);
        assert_eq!(ids(&model), [4]);
        // A late change of the last conversation's call stays out.
        let mut late = item(2, Kind::Text, "main", "an answer, changed");
        late.status = Some(CallStatus::Failed);
        model.apply(Update::Change(late), &mut Changes::default());
        model.apply(
            Update::Item(item(5, Kind::User, "main", "second")),
            &mut Changes::default(),
        );
        assert_eq!(ids(&model), [4, 5]);
        assert_eq!(
            keys(&model, View::Full),
            [RowKey::Separator(4), RowKey::User(5)]
        );
        // A worker's transcript has no conversations.
        let mut worker = Model::new(AgentFilter::Worker("r1".into()));
        let mut changes = Changes::default();
        worker.apply(
            Update::Item(item(1, Kind::Text, "r1", "working")),
            &mut changes,
        );
        worker.apply(
            Update::Item(item(2, Kind::Conversation, "main", "New conversation")),
            &mut changes,
        );
        assert_eq!(ids(&worker), [1]);
    }

    #[test]
    fn a_truncate_drops_the_main_agents_entries_from_there() {
        let mut model = model(vec![
            item(1, Kind::User, "main", "first"),
            item(2, Kind::User, "main", "second"),
            item(3, Kind::Call, "r1", "read a.rs"),
            item(4, Kind::Text, "main", "the reply"),
        ]);
        let mut changes = Changes::default();
        model.apply(Update::Truncate { from: 2 }, &mut changes);
        assert_eq!(ids(&model), [1, 3]);
        assert_eq!(changes.removed, [2, 4]);
    }

    #[test]
    fn appends_and_changes_reach_their_entry_and_results_go_under_their_call() {
        let mut model = model(vec![item(1, Kind::Text, "main", "Hel")]);
        let mut changes = Changes::default();
        model.apply(
            Update::Append {
                id: 1,
                text: "lo".into(),
            },
            &mut changes,
        );
        model.apply(
            Update::Item(item(2, Kind::Call, "main", "cargo test")),
            &mut changes,
        );
        let mut result = item(3, Kind::Result, "main", "exit code 1, 2.0 s");
        result.call = Some(2);
        model.apply(Update::Item(result), &mut changes);
        let mut settled = item(2, Kind::Call, "main", "cargo test");
        settled.status = Some(CallStatus::Failed);
        settled.outcome = Some("exit 1".into());
        model.apply(Update::Change(settled), &mut changes);
        assert_eq!(model.entries()[0].item.text, "Hello");
        assert_eq!(ids(&model), [1, 2]);
        let call = model.entry(2).unwrap();
        assert_eq!(call.item.status, Some(CallStatus::Failed));
        assert_eq!(call.result.as_ref().unwrap().text, "exit code 1, 2.0 s");
        assert_eq!(changes.text, [1]);
        assert_eq!(changes.added, [2]);
        assert_eq!(changes.changed, [2, 2]);
    }

    #[test]
    fn a_calls_change_keeps_what_the_feed_no_longer_holds() {
        let mut call = item(1, Kind::Call, "main", "read a.rs");
        call.tool = Some("read_file".into());
        call.parameters = Some("{\"path\":\"a.rs\"}".into());
        let mut model = model(vec![call]);
        let mut settled = item(1, Kind::Call, "main", "read a.rs");
        settled.status = Some(CallStatus::Ok);
        settled.output = Some("1 | fn a() {}".into());
        model.apply(Update::Change(settled), &mut Changes::default());
        let call = &model.entry(1).unwrap().item;
        assert_eq!(call.tool.as_deref(), Some("read_file"));
        assert!(call.parameters.is_some() && call.output.is_some());
    }

    #[test]
    fn the_oldest_whole_outputs_go_past_the_budget() {
        let mut model = model(Vec::new());
        let big: std::sync::Arc<str> = "x".repeat(KEPT_OUTPUTS / 3).into();
        for id in 1..=4 {
            let mut call = item(id, Kind::Call, "main", "read a.rs");
            model.apply(Update::Item(call.clone()), &mut Changes::default());
            call.status = Some(CallStatus::Ok);
            call.output = Some(big.clone());
            model.apply(Update::Change(call), &mut Changes::default());
        }
        let kept: Vec<bool> = model
            .entries()
            .iter()
            .map(|entry| entry.item.output.is_some())
            .collect();
        assert_eq!(kept, [false, true, true, true]);
    }

    #[test]
    fn the_main_transcript_keeps_what_the_user_and_the_workers_calls_need() {
        let mut to_worker = item(5, Kind::User, "r1", "use plan B");
        to_worker.source = Some(Source::App);
        to_worker.delivery = Some(Delivery::Turn);
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::WorkerLine, "main", "Fix the parser"),
            item(3, Kind::Text, "r1", "worker text"),
            item(4, Kind::Call, "r1", "read a.rs"),
            to_worker,
            item(6, Kind::Notice, "r1", "retrying").level(Level::Warn),
            item(7, Kind::Notice, "r1", "handed over").level(Level::Info),
            item(8, Kind::Turn, "main", "12 s · $0.034"),
        ]);
        assert_eq!(ids(&model), [1, 2, 4, 5, 6, 7, 8]);
        assert_eq!(
            keys(&model, View::Full),
            [
                RowKey::User(1),
                RowKey::Worker(2),
                RowKey::Call(4),
                RowKey::User(5),
                RowKey::Notice(6),
                RowKey::Turn(8)
            ]
        );
        assert_eq!(
            keys(&model, View::Quiet),
            [
                RowKey::User(1),
                RowKey::Worker(2),
                RowKey::User(5),
                RowKey::Turn(8)
            ]
        );
        assert_eq!(
            keys(&model, View::Messages),
            [RowKey::User(1), RowKey::Worker(2), RowKey::User(5)]
        );
    }

    #[test]
    fn quiet_folds_more_than_three_calls_and_messages_counts_them() {
        let calls = |from: u64, count: u64| {
            (from..from + count)
                .map(|id| item(id, Kind::Call, "main", "read a.rs"))
                .collect::<Vec<_>>()
        };
        let mut items = vec![item(1, Kind::User, "main", "go")];
        items.extend(calls(2, 3));
        items.push(item(5, Kind::Text, "main", "now the rest"));
        items.extend(calls(6, 2));
        items.push(item(8, Kind::Thinking, "main", "hmm"));
        items.extend(calls(9, 2));
        items.push(item(11, Kind::Notice, "main", "retrying").level(Level::Warn));
        let model = model(items);
        assert_eq!(
            keys(&model, View::Quiet),
            [
                RowKey::User(1),
                RowKey::Call(2),
                RowKey::Call(3),
                RowKey::Call(4),
                block(5),
                RowKey::Group(6),
                RowKey::Notice(11),
            ]
        );
        assert_eq!(run_of(&model, 6, View::Quiet), [5, 6, 7, 8, 9]);
        let expanded = Layout {
            expanded: &|id| (id == 6).then_some(true),
            ..layout(View::Quiet)
        };
        let rows: Vec<RowKey> = rows(&model, &expanded).iter().map(|r| r.key).collect();
        assert_eq!(
            rows[5..],
            [
                RowKey::Group(6),
                RowKey::Call(6),
                RowKey::Call(7),
                RowKey::Thinking(8),
                RowKey::Call(9),
                RowKey::Call(10),
                RowKey::Notice(11),
            ]
        );
        assert_eq!(
            keys(&model, View::Messages),
            [
                RowKey::User(1),
                RowKey::Count(2),
                block(5),
                RowKey::Count(6),
                RowKey::Notice(11),
            ]
        );
        assert_eq!(run_of(&model, 6, View::Messages), [5, 6, 8, 9]);
        // Full shows every call and the thinking, folded.
        assert_eq!(keys(&model, View::Full).len(), 11);
    }

    #[test]
    fn rows_start_at_the_first_entry_ready() {
        let model = model(vec![
            item(1, Kind::User, "main", "first"),
            item(2, Kind::User, "main", "second"),
            item(3, Kind::User, "main", "third"),
        ]);
        let from_two = Layout {
            first: Some(2),
            ..layout(View::Full)
        };
        let keys: Vec<RowKey> = rows(&model, &from_two).iter().map(|r| r.key).collect();
        assert_eq!(keys, [RowKey::User(2), RowKey::User(3)]);
    }

    #[test]
    fn thinking_expands_by_default_only_in_full_and_replies_get_actions() {
        let model = model(vec![
            item(1, Kind::Thinking, "main", "hmm"),
            item(2, Kind::Reply, "main", "done"),
        ]);
        let expand = Layout {
            expand_thinking: true,
            ..layout(View::Full)
        };
        let keys_of = |layout: &Layout| -> Vec<RowKey> {
            rows(&model, layout).iter().map(|r| r.key).collect()
        };
        assert_eq!(
            keys_of(&expand),
            [RowKey::Thinking(1), block(1), block(2), RowKey::Actions(2)]
        );
        let quiet = Layout {
            expand_thinking: true,
            ..layout(View::Quiet)
        };
        assert_eq!(
            keys_of(&quiet),
            [RowKey::Thinking(1), block(2), RowKey::Actions(2)]
        );
        let folded = Layout {
            expanded: &|_| Some(false),
            ..expand
        };
        assert_eq!(
            keys_of(&folded),
            [RowKey::Thinking(1), block(2), RowKey::Actions(2)]
        );
    }

    #[test]
    fn rows_are_spaced_by_what_they_follow() {
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::Call, "main", "read a.rs"),
            item(3, Kind::Call, "main", "read b.rs"),
            item(4, Kind::Reply, "main", "done"),
            item(5, Kind::Turn, "main", "3 s"),
            item(6, Kind::User, "main", "again"),
        ]);
        let gaps: Vec<Gap> = rows(&model, &layout(View::Full))
            .iter()
            .map(|row| row.gap)
            .collect();
        assert_eq!(
            gaps,
            [
                Gap::None,
                Gap::Section,
                Gap::Tight,
                Gap::Section,
                Gap::None,
                Gap::Small,
                Gap::Turn
            ]
        );
    }

    #[test]
    fn text_a_failed_response_streamed_goes() {
        let mut model = model(vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::Text, "main", "Broken"),
        ]);
        let mut changes = Changes::default();
        model.apply(Update::Remove { id: 2 }, &mut changes);
        model.apply(Update::Remove { id: 9 }, &mut changes);
        assert_eq!(ids(&model), [1]);
        assert_eq!(changes.removed, [2]);
    }

    #[test]
    fn a_cache_refresh_and_its_answer_are_one_quiet_row() {
        let mut refresh = item(2, Kind::User, "main", "(keep the cache warm)");
        refresh.delivery = Some(Delivery::Refresh);
        let model = model(vec![
            item(1, Kind::Reply, "main", "done"),
            refresh,
            item(3, Kind::Thinking, "main", "short"),
            item(4, Kind::Reply, "main", "Acknowledged"),
            item(5, Kind::Turn, "main", "1 s"),
            item(6, Kind::User, "main", "next"),
        ]);
        for view in [View::Full, View::Quiet, View::Messages] {
            let rows = keys(&model, view);
            assert_eq!(
                rows[rows.len() - 2..],
                [RowKey::Refresh(2), RowKey::User(6)],
                "{view:?}"
            );
        }
    }

    #[test]
    fn thinking_streams_until_its_agent_says_more() {
        let mut model = model(vec![item(1, Kind::Thinking, "main", "hmm")]);
        let mut changes = Changes::default();
        model.set_activity(
            Activity {
                working: true,
                ..Activity::default()
            },
            &mut changes,
        );
        assert!(changes.activity);
        assert!(model.is_streaming(0));
        assert_eq!(model.thought_for(0), None);
        let mut text = item(2, Kind::Text, "main", "so");
        text.at = model.entries()[0].item.at + 12_000;
        model.apply(Update::Item(text), &mut changes);
        assert!(!model.is_streaming(0));
        assert_eq!(model.thought_for(0), Some(12_000));
    }

    #[test]
    fn thinking_under_a_second_has_no_time() {
        let mut thinking = item(1, Kind::Thinking, "main", "hmm");
        thinking.at = 1_000;
        let mut text = item(2, Kind::Text, "main", "so");
        text.at = 1_001;
        let model = model(vec![thinking, text]);
        assert_eq!(model.thought_for(0), None);
    }
}
