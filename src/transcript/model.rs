//! What a transcript shows of a session's feed, apart from how it's drawn:
//! one agent's items (the main agent's, or a worker's), kept from the feed's
//! snapshot and updates, and the rows a view makes of them.
//!
//! The app shows one conversation at a time: a conversation marker (`/new`,
//! `/resume`) starts the transcript over. A call's result block goes under
//! its call rather than in a row of its own.
//!
//! Quiet's rows are Arko's ([`quiet::rows`]), the same as the terminal's and
//! the web page's. Neighbouring calls of a kind, or the reads and changes of
//! one file, are one row that says what they did, and a thought between two
//! calls of a row goes inside it. Every other thought is a line of its own,
//! where it happened.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::ops::Range;

use arko::remote::{
    Activity, CallStatus, Delivery, ENDED_JOBS, Item, JobSummary, Kind, Level, Snapshot, Update,
    WorkerState, WorkerSummary, quiet,
};
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
    /// it). The main transcript keeps the workers' calls, what went wrong
    /// for them and their messages to the user, as the terminal does. What
    /// the user wrote to a worker is that worker's own, and so are the cards
    /// of the workers it started. (The main transcript has only the cards of
    /// the main agent's workers.)
    fn keeps(&self, item: &Item) -> bool {
        match self {
            AgentFilter::Main => {
                item.is_main()
                    || matches!(item.kind, Kind::Call | Kind::Result | Kind::Notice)
                    || is_to_user(item)
            }
            AgentFilter::Worker(id) => item.agent == id.as_ref(),
        }
    }
}

impl AgentFilter {
    /// Whether `item` is of the agent whose transcript this is.
    pub(crate) fn owns(&self, item: &Item) -> bool {
        match self {
            AgentFilter::Main => item.is_main(),
            AgentFilter::Worker(id) => item.agent == id.as_ref(),
        }
    }
}

/// Whether `item` is an agent's message to the user (`message_user`). A
/// message an agent got has its sender in `from`.
pub(crate) fn is_to_user(item: &Item) -> bool {
    item.is_to_user()
}

/// "r3 to you": the line over a worker's message to the user in the main
/// transcript. The worker's own transcript doesn't repeat whose it is.
pub(crate) fn to_you(filter: &AgentFilter, item: &Item) -> Option<String> {
    (matches!(filter, AgentFilter::Main) && !item.is_main() && is_to_user(item))
        .then(|| format!("{} to you", item.agent))
}

/// The bytes of calls' parameters and whole outputs a transcript keeps, the
/// latest first: as much again as the feed keeps per lane (it keeps them for
/// the latest calls only, and the transcript keeps what came in its
/// updates).
pub(crate) const KEPT_OUTPUTS: usize = 8 * 1024 * 1024;

/// The bytes of `item` that count in [`KEPT_OUTPUTS`].
fn kept_size(item: &Item) -> usize {
    [&item.parameters, &item.output, &item.whole]
        .into_iter()
        .map(|part| part.as_ref().map_or(0, |part| part.len()))
        .sum()
}

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
    /// A background job started, changed or ended.
    pub jobs: bool,
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
            || self.jobs
            || self.activity
    }
}

/// The items of one transcript, in the feed's order (by id).
pub(crate) struct Model {
    filter: AgentFilter,
    entries: Vec<Entry>,
    workers: Vec<WorkerSummary>,
    /// The session's background jobs, running or ended lately, in the order
    /// they came.
    jobs: Vec<JobSummary>,
    activity: Activity,
    /// The latest conversation marker: items before it are another
    /// conversation's.
    cut: Option<u64>,
    /// [`Self::last_reply`], once worked out for the entries as they are:
    /// each reply row asks for it as it draws.
    last_reply: Cell<Option<Option<u64>>>,
    /// How many times the latest reply was sought through the entries.
    #[cfg(test)]
    replies_sought: Cell<usize>,
}

