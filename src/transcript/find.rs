//! Find in the transcript (Mod+F): where the reader's words are in what the
//! rows show, collapsed rows included, highlighted, and moving between
//! them. Landing on a match opens what hides it (a thinking, a call's
//! details, a group of calls, a long code block), and closes it again when
//! the find moves on.
//!
//! The find ignores case and how accents are written ([`super::fold`]). It
//! keeps each entry's matches apart, and each entry's texts folded for as
//! long as it goes on. A change of the transcript searches only the entries
//! it changed, as it comes, and words that grow search only the entries
//! that matched the shorter ones. The texts of a long transcript are folded
//! off the UI thread the first time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use arko::remote::Kind;
use gpui_kit::base::text::{RangeHighlight, RenderedText};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use super::fold::{Folded, fold};
use super::model::{self, Entry, RowKey, Show};
use super::{CODE_LINES, TranscriptEvent, TranscriptView, row};
use crate::prefs::TranscriptView as View;

/// The bytes of text above which the first search of a find folds
/// the transcript's texts off the UI thread.
const FOLD_IN_BACKGROUND: usize = 256 * 1024;

/// Where in an entry a match is, in the order the entry shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Place {
    /// The entry's plain text: a message of the user's, a notice, an
    /// output, a call's line, a turn's footer, a worker card's title.
    Text,
    /// Why a call failed, beside its line.
    Outcome,
    /// Block `n` of the entry's Markdown, as it renders.
    Block(usize),
    /// Block `n` of the entry's Markdown, shown as its source.
    Source(usize),
    /// A call's parameters, as its details show them (all of them).
    Parameters,
    /// A call's output, as its details show them (all of it).
    Output,
    /// An entry's Markdown before its rows are ready: its source.
    Unready,
    /// Question `n` of a card of the agent's questions: its words, or what
    /// became of it under them in the record.
    Question(usize, QuestionPart),
    /// The answer to question `n` of a card, given in the answer panel, in
    /// the bubble under the card.
    Answer(usize),
}

/// A part of a question in the record of a card, in reading order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum QuestionPart {
    Words,
    /// What became of it, when that isn't the user's answer.
    Outcome,
}

/// One match.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Match {
    pub entry: u64,
    pub place: Place,
    /// Bytes of the place's text.
    pub range: Range<usize>,
    /// Which match of its place it is, in order.
    pub ordinal: usize,
}

/// What the find bar shows: the match the transcript is on, of how many.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FindStatus {
    /// The match shown, from 0.
    pub current: Option<usize>,
    pub total: usize,
    /// The texts are being made ready to search: the count comes then.
    pub searching: bool,
    /// The match shown is past what its row shows: in the whole output a
    /// call's "Show all N lines" opens.
    pub in_full_output: bool,
}

/// An entry's texts, as gathered on the UI thread. A Markdown entry's
/// rendered blocks are read out of its rendered text later, off the UI
/// thread for a long transcript's first search.
enum Gathered {
    Texts(Vec<(Place, String)>),
    Rendered {
        text: RenderedText,
        /// Each block's source bytes.
        blocks: Vec<(usize, Range<usize>)>,
    },
}

impl Gathered {
    /// About how many bytes it holds.
    fn size(&self) -> usize {
        match self {
            Self::Texts(texts) => texts.iter().map(|(_, text)| text.len()).sum(),
            Self::Rendered { blocks, .. } => blocks.iter().map(|(_, range)| range.len()).sum(),
        }
    }

    fn folded(self) -> Vec<(Place, Folded)> {
        match self {
            Self::Texts(texts) => texts
                .into_iter()
                .map(|(place, text)| (place, Folded::new(&text)))
                .collect(),
            Self::Rendered { text, blocks } => blocks
                .into_iter()
                .filter_map(|(block, source)| {
                    let range = text.range_for_source(source)?;
                    let block_text = text.as_str().get(range)?;
                    Some((Place::Block(block), Folded::new(block_text)))
                })
                .collect(),
        }
    }
}

/// Every place `needle` is in `haystack`, ignoring case and how accents
/// are written, as byte ranges of `haystack`, one after another.
#[cfg(test)]
pub(crate) fn find_all(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    Folded::new(haystack).find(&fold(needle))
}

/// The bytes at the start of a call's text that its details show as they
/// are: their first lines, up to the first one cut short.
pub(crate) fn shown_prefix(text: &str) -> usize {
    let mut end = 0;
    for (index, line) in text.split_inclusive('\n').enumerate() {
        if index == row::OUTPUT_LINES {
            break;
        }
        let content = line.strip_suffix('\n').unwrap_or(line);
        // The preview cuts long lines, and reads lines without their `\r`.
        if let Some((cut, _)) = content.char_indices().nth(row::PREVIEW_LINE_CHARS) {
            return end + cut;
        }
        if content.ends_with('\r') {
            return end + content.len() - 1;
        }
        end += line.len();
    }
    end.min(text.len())
}

