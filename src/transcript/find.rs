//! Find in the transcript (Mod+F): where the reader's words are in what the
//! rows show, collapsed rows included, highlighted, and moving between
//! them. Landing on a match opens what hides it (a thinking, a call's
//! details, a group of calls, a long code block), and closes it again when
//! the find moves on.
//!
//! The find keeps each entry's matches apart, and each entry's texts
//! lowercased for as long as it goes on. A change of the transcript
//! searches only the entries it changed, as it comes, and words that grow
//! search only the entries that matched the shorter ones. The texts of a
//! long transcript are lowercased off the UI thread the first time.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;

use arko::remote::Kind;
use gpui_kit::base::text::{RangeHighlight, RenderedText};
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::*;

use super::model::{self, Entry, RowKey, Show};
use super::{CODE_LINES, TranscriptEvent, TranscriptView, row};
use crate::prefs::TranscriptView as View;

/// The bytes of text above which the first search of a find lowercases
/// the transcript's texts off the UI thread.
const LOWERCASE_IN_BACKGROUND: usize = 256 * 1024;

/// Where in an entry a match is, in the order the entry shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Place {
    /// The entry's plain text: a message of the user's, a notice, an
    /// output, a call's line.
    Text,
    /// Block `n` of the entry's Markdown, as it renders.
    Block(usize),
    /// Block `n` of the entry's Markdown, shown as its source.
    Source(usize),
    /// A call's parameters, as its details show them.
    Parameters,
    /// A call's output, as its details show them.
    Output,
    /// An entry's Markdown before its rows are ready: its source.
    Unready,
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
}

/// A text lowercased to find words in, and how to map its bytes back.
pub(crate) struct Lowered {
    lower: String,
    /// For each byte of `lower`, the bytes of the character of the text it
    /// comes from: only when lowercasing moved them (a rare character whose
    /// lowercase is longer or shorter).
    origin: Option<Vec<(usize, usize)>>,
}

impl Lowered {
    pub fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self {
                lower: text.to_ascii_lowercase(),
                origin: None,
            };
        }
        let mut lower = String::with_capacity(text.len());
        let mut origin = Vec::new();
        let mut moved = false;
        for (at, character) in text.char_indices() {
            let start = lower.len();
            let mut count = 0;
            for lowered in character.to_lowercase() {
                lower.push(lowered);
                count += 1;
            }
            let end = at + character.len_utf8();
            moved |= start != at || lower.len() != end || count != 1;
            origin.extend(std::iter::repeat_n((at, end), lower.len() - start));
        }
        Self {
            lower,
            origin: moved.then_some(origin),
        }
    }

    /// Every place the lowercase `needle` is, as byte ranges of the text,
    /// one after another.
    pub fn find(&self, needle: &str) -> Vec<Range<usize>> {
        let mut found = Vec::new();
        if needle.is_empty() {
            return found;
        }
        let mut from = 0;
        while let Some(position) = self.lower[from..].find(needle) {
            let start = from + position;
            let end = start + needle.len();
            found.push(match &self.origin {
                None => start..end,
                Some(origin) => origin[start].0..origin[end - 1].1,
            });
            from = end;
        }
        found
    }
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

    fn lowered(self) -> Vec<(Place, Lowered)> {
        match self {
            Self::Texts(texts) => texts
                .into_iter()
                .map(|(place, text)| (place, Lowered::new(&text)))
                .collect(),
            Self::Rendered { text, blocks } => blocks
                .into_iter()
                .filter_map(|(block, source)| {
                    let range = text.range_for_source(source)?;
                    let block_text = text.as_str().get(range)?;
                    Some((Place::Block(block), Lowered::new(block_text)))
                })
                .collect(),
        }
    }
}

/// The first lines of a call's text, as its details show them.
pub(crate) fn shown_lines(text: &str) -> String {
    row::preview(text).0
}

/// `words` lowercased, as the texts are.
fn lowercase(words: &str) -> String {
    words.chars().flat_map(char::to_lowercase).collect()
}