impl Model {
    pub fn new(filter: AgentFilter) -> Self {
        Self {
            filter,
            entries: Vec::new(),
            workers: Vec::new(),
            jobs: Vec::new(),
            activity: Activity::default(),
            cut: None,
            last_reply: Cell::new(None),
            #[cfg(test)]
            replies_sought: Cell::new(0),
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

    /// The background job `id`, while the feed keeps it.
    pub fn job(&self, id: &str) -> Option<&JobSummary> {
        self.jobs.iter().find(|job| job.id == id)
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
        self.last_reply.set(None);
        self.entries.clear();
        self.cut = None;
        let mut kept = false;
        for item in snapshot.items {
            kept |= self.add(item, changes);
        }
        if kept {
            self.keep_outputs_within_budget();
        }
        self.workers = snapshot.workers;
        self.jobs = snapshot.jobs;
        self.activity = snapshot.activity;
        *changes = Changes {
            reset: true,
            ..Changes::default()
        };
    }

    /// Whether `update` would change nothing here: an item the filter leaves
    /// out, or text streamed into one, such as a worker's text in the main
    /// transcript. (The activity comes apart from its update.)
    pub fn ignores(&self, update: &Update) -> bool {
        match update {
            Update::Item(item) | Update::Change(item) => {
                item.kind != Kind::Conversation
                    && !self.filter.keeps(item)
                    && self.position(item.id).is_err()
            }
            Update::Append { id, .. } | Update::Remove { id } => self.position(*id).is_err(),
            Update::Activity => true,
            _ => false,
        }
    }

    /// Whether `activity` is the one the model has.
    pub fn has_activity(&self, activity: &Activity) -> bool {
        self.activity == *activity
    }

    pub fn set_activity(&mut self, activity: Activity, changes: &mut Changes) {
        if self.activity != activity {
            self.activity = activity;
            changes.activity = true;
        }
    }

    pub fn apply(&mut self, update: Update, changes: &mut Changes) {
        // Streamed text, workers, jobs and activity leave the replies where they
        // are. The rest may move the latest.
        if !matches!(
            update,
            Update::Append { .. } | Update::Worker(_) | Update::Job(_) | Update::Activity
        ) {
            self.last_reply.set(None);
        }
        match update {
            Update::Item(item) => {
                if self.add(item, changes) {
                    self.keep_outputs_within_budget();
                }
            }
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
                        if item.images.is_empty() {
                            item.images = std::mem::take(&mut old.images);
                        }
                        let kept = kept_size(&item) > 0;
                        entry.item = item;
                        changes.changed.push(id);
                        if kept {
                            self.keep_outputs_within_budget();
                        }
                    }
                    // A change of an item never seen: one from before this
                    // client, or one the filter left out.
                    Err(_) => {
                        if self.add(item, changes) {
                            self.keep_outputs_within_budget();
                        }
                    }
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
            // Text a failed response streamed, which its retry streams again,
            // or a note the agent read, which comes again where it read it.
            Update::Remove { id } => {
                if let Ok(ix) = self.position(id) {
                    self.entries.remove(ix);
                    changes.removed.push(id);
                }
            }
            Update::Job(summary) => {
                match self.jobs.iter_mut().find(|job| job.id == summary.id) {
                    Some(job) => *job = summary,
                    None => self.jobs.push(summary),
                }
                self.forget_old_jobs();
                changes.jobs = true;
            }
            // An update a later arko adds.
            _ => {}
        }
    }

    /// Forgets the jobs that started first of those that ended, past arko's
    /// [`ENDED_JOBS`], so it holds the jobs a snapshot does. Their calls'
    /// rows say how they ended without them.
    fn forget_old_jobs(&mut self) {
        let ended = self
            .jobs
            .iter()
            .filter(|job| !job.state.is_running())
            .count();
        let mut extra = ended.saturating_sub(ENDED_JOBS);
        self.jobs.retain(|job| {
            let forget = extra > 0 && !job.state.is_running();
            extra -= usize::from(forget);
            !forget
        });
    }

    /// Adds `item`, and returns whether it brought bytes that count in
    /// [`KEPT_OUTPUTS`].
    fn add(&mut self, item: Item, changes: &mut Changes) -> bool {
        let kept = kept_size(&item) > 0;
        let main = matches!(self.filter, AgentFilter::Main);
        if item.kind == Kind::Conversation {
            if main && item.is_main() {
                // Another conversation: what came before was the last one's.
                self.cut = Some(item.id);
                self.entries.clear();
                self.entries.push(Entry { item, result: None });
                changes.reset = true;
            }
            return false;
        }
        if self.cut.is_some_and(|cut| item.id < cut) || !self.filter.keeps(&item) {
            return false;
        }
        if item.kind == Kind::Result {
            if let Some(call) = item.call
                && let Ok(ix) = self.position(call)
            {
                self.entries[ix].result = Some(item);
                changes.changed.push(call);
                return kept;
            }
            return false;
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
        kept
    }

    /// Lets go of the oldest calls' parameters and whole outputs past
    /// [`KEPT_OUTPUTS`] bytes: the latest stay, and the older calls keep
    /// their lines.
    fn keep_outputs_within_budget(&mut self) {
        let size =
            |entry: &Entry| kept_size(&entry.item) + entry.result.as_ref().map_or(0, kept_size);
        let mut total: usize = self.entries.iter().map(size).sum();
        for entry in &mut self.entries {
            if total <= KEPT_OUTPUTS {
                break;
            }
            total -= size(entry);
            entry.item.parameters = None;
            entry.item.output = None;
            entry.item.whole = None;
            if let Some(result) = &mut entry.result {
                result.parameters = None;
                result.output = None;
                result.whole = None;
            }
        }
    }

    /// Whether the user's message `item` can be edited, sent again or
    /// branched from: one of the main agent's conversation that started a
    /// turn, as the engine marks them.
    pub fn is_changeable(item: &Item) -> bool {
        item.kind == Kind::User
            && item.is_main()
            && item.message_index.is_some()
            && !is_refresh(item)
    }

    /// Whether the conversation's first messages are missing: its first
    /// message here isn't the conversation's first.
    pub fn starts_partway(&self) -> bool {
        self.entries
            .iter()
            .map(|entry| &entry.item)
            .find(|item| Self::is_changeable(item))
            .is_some_and(|item| item.message_index != Some(0))
    }

    /// The latest message of the user's that can be edited.
    pub fn last_message(&self) -> Option<u64> {
        self.entries
            .iter()
            .rev()
            .find(|entry| Self::is_changeable(&entry.item))
            .map(|entry| entry.item.id)
    }

    /// The main agent's latest reply, if a message of the user's that can
    /// be sent again comes before it: the reply Regenerate replaces.
    pub fn last_reply(&self) -> Option<u64> {
        if let Some(known) = self.last_reply.get() {
            return known;
        }
        let found = self.find_last_reply();
        self.last_reply.set(Some(found));
        found
    }

    fn find_last_reply(&self) -> Option<u64> {
        #[cfg(test)]
        self.replies_sought.set(self.replies_sought.get() + 1);
        let message = self.last_message()?;
        let start = self.position(message).ok()?;
        let mut last = None;
        for (item, in_refresh) in self.runs_after(start) {
            // A cache refresh's answer ("Acknowledged") isn't a reply to
            // send again: the engine sends the user's last message.
            if item.kind == Kind::Reply && !in_refresh {
                last = Some(item.id);
            }
        }
        last
    }

    /// The user's latest message, when its turn ended without a reply
    /// (stopped, or failed after its retries) and nothing runs. Regenerate
    /// sends it again from the turn's last line.
    pub fn regenerable_message(&self) -> Option<u64> {
        if self.busy_reason().is_some() || self.last_reply().is_some() {
            return None;
        }
        self.last_message()
    }

    /// How many messages come after the user's message `id`: the user's,
    /// and one reply per turn (cache refreshes and their answers left out).
    /// Older items without a turn are grouped by their place.
    pub fn messages_after(&self, id: u64) -> usize {
        let Ok(start) = self.position(id) else {
            return 0;
        };
        let mut count = 0;
        let mut in_reply = false;
        let mut replies = std::collections::HashSet::new();
        for (item, in_refresh) in self.runs_after(start) {
            match item.kind {
                Kind::User if !in_refresh => {
                    count += 1;
                    in_reply = false;
                }
                Kind::Text | Kind::Reply | Kind::Message | Kind::Question if !in_refresh => {
                    if item.turn.map_or(!in_reply, |turn| replies.insert(turn)) {
                        count += 1;
                    }
                    in_reply = true;
                }
                Kind::Turn | Kind::User => in_reply = false,
                _ => {}
            }
        }
        count
    }

    /// The main agent's items after the entry at `start`, each with whether
    /// it is part of a cache refresh: the refresh message, and what follows
    /// it until its turn ends or the user writes again.
    fn runs_after(&self, start: usize) -> impl Iterator<Item = (&Item, bool)> {
        let mut in_refresh = false;
        self.entries[start + 1..]
            .iter()
            .map(|entry| &entry.item)
            .filter(|item| item.is_main())
            .map(move |item| {
                if item.kind == Kind::User {
                    in_refresh = is_refresh(item);
                }
                let refreshing = in_refresh;
                if item.kind == Kind::Turn {
                    in_refresh = false;
                }
                (item, refreshing)
            })
    }

    /// Why the conversation can't be changed now: a turn runs, or workers
    /// do.
    pub fn busy_reason(&self) -> Option<&'static str> {
        if self.activity.working {
            Some("Wait for the turn to end")
        } else if self.workers.iter().any(|worker| worker.state.is_active()) {
            Some("Wait for the workers to finish, or stop them")
        } else {
            None
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
            .any(|later| &later.item.agent == agent && !is_aside(&later.item));
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

    /// Whether the agent shown waits on the model with no row saying it's
    /// busy, by its latest output.
    pub fn waiting(&self) -> Waiting {
        let busy = match &self.filter {
            AgentFilter::Main => self.activity.working && self.activity.question.is_none(),
            AgentFilter::Worker(id) => self
                .worker(id)
                .is_some_and(|worker| worker.state == WorkerState::Running),
        };
        if !busy {
            return Waiting::No;
        }
        // What went beside the agent's output, and what Arko said of it (a
        // retry, a change of model), leave it waiting.
        let latest = self
            .entries
            .iter()
            .rev()
            .map(|entry| &entry.item)
            .find(|item| {
                self.filter.owns(item)
                    && !is_aside(item)
                    && !matches!(item.kind, Kind::Notice | Kind::Result)
            });
        let Some(latest) = latest else {
            return Waiting::Yes(0);
        };
        let after = latest.id;
        match latest.kind {
            // A cache refresh's quiet row stands for its short turn.
            Kind::User if is_refresh(latest) => Waiting::No,
            // (A question no longer asked, as the agent goes on without its
            // answer.)
            Kind::User | Kind::Turn | Kind::Conversation | Kind::Question => Waiting::Yes(after),
            Kind::Call if latest.status == Some(CallStatus::Running) => Waiting::No,
            Kind::Call => Waiting::Yes(after),
            // Its row says "Thinking…", where the view shows it.
            Kind::Thinking => Waiting::Yes(after),
            Kind::Text | Kind::Message => Waiting::AfterText(after),
            _ => Waiting::No,
        }
    }
}

/// What the agent shown waits on ([`Model::waiting`]), after which of its
/// entries (0 before any).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Waiting {
    /// Nothing, or a row says it's busy: a call that runs, a question for
    /// the user.
    No,
    /// The model, after the user's message, a call's result or thinking
    /// (whose row, where the view shows it, says so itself).
    Yes(u64),
    /// The model, unless its text still comes: the text shows it's busy
    /// while it grows.
    AfterText(u64),
}

/// Whether `item` goes beside its agent's output rather than after it, as
/// the feed places it: what the user typed that the agent hasn't read (a
/// note, a command, a line waiting for a turn), Arko's output and a
/// worker's card. The agent's text and thinking go on above it.
fn is_aside(item: &Item) -> bool {
    match item.kind {
        Kind::Output | Kind::WorkerLine => true,
        Kind::User => {
            item.pending || matches!(item.delivery, Some(Delivery::Command | Delivery::Queued))
        }
        _ => false,
    }
}

/// How a view shows an entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Show {
    Hidden,
    Item,
    /// Counted in its run's line of tool activity (the Messages view).
    Count,
}

/// Whether `item` is a call that started its worker at once, which the Quiet
/// view leaves out because the worker's card stands for it. A call still
/// running, queued or refused has no worker yet, so it shows.
fn is_start(item: &Item) -> bool {
    item.kind == Kind::Call && item.started.is_some()
}

/// What `view` shows of an item of the agent whose transcript it is, the
/// main agent's in the main transcript and a worker's in its own. One rule
/// serves both, so a worker's transcript can't differ from the selected view.
fn shows(view: View, item: &Item) -> Show {
    let warning = item.level.is_some_and(|level| level != Level::Info);
    match view {
        View::Full => match item.kind {
            Kind::Result => Show::Hidden,
            _ => Show::Item,
        },
        View::Quiet => match item.kind {
            Kind::Result => Show::Hidden,
            _ if is_start(item) => Show::Hidden,
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
            // And a kind a later arko adds.
            _ => Show::Hidden,
        },
    }
}

pub(crate) fn show(filter: &AgentFilter, view: View, item: &Item) -> Show {
    if filter.owns(item) {
        return shows(view, item);
    }
    if item.kind == Kind::WorkerLine {
        return Show::Item;
    }
    let warning = item.level.is_some_and(|level| level != Level::Info);
    // A worker's message to the user shows in every view of the main
    // transcript. Its calls, what went wrong for it and its stop show in
    // Full. Its text, its report and what the user wrote to it are its own
    // transcript's.
    if is_to_user(item) {
        return Show::Item;
    }
    match (view, item.kind) {
        (View::Full, Kind::Call) => Show::Item,
        (View::Full, Kind::Notice) if warning || item.stop => Show::Item,
        _ => Show::Hidden,
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
    /// A thought's first line, while it's closed. Open, its blocks show.
    Thinking(u64),
    /// A call's line as Full writes it, and its details when it's open.
    Call(u64),
    /// A Quiet row of calls ([`QuietRow`]), by its first call.
    Group(u64),
    /// A turn's calls counted in one line (Messages), by its first call.
    Count(u64),
    Notice(u64),
    Output(u64),
    Turn(u64),
    Worker(u64),
    /// The card of the agent's questions, each with its state.
    Question(u64),
    /// "Working…" while the agent waits on the model with nothing else
    /// showing it, by the latest entry before it.
    Working(u64),
}

impl RowKey {
    /// The entry the row is of (the first of a group's or a count's run).
    pub fn entry(self) -> u64 {
        match self {
            RowKey::Separator(id)
            | RowKey::User(id)
            | RowKey::Refresh(id)
            | RowKey::Actions(id)
            | RowKey::Thinking(id)
            | RowKey::Call(id)
            | RowKey::Group(id)
            | RowKey::Count(id)
            | RowKey::Notice(id)
            | RowKey::Output(id)
            | RowKey::Turn(id)
            | RowKey::Worker(id)
            | RowKey::Question(id)
            | RowKey::Working(id)
            | RowKey::Block { id, .. } => id,
        }
    }
}

/// Rows `range` of the old list give way to `count` new ones.
pub(crate) type RowChange = (Range<usize>, usize);

/// The runs of `old` that change to make `new`, first to last: each a range
/// of `old`, and how many rows of `new` take its place. Rows in both, in the
/// same order, stay: the second list says where each went, from `old` to
/// `new`.
pub(crate) fn row_changes(old: &[Row], new: &[Row]) -> (Vec<RowChange>, Vec<(usize, usize)>) {
    // Rows change near the end, mostly: the rows before and after the
    // change are in both lists in the same places, and only those between
    // are looked up by key. (A key is in a list once, so a row of one end
    // can't be one of those between in the other list.)
    let start = old
        .iter()
        .zip(new)
        .take_while(|(old, new)| old.key == new.key)
        .count();
    let end = old[start..]
        .iter()
        .rev()
        .zip(new[start..].iter().rev())
        .take_while(|(old, new)| old.key == new.key)
        .count();
    let (old_end, new_end) = (old.len() - end, new.len() - end);
    let index: HashMap<RowKey, usize> = new[start..new_end]
        .iter()
        .enumerate()
        .map(|(ix, row)| (row.key, start + ix))
        .collect();
    let mut kept: Vec<(usize, usize)> = (0..start).map(|ix| (ix, ix)).collect();
    let mut next = start;
    for (old_ix, row) in old.iter().enumerate().take(old_end).skip(start) {
        if let Some(&new_ix) = index.get(&row.key)
            && new_ix >= next
        {
            kept.push((old_ix, new_ix));
            next = new_ix + 1;
        }
    }
    kept.extend((0..end).map(|ix| (old_end + ix, new_end + ix)));
    let mut changes = Vec::new();
    let (mut old_from, mut new_from) = (0, 0);
    for &(old_ix, new_ix) in kept.iter().chain([&(old.len(), new.len())]) {
        if old_ix > old_from || new_ix > new_from {
            changes.push((old_from..old_ix, new_ix - new_from));
        }
        old_from = old_ix + 1;
        new_from = new_ix + 1;
    }
    (changes, kept)
}

/// The space above a row, by what it follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Gap {
    None,
    /// Between two closed lines of work, and between a row's line and the
    /// first of what it holds on its rail.
    Stack,
    /// Between the paragraphs of an open thought, before reply actions or
    /// the turn's footer.
    Small,
    /// Between what is said and the work, and on both sides of anything
    /// open.
    Apart,
    /// Between the paragraphs of what is said.
    Section,
    /// Before a message of the user's: a new turn.
    Turn,
}

/// Whether a row stands on the rail of the open Quiet row above it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rail {
    No,
    /// The first thing on the rail, which starts under the gap above it.
    First,
    /// What follows another thing on the rail, which runs through the gap
    /// above it.
    Next,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Row {
    pub key: RowKey,
    pub gap: Gap,
    pub rail: Rail,
}

/// What kind of row a row is, for the spacing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Separator,
    User,
    /// What is said, the agent's text, a question, an output.
    Said,
    /// A worker's card.
    Card,
    /// A closed line of work, a call's, a Quiet row's or a thought's, a
    /// notice, "Working…".
    Work,
    /// A line of work that is open, with what it holds.
    Open,
    /// A block of an open thought, its first or one after it.
    Thought {
        first: bool,
    },
    Actions,
    Footer,
}