/// The bytes of the code of each fenced code block in `source` (a block of
/// Markdown, which a list item or a quote may indent), without its fences.
fn code_regions(source: &str) -> Vec<Range<usize>> {
    let mut regions = Vec::new();
    // The open fence: its character, its length, and where the code starts.
    let mut open: Option<(char, usize, usize)> = None;
    let mut at = 0;
    for line in source.split_inclusive('\n') {
        // A list item's or a quote's marks come before the fence.
        let trimmed = line.trim_start_matches([' ', '\t', '>', '-', '*', '+', '.', ')']);
        let trimmed = trimmed.trim_start_matches(|c: char| c.is_ascii_digit());
        let trimmed = trimmed.trim_start_matches([' ', '.', ')']);
        let fence = ['`', '~'].into_iter().find_map(|mark| {
            let length = trimmed.chars().take_while(|c| *c == mark).count();
            (length >= 3).then_some((mark, length))
        });
        match (open, fence) {
            (None, Some((mark, length))) => open = Some((mark, length, at + line.len())),
            (Some((mark, length, start)), Some((closing, closing_length)))
                if closing == mark
                    && closing_length >= length
                    && trimmed[closing_length..].trim().is_empty() =>
            {
                regions.push(start..at);
                open = None;
            }
            _ => {}
        }
        at += line.len();
    }
    if let Some((_, _, start)) = open {
        regions.push(start..source.len());
    }
    regions
}

/// A match, by its place and its order there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    entry: u64,
    place: Place,
    ordinal: usize,
}

/// An entry's matches: each place's, in order.
#[derive(Default)]
struct Found {
    places: Vec<(Place, Vec<Range<usize>>)>,
    count: usize,
}

/// Something landing opened for the reader to see a match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opening {
    /// A call's details or a thinking's text.
    Entry(u64),
    /// A Quiet group of calls, by its first.
    Group(u64),
    /// A long code block, whole.
    Code(u64, usize),
}

/// What landing opened, and how it was before.
#[derive(Debug, Clone, Copy)]
struct Opened {
    what: Opening,
    before: Option<bool>,
}

/// A find going on.
#[derive(Default)]
pub(crate) struct Find {
    pub query: String,
    /// Identifies the scope snapshot sent to the background task.
    generation: u64,
    /// The words folded.
    needle: String,
    /// The entries with matches, in the transcript's order (their ids).
    found: BTreeMap<u64, Found>,
    total: usize,
    /// Each entry's texts, folded: kept while the find goes on, and
    /// made again when the entry changes.
    texts: HashMap<u64, Vec<(Place, Folded)>>,
    current: Option<Key>,
    /// The Markdown blocks whose views show highlights now.
    pub highlighted: HashSet<(u64, usize)>,
    /// Entries whose matches changed since the views were painted.
    touched: HashSet<u64>,
    /// The block whose view shows the current match when it draws next:
    /// set as the find lands, never as it searches again.
    reveal: Option<(u64, usize)>,
    /// The plain-text match to reveal when its actual line is laid out.
    plain_reveal: std::rc::Rc<plain::Reveal>,
    /// The entry the find lands on once its rows are ready, and which of
    /// its matches.
    pending: Option<(u64, usize)>,
    /// What landing opened, closed again as the find moves on.
    opened: Vec<Opened>,
    /// Folds the texts off the UI thread (a long transcript's first
    /// search).
    folding: Option<Task<()>>,
    /// Entries that changed while their texts were folded off the UI
    /// thread: theirs are made again.
    changed_meanwhile: HashSet<u64>,
}

impl Find {
    /// Drops the entry's matches.
    fn forget(&mut self, entry: u64) {
        if let Some(found) = self.found.remove(&entry) {
            self.total -= found.count;
            self.touched.insert(entry);
        }
    }

    /// Searches the entry's texts kept.
    fn search_kept(&mut self, entry: u64) {
        self.forget(entry);
        let Some(texts) = self.texts.get(&entry) else {
            return;
        };
        let mut found = Found::default();
        for (place, text) in texts {
            let ranges = text.find(&self.needle);
            if !ranges.is_empty() {
                found.count += ranges.len();
                found.places.push((*place, ranges));
            }
        }
        if found.count > 0 {
            self.total += found.count;
            self.found.insert(entry, found);
            self.touched.insert(entry);
        }
    }

    fn range(&self, key: Key) -> Option<Range<usize>> {
        let found = self.found.get(&key.entry)?;
        let (_, ranges) = found.places.iter().find(|(place, _)| *place == key.place)?;
        ranges.get(key.ordinal).cloned()
    }

    fn matched(&self, key: Key) -> Option<Match> {
        Some(Match {
            entry: key.entry,
            place: key.place,
            range: self.range(key)?,
            ordinal: key.ordinal,
        })
    }

    /// Where `key` is among all the matches, from 0.
    fn index_of(&self, key: Key) -> Option<usize> {
        let before: usize = self
            .found
            .range(..key.entry)
            .map(|(_, found)| found.count)
            .sum();
        let found = self.found.get(&key.entry)?;
        let mut index = before;
        for (place, ranges) in &found.places {
            if *place == key.place {
                return (key.ordinal < ranges.len()).then_some(index + key.ordinal);
            }
            index += ranges.len();
        }
        None
    }

    /// The match `n` of all, from 0.
    fn nth(&self, mut n: usize) -> Option<Key> {
        for (entry, found) in &self.found {
            if n >= found.count {
                n -= found.count;
                continue;
            }
            return Self::nth_in(*entry, found, n);
        }
        None
    }

    /// The match `n` of the entry's, from 0.
    fn nth_in(entry: u64, found: &Found, mut n: usize) -> Option<Key> {
        for (place, ranges) in &found.places {
            if n < ranges.len() {
                return Some(Key {
                    entry,
                    place: *place,
                    ordinal: n,
                });
            }
            n -= ranges.len();
        }
        None
    }

    /// Which match of its entry's `key` is, from 0.
    fn ordinal_in_entry(&self, key: Key) -> usize {
        let Some(found) = self.found.get(&key.entry) else {
            return 0;
        };
        found
            .places
            .iter()
            .take_while(|(place, _)| *place != key.place)
            .map(|(_, ranges)| ranges.len())
            .sum::<usize>()
            + key.ordinal
    }