/// Every place `needle` is in `haystack`, ignoring case, as byte ranges of
/// `haystack`, one after another.
#[cfg(test)]
pub(crate) fn find_all(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    Lowered::new(haystack).find(&lowercase(needle))
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
    /// The words lowercased.
    needle: String,
    /// The entries with matches, in the transcript's order (their ids).
    found: BTreeMap<u64, Found>,
    total: usize,
    /// Each entry's texts, lowercased: kept while the find goes on, and
    /// made again when the entry changes.
    texts: HashMap<u64, Vec<(Place, Lowered)>>,
    current: Option<Key>,
    /// The Markdown blocks whose views show highlights now.
    pub highlighted: HashSet<(u64, usize)>,
    /// Entries whose matches changed since the views were painted.
    touched: HashSet<u64>,
    /// The block whose view shows the current match when it draws next:
    /// set as the find lands, never as it searches again.
    reveal: Option<(u64, usize)>,
    /// The entry the find lands on once its rows are ready, and which of
    /// its matches.
    pending: Option<(u64, usize)>,
    /// What landing opened, closed again as the find moves on.
    opened: Vec<Opened>,
    /// Lowercases the texts off the UI thread (a long transcript's first
    /// search).
    lowering: Option<Task<()>>,
    /// Entries that changed while their texts were lowercased off the UI
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
        let needle = lowercase(query);
        let mut find = self.find.take().unwrap_or_default();
        let anchor = find.current_at();
        // Longer words match only where the shorter ones did.
        let narrowing = !find.needle.is_empty()
            && find.lowering.is_none()
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
        } else if find.lowering.is_none() && !self.lower_in_background(&mut find, cx) {
            self.search_all(&mut find, cx);
        }
        if find.lowering.is_none() {
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
        if find.lowering.is_some() {
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
            searching: find.lowering.is_some(),
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
            if find.lowering.is_some() {
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
        if find.lowering.is_none() && !self.lower_in_background(&mut find, cx) {
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
        find.texts.entry(id).or_insert_with(|| {
            self.model
                .entry(id)
                .filter(|entry| self.finds_in(entry))
                .map(|entry| self.gather(entry, cx).lowered())
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

    /// Starts lowercasing the texts of the entries not searched yet off the
    /// UI thread, when there are many: the find searches once they're
    /// ready. Returns whether it started.
    fn lower_in_background(&self, find: &mut Find, cx: &mut Context<Self>) -> bool {
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
        if bytes < LOWERCASE_IN_BACKGROUND {
            for (id, gathered) in texts {
                find.texts.insert(id, gathered.lowered());
            }
            return false;
        }
        find.found.clear();
        find.total = 0;
        find.changed_meanwhile.clear();
        let lowering = cx.background_spawn(async move {
            texts
                .into_iter()
                .map(|(id, gathered)| (id, gathered.lowered()))
                .collect::<Vec<_>>()
        });
        find.lowering = Some(cx.spawn(async move |this, cx| {
            let lowered = lowering.await;
            this.update(cx, |this, cx| this.lowered(lowered, cx)).ok();
        }));
        true
    }

    /// The texts lowercased off the UI thread are ready: the find searches.
    fn lowered(&mut self, lowered: Vec<(u64, Vec<(Place, Lowered)>)>, cx: &mut Context<Self>) {
        let Some(mut find) = self.find.take() else {
            return;
        };
        find.lowering = None;
        let changed = std::mem::take(&mut find.changed_meanwhile);
        for (id, texts) in lowered {
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
            Kind::User | Kind::Notice | Kind::Output => vec![(Place::Text, item.text.clone())],
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
                let source = !self.settings.render_markdown
                    || self.ui.get(&item.id).is_some_and(|ui| ui.source);
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
                if let Some(parameters) = row::shown_parameters(item) {
                    texts.push((Place::Parameters, shown_lines(&parameters)));
                }
                if let Some(output) = row::shown_output(entry) {
                    texts.push((Place::Output, shown_lines(&output)));
                }
                texts
            }
            _ => Vec::new(),
        };
        Gathered::Texts(texts)
    }

    /// Opens what hides the current match, scrolls to it and highlights.
    /// What the last landing opened closes again, unless this one needs it.
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
            _ => {
                // Plain text has no place for its lines: the match's share
                // of the text says how far down the row it is.
                let length = self.place_length(found).max(1);
                let share = found.range.start as f32 / length as f32;
                self.list.reveal_within(ix, share);
            }
        }
    }

    /// How long the text of `found`'s place is, as its row shows it.
    fn place_length(&self, found: &Match) -> usize {
        let Some(entry) = self.model.entry(found.entry) else {
            return 0;
        };
        match found.place {
            Place::Parameters => {
                row::shown_parameters(&entry.item).map_or(0, |text| shown_lines(&text).len())
            }
            Place::Output => row::shown_output(entry).map_or(0, |text| shown_lines(&text).len()),
            Place::Source(block) => self
                .ui
                .get(&found.entry)
                .and_then(|ui| ui.doc.as_ref())
                .and_then(|doc| doc.blocks.get(block)?.range.clone())
                .map_or(0, |range| range.len()),
            _ => entry.item.text.len(),
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
            self.add_ui(id, cx);
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
            if let Some(first) = group
                && !self.group_open(first)
            {
                needs.push(Opening::Group(first));
            }
        }
        match (entry.item.kind, found.place) {
            (Kind::Thinking, _) | (Kind::Call, Place::Parameters | Place::Output) => {
                needs.push(Opening::Entry(found.entry))
            }
            (_, Place::Block(block)) if self.hides_in_code(found, block, cx) => {
                needs.push(Opening::Code(found.entry, block))
            }
            _ => {}
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
                let before = self.ui.get(&first).and_then(|ui| ui.group_open);
                self.ui.entry(first).or_default().group_open = Some(true);
                self.refresh_rows(cx);
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
            let ranges = Lowered::new(text.as_str()).find(&needle);
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
    /// is the current one, for a row of plain text.
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
        let current = find
            .current
            .filter(|key| key.entry == entry && key.place == place)
            .map(|key| key.ordinal);
        ranges
            .iter()
            .enumerate()
            .map(|(ordinal, range)| (range.clone(), Some(ordinal) == current))
            .collect()
    }

    /// `text` as the plain text of `place` in a row, its matches marked:
    /// selectable as usual, and while it has matches, painted with them.
    pub(super) fn marked(
        &self,
        id: impl Into<ElementId>,
        entry: u64,
        place: Place,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> AnyElement {
        let text = text.into();
        if self.match_ranges(entry, place).is_empty() {
            return super::selectable(id, text).into_any_element();
        }
        self.marked_plain(entry, place, text, cx)
    }

    /// `text` with the matches of `place` painted on it (for text that
    /// isn't selectable, such as a call's line in its button).
    pub(super) fn marked_plain(
        &self,
        entry: u64,
        place: Place,
        text: impl Into<SharedString>,
        cx: &App,
    ) -> AnyElement {
        let text = text.into();
        let ranges = self.match_ranges(entry, place);
        if ranges.is_empty() {
            return text.into_any_element();
        }
        let (all, strong) = match_colors(cx);
        let highlights: Vec<(Range<usize>, HighlightStyle)> = ranges
            .into_iter()
            .filter(|(range, _)| text.get(range.clone()).is_some())
            .map(|(range, current)| {
                (
                    range,
                    HighlightStyle {
                        background_color: Some(if current { strong } else { all }),
                        ..HighlightStyle::default()
                    },
                )
            })
            .collect();
        StyledText::new(text)
            .with_highlights(highlights)
            .into_any_element()
    }
}

/// The background of matches, and of the current one.
fn match_colors(cx: &App) -> (Hsla, Hsla) {
    let yellow = cx.theme().yellow;
    (yellow.opacity(0.25), yellow.opacity(0.55))
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