fn gap(previous: Option<Class>, next: Class) -> Gap {
    let Some(previous) = previous else {
        return Gap::None;
    };
    match (previous, next) {
        (_, Class::Actions | Class::Footer) => Gap::Small,
        (Class::Separator, Class::User) => Gap::Section,
        // After a user's message's actions, as after a reply's.
        (Class::User, _) => Gap::Small,
        (_, Class::User) => Gap::Turn,
        (Class::Actions, _) => Gap::Small,
        (Class::Thought { .. }, Class::Thought { first: false }) => Gap::Small,
        (Class::Work, Class::Work) => Gap::Stack,
        (Class::Said, Class::Said) => Gap::Section,
        // What is said and the work stand apart, and so does anything open.
        _ => Gap::Apart,
    }
}

/// What a Quiet row shows ([`quiet::rows`]), by its first call.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct QuietRow {
    pub words: quiet::Words,
    /// Its calls and the thinking between them, by entry, in order.
    pub members: Vec<u64>,
    /// It holds several calls, each of which shows as its Full line on the
    /// row's rail when the row is open. A row of one call shows the call's
    /// detail and content there.
    pub merged: bool,
    /// It has more to show than its words.
    pub opens: bool,
}

/// A turn's calls, counted in one line (Messages).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Count {
    pub calls: usize,
    pub failed: usize,
    pub running: usize,
}

/// The rows a view shows, and what its Quiet rows and counts say.
#[derive(Debug, Default)]
pub(crate) struct Laid {
    pub rows: Vec<Row>,
    /// Each Quiet row, by its first call.
    pub quiet: HashMap<u64, QuietRow>,
    /// Each count of a turn's calls (Messages), by its first call.
    pub counts: HashMap<u64, Count>,
}

/// What the rows depend on besides the entries.
pub(crate) struct Layout<'a> {
    pub view: View,
    /// Thinking shows open unless the reader closes it (Full only).
    pub expand_thinking: bool,
    /// The number of Markdown blocks of an entry's text so far, and the
    /// generation of its document.
    pub blocks: &'a dyn Fn(u64) -> (usize, u32),
    /// The reader's choice for an entry's thinking or a call's details, if
    /// any.
    pub expanded: &'a dyn Fn(u64) -> Option<bool>,
    /// The reader's choice for a Quiet row, by its first call.
    pub group_open: &'a dyn Fn(u64) -> Option<bool>,
    /// The calls of Quiet rows of one call that the reader opened, and the
    /// thoughts the reader opened on lines of their own, which keep their
    /// form ([`quiet::Call::sealed`], [`quiet::Step::Other`]).
    pub sealed: &'a dyn Fn(u64) -> bool,
    /// Whether the reader opened a turn's count (Messages), by its first
    /// call. The turn's work shows under it from there, as Quiet shows it.
    pub count_open: &'a dyn Fn(u64) -> bool,
    /// Whether an entry's text shows as its source.
    pub source: &'a dyn Fn(u64) -> bool,
    /// The oldest entry ready to show, when older ones are still being made
    /// ready: the rows start there.
    pub first: Option<u64>,
    /// "Working…" below the agent's output, by the agent's entry it follows
    /// ([`Waiting`]).
    pub working: Option<u64>,
}

struct Rows<'a> {
    out: Vec<Row>,
    previous: Option<Class>,
    /// While the rows go on the rail of an open Quiet row, whether anything
    /// stands on it yet.
    rail: Option<bool>,
    layout: &'a Layout<'a>,
    quiet: HashMap<u64, QuietRow>,
}

impl Rows<'_> {
    fn push(&mut self, key: RowKey, class: Class) {
        let (gap, rail) = match self.rail {
            None => (gap(self.previous, class), Rail::No),
            // What a row holds stacks under its line.
            Some(false) => (Gap::Stack, Rail::First),
            Some(true) => (gap(self.previous, class), Rail::Next),
        };
        if let Some(started) = &mut self.rail {
            *started = true;
        }
        self.out.push(Row { key, gap, rail });
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

    /// A thought's rows, its line or, open, its text.
    fn thinking(&mut self, id: u64, view: View) {
        let default = self.layout.expand_thinking && view == View::Full;
        let open = (self.layout.expanded)(id).unwrap_or(default);
        let (count, generation) = (self.layout.blocks)(id);
        if !open || count == 0 {
            self.push(RowKey::Thinking(id), Class::Work);
            return;
        }
        let source = (self.layout.source)(id);
        for block in 0..count {
            self.push(
                RowKey::Block {
                    id,
                    generation,
                    block,
                    source,
                },
                Class::Thought { first: block == 0 },
            );
        }
    }

    /// A call's line as Full writes it, with its details when it's open.
    fn call(&mut self, id: u64) {
        let open = (self.layout.expanded)(id) == Some(true);
        let class = if open { Class::Open } else { Class::Work };
        self.push(RowKey::Call(id), class);
    }

    /// The rows of one entry, as `view` shows it.
    fn entry(&mut self, entry: &Entry, view: View) {
        let item = &entry.item;
        let id = item.id;
        match item.kind {
            Kind::Conversation => self.push(RowKey::Separator(id), Class::Separator),
            Kind::User => self.push(RowKey::User(id), Class::User),
            // A card of the agent's questions, with each one's state.
            Kind::Question if has_questions(item) => self.push(RowKey::Question(id), Class::Said),
            Kind::Text | Kind::Message | Kind::Question => self.blocks(id, Class::Said),
            Kind::Reply => {
                self.blocks(id, Class::Said);
                if (self.layout.blocks)(id).0 > 0 {
                    self.push(RowKey::Actions(id), Class::Actions);
                }
            }
            Kind::Thinking => self.thinking(id, view),
            Kind::Call => self.call(id),
            Kind::Notice => self.push(RowKey::Notice(id), Class::Work),
            Kind::Output => self.push(RowKey::Output(id), Class::Said),
            Kind::Turn => self.push(RowKey::Turn(id), Class::Footer),
            Kind::WorkerLine => self.push(RowKey::Worker(id), Class::Card),
            // A result shows in its call's row. A kind a later arko adds
            // has no rows.
            _ => {}
        }
    }

    /// A Quiet row of the entries `members` (indexes of `entries`), and
    /// open, what its calls and thinking show on its rail when it holds
    /// several.
    fn quiet_row(&mut self, row: quiet::Row, members: Vec<usize>, entries: &[Entry]) {
        let Some(&first) = members.first() else {
            return;
        };
        let first = entries[first].item.id;
        let open = (self.layout.group_open)(first) == Some(true);
        self.push(
            RowKey::Group(first),
            if open { Class::Open } else { Class::Work },
        );
        if open && row.merged {
            self.rail = Some(false);
            for ix in &members {
                let item = &entries[*ix].item;
                match item.kind {
                    Kind::Thinking => self.thinking(item.id, View::Quiet),
                    _ => self.call(item.id),
                }
            }
            // What follows stands apart from the open row.
            if self.rail.take() == Some(true) {
                self.previous = Some(Class::Open);
            }
        }
        self.quiet.insert(
            first,
            QuietRow {
                words: row.words,
                members: members.iter().map(|ix| entries[*ix].item.id).collect(),
                merged: row.merged,
                opens: row.opens,
            },
        );
    }
}

/// Whether `item` is a card of the agent's questions with their states (an
/// ask from before questions had them shows as its text).
pub(crate) fn has_questions(item: &Item) -> bool {
    item.questions
        .as_ref()
        .is_some_and(|questions| !questions.is_empty())
}

/// Whether `item` belongs in a run of the shown agent's work, which Quiet's
/// rows merge, its calls and its thinking between them. A call the Quiet
/// view leaves out ([`is_start`]) isn't in it, so it neither joins a row
/// nor ends one.
fn in_run(filter: &AgentFilter, item: &Item) -> bool {
    filter.owns(item) && matches!(item.kind, Kind::Call | Kind::Thinking) && !is_start(item)
}

