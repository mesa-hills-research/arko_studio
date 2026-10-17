//! Find in the transcript (Mod+F): where the reader's words are in what the
//! rows show, collapsed rows included, highlighted, and moving between
//! them. Landing on a match opens what hides it: a thinking, a call's
//! details, a group of calls, a long code block.

use std::collections::{HashMap, HashSet};
use std::ops::Range;

use arko::remote::Kind;
use gpui_kit::base::text::RangeHighlight;
use gpui_kit::component::ActiveTheme as _;
use gpui_kit::component::text::TextBlockContent;
use gpui_kit::*;

use super::model::{self, Entry, RowKey, Show};
use super::{CODE_LINES, TranscriptEvent, TranscriptView, row};
use crate::prefs::TranscriptView as View;

/// Where in an entry a match is.
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
}

/// A find going on.
#[derive(Default)]
pub(crate) struct Find {
    pub query: String,
    pub matches: Vec<Match>,
    /// The matches of each place, by their index in `matches`.
    pub places: HashMap<(u64, Place), Vec<usize>>,
    pub current: Option<usize>,
    /// The Markdown blocks whose views show highlights now.
    pub highlighted: HashSet<(u64, usize)>,
    /// The transcript changed since the search: it runs again.
    pub stale: bool,
    pub _again: Option<Task<()>>,
}

/// Every place `needle` is in `haystack`, ignoring case, as byte ranges of
/// `haystack`, one after another.
pub(crate) fn find_all(haystack: &str, needle: &str) -> Vec<Range<usize>> {
    let needle: String = needle.chars().flat_map(char::to_lowercase).collect();
    if needle.is_empty() {
        return Vec::new();
    }
    // The lowercase text, and for each of its bytes the start of the
    // character of `haystack` it comes from.
    let mut lower = String::with_capacity(haystack.len());
    let mut origin = Vec::with_capacity(haystack.len());
    for (at, character) in haystack.char_indices() {
        for lowered in character.to_lowercase() {
            let start = lower.len();
            lower.push(lowered);
            origin.extend(std::iter::repeat_n(at, lower.len() - start));
        }
    }
    let end_of = |at: usize| at + haystack[at..].chars().next().map_or(0, char::len_utf8);
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(position) = lower[from..].find(&needle) {
        let start = from + position;
        let end = start + needle.len();
        found.push(origin[start]..end_of(origin[end - 1]));
        from = end;
    }
    found
}