    /// The first match at `at` (an entry, a place, a byte) or after it.
    fn first_from(&self, at: (u64, Place, usize)) -> Option<Key> {
        let (entry, place, start) = at;
        for (id, found) in self.found.range(entry..) {
            for (found_place, ranges) in &found.places {
                let ordinal = if *id > entry || *found_place > place {
                    Some(0)
                } else if *found_place == place {
                    ranges.iter().position(|range| range.start >= start)
                } else {
                    None
                };
                if let Some(ordinal) = ordinal {
                    return Some(Key {
                        entry: *id,
                        place: *found_place,
                        ordinal,
                    });
                }
            }
        }
        None
    }

    fn last(&self) -> Option<Key> {
        self.total.checked_sub(1).and_then(|n| self.nth(n))
    }

    /// The current match after a search again: the same, or the next one.
    fn settle_current(&mut self, anchor: Option<(u64, Place, usize)>) {
        let old = self.current;
        self.current = anchor.and_then(|anchor| self.first_from(anchor));
        if old != self.current {
            for key in [old, self.current].into_iter().flatten() {
                self.touched.insert(key.entry);
            }
        }
    }

    /// Where the current match is: its entry, place and first byte.
    fn current_at(&self) -> Option<(u64, Place, usize)> {
        let key = self.current?;
        Some((key.entry, key.place, self.range(key)?.start))
    }
}

impl TranscriptView {
    /// Finds `query` (ignoring case) in what the transcript shows, and goes
    /// to a match: the one it was on while the words grow, else the latest
    /// one. Empty words end the find.
    pub fn find(&mut self, query: &str, cx: &mut Context<Self>) -> FindStatus {
        if query.trim().is_empty() {
            self.clear_find(cx);
            return FindStatus::default();
        }
        let needle = fold(query);
        let mut find = self.find.take().unwrap_or_default();
        let anchor = find.current_at();
        // Longer words match only where the shorter ones did.
        let narrowing = !find.needle.is_empty()
            && find.folding.is_none()
            && needle.contains(find.needle.as_str());
        find.query = query.to_string();
        find.needle = needle;
        find.current = None;
        find.pending = None;
        if narrowing {
            let candidates: Vec<u64> = find.found.keys().copied().collect();
            for entry in candidates {
                find.search_kept(entry);
            }
        } else if find.folding.is_none() && !self.fold_in_background(&mut find, cx) {
            self.search_all(&mut find, cx);
        }
        if find.folding.is_none() {
            find.current = anchor
                .and_then(|anchor| find.first_from(anchor))
                .or_else(|| find.last());
        }
        self.find = Some(find);
        self.land(cx);
        self.find_status()
    }