/// Whether `item` is a message that only kept the prompt cache warm.
pub(crate) fn is_refresh(item: &Item) -> bool {
    item.is_main()
        && item.kind == Kind::User
        && (item.delivery == Some(Delivery::Refresh)
            || item.text == arko::remote::REFRESH_MESSAGE
            || item.text
                == "(Arko: this message only keeps the prompt cache warm. Reply with \"Acknowledged\" and nothing else.)")
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

/// What a view shows, in order, before Quiet's rows take its work.
#[derive(Debug, Clone, Copy)]
enum Unit {
    /// An entry's rows, by its index.
    Entry(usize),
    /// A call or a thinking that Quiet's rows take, by its index.
    Work(usize),
    /// A cache refresh and its answer, by the refresh's index.
    Refresh(usize),
    /// A turn's count of calls (Messages), by its first call.
    Count(u64),
}

/// Where each turn's count of calls (Messages) stands, and what it counts,
/// as Arko places them for every client ([`quiet::counts`]).
#[derive(Default)]
struct Counted {
    /// Each count's first call, by the item its line stands at. That is its
    /// turn's first call, or the thought of that turn that led to it with
    /// nothing shown between them, so open it shows the turn's work from
    /// its first thought.
    anchors: HashMap<u64, u64>,
    /// Each turn's count, by its first call.
    turns: HashMap<Option<u64>, u64>,
    counts: HashMap<u64, Count>,
}

impl Counted {
    /// The count of the turn `item` is of.
    fn count_of(&self, item: &Item) -> Option<u64> {
        self.turns.get(&item.turn).copied()
    }
}

/// Counts each turn's calls (Messages), from the entry at `start` on.
fn counted(model: &Model, start: usize) -> Counted {
    let filter = model.filter();
    let entries = model.entries();
    let mut seen = Vec::with_capacity(entries.len().saturating_sub(start));
    let mut ix = start;
    while ix < entries.len() {
        let item = &entries[ix].item;
        if item.pending {
            ix += 1;
            continue;
        }
        if is_refresh(item) {
            seen.push(quiet::Seen::Shown);
            ix = after_refresh(entries, ix);
            continue;
        }
        seen.push(match show(filter, View::Messages, item) {
            Show::Count => quiet::Seen::Call {
                id: item.id,
                turn: item.turn,
                status: item.status,
            },
            Show::Item => quiet::Seen::Shown,
            Show::Hidden if filter.owns(item) && item.kind == Kind::Thinking => {
                quiet::Seen::Thinking {
                    id: item.id,
                    turn: item.turn,
                }
            }
            Show::Hidden => quiet::Seen::Hidden,
        });
        ix += 1;
    }
    let mut counted = Counted::default();
    for count in quiet::counts(seen) {
        counted.anchors.insert(count.at, count.first);
        counted.turns.insert(count.turn, count.first);
        counted.counts.insert(
            count.first,
            Count {
                calls: count.calls,
                failed: count.failed,
                running: count.running,
            },
        );
    }
    counted
}

/// A call or a thinking, as Quiet's rows read it.
fn step<'a>(entry: &'a Entry, layout: &Layout) -> quiet::Step<'a> {
    let item = &entry.item;
    if item.kind == Kind::Call {
        return quiet::Step::Call(quiet::Call {
            id: item.id,
            act: item.act.as_ref(),
            status: item.status,
            outcome: item.outcome.as_deref(),
            job: item.job.is_some(),
            turn: item.turn,
            content: super::row::opens(entry),
            ended: item.ended,
            sealed: (layout.sealed)(item.id),
        });
    }
    // A thought the reader opened on a line of its own stays there.
    if (layout.sealed)(item.id) {
        quiet::Step::Other(item.id)
    } else {
        quiet::Step::Thinking
    }
}

/// The rows `view` shows of `model`.
pub(crate) fn rows(model: &Model, layout: &Layout) -> Laid {
    let filter = model.filter();
    let view = layout.view;
    let entries = model.entries();
    let start = layout
        .first
        .map_or(0, |first| model.position(first).unwrap_or_else(|ix| ix));
    // Notes the agent hasn't read wait at the end, below what it goes on
    // with, until they come again where it read them. Those older than the
    // entries ready to show wait there too.
    let waits = |at: usize| {
        let item = &entries[at].item;
        item.pending && show(filter, view, item) != Show::Hidden
    };
    let mut waiting: Vec<usize> = (0..start).filter(|at| waits(*at)).collect();
    let counted = (view == View::Messages).then(|| counted(model, start));
    // The counts the reader opened so far, whose turns show from there as
    // Quiet shows them.
    let mut opened: HashSet<u64> = HashSet::new();
    let mut units = Vec::with_capacity(entries.len().saturating_sub(start));
    let mut ix = start;
    while ix < entries.len() {
        let item = &entries[ix].item;
        if item.pending {
            if waits(ix) {
                waiting.push(ix);
            }
            ix += 1;
            continue;
        }
        if is_refresh(item) {
            units.push(Unit::Refresh(ix));
            ix = after_refresh(entries, ix);
            continue;
        }
        let mut seen = view;
        if let Some(counted) = &counted {
            if let Some(&first) = counted.anchors.get(&item.id) {
                units.push(Unit::Count(first));
                if (layout.count_open)(first) {
                    opened.insert(first);
                }
            }
            if counted
                .count_of(item)
                .is_some_and(|first| opened.contains(&first))
            {
                seen = View::Quiet;
            }
        }
        match show(filter, seen, item) {
            Show::Item if seen == View::Quiet && in_run(filter, item) => units.push(Unit::Work(ix)),
            Show::Item => units.push(Unit::Entry(ix)),
            Show::Hidden | Show::Count => {}
        }
        ix += 1;
    }
    // Quiet's rows take each run of work, the calls and the thinking
    // between two other things shown.
    let pieces = if units.iter().any(|unit| matches!(unit, Unit::Work(_))) {
        let steps: Vec<quiet::Step> = units
            .iter()
            .map(|unit| match *unit {
                Unit::Work(at) => step(&entries[at], layout),
                Unit::Entry(at) | Unit::Refresh(at) => quiet::Step::Other(entries[at].item.id),
                Unit::Count(first) => quiet::Step::Other(first),
            })
            .collect();
        quiet::rows(&steps)
    } else {
        (0..units.len()).map(quiet::Piece::Step).collect()
    };
    let mut rows = Rows {
        out: Vec::with_capacity(units.len()),
        previous: None,
        rail: None,
        layout,
        quiet: HashMap::new(),
    };
    for piece in pieces {
        match piece {
            quiet::Piece::Step(at) => match units[at] {
                Unit::Entry(ix) => rows.entry(&entries[ix], view),
                Unit::Work(ix) => rows.entry(&entries[ix], View::Quiet),
                Unit::Refresh(ix) => {
                    rows.push(RowKey::Refresh(entries[ix].item.id), Class::Work);
                }
                Unit::Count(first) => rows.push(RowKey::Count(first), Class::Work),
            },
            quiet::Piece::Row(row) => {
                let members = row
                    .members
                    .iter()
                    .filter_map(|at| match units[*at] {
                        Unit::Work(ix) => Some(ix),
                        _ => None,
                    })
                    .collect();
                rows.quiet_row(row, members, entries);
            }
        }
    }
    // What the agent goes on with, above the notes it hasn't read, unless
    // its thought, the live line, says it.
    if let Some(after) = layout.working {
        let thinking = model
            .entry(after)
            .is_some_and(|entry| entry.item.kind == Kind::Thinking)
            && rows.out.iter().rev().any(|row| {
                matches!(row.key, RowKey::Thinking(id) | RowKey::Block { id, .. } if id == after)
            });
        if !thinking {
            rows.push(RowKey::Working(after), Class::Work);
        }
    }
    for ix in waiting {
        rows.entry(&entries[ix], view);
    }
    Laid {
        rows: rows.out,
        quiet: rows.quiet,
        counts: counted.map(|counted| counted.counts).unwrap_or_default(),
    }
}