/// The first lines of a call's text, as its details show them.
pub(crate) fn shown_lines(text: &str) -> String {
    text.lines()
        .take(row::OUTPUT_LINES)
        .collect::<Vec<_>>()
        .join("\n")
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
        let anchor = self.current_match();
        self.search(query, cx);
        let find = self.find.as_mut().expect("searched");
        find.current = match anchor {
            Some(anchor) => find
                .matches
                .iter()
                .position(|found| !before(found, &anchor, &self.model))
                .or_else(|| find.matches.len().checked_sub(1)),
            None => find.matches.len().checked_sub(1),
        };
        self.land(cx);
        self.find_status()
    }

    /// Goes to the next match (`forward`, down the transcript) or the
    /// previous one, around at the ends.
    pub fn find_next(&mut self, forward: bool, cx: &mut Context<Self>) -> FindStatus {
        let Some(query) = self.find.as_ref().map(|find| find.query.clone()) else {
            return FindStatus::default();
        };
        if self.find.as_ref().is_some_and(|find| find.stale) {
            let anchor = self.current_match();
            self.search(&query, cx);
            let find = self.find.as_mut().expect("searched");
            find.current = anchor.and_then(|anchor| {
                find.matches
                    .iter()
                    .position(|found| found == &anchor || !before(found, &anchor, &self.model))
            });
        }
        let find = self.find.as_mut().expect("finding");
        let total = find.matches.len();
        if total == 0 {
            find.current = None;
        } else {
            find.current = Some(match (find.current, forward) {
                (None, true) => 0,
                (None, false) => total - 1,
                (Some(current), true) => (current + 1) % total,
                (Some(current), false) => (current + total - 1) % total,
            });
        }
        self.land(cx);
        self.find_status()
    }

    /// Where the find is.
    pub fn find_status(&self) -> FindStatus {
        self.find
            .as_ref()
            .map(|find| FindStatus {
                current: find.current,
                total: find.matches.len(),
            })
            .unwrap_or_default()
    }

    /// Ends the find: the highlights go.
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

    fn current_match(&self) -> Option<Match> {
        let find = self.find.as_ref()?;
        find.matches.get(find.current?).cloned()
    }

    /// The transcript changed while a find goes on: it searches again a
    /// moment later, keeping its place.
    pub(super) fn find_changed(&mut self, cx: &mut Context<Self>) {
        let Some(find) = self.find.as_mut() else {
            return;
        };
        find.stale = true;
        if find._again.is_some() {
            return;
        }
        find._again = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(250))
                .await;
            this.update(cx, |this, cx| {
                let Some(query) = this.find.as_ref().map(|find| find.query.clone()) else {
                    return;
                };
                let anchor = this.current_match();
                this.search(&query, cx);
                let model = &this.model;
                if let Some(find) = this.find.as_mut() {
                    find.current = anchor.and_then(|anchor| {
                        find.matches
                            .iter()
                            .position(|found| found == &anchor || !before(found, &anchor, model))
                    });
                }
                this.highlight(cx);
                cx.emit(TranscriptEvent::FindStatus(this.find_status()));
                cx.notify();
            })
            .ok();
        }));
    }

    /// Searches every entry the view shows (collapsed ones too) for
    /// `query`. Older entries not made ready yet are made ready first.
    fn search(&mut self, query: &str, cx: &mut Context<Self>) {
        while self.load_older(cx) {}
        let view = self.settings.view;
        let filter = self.model.filter().clone();
        let worker_view = matches!(filter, super::AgentFilter::Worker(_));
        let view = if worker_view { View::Full } else { view };
        let mut matches = Vec::new();
        for entry in self.model.entries() {
            if model::show(&filter, view, &entry.item) != Show::Item
                || model::is_refresh(&entry.item)
            {
                continue;
            }
            for (place, text) in self.searchable(entry, cx) {
                for (ordinal, range) in find_all(&text, query).into_iter().enumerate() {
                    matches.push(Match {
                        entry: entry.item.id,
                        place,
                        range,
                        ordinal,
                    });
                }
            }
        }
        let mut places: HashMap<(u64, Place), Vec<usize>> = HashMap::new();
        for (index, found) in matches.iter().enumerate() {
            places
                .entry((found.entry, found.place))
                .or_default()
                .push(index);
        }
        let highlighted = self
            .find
            .take()
            .map(|find| find.highlighted)
            .unwrap_or_default();
        self.find = Some(Find {
            query: query.to_string(),
            matches,
            places,
            current: None,
            highlighted,
            stale: false,
            _again: None,
        });
    }

    /// The texts of an entry the reader can find words in, as its rows
    /// show them.
    fn searchable(&self, entry: &Entry, cx: &App) -> Vec<(Place, String)> {
        let item = &entry.item;
        match item.kind {
            Kind::User | Kind::Notice | Kind::Output => vec![(Place::Text, item.text.clone())],
            Kind::Text | Kind::Reply | Kind::Message | Kind::Question | Kind::Thinking => {
                let Some(ui) = self.ui.get(&item.id) else {
                    return Vec::new();
                };
                let Some(doc) = ui.doc.as_ref() else {
                    return Vec::new();
                };
                let source = !self.settings.render_markdown || ui.source;
                if source {
                    return doc
                        .blocks
                        .iter()
                        .enumerate()
                        .filter_map(|(block, row)| {
                            let text = doc.text.get(row.range.clone()?)?.trim_end().to_string();
                            Some((Place::Source(block), text))
                        })
                        .collect();
                }
                let rendered = doc.state.read(cx).rendered_text();
                doc.blocks
                    .iter()
                    .enumerate()
                    .filter_map(|(block, row)| {
                        let range = rendered.range_for_source(row.range.clone()?)?;
                        let text = rendered.as_str().get(range)?.to_string();
                        Some((Place::Block(block), text))
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
        }
    }

    /// Opens what hides the current match, scrolls to it and highlights.
    fn land(&mut self, cx: &mut Context<Self>) {
        if let Some(found) = self.current_match() {
            self.open_for(&found, cx);
            let key = match found.place {
                Place::Block(block) | Place::Source(block) => self.rows.iter().position(|row| {
                    matches!(row.key, RowKey::Block { id, block: b, .. }
                            if id == found.entry && b == block)
                }),
                _ => self.row_of(found.entry),
            };
            if let Some(ix) = key {
                self.scroller
                    .update(cx, |scroller, cx| scroller.scroll_to_item(ix, cx));
            }
        }
        self.highlight(cx);
        cx.emit(TranscriptEvent::FindStatus(self.find_status()));
        cx.notify();
    }

    /// Opens a thinking, a call's details, a group of calls or a capped
    /// code block that hides `found`.
    fn open_for(&mut self, found: &Match, cx: &mut Context<Self>) {
        let Some(entry) = self.model.entry(found.entry) else {
            return;
        };
        let kind = entry.item.kind;
        // A folded group of calls (Quiet) opens first.
        if self.settings.view == View::Quiet && self.row_of(found.entry).is_none() {
            let group = self.rows.iter().find_map(|row| match row.key {
                RowKey::Group(first) => {
                    let position = self.model.position(found.entry).ok()?;
                    model::run_of(&self.model, first, View::Quiet)
                        .contains(&position)
                        .then_some(first)
                }
                _ => None,
            });
            if let Some(first) = group {
                self.ui.entry(first).or_default().expanded = Some(true);
                self.refresh_rows(cx);
            }
        }
        match (kind, found.place) {
            (Kind::Thinking, _) | (Kind::Call, Place::Parameters | Place::Output) => {
                self.set_expanded(found.entry, true, cx)
            }
            (_, Place::Block(block)) => self.uncap_for(found, block, cx),
            _ => {}
        }
    }

    /// Shows a long code block whole when `found` is past its first lines.
    fn uncap_for(&mut self, found: &Match, block: usize, cx: &mut Context<Self>) {
        let Some(doc) = self.ui.get(&found.entry).and_then(|ui| ui.doc.as_ref()) else {
            return;
        };
        let Some(row) = doc.blocks.get(block) else {
            return;
        };
        if !matches!(row.content, TextBlockContent::CodeBlock(_)) {
            return;
        }
        let rendered = doc.state.read(cx).rendered_text();
        let Some(text) = row
            .range
            .clone()
            .and_then(|range| rendered.range_for_source(range))
            .and_then(|range| rendered.as_str().get(range).map(str::to_string))
        else {
            return;
        };
        let line = text[..found.range.start.min(text.len())]
            .matches('\n')
            .count();
        let capped = !doc.blocks.is_empty()
            && !self
                .ui
                .get(&found.entry)
                .is_some_and(|ui| ui.uncapped.contains(&block));
        if capped && line >= CODE_LINES {
            self.toggle_code(found.entry, block, cx);
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

    /// Paints the matches on the block views drawn, and clears those whose
    /// matches went.
    fn highlight(&mut self, cx: &mut Context<Self>) {
        let mut now: HashSet<(u64, usize)> = HashSet::new();
        if let Some(find) = &self.find {
            for (entry, place) in find.places.keys() {
                if let Place::Block(block) = place {
                    now.insert((*entry, *block));
                }
            }
        }
        let before = self
            .find
            .as_mut()
            .map(|find| std::mem::take(&mut find.highlighted))
            .unwrap_or_default();
        for (entry, block) in before.difference(&now) {
            if let Some(view) = self.existing_block_view(*entry, *block) {
                view.update(cx, |view, cx| view.clear_range_highlights(cx));
            }
        }
        for (entry, block) in &now {
            if let Some(view) = self.existing_block_view(*entry, *block) {
                self.highlight_block(*entry, *block, &view, cx);
            }
        }
        if let Some(find) = self.find.as_mut() {
            find.highlighted = now;
        }
    }

    /// Paints the matches of block `block` of the entry `entry` on its
    /// view, the current one stronger. The view's own text is searched, so
    /// the ranges are its own.
    pub(super) fn highlight_block(
        &self,
        entry: u64,
        block: usize,
        view: &Entity<gpui_kit::component::text::TextViewState>,
        cx: &mut App,
    ) {
        let Some(find) = &self.find else {
            return;
        };
        let Some(indices) = find.places.get(&(entry, Place::Block(block))) else {
            return;
        };
        let current = indices
            .iter()
            .find(|index| Some(**index) == find.current)
            .map(|index| find.matches[*index].ordinal);
        let (all, strong) = match_colors(cx);
        let query = find.query.clone();
        view.update(cx, |view, cx| {
            let text = view.rendered_text();
            let ranges = find_all(text.as_str(), &query);
            let highlights = ranges.iter().enumerate().map(|(ordinal, range)| {
                let color = if Some(ordinal) == current {
                    strong
                } else {
                    all
                };
                RangeHighlight::new(range.clone(), color)
            });
            let _ = view.set_range_highlights(highlights, cx);
            if let Some(range) = current.and_then(|ordinal| ranges.get(ordinal)) {
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
        find.places
            .get(&(entry, place))
            .map(|indices| {
                indices
                    .iter()
                    .map(|index| {
                        (
                            find.matches[*index].range.clone(),
                            Some(*index) == find.current,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default()
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

/// Whether `found` comes before `anchor` in the transcript.
fn before(found: &Match, anchor: &Match, model: &model::Model) -> bool {
    let position = |found: &Match| model.position(found.entry).unwrap_or_else(|ix| ix);
    (position(found), found.place, found.range.start)
        < (position(anchor), anchor.place, anchor.range.start)
}

#[cfg(test)]
#[path = "find_tests.rs"]
mod tests;