    /// Goes to the next match (`forward`, down the transcript) or the
    /// previous one, around at the ends.
    pub fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) -> FindStatus {
        let Some(find) = self.find.as_mut() else {
            return FindStatus::default();
        };
        if find.folding.is_some() {
            return self.find_status();
        }
        let total = find.total;
        let index = find.current.and_then(|key| find.index_of(key));
        let next = match (index, forward) {
            _ if total == 0 => None,
            (None, true) => Some(0),
            (None, false) => Some(total - 1),
            (Some(current), true) => Some((current + 1) % total),
            (Some(current), false) => Some((current + total - 1) % total),
        };
        let old = find.current;
        find.current = next.and_then(|n| find.nth(n));
        for key in [old, find.current].into_iter().flatten() {
            find.touched.insert(key.entry);
        }
        find.pending = None;
        self.land(cx);
        self.find_status()
    }

    /// Where the find is.
    pub fn find_status(&self) -> FindStatus {
        let Some(find) = &self.find else {
            return FindStatus::default();
        };
        FindStatus {
            current: find.current.and_then(|key| find.index_of(key)),
            total: find.total,
            searching: find.folding.is_some(),
            in_full_output: self.current_in_full_output(),
        }
    }

    /// Ends the find: the highlights go. What landing opened for the match
    /// on show stays open, for the reader is at it.
    pub fn clear_find(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.take() else {
            return;
        };
        for (entry, block) in find.highlighted {
            if let Some(view) = self.existing_block_view(entry, block) {
                view.update(cx, |view, cx| view.clear_range_highlights(cx));
            }
        }
        cx.emit(TranscriptEvent::FindStatus(FindStatus::default()));
        cx.notify();
    }

    pub(super) fn current_match(&self) -> Option<Match> {
        let find = self.find.as_ref()?;
        find.matched(find.current?)
    }

    /// The entries `changed` changed (new ones too) and `removed` went,
    /// while a find goes on: it searches them again now, keeping its place.
    pub(super) fn find_changed(
        &mut self,
        removed: &[u64],
        changed: impl IntoIterator<Item = u64>,
        cx: &mut Context<Self>,
    ) {
        let Some(mut find) = self.find.take() else {
            return;
        };
        let anchor = find.current_at();
        for id in removed {
            find.texts.remove(id);
            find.forget(*id);
        }
        let mut searched = false;
        for id in changed.into_iter().collect::<HashSet<_>>() {
            find.texts.remove(&id);
            if find.folding.is_some() {
                find.changed_meanwhile.insert(id);
                continue;
            }
            self.search_entry(&mut find, id, cx);
            searched = true;
        }
        if !searched && removed.is_empty() {
            self.find = Some(find);
            return;
        }
        find.settle_current(anchor);
        self.find = Some(find);
        if !self.land_pending(cx) {
            self.highlight(cx);
            cx.emit(TranscriptEvent::FindStatus(self.find_status()));
        }
        cx.notify();
    }

    /// Everything the find covers changed (the view, Markdown on or off, a
    /// new history): it searches all again, keeping its place.
    pub(super) fn find_changed_all(&mut self, cx: &mut Context<Self>) {
        let Some(mut find) = self.find.take() else {
            return;
        };
        let anchor = find.current_at();
        find.texts.clear();
        find.generation += 1;
        find.folding = None;
        if !self.fold_in_background(&mut find, cx) {
            self.search_all(&mut find, cx);
        }
        find.settle_current(anchor);
        self.find = Some(find);
        self.highlight(cx);
        cx.emit(TranscriptEvent::FindStatus(self.find_status()));
        cx.notify();
    }

    /// Searches every entry the find covers.
    fn search_all(&self, find: &mut Find, cx: &App) {
        find.found.clear();
        find.total = 0;
        let ids: Vec<u64> = self
            .model
            .entries()
            .iter()
            .map(|entry| entry.item.id)
            .collect();
        find.texts.retain(|id, _| self.model.entry(*id).is_some());
        for id in ids {
            self.search_entry(find, id, cx);
        }
        find.touched
            .extend(find.highlighted.iter().map(|(entry, _)| *entry));
    }

    /// Searches the entry `id`: its texts kept, else made now.
    fn search_entry(&self, find: &mut Find, id: u64, cx: &App) {
        if !self
            .model
            .entry(id)
            .is_some_and(|entry| self.finds_in(entry))
        {
            find.texts.insert(id, Vec::new());
        }
        find.texts.entry(id).or_insert_with(|| {
            self.model
                .entry(id)
                .filter(|entry| self.finds_in(entry))
                .map(|entry| self.gather(entry, cx).folded())
                .unwrap_or_default()
        });
        find.search_kept(id);
    }

    /// Whether the find covers `entry`: what the view shows of it.
    fn finds_in(&self, entry: &Entry) -> bool {
        let filter = self.model.filter();
        let view = match filter {
            super::AgentFilter::Worker(_) => View::Full,
            _ => self.settings.view,
        };
        model::show(filter, view, &entry.item) == Show::Item && !model::is_refresh(&entry.item)
    }

    /// Starts folding the texts of the entries not searched yet off the
    /// UI thread, when there are many: the find searches once they're
    /// ready. Returns whether it started.
    fn fold_in_background(&self, find: &mut Find, cx: &mut Context<Self>) -> bool {
        let mut texts: Vec<(u64, Gathered)> = Vec::new();
        let mut bytes = 0;
        for entry in self.model.entries() {
            let id = entry.item.id;
            if find.texts.contains_key(&id) || !self.finds_in(entry) {
                continue;
            }
            let gathered = self.gather(entry, cx);
            bytes += gathered.size();
            texts.push((id, gathered));
        }
        if bytes < FOLD_IN_BACKGROUND {
            for (id, gathered) in texts {
                find.texts.insert(id, gathered.folded());
            }
            return false;
        }
        find.found.clear();
        find.total = 0;
        find.changed_meanwhile.clear();
        let folding = cx.background_spawn(async move {
            texts
                .into_iter()
                .map(|(id, gathered)| (id, gathered.folded()))
                .collect::<Vec<_>>()
        });
        let generation = find.generation;
        find.folding = Some(cx.spawn(async move |this, cx| {
            let folded = folding.await;
            this.update(cx, |this, cx| this.folded(generation, folded, cx))
                .ok();
        }));
        true
    }

    /// The texts folded off the UI thread are ready: the find searches.
    fn folded(
        &mut self,
        generation: u64,
        folded: Vec<(u64, Vec<(Place, Folded)>)>,
        cx: &mut Context<Self>,
    ) {
        if self
            .find
            .as_ref()
            .is_none_or(|find| find.generation != generation)
        {
            return;
        }
        let Some(mut find) = self.find.take() else {
            return;
        };
        find.folding = None;
        let changed = std::mem::take(&mut find.changed_meanwhile);
        for (id, texts) in folded {
            if !changed.contains(&id) && self.model.entry(id).is_some() {
                find.texts.insert(id, texts);
            }
        }
        self.search_all(&mut find, cx);
        find.current = find.last();
        self.find = Some(find);
        self.land(cx);
        cx.notify();
    }

    /// The texts of an entry the reader can find words in, as its rows
    /// show them.
    fn gather(&self, entry: &Entry, cx: &App) -> Gathered {
        let item = &entry.item;
        let texts = match item.kind {
            Kind::User | Kind::Notice | Kind::Output | Kind::Turn => {
                vec![(Place::Text, item.text.clone())]
            }
            Kind::WorkerLine => {
                // The card's title: the worker's task, as its summary says.
                let title = item
                    .worker
                    .as_deref()
                    .and_then(|worker| self.model.worker(worker))
                    .map(|summary| summary.title.clone())
                    .filter(|title| !title.is_empty())
                    .unwrap_or_else(|| item.text.clone());
                vec![(Place::Text, title)]
            }
            // The record's questions, each with what became of it, then the
            // answers given in the answer panel, in the bubble under it. The
            // open ones are the panel's.
            Kind::Question if super::model::has_questions(item) => {
                let questions = item.questions.iter().flatten().enumerate();
                let answers = questions.clone().filter_map(|(n, question)| {
                    let answer = crate::questions::answer(&question.state)?;
                    Some((Place::Answer(n), answer.to_string()))
                });
                questions
                    .filter(|(_, question)| !question.state.is_open())
                    .flat_map(|(n, question)| {
                        let words = crate::questions::text(question);
                        let outcome = crate::questions::record_outcome(&question.state);
                        [
                            Some((Place::Question(n, QuestionPart::Words), words)),
                            outcome.map(|outcome| {
                                (Place::Question(n, QuestionPart::Outcome), outcome)
                            }),
                        ]
                    })
                    .flatten()
                    .chain(answers)
                    .collect()
            }
            Kind::Text | Kind::Reply | Kind::Message | Kind::Question | Kind::Thinking => {
                let doc = self
                    .ui
                    .get(&item.id)
                    .and_then(|ui| ui.doc.as_ref())
                    .filter(|doc| !doc.blocks.is_empty());
                let Some(doc) = doc else {
                    // Not ready to show yet: its source stands for it.
                    return Gathered::Texts(if item.text.trim().is_empty() {
                        Vec::new()
                    } else {
                        vec![(Place::Unready, item.text.clone())]
                    });
                };
                let source = !self.settings.render_markdown || self.shows_source(item.id);
                if !source {
                    return Gathered::Rendered {
                        text: doc.state.read(cx).rendered_text(),
                        blocks: doc
                            .blocks
                            .iter()
                            .enumerate()
                            .filter_map(|(block, row)| Some((block, row.range.clone()?)))
                            .collect(),
                    };
                }
                doc.blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(block, row)| {
                        let text = doc.text.get(row.range.clone()?)?.trim_end().to_string();
                        Some((Place::Source(block), text))
                    })
                    .collect()
            }
            Kind::Call => {
                let mut texts = vec![(
                    Place::Text,
                    item.text.lines().next().unwrap_or_default().to_string(),
                )];
                if item.status == Some(arko::remote::CallStatus::Failed)
                    && let Some(outcome) = &item.outcome
                {
                    texts.push((Place::Outcome, outcome.clone()));
                }
                if let Some(parameters) = row::shown_parameters(item) {
                    texts.push((Place::Parameters, parameters.to_string()));
                }
                if let Some(output) = row::shown_output(entry) {
                    texts.push((Place::Output, output.to_string()));
                }
                texts
            }
            _ => Vec::new(),
        };
        Gathered::Texts(texts)
    }

    /// Whether the current match is past what its call's details show.
    fn current_in_full_output(&self) -> bool {
        let Some(found) = self.current_match() else {
            return false;
        };
        self.full_text(&found)
            .is_some_and(|text| found.range.end > shown_prefix(&text))
    }

    /// The whole text of a call's parameters or output `found` is in.
    fn full_text(&self, found: &Match) -> Option<std::sync::Arc<str>> {
        let entry = self.model.entry(found.entry)?;
        match found.place {
            Place::Parameters => row::shown_parameters(&entry.item),
            Place::Output => row::shown_output(entry),
            _ => None,
        }
    }

    /// Opens the whole output (or parameters) at the current match, when
    /// its row's first lines don't show it.
    pub fn show_match_in_full(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(found) = self.current_match() else {
            return;
        };
        let Some(text) = self.full_text(&found) else {
            return;
        };
        let line = self
            .model
            .entry(found.entry)
            .map(|entry| entry.item.text.trim().to_string())
            .unwrap_or_default();
        let title = match found.place {
            Place::Parameters => format!("Parameters of {line}"),
            _ => format!("Output of {line}"),
        };
        row::open_output_at(title.into(), text, Some(found.range), window, cx);
    }

    /// Opens what hides the current match, scrolls to it if it isn't on
    /// screen, and highlights. What the last landing opened closes again,
    /// unless this one needs it.
    fn land(&mut self, cx: &mut Context<Self>) {
        let found = self.current_match();
        let needs = found
            .as_ref()
            .map(|found| self.openings_for(found, cx))
            .unwrap_or_default();
        let opened = self
            .find
            .as_mut()
            .map(|find| std::mem::take(&mut find.opened))
            .unwrap_or_default();
        let mut kept = Vec::new();
        for opened in opened {
            if needs.contains(&opened.what) {
                kept.push(opened);
            } else {
                self.close_opening(opened, cx);
            }
        }
        for what in needs {
            if !kept.iter().any(|opened| opened.what == what)
                && let Some(opened) = self.open(what, cx)
            {
                kept.push(opened);
            }
        }
        if let Some(find) = self.find.as_mut() {
            find.opened = kept;
        }
        if let Some(found) = found {
            self.go_to(&found, cx);
        }
        self.highlight(cx);
        cx.emit(TranscriptEvent::FindStatus(self.find_status()));
        cx.notify();
    }

    /// Scrolls to `found`, by as little as it takes, and has its block's
    /// view show its line when it draws.
    fn go_to(&mut self, found: &Match, cx: &mut Context<Self>) {
        if let Some(find) = self.find.as_mut() {
            find.plain_reveal.set(None);
        }
        // An older entry not ready to show: it is made ready, and the find
        // lands once its rows are.
        if self.loaded_from.is_some_and(|from| found.entry < from) {
            self.load_from(found.entry, cx);
        }
        if found.place == Place::Unready {
            if let Some(find) = self.find.as_mut()
                && let Some(key) = find.current
            {
                find.pending = Some((found.entry, find.ordinal_in_entry(key)));
            }
            return;
        }
        let row = match found.place {
            Place::Block(block) | Place::Source(block) => self.rows.iter().position(|row| {
                matches!(row.key, RowKey::Block { id, block: b, .. }
                        if id == found.entry && b == block)
            }),
            _ => self.row_of(found.entry),
        };
        let Some(ix) = row else {
            return;
        };
        match found.place {
            Place::Block(block) => {
                if !self.list.on_screen().contains(&ix) {
                    self.list.reveal(ix);
                }
                if let Some(find) = self.find.as_mut() {
                    find.reveal = Some((found.entry, block));
                    find.touched.insert(found.entry);
                }
            }
            // Plain text scrolls to the line the match is laid out on,
            // once its row draws (see `FoundText`).
            _ => {
                self.list.hold_place(ix);
                if !self.list.on_screen().contains(&ix) {
                    self.list.reveal(ix);
                }
                if let Some(find) = self.find.as_mut() {
                    find.plain_reveal.set(find.current);
                }
            }
        }
    }

    /// The entry the find waits for is ready: it lands there. Returns
    /// whether it did.
    fn land_pending(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(find) = self.find.as_mut() else {
            return false;
        };
        let Some((entry, ordinal)) = find.pending else {
            return false;
        };
        let Some(found) = find.found.get(&entry) else {
            return false;
        };
        if found
            .places
            .iter()
            .any(|(place, _)| *place == Place::Unready)
        {
            return false;
        }
        let ordinal = ordinal.min(found.count.saturating_sub(1));
        find.current = Find::nth_in(entry, found, ordinal);
        find.pending = None;
        self.land(cx);
        true
    }

    /// Makes the entries from `id` on ready to show at once (a find lands
    /// on an older entry).
    fn load_from(&mut self, id: u64, cx: &mut Context<Self>) {
        let Some(from) = self.loaded_from else {
            return;
        };
        let entries = self.model.entries();
        let start = self.model.position(id).unwrap_or_else(|ix| ix);
        let end = self.model.position(from).unwrap_or_else(|ix| ix);
        let ids: Vec<u64> = entries[start..end].iter().map(|e| e.item.id).collect();
        self.loaded_from = entries.get(start).map(|e| e.item.id).filter(|_| start > 0);
        for id in ids {
            if !self.ui.contains_key(&id) {
                self.add_ui(id, cx);
            }
        }
        self.refresh_rows(cx);
    }

    /// What has to open for the reader to see `found`: a folded group of
    /// calls, a thinking or a call's details, a long code block.
    fn openings_for(&self, found: &Match, cx: &App) -> Vec<Opening> {
        let Some(entry) = self.model.entry(found.entry) else {
            return Vec::new();
        };
        let mut needs = Vec::new();
        // A folded group of calls (Quiet) opens first.
        if self.settings.view == View::Quiet {
            let position = self.model.position(found.entry).ok();
            let group = self.rows.iter().find_map(|row| match row.key {
                RowKey::Group(first) => model::run_of(&self.model, first, View::Quiet)
                    .contains(&position?)
                    .then_some(first),
                _ => None,
            });
            if let Some(first) = group {
                needs.push(Opening::Group(first));
            }
        }
        match (entry.item.kind, found.place) {
            (Kind::Thinking, _) | (Kind::Call, Place::Parameters | Place::Output) => {
                needs.push(Opening::Entry(found.entry))
            }
            _ => {}
        }
        if let Place::Block(block) = found.place
            && self.hides_in_code(found, block, cx)
        {
            needs.push(Opening::Code(found.entry, block));
        }
        needs
    }

    /// Whether a long code block of block `block` hides `found`: the block
    /// shows the first [`CODE_LINES`] lines of each of its code blocks (at
    /// the top level, or in a list item or a quote), and the match is in
    /// one, past them.
    fn hides_in_code(&self, found: &Match, block: usize, cx: &App) -> bool {
        let Some(doc) = self.ui.get(&found.entry).and_then(|ui| ui.doc.as_ref()) else {
            return false;
        };
        let Some(row) = doc.blocks.get(block) else {
            return false;
        };
        let Some(source) = row.range.clone() else {
            return false;
        };
        if row.code_lines <= CODE_LINES {
            return false;
        }
        let rendered = doc.state.read(cx).rendered_text();
        let Some(start) = rendered
            .range_for_source(source.clone())
            .map(|range| range.start)
        else {
            return false;
        };
        let at = start + found.range.start;
        let text = doc.text.get(source.clone()).unwrap_or_default();
        code_regions(text).into_iter().any(|code| {
            let code = source.start + code.start..source.start + code.end;
            rendered.range_for_source(code).is_some_and(|code| {
                code.contains(&at)
                    && rendered
                        .as_str()
                        .get(code.start..at)
                        .is_some_and(|before| before.matches('\n').count() >= CODE_LINES)
            })
        })
    }

    /// Opens `what`, if it isn't open. Returns what it opened.
    fn open(&mut self, what: Opening, cx: &mut Context<Self>) -> Option<Opened> {
        match what {
            Opening::Group(first) => {
                if self.group_open(first) {
                    return None;
                }
                let before = self.ui.get(&first).and_then(|ui| ui.group_open);
                self.ui.entry(first).or_default().group_open = Some(true);
                self.refresh_rows_after_opening(cx);
                Some(Opened { what, before })
            }
            Opening::Entry(id) => {
                let before = self.ui.get(&id).and_then(|ui| ui.expanded);
                let default = self
                    .model
                    .entry(id)
                    .is_some_and(|entry| entry.item.kind == Kind::Thinking)
                    && self.thinking_open_by_default();
                if before.unwrap_or(default) {
                    return None;
                }
                self.set_expanded(id, true, cx);
                Some(Opened { what, before })
            }
            Opening::Code(id, block) => {
                if self
                    .ui
                    .get(&id)
                    .is_some_and(|ui| ui.uncapped.contains(&block))
                {
                    return None;
                }
                self.toggle_code(id, block, cx);
                Some(Opened { what, before: None })
            }
        }
    }

    /// Closes what landing opened, unless the reader changed it since.
    fn close_opening(&mut self, opened: Opened, cx: &mut Context<Self>) {
        match opened.what {
            Opening::Group(first) => {
                let Some(ui) = self.ui.get_mut(&first) else {
                    return;
                };
                if ui.group_open == Some(true) {
                    ui.group_open = opened.before;
                    self.refresh_rows(cx);
                }
            }
            Opening::Entry(id) => {
                if self.ui.get(&id).and_then(|ui| ui.expanded) == Some(true) {
                    if let Some(ix) = self.rows.iter().position(|row| {
                        matches!(row.key, RowKey::Call(entry) | RowKey::Thinking(entry) if entry == id)
                    }) {
                        self.list.remeasure_row(ix);
                    }
                    if let Some(ui) = self.ui.get_mut(&id) {
                        ui.expanded = opened.before;
                    }
                    self.refresh_rows(cx);
                }
            }
            Opening::Code(id, block) => {
                if self
                    .ui
                    .get(&id)
                    .is_some_and(|ui| ui.uncapped.contains(&block))
                {
                    self.toggle_code(id, block, cx);
                }
            }
        }
    }

    /// The view of a block, if its row was drawn.
    fn existing_block_view(
        &self,
        entry: u64,
        block: usize,
    ) -> Option<Entity<gpui_kit::component::text::TextViewState>> {
        self.ui
            .get(&entry)?
            .doc
            .as_ref()?
            .blocks
            .get(block)?
            .view
            .clone()
    }

    /// Paints the matches on the block views drawn whose matches changed,
    /// and clears those whose matches went.
    fn highlight(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        let touched = std::mem::take(&mut find.touched);
        let mut now: HashSet<(u64, usize)> = find
            .highlighted
            .iter()
            .filter(|(entry, _)| !touched.contains(entry))
            .copied()
            .collect();
        for entry in &touched {
            if let Some(found) = find.found.get(entry) {
                for (place, _) in &found.places {
                    if let Place::Block(block) = place {
                        now.insert((*entry, *block));
                    }
                }
            }
        }
        let before = std::mem::take(&mut find.highlighted);
        for (entry, block) in before.difference(&now) {
            if let Some(view) = self.existing_block_view(*entry, *block) {
                view.update(cx, |view, cx| view.clear_range_highlights(cx));
            }
        }
        for (entry, block) in &now {
            if !touched.contains(entry) {
                continue;
            }
            if let Some(view) = self.existing_block_view(*entry, *block) {
                self.highlight_block(*entry, *block, &view, cx);
            }
        }
        if let Some(find) = self.find.as_mut() {
            find.highlighted = now;
        }
    }

    /// Paints the matches of block `block` of the entry `entry` on its
    /// view, the current one stronger, and shows the current match's line
    /// if the find just landed there. The view's own text is searched, so
    /// the ranges are its own.
    pub(super) fn highlight_block(
        &mut self,
        entry: u64,
        block: usize,
        view: &Entity<gpui_kit::component::text::TextViewState>,
        cx: &mut App,
    ) {
        let Some(find) = &mut self.find else {
            return;
        };
        let Some(found) = find.found.get(&entry) else {
            return;
        };
        if !found
            .places
            .iter()
            .any(|(place, _)| *place == Place::Block(block))
        {
            return;
        }
        let current = find
            .current
            .filter(|key| key.entry == entry && key.place == Place::Block(block))
            .map(|key| key.ordinal);
        let reveal = current.is_some() && find.reveal == Some((entry, block));
        if reveal {
            find.reveal = None;
        }
        let (all, strong) = match_colors(cx);
        let needle = find.needle.clone();
        view.update(cx, |view, cx| {
            let text = view.rendered_text();
            let ranges = Folded::new(text.as_str()).find(&needle);
            let highlights = ranges.iter().enumerate().map(|(ordinal, range)| {
                let color = if Some(ordinal) == current {
                    strong
                } else {
                    all
                };
                RangeHighlight::new(range.clone(), color)
            });
            let _ = view.set_range_highlights(highlights, cx);
            if reveal && let Some(range) = current.and_then(|ordinal| ranges.get(ordinal)) {
                let _ = view.reveal_range(range.clone(), cx);
            }
        });
    }

    /// The matches in the text of `place` of the entry `entry`, and which
    /// is the current one, for a row of plain text. A call's parameters
    /// and output show their first lines: the matches past them aren't.
    pub(super) fn match_ranges(&self, entry: u64, place: Place) -> Vec<(Range<usize>, bool)> {
        let Some(find) = &self.find else {
            return Vec::new();
        };
        let Some(found) = find.found.get(&entry) else {
            return Vec::new();
        };
        let Some((_, ranges)) = found.places.iter().find(|(found, _)| *found == place) else {
            return Vec::new();
        };
        let shown = match place {
            Place::Parameters | Place::Output => self
                .model
                .entry(entry)
                .and_then(|item| match place {
                    Place::Parameters => row::shown_parameters(&item.item),
                    _ => row::shown_output(item),
                })
                .map_or(0, |text| shown_prefix(&text)),
            _ => usize::MAX,
        };
        let current = find
            .current
            .filter(|key| key.entry == entry && key.place == place)
            .map(|key| key.ordinal);
        ranges
            .iter()
            .enumerate()
            .filter(|(_, range)| range.end <= shown)
            .map(|(ordinal, range)| (range.clone(), Some(ordinal) == current))
            .collect()
    }

    /// `text` as the plain text of `place` in a row, selectable as usual,
    /// with its matches painted on it. The row draws it in `ink`.
    pub(super) fn marked(
        &self,
        id: impl Into<ElementId>,
        entry: u64,
        place: Place,
        text: impl Into<SharedString>,
        ink: Ink,
        cx: &App,
    ) -> AnyElement {
        let text = text.into();
        let highlights = self.highlights(entry, place, &text, ink, cx);
        let selectable = self.selectable(id, entry, place, text);
        if highlights.is_empty() {
            return selectable.into_any_element();
        }
        self.found(entry, place, selectable.highlights(highlights))
    }

    /// `text` with the matches of `place` painted on it (for text that
    /// isn't selectable, such as a call's line in its button).
    pub(super) fn marked_plain(
        &self,
        entry: u64,
        place: Place,
        text: impl Into<SharedString>,
        ink: Ink,
        cx: &App,
    ) -> AnyElement {
        let text = text.into();
        let highlights = self.highlights(entry, place, &text, ink, cx);
        if highlights.is_empty() {
            return text.into_any_element();
        }
        self.found(
            entry,
            place,
            StyledText::new(text).with_highlights(highlights),
        )
    }

    /// The matches of `place` in `text`, drawn in `ink`, painted with the
    /// current one stronger, their text in a shade of its color that reads
    /// on their highlight.
    fn highlights(
        &self,
        entry: u64,
        place: Place,
        text: &str,
        ink: Ink,
        cx: &App,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let ranges = self.match_ranges(entry, place);
        if ranges.is_empty() {
            return Vec::new();
        }
        let (all, strong) = match_colors(cx);
        let (on_all, on_strong) = (readable_on(ink, all, cx), readable_on(ink, strong, cx));
        ranges
            .into_iter()
            .filter(|(range, _)| text.get(range.clone()).is_some())
            .map(|(range, current)| {
                (
                    range,
                    HighlightStyle {
                        color: Some(if current { on_strong } else { on_all }),
                        background_color: Some(if current { strong } else { all }),
                        ..HighlightStyle::default()
                    },
                )
            })
            .collect()
    }

    /// `text`, which shows `place`'s matches, and which scrolls the
    /// transcript to the current one as the find lands on it.
    fn found<T: plain::Lines>(&self, entry: u64, place: Place, text: T) -> AnyElement {
        if let Some(find) = &self.find
            && let Some(key) = find
                .current
                .filter(|key| key.entry == entry && key.place == place)
            && let Some(found) = self.current_match()
        {
            return plain::FoundText::new(
                text,
                key,
                found.range.start,
                find.plain_reveal.clone(),
                self.list.handle(),
            )
            .into_any_element();
        }
        text.into_any_element()
    }
}