/// Whether `rows` show "Working…". Only the notes the agent hasn't read
/// come after it.
pub(crate) fn shows_working(rows: &[Row]) -> bool {
    rows.iter()
        .rev()
        .take_while(|row| matches!(row.key, RowKey::User(_) | RowKey::Working(_)))
        .any(|row| matches!(row.key, RowKey::Working(_)))
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

    /// The model keeps the session's background jobs, from the snapshot and
    /// as their summaries come, and forgets those that ended first once
    /// more than [`ENDED_JOBS`] ended. Those running stay.
    #[test]
    fn background_jobs_come_with_the_snapshot_and_their_updates() {
        use arko::remote::{Feed, JobKind, JobStart, JobState, MAIN_AGENT};

        let feed = Feed::new("main");
        let start = |kind| {
            let id = uuid::Uuid::new_v4();
            feed.job_started(JobStart::new(id, MAIN_AGENT, kind, "npm run dev"));
            id
        };
        let first = start(JobKind::Command);
        let (snapshot, mut updates) = feed.subscribe();
        let mut model = Model::new(AgentFilter::Main);
        model.load(snapshot, &mut Changes::default());
        let state = |model: &Model, id: uuid::Uuid| model.job(&id.to_string()).map(|job| job.state);
        assert_eq!(state(&model, first), Some(JobState::Running));

        feed.job_ended(first, JobState::Stopped, None);
        let running = start(JobKind::Call);
        let ended: Vec<uuid::Uuid> = (0..ENDED_JOBS + 2)
            .map(|_| {
                let id = start(JobKind::Command);
                feed.job_ended(id, JobState::Finished, Some("exit 0".into()));
                id
            })
            .collect();
        let mut changes = Changes::default();
        while let Ok(update) = updates.try_recv() {
            model.apply(update, &mut changes);
        }
        assert!(changes.jobs && changes.any());
        assert_eq!(state(&model, first), None, "forgotten");
        assert_eq!(state(&model, ended[0]), None, "forgotten");
        assert_eq!(state(&model, ended[2]), Some(JobState::Finished));
        assert_eq!(state(&model, running), Some(JobState::Running));
        assert_eq!(model.jobs.len(), ENDED_JOBS + 1);
    }

    /// Every text has one block, and nothing is expanded or in source.
    fn layout(view: View) -> Layout<'static> {
        Layout {
            view,
            expand_thinking: false,
            blocks: &|_| (1, 0),
            expanded: &|_| None,
            group_open: &|_| None,
            sealed: &|_| false,
            count_open: &|_| false,
            source: &|_| false,
            first: None,
            working: None,
        }
    }

    /// A read of `path` that ended, in turn 1, as the feed has it.
    fn read(id: u64, path: &str) -> Item {
        let mut call = item(id, Kind::Call, "main", &format!("read {path}"));
        call.status = Some(CallStatus::Ok);
        call.turn = Some(1);
        call.act = Some(arko::remote::Act {
            kind: arko::remote::ActKind::Read,
            target: path.into(),
            ..arko::remote::Act::default()
        });
        call
    }

    /// A thought of turn 1.
    fn thought(id: u64, text: &str) -> Item {
        let mut thought = item(id, Kind::Thinking, "main", text);
        thought.turn = Some(1);
        thought
    }

    fn keys_of(model: &Model, layout: &Layout) -> Vec<RowKey> {
        rows(model, layout).rows.iter().map(|row| row.key).collect()
    }

    fn keys(model: &Model, view: View) -> Vec<RowKey> {
        keys_of(model, &layout(view))
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
    fn the_parameters_of_the_oldest_calls_go_past_the_budget_too() {
        let mut model = model(Vec::new());
        // Twenty files of 1 MB written, as their calls come from the feed:
        // the call with its parameters, then its change with the result.
        let file = "x".repeat(1024 * 1024);
        for id in 1..=20 {
            let mut call = item(id, Kind::Call, "main", "write a.rs");
            call.tool = Some("write_file".into());
            call.parameters = Some(format!("{{\"content\":\"{file}\"}}").into());
            model.apply(Update::Item(call.clone()), &mut Changes::default());
            call.parameters = None;
            call.status = Some(CallStatus::Ok);
            call.output = Some("Wrote a.rs".into());
            model.apply(Update::Change(call), &mut Changes::default());
        }
        let kept: usize = model.entries().iter().map(|e| kept_size(&e.item)).sum();
        assert!(kept <= KEPT_OUTPUTS, "{kept}");
        let with_parameters: Vec<u64> = model
            .entries()
            .iter()
            .filter(|entry| entry.item.parameters.is_some())
            .map(|entry| entry.item.id)
            .collect();
        assert_eq!(with_parameters, (14..=20).collect::<Vec<_>>());
        // A history loaded at once keeps within it as well.
        let mut items: Vec<Item> = model.entries().iter().map(|e| e.item.clone()).collect();
        for item in &mut items {
            item.parameters = Some(format!("{{\"content\":\"{file}\"}}").into());
        }
        let loaded = self::model(items);
        let kept: usize = loaded.entries().iter().map(|e| kept_size(&e.item)).sum();
        assert!(kept <= KEPT_OUTPUTS, "{kept}");
    }

    /// A worker's stop shows in the main transcript's Full view, whether the
    /// user stopped the worker or its starter's stop did, and the worker's
    /// other Info notices stay in its own transcript.
    #[test]
    fn a_workers_stop_shows_in_the_main_transcript() {
        let mut stop =
            item(2, Kind::Notice, "r2", "stopped (its lead r1 was stopped)").level(Level::Info);
        stop.stop = true;
        stop.parent = Some("r1".into());
        let items = vec![
            item(1, Kind::User, "main", "go"),
            stop,
            item(3, Kind::Notice, "r2", "handed over").level(Level::Info),
        ];
        assert_eq!(
            keys(&model(items), View::Full),
            [RowKey::User(1), RowKey::Notice(2)]
        );
    }

    /// The main transcript keeps the workers' calls and what went wrong for
    /// them. What the user wrote to a worker is the worker's transcript's
    /// alone, as are its text and report.
    #[test]
    fn the_main_transcript_keeps_what_the_workers_calls_need() {
        let mut to_worker = item(5, Kind::User, "r1", "use plan B");
        to_worker.source = Some(Source::App);
        to_worker.delivery = Some(Delivery::Turn);
        let items = vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::WorkerLine, "main", "Fix the parser"),
            item(3, Kind::Text, "r1", "worker text"),
            item(4, Kind::Call, "r1", "read a.rs"),
            to_worker,
            item(6, Kind::Notice, "r1", "retrying").level(Level::Warn),
            item(7, Kind::Notice, "r1", "handed over").level(Level::Info),
            item(8, Kind::Turn, "main", "12 s · $0.034"),
            item(9, Kind::Text, "r2", "other worker text"),
        ];
        let main = model(items.clone());
        assert_eq!(ids(&main), [1, 2, 4, 6, 7, 8]);
        assert_eq!(
            keys(&main, View::Full),
            [
                RowKey::User(1),
                RowKey::Worker(2),
                RowKey::Call(4),
                RowKey::Notice(6),
                RowKey::Turn(8)
            ]
        );
        assert_eq!(
            keys(&main, View::Quiet),
            [RowKey::User(1), RowKey::Worker(2), RowKey::Turn(8)]
        );
        assert_eq!(
            keys(&main, View::Messages),
            [RowKey::User(1), RowKey::Worker(2)]
        );
        // r1's own transcript has the message, r2's doesn't.
        let worker = |id: &str| {
            let mut worker = Model::new(AgentFilter::Worker(id.to_string().into()));
            let snapshot = Snapshot {
                items: items.clone(),
                ..Snapshot::default()
            };
            worker.load(snapshot, &mut Changes::default());
            worker
        };
        for view in [View::Full, View::Quiet, View::Messages] {
            assert!(keys(&worker("r1"), view).contains(&RowKey::User(5)));
            assert!(!keys(&worker("r2"), view).contains(&RowKey::User(5)));
        }
    }

    /// The card of a worker's worker is in the transcript of the worker that
    /// started it (its lane), and the main transcript has the cards of the
    /// main agent's workers only.
    #[test]
    fn a_workers_transcript_keeps_the_cards_of_the_workers_it_started() {
        let card = |id: u64, lane: &str, worker: &str| {
            let mut card = item(id, Kind::WorkerLine, lane, "Count the files");
            card.worker = Some(worker.into());
            card
        };
        let items = vec![
            item(1, Kind::User, "main", "go"),
            card(2, "main", "r1"),
            item(3, Kind::Text, "r1", "Looking."),
            card(4, "r1", "r2"),
            card(5, "r2", "r3"),
        ];
        let main = model(items.clone());
        assert_eq!(ids(&main), [1, 2]);
        let worker = |id: &str| {
            let mut worker = Model::new(AgentFilter::Worker(id.to_string().into()));
            let snapshot = Snapshot {
                items: items.clone(),
                ..Snapshot::default()
            };
            worker.load(snapshot, &mut Changes::default());
            worker
        };
        assert_eq!(ids(&worker("r1")), [3, 4]);
        assert_eq!(ids(&worker("r2")), [5]);
        assert_eq!(
            keys(&worker("r1"), View::Full),
            [block(3), RowKey::Worker(4)]
        );
    }

    /// Quiet's rows merge the calls of a kind that follow one another, and
    /// a thought between two calls of a row goes inside it. Messages counts
    /// a turn's calls in one line, where its first call is.
    #[test]
    fn quiet_merges_calls_of_a_kind_and_messages_counts_a_turn() {
        let mut text = item(5, Kind::Text, "main", "now the rest");
        text.turn = Some(1);
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            read(2, "src/a.rs"),
            read(3, "src/b.rs"),
            read(4, "src/c.rs"),
            text,
            read(6, "src/d.rs"),
            read(7, "src/e.rs"),
            thought(8, "hmm"),
            read(9, "src/f.rs"),
            read(10, "src/g.rs"),
            item(11, Kind::Notice, "main", "retrying").level(Level::Warn),
        ]);
        let laid = rows(&model, &layout(View::Quiet));
        let shown: Vec<RowKey> = laid.rows.iter().map(|row| row.key).collect();
        assert_eq!(
            shown,
            [
                RowKey::User(1),
                RowKey::Group(2),
                block(5),
                RowKey::Group(6),
                RowKey::Notice(11),
            ]
        );
        assert_eq!(laid.quiet[&2].words.text, "Read 3 files in src");
        assert_eq!(laid.quiet[&2].members, [2, 3, 4]);
        assert_eq!(laid.quiet[&6].words.text, "Read 4 files in src");
        assert_eq!(laid.quiet[&6].members, [6, 7, 8, 9, 10]);
        // Open, a row of several shows its calls and the thought between
        // them on its rail.
        let open = Layout {
            group_open: &|id| (id == 6).then_some(true),
            ..layout(View::Quiet)
        };
        let laid = rows(&model, &open);
        let shown: Vec<(RowKey, Rail)> = laid.rows[3..]
            .iter()
            .map(|row| (row.key, row.rail))
            .collect();
        assert_eq!(
            shown,
            [
                (RowKey::Group(6), Rail::No),
                (RowKey::Call(6), Rail::First),
                (RowKey::Call(7), Rail::Next),
                (RowKey::Thinking(8), Rail::Next),
                (RowKey::Call(9), Rail::Next),
                (RowKey::Call(10), Rail::Next),
                (RowKey::Notice(11), Rail::No),
            ]
        );
        let laid = rows(&model, &layout(View::Messages));
        let shown: Vec<RowKey> = laid.rows.iter().map(|row| row.key).collect();
        assert_eq!(
            shown,
            [
                RowKey::User(1),
                RowKey::Count(2),
                block(5),
                RowKey::Notice(11)
            ]
        );
        assert_eq!(laid.counts[&2].calls, 7);
        // Full shows every call and the thought.
        assert_eq!(keys(&model, View::Full).len(), 11);
    }

    /// A thought before a row's first call, or after its last, is a line of
    /// its own, where it happened. One the reader opened there stays there
    /// when the next call comes, which starts a row of its own.
    #[test]
    fn a_thought_goes_in_a_row_only_between_its_calls() {
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            thought(2, "first, read"),
            read(3, "src/a.rs"),
            thought(4, "then the next"),
            read(5, "src/b.rs"),
            thought(6, "now edit"),
        ]);
        assert_eq!(
            keys(&model, View::Quiet),
            [
                RowKey::User(1),
                RowKey::Thinking(2),
                RowKey::Group(3),
                RowKey::Thinking(6)
            ]
        );
        let opened = model_with(&model, read(7, "src/c.rs"));
        let sealed = Layout {
            sealed: &|id| id == 6,
            ..layout(View::Quiet)
        };
        assert_eq!(
            keys_of(&opened, &sealed),
            [
                RowKey::User(1),
                RowKey::Thinking(2),
                RowKey::Group(3),
                RowKey::Thinking(6),
                RowKey::Group(7),
            ]
        );
        // Not opened, it goes in the row the next call joins.
        assert_eq!(
            keys(&opened, View::Quiet),
            [RowKey::User(1), RowKey::Thinking(2), RowKey::Group(3)]
        );
    }

    /// A thought leads only to a call of its own turn. A turn that ended in
    /// a thought, and one Arko started for news that calls at once, keep
    /// their count where the call is.
    #[test]
    fn a_count_stands_at_a_thought_of_its_own_turn_only() {
        let mut news = read(3, "src/a.rs");
        news.turn = Some(2);
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            thought(2, "nothing to do"),
            news,
        ]);
        assert_eq!(
            keys(&model, View::Messages),
            [RowKey::User(1), RowKey::Count(3)]
        );
        let open = Layout {
            count_open: &|id| id == 3,
            ..layout(View::Messages)
        };
        assert_eq!(
            keys_of(&model, &open),
            [RowKey::User(1), RowKey::Count(3), RowKey::Group(3)]
        );
    }

    /// `model`'s entries and `item` after them.
    fn model_with(model: &Model, item: Item) -> Model {
        let mut items: Vec<Item> = model
            .entries()
            .iter()
            .map(|entry| entry.item.clone())
            .collect();
        items.push(item);
        self::model(items)
    }

    /// An open count shows its turn's work as Quiet shows it, from the
    /// thought that led to its first call.
    #[test]
    fn an_open_count_shows_its_turn_from_its_first_thought() {
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            thought(2, "first, read"),
            read(3, "src/a.rs"),
            read(4, "src/b.rs"),
        ]);
        assert_eq!(
            keys(&model, View::Messages),
            [RowKey::User(1), RowKey::Count(3)]
        );
        let open = Layout {
            count_open: &|id| id == 3,
            ..layout(View::Messages)
        };
        assert_eq!(
            keys_of(&model, &open),
            [
                RowKey::User(1),
                RowKey::Count(3),
                RowKey::Thinking(2),
                RowKey::Group(3)
            ]
        );
    }

    /// A `start_agent` call as the feed has it: it names its worker once it
    /// started one at once.
    fn start_call(id: u64, lane: &str, status: CallStatus, started: Option<&str>) -> Item {
        let mut call = item(id, Kind::Call, lane, "start worker");
        call.status = Some(status);
        call.started = started.map(str::to_string);
        call
    }

    fn worker_card(id: u64, lane: &str, worker: &str) -> Item {
        let mut card = item(id, Kind::WorkerLine, lane, "Fix the parser");
        card.worker = Some(worker.into());
        card
    }

    /// Quiet leaves out the call that started a worker, whose card stands for
    /// it. A call still running, queued or refused shows, and Full and
    /// Messages keep every call, in a worker's own transcript too.
    #[test]
    fn quiet_leaves_out_the_call_that_started_a_worker() {
        let items = vec![
            item(1, Kind::User, "main", "go"),
            start_call(2, "main", CallStatus::Ok, Some("r1")),
            worker_card(3, "main", "r1"),
            // Queued, refused, and waiting for the billing answer.
            start_call(4, "main", CallStatus::Ok, None),
            start_call(5, "main", CallStatus::Failed, None),
            start_call(6, "main", CallStatus::Running, None),
            // A call of r1 that started r2, with r2's card in r1's transcript.
            start_call(7, "r1", CallStatus::Ok, Some("r2")),
            worker_card(8, "r1", "r2"),
        ];
        let main = model(items.clone());
        assert_eq!(
            keys(&main, View::Quiet),
            [
                RowKey::User(1),
                RowKey::Worker(3),
                RowKey::Call(4),
                RowKey::Call(5),
                RowKey::Call(6),
            ]
        );
        assert_eq!(
            keys(&main, View::Full),
            [
                RowKey::User(1),
                RowKey::Call(2),
                RowKey::Worker(3),
                RowKey::Call(4),
                RowKey::Call(5),
                RowKey::Call(6),
                RowKey::Call(7),
            ]
        );
        // (One count for the turn's calls, where its first is.)
        assert_eq!(
            keys(&main, View::Messages),
            [RowKey::User(1), RowKey::Count(2), RowKey::Worker(3)]
        );
        // The call is hidden for the find and the reader's keys too.
        let filter = AgentFilter::Main;
        let shown = |view| show(&filter, view, &items[1]);
        assert_eq!(shown(View::Quiet), Show::Hidden);
        assert_eq!(shown(View::Full), Show::Item);
        assert_eq!(shown(View::Messages), Show::Count);
        for call in &items[3..6] {
            assert_eq!(show(&filter, View::Quiet, call), Show::Item);
        }
        // r1's own transcript follows the view: the card of r2 stands for the
        // call that started it in Quiet, and Messages counts the call.
        let mut own = Model::new(AgentFilter::Worker("r1".into()));
        let snapshot = Snapshot {
            items,
            ..Snapshot::default()
        };
        own.load(snapshot, &mut Changes::default());
        assert_eq!(keys(&own, View::Full), [RowKey::Call(7), RowKey::Worker(8)]);
        assert_eq!(keys(&own, View::Quiet), [RowKey::Worker(8)]);
        assert_eq!(
            keys(&own, View::Messages),
            [RowKey::Count(7), RowKey::Worker(8)]
        );
    }

    /// A worker's own transcript is the selected view over its own items, as
    /// the main transcript is over the main agent's: the same kinds show,
    /// fold and count in each view.
    #[test]
    fn a_workers_own_transcript_follows_the_selected_view() {
        let items_of = |lane: &str| {
            let mut result = item(5, Kind::Result, lane, "ok");
            result.call = Some(4);
            vec![
                item(1, Kind::User, lane, "use tabs"),
                item(2, Kind::Thinking, lane, "hmm"),
                item(3, Kind::Text, lane, "On it."),
                item(4, Kind::Call, lane, "cargo test"),
                result,
                start_call(6, lane, CallStatus::Ok, Some("r2")),
                worker_card(7, lane, "r2"),
                item(8, Kind::Call, lane, "cargo fmt"),
                item(9, Kind::Call, lane, "cargo build"),
                item(10, Kind::Call, lane, "cargo clippy"),
                item(11, Kind::Notice, lane, "slow").level(Level::Warn),
                item(12, Kind::Message, lane, "Found it."),
                item(13, Kind::Reply, lane, "Fixed."),
            ]
        };
        let own = |items| {
            let mut own = Model::new(AgentFilter::Worker("r1".into()));
            let snapshot = Snapshot {
                items,
                ..Snapshot::default()
            };
            own.load(snapshot, &mut Changes::default());
            own
        };
        let (worker, main) = (own(items_of("r1")), model(items_of("main")));
        for view in [View::Full, View::Quiet, View::Messages] {
            assert_eq!(keys(&worker, view), keys(&main, view), "{view:?}");
        }
        // And the kinds of each view are the ones the view is for.
        assert_eq!(
            keys(&worker, View::Messages),
            [
                RowKey::User(1),
                block(3),
                RowKey::Count(4),
                RowKey::Worker(7),
                RowKey::Notice(11),
                block(12),
                block(13),
                RowKey::Actions(13),
            ]
        );
        let quiet = keys(&worker, View::Quiet);
        assert!(!quiet.contains(&RowKey::Call(6)), "{quiet:?}");
        assert!(quiet.contains(&RowKey::Thinking(2)), "{quiet:?}");
    }

    /// A worker's message to the user shows in every view of the main
    /// transcript, and in the worker's own. A message the worker got and its
    /// text stay in its own transcript.
    #[test]
    fn a_workers_message_to_the_user_shows_in_every_view() {
        let mut from_lead = item(4, Kind::Message, "r1", "Check the tests too.");
        from_lead.from = Some(arko::remote::MAIN_AGENT.into());
        let items = vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::Text, "r1", "Looking."),
            item(3, Kind::Message, "r1", "Found it."),
            from_lead,
            item(5, Kind::Message, "main", "Done."),
        ];
        let main = model(items.clone());
        assert_eq!(ids(&main), [1, 3, 5]);
        for view in [View::Full, View::Quiet, View::Messages] {
            assert_eq!(
                keys(&main, view),
                [RowKey::User(1), block(3), block(5)],
                "{view:?}"
            );
        }
        let mut own = Model::new(AgentFilter::Worker("r1".into()));
        let snapshot = Snapshot {
            items: items.clone(),
            ..Snapshot::default()
        };
        own.load(snapshot, &mut Changes::default());
        assert_eq!(keys(&own, View::Messages), [block(2), block(3), block(4)]);
    }

    /// A start the Quiet view leaves out neither joins a row nor ends one,
    /// and starts alone leave their workers' cards.
    #[test]
    fn a_hidden_start_is_not_in_a_quiet_row() {
        let start = start_call(4, "main", CallStatus::Ok, Some("r1"));
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            read(2, "src/a.rs"),
            read(3, "src/b.rs"),
            start.clone(),
            read(5, "src/c.rs"),
        ]);
        let laid = rows(&model, &layout(View::Quiet));
        let shown: Vec<RowKey> = laid.rows.iter().map(|row| row.key).collect();
        assert_eq!(shown, [RowKey::User(1), RowKey::Group(2)]);
        assert_eq!(laid.quiet[&2].members, [2, 3, 5]);
        assert_eq!(keys(&model, View::Full).len(), 5);
        // Starts alone make no row at all, with their cards between.
        let starts = self::model(vec![
            item(1, Kind::User, "main", "go"),
            start_call(2, "main", CallStatus::Ok, Some("r1")),
            worker_card(3, "main", "r1"),
            start_call(4, "main", CallStatus::Ok, Some("r2")),
            worker_card(5, "main", "r2"),
        ]);
        assert_eq!(
            keys(&starts, View::Quiet),
            [RowKey::User(1), RowKey::Worker(3), RowKey::Worker(5)]
        );
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
        let keys = keys_of(&model, &from_two);
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
        let keys_of = |layout: &Layout| keys_of(&model, layout);
        // Open, a thought is its text.
        assert_eq!(keys_of(&expand), [block(1), block(2), RowKey::Actions(2)]);
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

    /// A note the agent hasn't read waits below what the agent goes on
    /// with, and doesn't break a run of calls it came in. Read, it goes and
    /// comes again with a new id where the agent read it.
    #[test]
    fn a_pending_note_waits_at_the_end_until_it_comes_again_read() {
        let mut note = item(3, Kind::User, "main", "Keep the old name.");
        note.delivery = Some(Delivery::Note);
        note.pending = true;
        let calls = (4..9).map(|id| read(id, &format!("src/{id}.rs")));
        let mut model = model(
            [
                item(1, Kind::User, "main", "go"),
                item(2, Kind::Text, "main", "Renaming it"),
                note.clone(),
            ]
            .into_iter()
            .chain(calls)
            .collect(),
        );
        assert_eq!(
            keys(&model, View::Full),
            [
                RowKey::User(1),
                block(2),
                RowKey::Call(4),
                RowKey::Call(5),
                RowKey::Call(6),
                RowKey::Call(7),
                RowKey::Call(8),
                RowKey::User(3),
            ]
        );
        assert_eq!(
            keys(&model, View::Quiet),
            [RowKey::User(1), block(2), RowKey::Group(4), RowKey::User(3)]
        );
        assert_eq!(
            keys(&model, View::Messages),
            [RowKey::User(1), block(2), RowKey::Count(4), RowKey::User(3)]
        );

        let mut changes = Changes::default();
        model.apply(Update::Remove { id: 3 }, &mut changes);
        note.id = 9;
        note.pending = false;
        model.apply(Update::Item(note), &mut changes);
        model.apply(
            Update::Item(item(10, Kind::Text, "main", "Kept it.")),
            &mut changes,
        );
        assert_eq!(changes.removed, [3]);
        assert_eq!(changes.added, [9, 10]);
        assert_eq!(
            keys(&model, View::Quiet),
            [
                RowKey::User(1),
                block(2),
                RowKey::Group(4),
                RowKey::User(9),
                block(10)
            ]
        );
    }

    /// Notes that came during a run of calls, after its first call, wait at
    /// the end in every view rather than going with the run.
    #[test]
    fn notes_that_came_during_a_run_of_calls_wait_at_the_end() {
        let note = |id: u64, text: &str| {
            let mut note = item(id, Kind::User, "main", text);
            note.delivery = Some(Delivery::Note);
            note.pending = true;
            note
        };
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::Call, "main", "read a.rs"),
            note(3, "Keep the old name."),
            item(4, Kind::Call, "main", "read b.rs"),
            note(5, "And the tests."),
        ]);
        assert_eq!(
            keys(&model, View::Full),
            [
                RowKey::User(1),
                RowKey::Call(2),
                RowKey::Call(4),
                RowKey::User(3),
                RowKey::User(5),
            ]
        );
        assert_eq!(
            keys(&model, View::Quiet),
            [
                RowKey::User(1),
                RowKey::Call(2),
                RowKey::Call(4),
                RowKey::User(3),
                RowKey::User(5),
            ]
        );
        assert_eq!(
            keys(&model, View::Messages),
            [
                RowKey::User(1),
                RowKey::Count(2),
                RowKey::User(3),
                RowKey::User(5),
            ]
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
            .rows
            .iter()
            .map(|row| row.gap)
            .collect();
        assert_eq!(
            gaps,
            [
                Gap::None,
                Gap::Small,
                Gap::Stack,
                Gap::Apart,
                Gap::Small,
                Gap::Small,
                Gap::Turn
            ]
        );
        // Anything open stands apart from its neighbours.
        let open = Layout {
            expanded: &|id| (id == 2).then_some(true),
            ..layout(View::Full)
        };
        let gaps: Vec<Gap> = rows(&model, &open).rows.iter().map(|row| row.gap).collect();
        // (The user's message keeps its actions' gap.)
        assert_eq!(gaps[1..4], [Gap::Small, Gap::Apart, Gap::Apart]);
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
    fn web_command_replies_preserve_both_origin_prefixes() {
        for origin in ["Changed from the web page.", "From the web page."] {
            let text = format!("{origin} The main agent keeps text replies on for this session.");
            let model = model(vec![item(1, Kind::Notice, "main", &text)]);
            assert_eq!(keys(&model, View::Full), [RowKey::Notice(1)]);
            assert_eq!(model.entry(1).unwrap().item.text, text);
        }
    }

    #[test]
    fn old_and_new_refresh_messages_are_one_quiet_row() {
        for text in [
            arko::remote::REFRESH_MESSAGE,
            "(Arko: this message only keeps the prompt cache warm. Reply with \"Acknowledged\" and nothing else.)",
        ] {
            let model = model(vec![
                item(1, Kind::User, "main", text),
                item(2, Kind::Reply, "main", "Acknowledged"),
            ]);
            assert_eq!(keys(&model, View::Full), [RowKey::Refresh(1)]);
        }
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
        model.apply(
            Update::Item(item(2, Kind::Text, "main", "so")),
            &mut changes,
        );
        assert!(!model.is_streaming(0));
    }

    /// What the user typed while the agent thinks, a worker's card and
    /// Arko's output go beside the thinking, which goes on.
    #[test]
    fn thinking_streams_on_past_what_goes_beside_it() {
        let mut note = item(2, Kind::User, "main", "Keep the old name.");
        note.delivery = Some(Delivery::Note);
        note.pending = true;
        let mut command = item(3, Kind::User, "main", "/model sonnet");
        command.delivery = Some(Delivery::Command);
        let mut model = model(vec![
            item(1, Kind::Thinking, "main", "hmm"),
            note,
            command,
            item(4, Kind::WorkerLine, "main", "Write the fuzz target"),
            item(5, Kind::Output, "main", "Model changed."),
        ]);
        let mut changes = Changes::default();
        model.set_activity(
            Activity {
                working: true,
                ..Activity::default()
            },
            &mut changes,
        );
        assert!(model.is_streaming(0));
    }

    fn working(model: &mut Model) {
        let mut changes = Changes::default();
        model.set_activity(
            Activity {
                working: true,
                ..Activity::default()
            },
            &mut changes,
        );
    }

    /// The agent waits on the model after the user's message and after a
    /// call's result, beside what the user typed and Arko said, and not
    /// while a call runs, thinking streams, a question waits or no turn runs.
    #[test]
    fn the_agent_waits_on_the_model_between_its_outputs() {
        let mut model = model(vec![item(1, Kind::User, "main", "go")]);
        assert_eq!(model.waiting(), Waiting::No);
        working(&mut model);
        assert_eq!(model.waiting(), Waiting::Yes(1));

        let mut changes = Changes::default();
        let mut call = item(2, Kind::Call, "main", "cargo build");
        call.status = Some(CallStatus::Running);
        model.apply(Update::Item(call.clone()), &mut changes);
        assert_eq!(model.waiting(), Waiting::No);
        call.status = Some(CallStatus::Ok);
        model.apply(Update::Change(call), &mut changes);
        assert_eq!(model.waiting(), Waiting::Yes(2));

        // A note, a worker's call and a notice leave it waiting.
        let mut note = item(3, Kind::User, "main", "Use release mode.");
        note.delivery = Some(Delivery::Note);
        note.pending = true;
        model.apply(Update::Item(note), &mut changes);
        let mut worker_call = item(4, Kind::Call, "r1", "read a.rs");
        worker_call.status = Some(CallStatus::Running);
        model.apply(Update::Item(worker_call), &mut changes);
        let retry = item(5, Kind::Notice, "main", "retrying in 5 s").level(Level::Warn);
        model.apply(Update::Item(retry), &mut changes);
        assert_eq!(model.waiting(), Waiting::Yes(2));

        model.apply(
            Update::Item(item(6, Kind::Thinking, "main", "hmm")),
            &mut changes,
        );
        assert_eq!(model.waiting(), Waiting::Yes(6));
        model.apply(
            Update::Item(item(7, Kind::Text, "main", "Built.")),
            &mut changes,
        );
        assert_eq!(model.waiting(), Waiting::AfterText(7));

        let mut asking = model.activity.clone();
        asking.question = Some(arko::remote::Question {
            id: 1,
            text: "Release or debug?".into(),
        });
        model.set_activity(asking, &mut changes);
        assert_eq!(model.waiting(), Waiting::No);
    }

    /// A cache refresh's short turn shows as its quiet row only.
    #[test]
    fn a_cache_refresh_waits_without_saying_so() {
        let mut refresh = item(1, Kind::User, "main", "(keep the cache warm)");
        refresh.delivery = Some(Delivery::Refresh);
        let mut model = model(vec![refresh]);
        working(&mut model);
        assert_eq!(model.waiting(), Waiting::No);
    }

    /// A worker waits on the model while it runs, and not while it waits
    /// for a place or once it ended.
    #[test]
    fn a_worker_waits_on_the_model_while_it_runs() {
        let mut model = Model::new(AgentFilter::Worker("r1".into()));
        let mut changes = Changes::default();
        let mut summary = WorkerSummary::new("r1");
        summary.title = "Fuzz".into();
        model.apply(Update::Worker(summary.clone()), &mut changes);
        assert_eq!(model.waiting(), Waiting::No);
        summary.state = WorkerState::Running;
        model.apply(Update::Worker(summary.clone()), &mut changes);
        assert_eq!(model.waiting(), Waiting::Yes(0));
        let mut call = item(3, Kind::Call, "r1", "read a.rs");
        call.status = Some(CallStatus::Ok);
        model.apply(Update::Item(call), &mut changes);
        assert_eq!(model.waiting(), Waiting::Yes(3));
        summary.state = WorkerState::Finished;
        model.apply(Update::Worker(summary), &mut changes);
        assert_eq!(model.waiting(), Waiting::No);
    }

    /// Thinking's row says "Thinking…" where the view shows it. Where the
    /// view hides it (Messages, a folded group of calls), "Working…" says it.
    #[test]
    fn working_shows_where_the_view_hides_the_thinking() {
        let done = |id: u64| {
            let mut call = item(id, Kind::Call, "main", "read a.rs");
            call.status = Some(CallStatus::Ok);
            call
        };
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            done(2),
            done(3),
            done(4),
            done(5),
            item(6, Kind::Thinking, "main", "hmm"),
        ]);
        let shown = |view: View| {
            let layout = Layout {
                working: Some(6),
                ..layout(view)
            };
            let rows = rows(&model, &layout).rows;
            (shows_working(&rows), rows.last().map(|row| row.key))
        };
        // The thought that streams is the live line where it shows.
        assert_eq!(shown(View::Full), (false, Some(RowKey::Thinking(6))));
        assert_eq!(shown(View::Quiet), (false, Some(RowKey::Thinking(6))));
        assert_eq!(shown(View::Messages), (true, Some(RowKey::Working(6))));
    }

    /// "Working…" goes below the agent's output, above the notes it hasn't
    /// read.
    #[test]
    fn working_goes_above_the_notes_not_read_yet() {
        let mut note = item(3, Kind::User, "main", "Use release mode.");
        note.delivery = Some(Delivery::Note);
        note.pending = true;
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            item(2, Kind::Call, "main", "cargo build"),
            note,
        ]);
        let layout = Layout {
            working: Some(2),
            ..layout(View::Full)
        };
        let keys = keys_of(&model, &layout);
        assert_eq!(
            keys,
            [
                RowKey::User(1),
                RowKey::Call(2),
                RowKey::Working(2),
                RowKey::User(3)
            ]
        );
    }

    /// A user's message of turn `turn` that started it, at `index` of the
    /// conversation.
    fn message(id: u64, turn: u64, index: usize, text: &str) -> Item {
        let mut item = item(id, Kind::User, "main", text);
        item.turn = Some(turn);
        item.message_index = Some(index);
        item.delivery = Some(Delivery::Turn);
        item
    }

    fn said(id: u64, kind: Kind, turn: u64, text: &str) -> Item {
        let mut item = item(id, kind, "main", text);
        item.turn = Some(turn);
        item
    }

    #[test]
    fn edits_count_the_messages_after_them_and_regenerate_the_last_reply() {
        let mut refresh = message(9, 4, 6, "(keep the cache warm)");
        refresh.delivery = Some(Delivery::Refresh);
        let mut note = item(5, Kind::User, "main", "and the tests");
        note.turn = Some(2);
        note.delivery = Some(Delivery::Note);
        let model = model(vec![
            message(1, 1, 0, "Fix the parser."),
            said(2, Kind::Text, 1, "Reading it."),
            said(3, Kind::Reply, 1, "Fixed."),
            message(4, 2, 2, "Now the docs."),
            note,
            said(6, Kind::Reply, 2, "Done."),
            message(7, 3, 4, "Thanks."),
            said(8, Kind::Reply, 3, "You're welcome."),
            refresh,
            said(10, Kind::Reply, 4, "Acknowledged"),
        ]);
        // After the first: its reply, two messages and a note, two replies.
        assert_eq!(model.messages_after(1), 6);
        assert_eq!(model.messages_after(7), 1);
        assert_eq!(model.last_message(), Some(7));
        assert_eq!(model.last_reply(), Some(8));
        assert!(Model::is_changeable(&model.entry(4).unwrap().item));
        assert!(
            !Model::is_changeable(&model.entry(5).unwrap().item),
            "a note"
        );
        assert!(
            !Model::is_changeable(&model.entry(9).unwrap().item),
            "a refresh"
        );
        assert_eq!(model.busy_reason(), None);
    }

    /// The rows ask for the latest reply as they draw: it is sought once
    /// for each change of the entries, and streamed text changes nothing.
    #[test]
    fn the_latest_reply_is_sought_once_for_each_change() {
        let mut model = model(vec![
            message(1, 1, 0, "Fix the parser."),
            said(2, Kind::Reply, 1, "Fixed."),
        ]);
        for _ in 0..10 {
            assert_eq!(model.last_reply(), Some(2));
        }
        assert_eq!(model.replies_sought.get(), 1);
        let mut changes = Changes::default();
        model.apply(
            Update::Append {
                id: 2,
                text: " All of it.".into(),
            },
            &mut changes,
        );
        assert_eq!(model.last_reply(), Some(2));
        assert_eq!(model.replies_sought.get(), 1);
        // A new message: no reply after it yet.
        model.apply(
            Update::Item(message(3, 2, 2, "Now the docs.")),
            &mut changes,
        );
        assert_eq!(model.last_reply(), None);
        model.apply(Update::Item(said(4, Kind::Reply, 2, "Done.")), &mut changes);
        assert_eq!(model.last_reply(), Some(4));
        // Cut back to before the message: the first reply is the latest.
        model.apply(Update::Truncate { from: 3 }, &mut changes);
        assert_eq!(model.last_reply(), Some(2));
        assert_eq!(model.replies_sought.get(), 4);
    }

    #[test]
    fn a_transcript_without_the_first_message_starts_partway() {
        let whole = model(vec![
            message(1, 1, 0, "Fix the parser."),
            said(2, Kind::Reply, 1, "Fixed."),
        ]);
        assert!(!whole.starts_partway());
        // The feed kept the latest items only.
        let cut = model(vec![
            said(5, Kind::Reply, 2, "…and the lexer."),
            message(7, 3, 4, "Thanks."),
        ]);
        assert!(cut.starts_partway());
        assert!(!model(Vec::new()).starts_partway());
    }

    #[test]
    fn turn_ids_keep_a_reply_whole_across_a_note_after_reopening() {
        let mut note = said(3, Kind::User, 1, "Also check the lexer.");
        note.delivery = Some(Delivery::Note);
        let model = model(vec![
            message(1, 1, 0, "Fix the parser."),
            said(2, Kind::Text, 1, "Reading it."),
            note,
            said(4, Kind::Reply, 1, "Fixed."),
            message(5, 2, 2, "Now the docs."),
            said(6, Kind::Reply, 2, "Done."),
        ]);
        assert_eq!(model.messages_after(1), 4);
    }

    /// Histories saved without turn IDs still count by place.
    #[test]
    fn a_restored_conversation_counts_as_it_did_live() {
        let restored = |id: u64, kind: Kind, text: &str| item(id, kind, "main", text);
        let mut first = restored(1, Kind::User, "Fix the parser.");
        first.message_index = Some(0);
        first.delivery = Some(Delivery::Turn);
        let mut second = restored(4, Kind::User, "Now the docs.");
        second.message_index = Some(2);
        second.delivery = Some(Delivery::Turn);
        let mut third = restored(7, Kind::User, "Thanks.");
        third.message_index = Some(4);
        third.delivery = Some(Delivery::Turn);
        let mut refresh = restored(9, Kind::User, "(keep the cache warm)");
        refresh.delivery = Some(Delivery::Refresh);
        let model = model(vec![
            first,
            restored(2, Kind::Text, "Reading it."),
            restored(3, Kind::Reply, "Fixed."),
            second,
            restored(6, Kind::Reply, "Done."),
            third,
            restored(8, Kind::Reply, "You're welcome."),
            refresh,
            restored(10, Kind::Reply, "Acknowledged"),
        ]);
        // After the first: its reply, two messages and their replies.
        assert_eq!(model.messages_after(1), 5);
        assert_eq!(model.messages_after(7), 1);
        assert_eq!(model.last_reply(), Some(8));
    }

    #[test]
    fn a_turn_or_a_worker_running_keeps_the_conversation_as_it_is() {
        let mut model = model(vec![message(1, 1, 0, "Go.")]);
        let mut changes = Changes::default();
        model.set_activity(
            Activity {
                working: true,
                ..Activity::default()
            },
            &mut changes,
        );
        assert_eq!(model.busy_reason(), Some("Wait for the turn to end"));
        model.set_activity(Activity::default(), &mut changes);
        model.apply(
            Update::Worker({
                let mut summary = WorkerSummary::new("r1");
                summary.title = "Fix it".into();
                summary.state = arko::remote::WorkerState::Running;
                summary.started_at = Some(0);
                summary.running_since = Some(0);
                summary
            }),
            &mut changes,
        );
        assert_eq!(
            model.busy_reason(),
            Some("Wait for the workers to finish, or stop them")
        );
        // No reply yet: nothing to regenerate.
        assert_eq!(model.last_reply(), None);
    }

    /// A Quiet row opens as the reader chose for it, apart from its calls:
    /// a call the reader opened inside it doesn't open it.
    #[test]
    fn a_rows_open_state_is_its_own() {
        let model = model(vec![
            item(1, Kind::User, "main", "go"),
            read(2, "src/a.rs"),
            read(3, "src/b.rs"),
        ]);
        let call_open = Layout {
            expanded: &|id| (id == 3).then_some(true),
            ..layout(View::Quiet)
        };
        assert_eq!(
            keys_of(&model, &call_open),
            [RowKey::User(1), RowKey::Group(2)]
        );
        let open = Layout {
            group_open: &|id| (id == 2).then_some(true),
            ..call_open
        };
        let laid = rows(&model, &open);
        let gaps: Vec<(RowKey, Gap)> = laid.rows.iter().map(|row| (row.key, row.gap)).collect();
        // The rail never starts with a gap, and what is open on it stands
        // apart.
        assert_eq!(
            gaps,
            [
                (RowKey::User(1), Gap::None),
                (RowKey::Group(2), Gap::Small),
                (RowKey::Call(2), Gap::Stack),
                (RowKey::Call(3), Gap::Apart),
            ]
        );
    }

    #[test]
    fn rows_change_in_runs_and_the_rest_stay() {
        let row = |key| Row {
            key,
            gap: Gap::None,
            rail: Rail::No,
        };
        let old = [
            row(RowKey::User(1)),
            row(RowKey::Call(2)),
            row(RowKey::Call(3)),
            row(RowKey::User(4)),
            row(RowKey::Call(5)),
            row(RowKey::User(6)),
        ];
        let new = [
            row(RowKey::User(1)),
            row(RowKey::Group(2)),
            row(RowKey::User(4)),
            row(RowKey::Call(5)),
            row(RowKey::Thinking(5)),
            row(RowKey::User(6)),
            row(RowKey::User(7)),
        ];
        let (changes, kept) = row_changes(&old, &new);
        assert_eq!(changes, [(1..3, 1), (5..5, 1), (6..6, 1)]);
        assert_eq!(kept, [(0, 0), (3, 2), (4, 3), (5, 5)]);
    }

    /// The rows after a change stay too, and rows that swap places between
    /// them keep the first one's place.
    #[test]
    fn rows_after_a_change_stay_and_swapped_rows_keep_one() {
        let row = |key| Row {
            key,
            gap: Gap::None,
            rail: Rail::No,
        };
        let old = [
            row(RowKey::User(1)),
            row(RowKey::Call(2)),
            row(RowKey::Call(3)),
            row(RowKey::Working(3)),
            row(RowKey::User(9)),
        ];
        let new = [
            row(RowKey::User(1)),
            row(RowKey::Call(3)),
            row(RowKey::Call(2)),
            row(RowKey::Call(4)),
            row(RowKey::Working(3)),
            row(RowKey::User(9)),
        ];
        let (changes, kept) = row_changes(&old, &new);
        assert_eq!(kept, [(0, 0), (1, 2), (3, 4), (4, 5)]);
        assert_eq!(changes, [(1..1, 1), (2..3, 1)]);
        // Nothing changed.
        let (changes, kept) = row_changes(&old, &old);
        assert!(changes.is_empty());
        assert_eq!(kept, (0..old.len()).map(|ix| (ix, ix)).collect::<Vec<_>>());
    }
}