/// The background of matches, and of the current one.
pub(super) fn match_colors(cx: &App) -> (Hsla, Hsla) {
    let theme = cx.theme();
    (theme.search_match, theme.search_match_active)
}

/// The contrast matched text needs on its highlight, WCAG's for body text.
pub(super) const MATCH_CONTRAST: f32 = 4.5;

/// The colors a plain text of a row is drawn in: its own, and what it's
/// drawn on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Ink {
    pub text: Hsla,
    /// A surface of its own, such as a bubble's. `None` for the
    /// transcript's, which is the window's background in a session and the
    /// sidebar's in the workers panel.
    pub surface: Option<Hsla>,
}

impl Ink {
    /// `text` on the transcript.
    pub fn new(text: Hsla) -> Self {
        Self {
            text,
            surface: None,
        }
    }

    /// `text` on `surface`, such as a bubble or a call's details.
    pub fn on(text: Hsla, surface: Hsla) -> Self {
        Self {
            text,
            surface: Some(surface),
        }
    }

    /// What it may be drawn on.
    fn surfaces(&self, cx: &App) -> Vec<Hsla> {
        let theme = cx.theme();
        match self.surface {
            Some(surface) => vec![surface],
            None => vec![theme.background, theme.sidebar],
        }
    }
}

/// The text of `ink`'s matches on the highlight `highlight`: its color as
/// it is while it reads there at [`MATCH_CONTRAST`], else darkened (or, on
/// a dark highlight, lightened) until it does. Plain text is too light on
/// the current match's highlight in most dark themes, and a warning's amber
/// on both highlights in light ones.
pub(super) fn readable_on(ink: Ink, highlight: Hsla, cx: &App) -> Hsla {
    use crate::segmented::contrast_ratio;
    let under: Vec<Hsla> = ink
        .surfaces(cx)
        .into_iter()
        .map(|surface| surface.blend(highlight))
        .collect();
    // The least it reads at on any surface it may be drawn on.
    let contrast = |color: Hsla| {
        under
            .iter()
            .map(|under| contrast_ratio(under.blend(color), *under))
            .fold(f32::INFINITY, f32::min)
    };
    // Black or white, whichever stands out more, always reads.
    let (black, white) = (gpui_kit::black(), gpui_kit::white());
    let toward = if contrast(black) >= contrast(white) {
        black
    } else {
        white
    };
    (0..=20)
        .map(|step| ink.text.blend(toward.opacity(step as f32 / 20.)))
        .find(|shade| contrast(*shade) >= MATCH_CONTRAST)
        .unwrap_or(toward)
}

#[path = "find_text.rs"]
mod plain;

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
