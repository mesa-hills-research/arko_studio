//! How each row of the transcript draws.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use arko::remote::{
    CallStatus, ContentKind, Delivery, Item, JobKind, JobState, JobSummary, Kind, Level, Source,
    WorkerState,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::image_cache::LruImageCache;
use gpui_kit::base::text::{TextView, TextViewMotion, TextViewStyle};
use gpui_kit::base::{Easing, SelectableText};
use gpui_kit::component::{
    ActiveTheme as _, Colorize as _, Disableable as _, Icon, IconName, Sizable as _, Theme,
    WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    diff_view::{Diff, DiffView},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    tooltip::{ManagedTooltipExt as _, Tooltip},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::find::{Ink, Place, QuestionPart};
use crate::fonts::CodeFont as _;

use super::height::Metrics;
use super::model::{Gap, QuietRow, Rail, Row, RowKey};
use super::tool::Tool;
use super::{AgentFilter, CODE_LINES, TranscriptEvent, TranscriptView};

/// How often a label shimmers, in frames a second.
pub(crate) const SHIMMER_FPS: f32 = 12.;

/// The reading column's widest.
const COLUMN: f32 = 46.;
/// The width, in rem, a user's message may take even when that is more than
/// half the column, the width of a comfortable line.
const USER_BUBBLE_WIDTH: f32 = 17.5;
/// The tag of the user's answers to the agent's questions, typed or given
/// in the answer panel.
const ANSWER_TAG: &str = "Answer";
/// The space, in rem, between a card of questions and the bubble of the
/// answers given in the answer panel under it.
const ANSWERS_GAP: f32 = 0.5;
/// The least room, in rem, between a reply's other actions and Regenerate.
const REGENERATE_GAP: f32 = 2.;
/// What the Regenerate buttons' tooltips say.
const REGENERATE_HINT: &str = "Send your last message again";
/// What a message that only kept the prompt cache warm reads, and what a
/// screen reader says of it.
pub(super) const CACHE_REFRESHED: &str = "Cache refreshed";
/// The room, in rem, a line keeps at its right for Regenerate laid over it.
const REGENERATE_ROOM: f32 = 1.75;
/// Lines of a call's output shown before "Show all N lines".
pub(super) const OUTPUT_LINES: usize = 40;
/// Rows of an edit's diff shown before it scrolls inside.
const DIFF_ROWS: usize = 24;
/// The space between the commands of a chain in a command's strip, which
/// its timeline runs through.
pub(super) const CHAIN_GAP: f32 = 6.;
/// How strong a command's timeline is, as an opacity of the muted text. Its
/// line is 2 px wide, as the `$`'s stem is, so it's lighter than the text to
/// keep it thin.
const TIMELINE_ALPHA: f32 = 0.35;
/// How far a `$` reaches above and below the middle of its line, as parts
/// of the code font's size (JetBrains Mono's sits a little high), and how
/// far the timeline stays from it.
const DOLLAR_ABOVE: f32 = 0.58;
const DOLLAR_BELOW: f32 = 0.5;
const TIMELINE_GAP: f32 = 3.;
/// The width of a line of work's markers' column, in rem. ▸, ▾ or the
/// spinner stands centred in it.
const MARKER: f32 = 0.625;
/// The space between a line's marker and its words, and between its
/// parts, in rem.
const GAP: f32 = 0.375;
/// Where a line's words start, past its marker, in rem, and where what an
/// open line holds starts.
pub(super) const WORDS: f32 = 1.;
/// A line of work's height, in rem, and a thought's line height.
pub(super) const LINE: f32 = 1.25;
/// Where an open line's rail stands, in rem, under its marker's centre,
/// and how wide it is, in pixels.
const RAIL_AT: f32 = 0.25;
const RAIL_WIDTH: f32 = 2.;
/// A thought's text size, and "Working…"'s, in rem.
pub(super) const THOUGHT_SIZE: f32 = 0.875;
/// What the live line says while the agent waits on the model.
const WORKING: &str = "Working…";

/// What stands in a line's markers' column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Marker {
    /// Nothing, as the line shows all there is.
    None,
    /// ▸, as the line opens.
    Closed,
    /// ▾, as the line is open.
    Open,
    /// The spinner, on the one line that is live.
    Live,
}

impl Marker {
    /// The marker of a line that `opens`, `open` or not.
    fn of(opens: bool, open: bool) -> Self {
        match (opens, open) {
            (false, _) => Marker::None,
            (true, false) => Marker::Closed,
            (true, true) => Marker::Open,
        }
    }

    fn live(live: bool) -> Self {
        if live { Marker::Live } else { Marker::None }
    }
}

/// A line of work, `marker` in the markers' column, then whose it is,
/// `who`, then its `words`, cut to fit, then `after`, and at its right
/// `right`.
fn work_line(
    marker: AnyElement,
    who: Option<AnyElement>,
    words: AnyElement,
    after: Vec<AnyElement>,
    right: Option<AnyElement>,
) -> Div {
    h_flex()
        .w_full()
        .min_w_0()
        .h_full()
        .gap(rems(GAP))
        .line_height(rems(LINE))
        .child(marker)
        .children(who.map(|who| div().flex_none().child(who)))
        .child(div().flex_initial().min_w_0().truncate().child(words))
        .children(after)
        .when_some(right, |this, right| {
            this.child(div().flex_1())
                .child(div().flex_none().child(right))
        })
}

/// What an open line holds, under it on a rail that hangs from its
/// marker's centre, starting at the line's words.
fn on_rail(held: AnyElement, theme: &Theme) -> Div {
    div()
        .relative()
        .mt_1()
        .pl(rems(WORDS))
        .child(
            div()
                .absolute()
                .left(rems(RAIL_AT))
                .top_0()
                .bottom_0()
                .w(px(RAIL_WIDTH))
                .bg(theme.border),
        )
        .child(held)
}

/// Whose a call is, a worker's ID on the secondary fill.
fn who(agent: &str, theme: &Theme) -> AnyElement {
    div()
        .px_1()
        .rounded(theme.semantic_tokens().radius.sm)
        .bg(theme.secondary)
        .text_color(theme.secondary_foreground)
        .font_family(theme.font_family.clone())
        .text_xs()
        .line_height(rems(1.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(agent.to_string())
        .into_any_element()
}

/// The characters whose a call is takes on its line, as mono characters.
fn who_chars(item: &Item) -> usize {
    if item.is_main() {
        0
    } else {
        item.agent.chars().count() + 2
    }
}

/// The room, in pixels, that a line's words and what follows them have in
/// a list `width` wide, on a rail (`inner`) or not.
pub(super) fn room(width: f32, inner: bool, rem: f32) -> f32 {
    let column = width.min(COLUMN * rem) - 2. * rem;
    column - if inner { 2. * WORDS } else { WORDS } * rem
}

/// The widest that what follows a line's words may be in `room` pixels.
/// The words give way first, down to about 8 characters.
pub(super) fn after_room(room: f32, m: &Metrics) -> f32 {
    (room - 8. * m.mono.advance() - 2. * GAP * m.rem).max(0.)
}

/// The first line of a thought with words in it, without Markdown's bold
/// marks, and whether lines with words follow it.
pub(super) fn first_line(text: &str) -> (String, bool) {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or_default().replace("**", "");
    (first, lines.next().is_some())
}

/// `count` and its noun, one or many.
fn plural(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Where a command's timeline stops above a `$` and starts again below it,
/// down from the top of its line, for a line `line` tall in a font `size`.
pub(super) fn timeline_gaps(line: f32, size: f32) -> (f32, f32) {
    (
        (line / 2. - size * DOLLAR_ABOVE - TIMELINE_GAP).max(0.),
        line / 2. + size * DOLLAR_BELOW + TIMELINE_GAP,
    )
}

/// The ranges of the numbers before the lines of `text`, `numbers`
/// characters each, as a call's content shows a file's lines ("100  text",
/// "-100  text"). A line without them (a note between a diff's parts) has
/// none.
pub(super) fn numbered_gutters(text: &str, numbers: usize) -> Vec<std::ops::Range<usize>> {
    if numbers == 0 {
        return Vec::new();
    }
    let mut gutters = Vec::new();
    let mut at = 0;
    for line in text.split('\n') {
        let gutter = line.get(..numbers).filter(|gutter| {
            gutter
                .bytes()
                .all(|byte| byte.is_ascii_digit() || b" +-".contains(&byte))
                && gutter.bytes().any(|byte| byte.is_ascii_digit())
                && line[numbers..].starts_with("  ")
        });
        if gutter.is_some() {
            gutters.push(at..at + numbers);
        }
        at += line.len() + 1;
    }
    gutters
}

/// The diff an edit or a write made, from the call's parameters, numbered
/// from the line the edit's result names ("Edited a.rs at lines 40-42").
pub(crate) fn edit_diff(item: &Item) -> Option<Diff> {
    let tool = item.tool.as_deref()?;
    let parameters: serde_json::Value = serde_json::from_str(item.parameters.as_deref()?).ok()?;
    let text = |name: &str| parameters.get(name).and_then(|value| value.as_str());
    let path = text("path")?.to_string();
    match tool {
        "edit_file" => {
            let diff = Diff::between(path, text("old_string")?, text("new_string")?);
            let line = item
                .output
                .as_deref()
                .filter(|output| output.starts_with("Edited "))
                .and_then(|output| output.split_once(" at lines "))
                .and_then(|(_, lines)| {
                    lines
                        .split(|c: char| !c.is_ascii_digit())
                        .next()?
                        .parse::<usize>()
                        .ok()
                });
            Some(match line {
                Some(line) if line > 1 => from_line(diff, line - 1),
                _ => diff,
            })
        }
        "write_file" => Some(Diff::between(path, "", text("content")?)),
        _ => None,
    }
}

/// `diff` with its hunks moved `by` lines down the file.
fn from_line(diff: Diff, by: usize) -> Diff {
    let shift = |range: &str| {
        let (start, rest) = range.split_once(',').unwrap_or((range, ""));
        let start = start.parse::<usize>().map_or(0, |start| start + by);
        if rest.is_empty() {
            start.to_string()
        } else {
            format!("{start},{rest}")
        }
    };
    let mut unified = String::new();
    for line in diff.to_unified().lines() {
        match line
            .strip_prefix("@@ -")
            .and_then(|rest| rest.split_once(" @@"))
            .and_then(|(ranges, tail)| {
                let (old, new) = ranges.split_once(" +")?;
                Some(format!("@@ -{} +{} @@{tail}", shift(old), shift(new)))
            }) {
            Some(header) => unified.push_str(&header),
            None => unified.push_str(line),
        }
        unified.push('\n');
    }
    Diff::parse(&unified)
}

/// A call's line as its row says it: the first line of its words, without
/// what its content holds. A message's line names whom it went to, and an
/// MCP call's names the tool, since opening them shows the message and the
/// arguments.
pub(super) fn call_line(item: &Item) -> String {
    let line = item.text.lines().next().unwrap_or_default();
    let tool = item.tool.as_deref().unwrap_or_default();
    let cut = if tool == "message_agent" {
        line.split_once(" “").map(|(to, _)| to)
    } else if tool.starts_with("mcp__") {
        line.split_once(" {").map(|(name, _)| name)
    } else {
        None
    };
    cut.unwrap_or(line).to_string()
}

/// The commands of a shell command, one for each part a chain joins with
/// `&&`, `||`, `;` or a new line, as ranges of `command` without their
/// spaces. A pipeline stays one command. Quotes, `$( )`, a line that ends in
/// `\` and a heredoc's body stay inside the command they belong to.
pub(super) fn chain(command: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = command.as_bytes();
    let mut parts = Vec::new();
    let mut start = 0;
    let (mut single, mut double, mut depth) = (false, false, 0i32);
    let mut heredocs: Vec<String> = Vec::new();
    let mut at = 0;
    let mut push = |from: usize, to: usize| {
        let part = &command[from..to];
        let trimmed = part.trim();
        if !trimmed.is_empty() {
            let lead = part.len() - part.trim_start().len();
            parts.push(from + lead..from + lead + trimmed.len());
        }
    };
    while at < bytes.len() {
        let c = bytes[at];
        let next = bytes.get(at + 1).copied();
        if single {
            single = c != b'\'';
            at += 1;
            continue;
        }
        if double {
            match c {
                b'\\' => at += 2,
                b'"' => {
                    double = false;
                    at += 1;
                }
                _ => at += 1,
            }
            continue;
        }
        match c {
            b'\\' => {
                at += 2;
                continue;
            }
            b'\'' => single = true,
            b'"' => double = true,
            b'(' => depth += 1,
            b')' => depth -= 1,
            b'<' if next == Some(b'<') && bytes.get(at + 2) != Some(&b'<') => {
                // A heredoc's word, after <<, <<- and its quotes.
                let mut end = at + 2;
                if bytes.get(end) == Some(&b'-') {
                    end += 1;
                }
                while bytes.get(end) == Some(&b' ') {
                    end += 1;
                }
                let word_start = end;
                while end < bytes.len()
                    && !bytes[end].is_ascii_whitespace()
                    && !b";&|<>()".contains(&bytes[end])
                {
                    end += 1;
                }
                let word: String = command[word_start..end]
                    .chars()
                    .filter(|c| !matches!(c, '\'' | '"'))
                    .collect();
                if !word.is_empty() {
                    heredocs.push(word);
                }
                at = end;
                continue;
            }
            b'\n' if depth <= 0 && !heredocs.is_empty() => {
                // The body, up to each word's own line, is the command's.
                at += 1;
                for word in std::mem::take(&mut heredocs) {
                    while at < bytes.len() {
                        let end = command[at..].find('\n').map_or(bytes.len(), |n| at + n);
                        let done = command[at..end].trim() == word;
                        at = end;
                        if done {
                            break;
                        }
                        at += 1;
                    }
                }
                continue;
            }
            b'\n' | b';' if depth <= 0 => {
                push(start, at);
                start = at + 1;
            }
            b'&' | b'|' if depth <= 0 && next == Some(c) => {
                push(start, at);
                at += 2;
                start = at;
                continue;
            }
            _ => {}
        }
        at += 1;
    }
    push(start, bytes.len());
    parts
}

/// What a call's details show of what it was given.
pub(super) enum Given {
    /// What its line cut (a command's whole text, a long query), muted, as
    /// Full's detail says it.
    Detail(Arc<str>),
    /// From a session saved before calls said what their lines cut, its
    /// parameters under a caption, or an old edit's patch.
    Parameters(Arc<str>),
}

impl Given {
    pub(super) fn text(&self) -> &Arc<str> {
        match self {
            Given::Detail(text) | Given::Parameters(text) => text,
        }
    }
}

/// What a call's details show of what it was given: none where its line
/// says it all or its diff does.
pub(super) fn shown_given(item: &Item) -> Option<Given> {
    if let Some(patch) = apply_diff_patch(item) {
        // An edit from a session saved before edit_file: its patch.
        return Some(Given::Parameters(patch.into()));
    }
    if item.act.is_some() {
        // (The line is Full's, so what it cut is Full's detail.)
        return item
            .detail
            .as_deref()
            .map(str::trim)
            .filter(|detail| !detail.is_empty())
            .map(|detail| Given::Detail(detail.into()));
    }
    shown_parameters(item).map(Given::Parameters)
}

/// The parameters an old session's call shows: none where its line says
/// them all or its diff does, else its parameters (or, from before the feed
/// kept them, the line's long part: a multi-line command, a worker's task).
fn shown_parameters(item: &Item) -> Option<Arc<str>> {
    let line = item.text.trim();
    if parameters_shown_by_line(item) {
        None
    } else if let Some(parameters) = item.parameters.clone() {
        Some(parameters)
    } else if line.contains('\n') || item.detail.is_some() {
        Some(item.detail.clone().unwrap_or(line.to_string()).into())
    } else {
        None
    }
}

/// What a call's details show came back: its content (the lines it read
/// or wrote, its output, its message, its result), or from a session
/// without one, its tool's result. An edit's diff stands for its content,
/// and a failure whose words its line says whole has none.
pub(super) fn shown_output(entry: &super::model::Entry) -> Option<Arc<str>> {
    let item = &entry.item;
    if shows_diff(entry) && item.status == Some(CallStatus::Ok) {
        return None;
    }
    match (&entry.result, item.output.clone()) {
        (Some(result), _) => {
            let text = content_text(result);
            let said = item.outcome.as_deref().unwrap_or_default().trim();
            (!(item.status == Some(CallStatus::Failed) && text.trim() == said)).then_some(text)
        }
        // (From a session saved before contents. A call with a content and
        // none to show, such as a monitor's, shows its detail alone.)
        (None, Some(output)) if item.act.is_none() && !output.trim().is_empty() => Some(output),
        _ => None,
    }
}

/// How a call's content draws: the characters of the numbers before a
/// file's lines (none for other text), and whether it's muted (a tool's
/// result, a failure).
pub(super) fn output_look(entry: &super::model::Entry) -> (usize, bool) {
    use arko::remote::content;
    let Some(result) = &entry.result else {
        return (0, false);
    };
    let kind = result.content.unwrap_or(ContentKind::Muted);
    let numbers = match kind {
        ContentKind::Lines | ContentKind::Diff => {
            let text = result.whole.as_deref().unwrap_or(&result.text);
            match (kind, content::number_width(kind, text)) {
                (_, 0) => 0,
                (ContentKind::Diff, width) => width + 1,
                (_, width) => width,
            }
        }
        _ => 0,
    };
    (numbers, kind == ContentKind::Muted)
}

/// A call's content whole, from its `result`, with a file's lines and a
/// diff's numbered in a gutter ("100  text", "-100  text").
fn content_text(result: &Item) -> Arc<str> {
    use arko::remote::content::{self, Line, Mark};
    let Some(kind) = result
        .content
        .filter(|kind| matches!(kind, ContentKind::Lines | ContentKind::Diff))
    else {
        return result
            .whole
            .clone()
            .unwrap_or_else(|| result.text.as_str().into());
    };
    let text = result.whole.as_deref().unwrap_or(&result.text);
    let width = content::number_width(kind, text);
    let lines: Vec<String> = text
        .lines()
        .map(|line| match content::line(kind, line) {
            Line::Numbered {
                number,
                mark: Mark::None,
                text,
            } => format!("{number:>width$}  {text}"),
            Line::Numbered { number, mark, text } => {
                format!("{}{number:>width$}  {text}", mark.symbol())
            }
            Line::Plain(line) => line.to_string(),
        })
        .collect();
    lines.join("\n").into()
}

/// Whether a call's details show a diff: an edit's, or a write's from a
/// session that has no content for it (one has its lines).
pub(super) fn shows_diff(entry: &super::model::Entry) -> bool {
    let item = &entry.item;
    item.parameters.is_some()
        && match item.tool.as_deref() {
            Some("edit_file") => true,
            Some("write_file") => entry.result.is_none(),
            _ => false,
        }
}

/// The diff a call's details show ([`shows_diff`]).
pub(super) fn shown_diff(entry: &super::model::Entry) -> Option<Diff> {
    shows_diff(entry).then(|| edit_diff(&entry.item)).flatten()
}

/// Whether a command's details show it whole, in their strip, so a line
/// that cut its words needn't show them again.
pub(super) fn strip_says_it(item: &Item) -> bool {
    item.tool.as_deref() == Some("shell") && matches!(shown_given(item), Some(Given::Detail(_)))
}

/// Whether a call opens to anything: what its line cut, its content, its
/// diff. One that has none of them doesn't open.
pub(super) fn opens(entry: &super::model::Entry) -> bool {
    shown_given(&entry.item).is_some() || shown_output(entry).is_some() || shows_diff(entry)
}

/// The patch an `apply_diff` call applied (sessions saved before
/// `edit_file` edit with it), from its parameters.
fn apply_diff_patch(item: &Item) -> Option<String> {
    if item.tool.as_deref() != Some("apply_diff") {
        return None;
    }
    let parameters: serde_json::Value = serde_json::from_str(item.parameters.as_deref()?).ok()?;
    Some(parameters.get("diff")?.as_str()?.to_string())
}

/// The longest line a preview shows, in characters: a minified file or a
/// one-line JSON shows its start.
pub(super) const PREVIEW_LINE_CHARS: usize = 500;

/// The preview of `text`, which is its first [`OUTPUT_LINES`] lines, and
/// whether it leaves any out. A line over [`PREVIEW_LINE_CHARS`] ends it
/// cut with "…", so what it leaves out all comes after what it shows.
pub(super) fn preview(text: &str) -> (String, bool) {
    let mut shown = String::new();
    for (index, line) in text.lines().enumerate() {
        if index == OUTPUT_LINES {
            return (shown, true);
        }
        if index > 0 {
            shown.push('\n');
        }
        if let Some((end, _)) = line.char_indices().nth(PREVIEW_LINE_CHARS) {
            shown.push_str(&line[..end]);
            shown.push('…');
            return (shown, true);
        }
        shown.push_str(line);
    }
    (shown, false)
}

/// What the button that opens a call's whole `text` says, when its preview
/// leaves some out.
fn show_all(text: &str) -> Option<String> {
    preview(text).1.then(|| match text.lines().count() {
        1 => "Show all".to_string(),
        total => format!("Show all {total} lines"),
    })
}

/// Whether a call's line already says all its parameters do: a one-line
/// command, a file read or listed, an edit (whose diff shows instead).
fn parameters_shown_by_line(item: &Item) -> bool {
    match item.tool.as_deref() {
        Some("shell") => item.detail.is_none(),
        Some("read_file" | "list_files" | "edit_file" | "write_file") => true,
        _ => false,
    }
}

/// Draws the row at `ix` of `view`'s list.
pub(super) fn render(
    view: &WeakEntity<TranscriptView>,
    ix: usize,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let Some(view) = view.upgrade() else {
        return div().into_any_element();
    };
    // A block's row gets its view the first time it is drawn, and a row of
    // plain text the handles that keep it in a selection.
    match view.read(cx).rows.get(ix).copied() {
        Some(
            row @ Row {
                key: RowKey::Block { id, block, .. },
                ..
            },
        ) => view.update(cx, |this, cx| {
            this.block_view(id, block, cx);
            this.run_handles(row, cx);
        }),
        Some(row) => view.update(cx, |this, cx| this.run_handles(row, cx)),
        None => {}
    }
    let this = view.read(cx);
    let Some(row) = this.rows.get(ix).copied() else {
        return div().into_any_element();
    };
    if let RowKey::Thinking(id) = row.key {
        this.measure_thought(id, window, cx);
    }
    this.drawing_row.set(ix);
    let content = this.row(row, &view, cx);
    let run_texts = this.drawn_runs.take();
    // The pointer over any row of an entry with hover actions shows them,
    // the gap above a row included, so crossing from one row to the next
    // or onto the actions keeps them.
    let hover = match row.key {
        RowKey::User(_) | RowKey::Actions(_) => true,
        RowKey::Block { id, .. } => this
            .model
            .entry(id)
            .is_some_and(|entry| entry.item.kind == Kind::Reply),
        RowKey::Turn(_) | RowKey::Notice(_) => this.offers_regenerate(row.key),
        _ => false,
    };
    let hover_view = view.downgrade();
    let key = row.key;
    let gap = gap_rems(row.gap);
    let rail = row.rail;
    // The row the keyboard is on shows a ring, laid over it so nothing
    // moves, while the keyboard is in use.
    let cursor = this.is_cursor(row.key, window, cx);
    let ring = (cursor && window.last_input_was_keyboard()).then(|| {
        let theme = cx.theme();
        div()
            .absolute()
            .top_neg_0p5()
            .bottom_neg_0p5()
            .left(rems(-0.5))
            .right(rems(-0.5))
            .border_2()
            .border_color(theme.ring)
            .rounded(theme.radius)
    });
    // To assistive technology, a list item it can move through: the one
    // the keyboard is on is the one in focus.
    let announced = this.row_announced(row.key);
    let label = this.row_label(row.key);
    let expanded = this.row_expanded(row);
    // The reading column: centred and at most ~46 rem wide, with the side
    // padding inside it, so narrow windows read edge to edge.
    let element = div()
        .id(element_id(row.key))
        .test_support()
        // (A row with no role is left out of what assistive technology sees.)
        .when(announced, |this| {
            this.role(Role::ListItem)
                .aria_label(label)
                .when_some(expanded, |this, expanded| this.aria_expanded(expanded))
                .when(cursor, |this| this.aria_active_descendant())
        })
        .w_full()
        .when(rail == Rail::No, |this| this.pt(rems(gap)))
        .flex()
        .justify_center()
        .when(hover, |this| {
            this.on_hover(move |hovered, _, cx| {
                hover_view
                    .update(cx, |this, cx| this.row_hovered(key, *hovered, cx))
                    .ok();
            })
        })
        .child(
            div()
                .relative()
                .w_full()
                .max_w(rems(COLUMN))
                .px_4()
                // On an open row's rail, the rail hangs from the row's
                // marker, through the gap above unless it starts here, and
                // the row stands at the row's words.
                .when(rail != Rail::No, |this| {
                    let line = cx.theme().border;
                    this.pt(rems(gap)).pl(rems(1. + WORDS)).child(
                        div()
                            .absolute()
                            .left(rems(1. + RAIL_AT))
                            .top(rems(if rail == Rail::First { gap } else { 0. }))
                            .bottom_0()
                            .w(px(RAIL_WIDTH))
                            .bg(line),
                    )
                })
                .child(div().relative().w_full().child(content).children(ring)),
        )
        .into_any_element();
    if !run_texts.is_empty() {
        view.update(cx, |this, cx| this.keep_run_texts(run_texts, cx));
    }
    element
}

/// The space above a row, in rem.
pub(super) fn gap_rems(gap: Gap) -> f32 {
    match gap {
        Gap::None => 0.,
        Gap::Stack => 0.25,
        Gap::Small => 0.5,
        Gap::Apart => 0.75,
        Gap::Section => 1.,
        Gap::Turn => 1.5,
    }
}

/// What the row at `ix` of `view`'s list will measure at `width`, the
/// space above it included, before the list draws it ([`super::height`]).
/// It depends only on the row and the width, so that the list can scale a
/// height it measured at another width by it.
pub(super) fn estimate(
    view: &Entity<TranscriptView>,
    ix: usize,
    width: Pixels,
    window: &mut Window,
    cx: &mut App,
) -> Pixels {
    let this = view.read(cx);
    let Some(row) = this.rows.get(ix).copied() else {
        return px(0.);
    };
    let Some(metrics) = &this.metrics else {
        return window.rem_size() * 2.;
    };
    px(this.row_height(row, f32::from(width), metrics, cx))
}

/// The height of `count` thumbnails wrapped in `width`, in pixels.
fn thumbnails_height(count: usize, width: f32, rem: f32) -> f32 {
    let (tile, height, gap) = (THUMBNAIL.0 * rem, THUMBNAIL.1 * rem, 0.5 * rem);
    let per_row = (((width + gap) / (tile + gap)).floor() as usize).max(1);
    let rows = count.div_ceil(per_row).max(1);
    rows as f32 * height + (rows - 1) as f32 * gap
}

impl TranscriptView {
    /// The height `row` will have at the list's `width`, in pixels, as its
    /// layout below draws it.
    pub(super) fn row_height(
        &self,
        row: super::model::Row,
        width: f32,
        m: &Metrics,
        cx: &App,
    ) -> f32 {
        let rem = m.rem;
        let inner = row.rail != Rail::No;
        // The reading column, inside its side padding, and on a rail past
        // the open row's marker.
        let column = width.min(COLUMN * rem) - 2. * rem - if inner { WORDS * rem } else { 0. };
        let body = match row.key {
            RowKey::Separator(_) | RowKey::Turn(_) => m.line_xs,
            // Lines of work, and extra-small buttons.
            RowKey::Working(_)
            | RowKey::Refresh(_)
            | RowKey::Actions(_)
            | RowKey::Thinking(_)
            | RowKey::Count(_) => LINE * rem,
            // A small button.
            RowKey::Worker(_) => 1.5 * rem,
            RowKey::User(id) => self.user_height(id, column, m),
            RowKey::Group(first) => self.group_height(first, width, column, m, cx),
            RowKey::Call(id) => self.call_height(id, width, inner, column, m, cx),
            RowKey::Notice(id) => self.notice_height(id, column, m),
            RowKey::Question(id) => self.question_height(id, column, m),
            RowKey::Output(id) => {
                let lines = self.ui.get(&id).map_or(&[][..], |ui| &ui.lines);
                // Inside its border and left padding.
                m.mono.text_lines(lines, column - 0.75 * rem - 2.) * m.line_mono
            }
            RowKey::Block {
                id, block, source, ..
            } => self.block_height(id, block, source, column, m),
        };
        gap_rems(row.gap) * rem + body
    }

    fn user_height(&self, id: u64, column: f32, m: &Metrics) -> f32 {
        let rem = m.rem;
        let Some(item) = self.item(id) else {
            return 0.;
        };
        let bubble = bubble_width(column, rem);
        let border = if item.pending { 2. } else { 0. };
        let inside = bubble - 1.5 * rem - border;
        let lines = self.ui.get(&id).map_or(&[][..], |ui| &ui.lines);
        let mut content = m.body.text_lines(lines, inside) * m.line;
        if !user_tags(item).is_empty() {
            content += m.line_xs + 0.25 * rem;
        }
        if !item.images.is_empty() {
            content += thumbnails_height(item.images.len(), inside, rem) + 0.25 * rem;
        }
        // The bubble's padding, then the actions under it.
        content + rem + border + 0.25 * rem + 1.25 * rem
    }

    /// The height of a call's row, its line and what shows under it, in
    /// a list `width` wide, on a rail (`inner`) or not, in `column`.
    fn call_height(
        &self,
        id: u64,
        width: f32,
        inner: bool,
        column: f32,
        m: &Metrics,
        cx: &App,
    ) -> f32 {
        let rem = m.rem;
        let Some(ui) = self.ui.get(&id) else {
            return 0.;
        };
        let mut height = LINE * rem;
        // The thumbnails and what it holds start at its words.
        let inside = column - WORDS * rem;
        if ui.images > 0 {
            height += 0.25 * rem + thumbnails_height(ui.images, inside, rem);
        }
        if let Some(entry) = self.model.entry(id)
            && ui.expanded == Some(true)
        {
            let cut = self.call_cut(&entry.item, room(width, inner, rem), m);
            if opens(entry) || cut {
                let whole = cut && !strip_says_it(&entry.item);
                let whole = whole.then(|| entry.item.text.trim());
                height += self.details_height(entry, whole, inside, m, cx);
            }
        }
        height
    }

    /// The height of a Quiet row, its line and what shows under it, in a
    /// list `width` wide, in `column`.
    fn group_height(&self, first: u64, width: f32, column: f32, m: &Metrics, cx: &App) -> f32 {
        let rem = m.rem;
        let mut height = LINE * rem;
        let Some(row) = self.quiet.get(&first) else {
            return height;
        };
        let cut = self.row_cut(row, room(width, false, rem), m);
        let open = (row.opens || cut) && self.group_open(first);
        let inside = column - WORDS * rem;
        if !(open && row.merged) {
            let tiles: Vec<f32> = row
                .members
                .iter()
                .filter_map(|id| self.ui.get(id))
                .filter(|ui| ui.images > 0)
                .map(|ui| thumbnails_height(ui.images, inside, rem))
                .collect();
            if !tiles.is_empty() {
                height +=
                    0.25 * rem + tiles.iter().sum::<f32>() + 0.5 * rem * (tiles.len() - 1) as f32;
            }
        }
        if open
            && !row.merged
            && let Some(entry) = self.model.entry(first)
        {
            let whole = cut && !strip_says_it(&entry.item);
            let whole = whole.then(|| entry.item.text.trim());
            height += self.details_height(entry, whole, inside, m, cx);
        }
        height
    }

    /// The height of a call's details, as [`Self::call_details`] lays them
    /// out in `width`.
    fn details_height(
        &self,
        entry: &super::model::Entry,
        whole: Option<&str>,
        width: f32,
        m: &Metrics,
        cx: &App,
    ) -> f32 {
        if !opens(entry) && whole.is_none() {
            return 0.;
        }
        let rem = m.rem;
        let item = &entry.item;
        let mono = |text: &str, width: f32| {
            m.mono.text_lines(&super::height::hard_lines(text), width) * m.line_mono
        };
        let door = |text: &str| preview(text).1.then_some(1.25 * rem);
        let given = shown_given(item);
        let output = shown_output(entry);
        let mut parts = Vec::new();
        parts.extend(whole.map(|whole| mono(whole, width)));
        if let Some(Given::Parameters(parameters)) = &given {
            // Captions over blocks padded 0.5 rem each side.
            let block = |text: &str| 0.75 * rem + mono(&preview(text).0, width - rem);
            parts.push(m.line_xs);
            parts.push(block(parameters));
            parts.extend(door(parameters));
            if let Some(output) = &output {
                parts.push(m.line_xs);
                parts.push(block(output));
                parts.extend(door(output));
            }
        } else {
            if let Some(diff) = self.ui.get(&item.id).and_then(|ui| ui.diff.clone()) {
                let rows = diff.read(cx).row_count().clamp(1, DIFF_ROWS);
                parts.push(f32::from(DiffView::row_height(cx)) * rows as f32 + 2.);
            }
            let detail = match &given {
                Some(Given::Detail(detail)) => Some(detail.clone()),
                _ => None,
            };
            let (strip, noted) = match detail {
                Some(detail) if item.tool.as_deref() == Some("shell") => (Some(detail), None),
                detail => (None, detail),
            };
            // Inside the card's border and its sides' padding.
            let inside = width - 2. - 1.5 * rem;
            let body = (noted.is_some() || output.is_some()).then(|| {
                let mut lines = Vec::new();
                if let Some(noted) = &noted {
                    lines.push(mono(noted, inside));
                }
                if let Some(output) = &output {
                    lines.push(mono(&preview(output).0, inside));
                }
                0.75 * rem + lines.iter().sum::<f32>() + 0.25 * rem * (lines.len() - 1) as f32
            });
            let strip = strip.map(|command| {
                // Beside the copy button, after each command's `$`.
                let advance = m.mono.advance();
                let text = inside - 0.5 * rem - 1.25 * rem - 2. * advance;
                let parts = chain(&command);
                let rows: f32 = parts
                    .iter()
                    .map(|part| mono(&command[part.clone()], text))
                    .sum::<f32>()
                    + CHAIN_GAP * parts.len().saturating_sub(1) as f32;
                0.5 * rem + rows.max(1.25 * rem) + if body.is_some() { 1. } else { 0. }
            });
            if strip.is_some() || body.is_some() {
                parts.push(2. + strip.unwrap_or(0.) + body.unwrap_or(0.));
            }
            parts.extend(output.as_deref().and_then(door));
        }
        // The space above them, and the gaps between them.
        0.25 * rem + parts.iter().sum::<f32>() + 0.25 * rem * parts.len().saturating_sub(1) as f32
    }

    fn notice_height(&self, id: u64, column: f32, m: &Metrics) -> f32 {
        let rem = m.rem;
        let Some(item) = self.item(id) else {
            return 0.;
        };
        // Past its icon, in the words' column.
        let mut width = column - WORDS * rem;
        if !item.is_main() {
            let agent = self.agent_label(item).chars().count() as f32;
            width -= agent * m.mono.advance_at(0.75 * rem) + GAP * rem;
        }
        if self.offers_regenerate(RowKey::Notice(id)) {
            width -= REGENERATE_ROOM * rem;
        }
        let lines = self.ui.get(&id).map_or(&[][..], |ui| &ui.lines);
        m.body_at(0.875).text_lines(lines, width) * m.line_sm
    }

    /// The height of a card of the agent's questions, as [`Self::questions`]
    /// lays it out: its line, then, in a box once one has settled, each
    /// settled question's text and what became of it, and under the box the
    /// bubble of the answers given in the answer panel.
    fn question_height(&self, id: u64, column: f32, m: &Metrics) -> f32 {
        let rem = m.rem;
        let Some(questions) = self.item(id).and_then(|item| item.questions.as_ref()) else {
            return 0.;
        };
        // The room a question's number takes before its text, past the gap.
        let number_room = |number: Option<u32>| {
            number.map_or(0., |number| {
                let chars = number.to_string().len() as f32 + 1.;
                (chars * m.body.advance_at(rem)).max(rem) + 0.375 * rem
            })
        };
        let mut height = m.line_sm;
        let mut inside = column;
        if questions.iter().any(|question| !question.state.is_open()) {
            // The box's padding (0.75 rem) and border (1 px) on each side.
            let inset = 2. * (0.75 * rem + 1.);
            height += inset;
            inside -= inset;
        }
        let mut answers = Vec::new();
        for question in questions {
            if question.state.is_open() {
                continue;
            }
            // The text and the outcome, past its number.
            let width = inside - number_room(question.number);
            let lines = super::height::hard_lines(&crate::questions::text(question));
            height += 0.5 * rem + m.body.text_lines(&lines, width) * m.line;
            match crate::questions::answer(&question.state) {
                Some(answer) => answers.push((question.number, answer)),
                None => {
                    let outcome = crate::questions::outcome(&question.state).unwrap_or_default();
                    let lines = super::height::hard_lines(&outcome);
                    height += m.body_at(0.875).text_lines(&lines, width) * m.line_sm;
                }
            }
        }
        if !answers.is_empty() {
            // The gap above the bubble, its padding, and its tag.
            let inside = bubble_width(column, rem) - 1.5 * rem;
            height += ANSWERS_GAP * rem + rem + m.line_xs;
            for (number, answer) in answers {
                let lines = super::height::hard_lines(answer);
                height +=
                    0.25 * rem + m.body.text_lines(&lines, inside - number_room(number)) * m.line;
            }
        }
        height
    }

    fn block_height(&self, id: u64, block: usize, source: bool, column: f32, m: &Metrics) -> f32 {
        use super::height::BlockFacts;
        let rem = m.rem;
        let Some(ui) = self.ui.get(&id) else {
            return 0.;
        };
        let Some(row) = ui.doc.as_ref().and_then(|doc| doc.blocks.get(block)) else {
            return 0.;
        };
        // A thought's text is small, on lines of work's height, in the
        // words' column.
        let (body, line, column) = if ui.small {
            (m.body_at(THOUGHT_SIZE), LINE * rem, column - WORDS * rem)
        } else {
            (m.body, m.line, column)
        };
        // "Question" or "From …" over the first block.
        let label = if block == 0 && ui.labeled {
            m.line_xs + 0.25 * rem
        } else {
            0.
        };
        let text = if source {
            if self.shows_source(id) {
                m.mono.text_lines(&row.lines, column) * m.line_mono
            } else {
                body.text_lines(&row.lines, column) * line
            }
        } else {
            match &row.facts {
                BlockFacts::Prose(chars) => body.lines(*chars, column) * line,
                BlockFacts::Quote(chars) => body.lines(*chars, column - 2. * rem - 3.) * line,
                BlockFacts::Heading { level, chars } => {
                    let (font, line) = m.headings[(*level).clamp(1, 6) as usize - 1];
                    font.lines(*chars, column) * line + 0.3 * rem
                }
                BlockFacts::List(items) => items
                    .iter()
                    .map(|(depth, chars)| {
                        let indent = rem * (1. + *depth as f32);
                        body.lines(*chars, column - indent) * line
                    })
                    .sum(),
                BlockFacts::Code(lines) => {
                    let shown = if ui.uncapped.contains(&block) {
                        lines.len()
                    } else {
                        lines.len().min(CODE_LINES)
                    };
                    // Inside its padding and the hairline around it, long lines wrapped.
                    let chrome = 1.5 * rem + 2.;
                    chrome + m.mono.text_lines(&lines[..shown], column - chrome) * m.line_mono
                }
                // Each block has its own text view, so its table has no trailing gap.
                BlockFacts::Table(rows) => *rows as f32 * (line + 0.5 * rem + 1.) + 1.,
                BlockFacts::Rule => 2.,
                BlockFacts::Other => line,
            }
        };
        // "Show all N lines" under code over the cap.
        let toggle = if !source && row.code_lines > CODE_LINES {
            0.25 * rem + 1.25 * rem
        } else {
            0.
        };
        label + text + toggle
    }
}

/// One number for a block's row: its entry, its document's generation, its
/// index and whether it shows as its source.
fn block_number(id: u64, generation: u32, block: usize, source: bool) -> u64 {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    (id, generation, block, source).hash(&mut hasher);
    hasher.finish()
}

/// The id of a row's element: stable while the row keeps its key. Made
/// with each frame, so it is made of numbers, not formatted.
pub(super) fn element_id(key: RowKey) -> ElementId {
    let (name, id): (&'static str, u64) = match key {
        RowKey::Separator(id) => ("separator", id),
        RowKey::User(id) => ("user", id),
        RowKey::Refresh(id) => ("refresh", id),
        RowKey::Block {
            id,
            generation,
            block,
            source,
        } => ("block", block_number(id, generation, block, source)),
        RowKey::Actions(id) => ("actions", id),
        RowKey::Thinking(id) => ("thinking", id),
        RowKey::Call(id) => ("call", id),
        RowKey::Group(id) => ("group", id),
        RowKey::Count(id) => ("count", id),
        RowKey::Notice(id) => ("notice", id),
        RowKey::Output(id) => ("output", id),
        RowKey::Turn(id) => ("turn", id),
        RowKey::Worker(id) => ("worker", id),
        RowKey::Question(id) => ("questions", id),
        RowKey::Working(id) => ("working", id),
    };
    (name, id).into()
}

/// The running indicator, turning unless the system asks for less motion. It
/// is the kit's spinner, which turns evenly in each frame the display shows
/// and turns as it paints, so the transcript isn't drawn again for each step,
/// however many turn.
fn spinner(id: impl Into<ElementId>, cx: &App) -> AnyElement {
    if cx.reduce_motion() {
        return Icon::new(IconName::Loader).xsmall().into_any_element();
    }
    div()
        .id(id)
        .test_support()
        .child(crate::spinner::spinner().xsmall())
        .into_any_element()
}

/// "12 s", "1 m 5 s", "2 h 3 m".
pub(crate) fn duration(ms: i64) -> String {
    let seconds = (ms.max(0) + 500) / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} m {} s", seconds / 60, seconds % 60),
        _ => format!("{} h {} m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// How long a background job has run, in a few words, as Arko says it:
/// `40 s`, `4 min`, `2 h 5 min`.
pub(super) fn job_time(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} min", seconds / 60),
        _ if seconds % 3600 < 60 => format!("{} h", seconds / 3600),
        _ => format!("{} h {} min", seconds / 3600, seconds % 3600 / 60),
    }
}

/// What a call's row says of the background job the call made: the call
/// moved there, it started a command there (`terminal_start`), or it
/// started a monitor (`monitor_start`).
#[derive(Debug, Clone, PartialEq)]
pub(super) enum JobLine {
    /// The job runs, since `since` (Unix milliseconds). The row offers to
    /// stop it.
    Running { job: SharedString, since: i64 },
    /// The monitor `job` runs, has alerted `alerts` times so far, and ends
    /// at `ends_at` (Unix milliseconds). The row offers to stop it.
    Monitoring {
        job: SharedString,
        alerts: u32,
        ends_at: Option<i64>,
    },
    /// The call moved to the background, and how its job does isn't known.
    Moved,
    /// How the job did, as Arko words it ("Finished in the background · 4
    /// min", "Stopped", "Ended after 30 min · 4 alerts"), with the failure
    /// mark when it failed.
    Outcome { text: String, failed: bool },
}

impl TranscriptView {
    pub(super) fn job_line(&self, item: &Item) -> Option<JobLine> {
        let job = item.job.as_deref()?;
        let summary = self.model.job(job);
        if let Some(summary) = summary.filter(|job| job.kind == JobKind::Monitor) {
            return Some(match summary.state.is_running() {
                true => JobLine::Monitoring {
                    job: job.to_string().into(),
                    alerts: summary.alerts.unwrap_or(0),
                    ends_at: summary.expires_at,
                },
                false => {
                    let (text, failed) = monitor_ended(summary);
                    JobLine::Outcome { text, failed }
                }
            });
        }
        if let Some(summary) = summary.filter(|job| job.state.is_running()) {
            return Some(JobLine::Running {
                job: job.to_string().into(),
                since: summary.started_at,
            });
        }
        // Arko's words for a moved call: "in the background · job 4d2c1a9b"
        // until its job ends, then how it ended. A monitor's call says
        // "monitoring · 3 alerts" while it runs, then how it ended, which
        // tells it when the transcript doesn't have the monitor's summary
        // (the summaries of jobs that ended long ago are let go).
        let outcome = item.outcome.as_deref()?;
        if outcome.starts_with("in the background") {
            return Some(JobLine::Moved);
        }
        let monitor = item.tool.as_deref() == Some("monitor_start")
            || (item.tool.is_none() && Tool::of(item) == Tool::Monitor);
        let shown = outcome.starts_with("finished in the background")
            || outcome.starts_with("failed in the background")
            || outcome == "stopped"
            || outcome == "interrupted"
            || (monitor
                && ["monitoring", "ended", "stopped", "command exited"]
                    .iter()
                    .any(|start| outcome.starts_with(start)));
        shown.then(|| JobLine::Outcome {
            text: capitalized(outcome),
            failed: item.status == Some(CallStatus::Failed) && !outcome.starts_with("stopped"),
        })
    }

    /// What a call's row says of its job, `line`, now.
    fn job_words(&self, line: &JobLine) -> String {
        match line {
            JobLine::Running { since, .. } => {
                format!("In the background · {}", job_time((self.clock)() - since))
            }
            JobLine::Monitoring {
                alerts, ends_at, ..
            } => monitoring(*alerts, ends_at.map(|at| at - (self.clock)())),
            JobLine::Moved => "In the background".into(),
            JobLine::Outcome { text, .. } => text.clone(),
        }
    }

    /// What the row of the call `item` says of its job, as it draws now.
    #[cfg(test)]
    pub(crate) fn job_text(&self, item: u64) -> Option<String> {
        let line = self.job_line(&self.model.entry(item)?.item)?;
        Some(self.job_words(&line))
    }
}

/// How a monitor that ended did, as Arko words it on its call's line, and
/// whether it failed: "Ended after 30 min · 4 alerts", "Stopped · 2
/// alerts", "Command exited (exit 1)".
pub(super) fn monitor_ended(job: &JobSummary) -> (String, bool) {
    let alerts = match job.alerts.unwrap_or(0) {
        0 => String::new(),
        1 => " · 1 alert".to_string(),
        count => format!(" · {count} alerts"),
    };
    match job.state {
        JobState::Expired => (
            match job.ended_at {
                Some(at) => format!("Ended after {}{alerts}", job_time(at - job.started_at)),
                None => format!("Ended{alerts}"),
            },
            false,
        ),
        JobState::Finished => (format!("Command exited{alerts}"), false),
        JobState::Failed => (
            match &job.outcome {
                Some(how) => format!("Command exited ({how}){alerts}"),
                None => format!("Command exited{alerts}"),
            },
            true,
        ),
        JobState::Interrupted => ("Interrupted".to_string(), true),
        _ => (format!("Stopped{alerts}"), false),
    }
}

/// What a running monitor's row says: "Monitoring · 3 alerts · ends in 22
/// min", `left` milliseconds before it ends. Its minutes round as Arko's
/// `/jobs` rounds them.
pub(super) fn monitoring(alerts: u32, left: Option<i64>) -> String {
    let mut parts = vec!["Monitoring".to_string()];
    match alerts {
        0 => {}
        1 => parts.push("1 alert".into()),
        count => parts.push(format!("{count} alerts")),
    }
    if let Some(left) = left {
        parts.push(match left.max(0) / 1000 {
            0..60 => "ends in less than a minute".to_string(),
            seconds => format!("ends in {} min", (seconds + 30) / 60),
        });
    }
    parts.join(" · ")
}

/// How long until a running monitor's "ends in" changes, `left`
/// milliseconds before it ends, or `None` once it says less than a minute
/// (its end comes from the feed).
pub(super) fn monitoring_changes_in(left: i64) -> Option<Duration> {
    if left < 60_000 {
        return None;
    }
    // The minutes round to the nearest one, and the last minute reads
    // "less than a minute".
    let rounded = (left + 30_000) % 60_000;
    let last = left - 60_000;
    Some(Duration::from_millis(rounded.min(last) as u64 + 50))
}

/// `text` with its first letter in capitals.
pub(super) fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// The line over a message in the transcript `filter`: who sent a message
/// an agent got ("From the lead", "From r1"), and in the main transcript
/// whose a worker's message to the user is ("r3 to you").
pub(super) fn message_label(filter: &AgentFilter, item: &Item) -> Option<String> {
    if item.kind != Kind::Message {
        return None;
    }
    match &item.from {
        Some(from) if from == arko::remote::MAIN_AGENT => Some("From the lead".to_string()),
        Some(from) => Some(format!("From {from}")),
        None => super::model::to_you(filter, item),
    }
}

/// The muted words on the first line of a user's message, for what its
/// place doesn't already show. A note the agent hasn't read is "Pending", and
/// once it has, it is a message like any other. A slash command reads as one
/// by itself.
fn user_tags(item: &Item) -> Vec<String> {
    let mut tags = Vec::new();
    match item.delivery {
        Some(Delivery::Note) if item.pending => tags.push("Pending".into()),
        Some(Delivery::Answer) => tags.push(ANSWER_TAG.into()),
        Some(Delivery::Queued) => tags.push("Queued".into()),
        // A turn, a read note, a command, a refresh, and a delivery a later
        // arko adds.
        _ => {}
    }
    if item.source == Some(Source::Web) {
        tags.push("From the web page".into());
    }
    tags
}

/// The lines of a fenced or indented code block's code, from its source.
pub(crate) fn code_lines(source: &str) -> usize {
    let fence = |line: &str| {
        let line = line.trim();
        line.starts_with("```") || line.starts_with("~~~")
    };
    let mut lines = source.lines();
    let mut count = 0;
    let mut first = true;
    let mut last_is_fence = false;
    let mut fenced = false;
    for line in lines.by_ref() {
        if first {
            first = false;
            if fence(line) {
                fenced = true;
                continue;
            }
        }
        count += 1;
        last_is_fence = fence(line);
    }
    if fenced && last_is_fence {
        count -= 1;
    }
    count
}

/// Images in Markdown come only from the text itself (`data:` URLs), never
/// from the network.
fn local_images_only(uri: &SharedUri) -> ImageSource {
    if uri.starts_with("data:") {
        ImageSource::from(uri.clone())
    } else {
        ImageSource::Resource(Resource::Embedded("".into()))
    }
}

/// How the rows' Markdown looks: the theme's text style, with code blocks on
/// the editor surface and a hairline, as in GPUI Kit's own Markdown, and tables
/// that scroll sideways inside their row rather than squeeze their columns.
pub(super) fn text_style(cx: &App) -> TextViewStyle {
    let theme = cx.theme();
    let radius = theme.semantic_tokens().radius.md;
    let mut rounded = StyleRefinement::default();
    rounded.corner_radii.top_left = Some(radius.into());
    rounded.corner_radii.top_right = Some(radius.into());
    rounded.corner_radii.bottom_left = Some(radius.into());
    rounded.corner_radii.bottom_right = Some(radius.into());
    let mut table = rounded.clone();
    table.overflow.x = Some(Overflow::Scroll);
    TextViewStyle::default()
        .with_foreground(theme.foreground)
        .with_muted_foreground(theme.muted_foreground)
        .with_link(theme.link)
        .with_selection(theme.selection)
        .with_code_background(theme.editor_background())
        .with_code_border(theme.border)
        .with_border(theme.border)
        .with_code_block(rounded)
        .with_table(table)
        .with_table_head(
            StyleRefinement::default()
                .bg(theme.table_head)
                .text_color(theme.table_head_foreground),
        )
        .with_inline_code(HighlightStyle {
            background_color: Some(theme.accent),
            ..Default::default()
        })
        .with_dark(theme.is_dark())
}

/// The lines of the longest fenced code block in `source`, which may be a
/// list or a quote that holds code blocks.
pub(crate) fn longest_code(source: &str) -> usize {
    // A line without its indentation and quote markers.
    let bare = |line: &str| {
        let mut line = line.trim_start();
        while let Some(rest) = line.strip_prefix('>') {
            line = rest.trim_start();
        }
        line.to_string()
    };
    let fence = |line: &str| {
        let mark = line.chars().next().filter(|c| *c == '`' || *c == '~')?;
        let length = line.chars().take_while(|c| *c == mark).count();
        (length >= 3).then_some((mark, length))
    };
    let mut longest = 0;
    let mut open: Option<(char, usize, usize)> = None;
    for line in source.lines() {
        let line = bare(line);
        match &mut open {
            Some((mark, length, lines)) => {
                let closes = fence(&line).is_some_and(|(m, n)| m == *mark && n >= *length)
                    && line.trim_start_matches(*mark).trim().is_empty();
                if closes {
                    longest = longest.max(*lines);
                    open = None;
                } else {
                    *lines += 1;
                }
            }
            None => open = fence(&line).map(|(mark, length)| (mark, length, 0)),
        }
    }
    open.map_or(longest, |(_, _, lines)| longest.max(lines))
}

fn stream_motion(fade: bool) -> TextViewMotion {
    if fade {
        TextViewMotion::default()
            .with_stream_fade(std::time::Duration::from_millis(280))
            .with_stream_fade_stagger(std::time::Duration::from_millis(10))
            .with_stream_fade_easing(Easing::EaseOut)
    } else {
        TextViewMotion::default()
    }
}

impl TranscriptView {
    fn row(&self, row: Row, view: &Entity<Self>, cx: &App) -> AnyElement {
        let inner = row.rail != Rail::No;
        match row.key {
            RowKey::Separator(id) => self.separator(id, cx),
            RowKey::User(id) => self.user(id, view, cx),
            RowKey::Refresh(_) => self.refresh(cx),
            RowKey::Block {
                id, block, source, ..
            } => self.block(id, block, source, view, cx),
            RowKey::Actions(id) => self.actions(id, view, cx),
            RowKey::Thinking(id) => self.thinking(id, inner, view, cx),
            RowKey::Call(id) => self.call(id, inner, view, cx),
            RowKey::Group(id) => self.group(id, view, cx),
            RowKey::Count(id) => self.count(id, view, cx),
            RowKey::Notice(id) => self.notice(id, view, cx),
            RowKey::Output(id) => self.output(id, cx),
            RowKey::Turn(id) => self.turn(id, view, cx),
            RowKey::Worker(id) => self.worker(id, view, cx),
            RowKey::Question(id) => self.questions(id, view, cx),
            RowKey::Working(id) => self.working(id, cx),
        }
    }

    fn item(&self, id: u64) -> Option<&Item> {
        self.model.entry(id).map(|entry| &entry.item)
    }

    fn separator(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let text = self
            .item(id)
            .map(|item| item.text.clone())
            .unwrap_or_default();
        let line = || div().flex_1().h_px().bg(theme.border);
        h_flex()
            .w_full()
            .gap_3()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(line())
            .child(text)
            .child(line())
            .into_any_element()
    }

    /// The quiet line of a message that only kept the prompt cache warm, a
    /// line of work in the words' column.
    fn refresh(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        div()
            .w_full()
            .h(rems(LINE))
            .pl(rems(WORDS))
            .flex()
            .items_center()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(CACHE_REFRESHED)
            .into_any_element()
    }

    fn user(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let tags = user_tags(item);
        // Made when the message comes, not with each frame.
        let text = self
            .ui
            .get(&id)
            .and_then(|ui| ui.text.clone())
            .unwrap_or_else(|| item.text.clone().into());
        let muted = theme.muted_foreground;
        // As the bubble draws it: a note the agent hasn't read is an
        // outline in muted text.
        let ink = if item.pending {
            Ink::new(muted)
        } else {
            Ink::on(theme.foreground, bubble_color(theme))
        };
        let copy_view = view.downgrade();
        // Edit and Branch, for a message the conversation still has.
        let changes = self.is_editable(id).then(|| {
            let busy = self.busy_reason();
            // The latest message: Alt+↑ in the empty composer edits it too.
            let last = self.model.last_message() == Some(id);
            (busy, last, view.downgrade(), view.downgrade())
        });
        // Edit, Branch and Copy show on hover, in a row under the bubble at
        // its right edge.
        let actions = HoverActions {
            id: ("user-actions", id).into(),
            shown: self.hovered == Some(id),
            build: Box::new(move |shown| {
                let action = |name: &'static str, icon: Lucide| {
                    Button::new((name, id))
                        .ghost()
                        .xsmall()
                        .icon(Icon::new(icon))
                        .text_color(muted)
                        .tab_stop(shown)
                };
                h_flex()
                    .when_some(changes, |this, (busy, last, edit_view, branch_view)| {
                        let edit = action("edit-message", Lucide::SquarePen)
                            .disabled(busy.is_some())
                            .accessibility_label("Edit");
                        let edit = match busy {
                            Some(busy) => edit.tooltip(busy),
                            None if last => edit.tooltip_with_action(
                                "Edit",
                                &crate::composer::EditLastMessage,
                                Some(crate::composer::CONTEXT),
                            ),
                            None => edit.tooltip("Edit"),
                        };
                        this.child(
                            edit
                                .on_click(move |_, _, cx| {
                                    edit_view
                                        .update(cx, |this, cx| {
                                            this.change(TranscriptEvent::Edit(id), cx)
                                        })
                                        .ok();
                                }),
                        )
                        .child(
                            action("branch-message", Lucide::GitBranch)
                                .disabled(busy.is_some())
                                .tooltip(busy.unwrap_or(
                                    "Branch into a new session with the conversation before this message",
                                ))
                                .accessibility_label("Branch")
                                .on_click(move |_, _, cx| {
                                    branch_view
                                        .update(cx, |this, cx| {
                                            this.change(TranscriptEvent::Branch(id), cx)
                                        })
                                        .ok();
                                }),
                        )
                    })
                    .child(
                        div().id(("copy-user", id)).test_support().child(
                            Clipboard::new("button")
                                .tooltip("Copy")
                                .accessibility_label("Copy message")
                                .text_color(muted)
                                .tab_stop(shown)
                                .value_fn(move |_, cx| {
                                    copy_view
                                        .upgrade()
                                        .and_then(|view| view.read(cx).source_text(id))
                                        .unwrap_or_default()
                                        .into()
                                }),
                        ),
                    )
                    .into_any_element()
            }),
        };
        // Its actions keep their room under it, shown or not, so showing
        // them moves nothing.
        let bubble = bubble(item.pending, theme)
            .id(("user-message", id))
            .test_support()
            .when(!tags.is_empty(), |this| {
                this.child(div().text_xs().text_color(muted).child(tags.join(" · ")))
            })
            .children(self.thumbnails(item, view, cx))
            // As wide as its words on one line, up to the bubble's widest,
            // where it wraps.
            .child(self.marked(("user-text", id), id, Place::Text, text, ink, cx));
        at_right(bubble, Some(actions.into_any_element())).into_any_element()
    }

    fn block(
        &self,
        id: u64,
        block: usize,
        source: bool,
        view: &Entity<Self>,
        cx: &App,
    ) -> AnyElement {
        let theme = cx.theme();
        let (Some(item), Some(ui)) = (self.item(id), self.ui.get(&id)) else {
            return div().into_any_element();
        };
        let Some(doc) = ui.doc.as_ref() else {
            return div().into_any_element();
        };
        let Some(row) = doc.blocks.get(block) else {
            return div().into_any_element();
        };
        let thinking = item.kind == Kind::Thinking;
        let label = (block == 0 && item.kind == Kind::Question).then(|| {
            h_flex()
                .gap_1()
                .pb_1()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(Icon::new(Lucide::MessageCircleQuestionMark).xsmall())
                .child("Arko asked")
        });
        let from = (block == 0)
            .then(|| message_label(self.model.filter(), item))
            .flatten()
            .map(|from| {
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(from)
            });
        let body = if source {
            // As text: the block's Markdown source, in the code font when
            // the reader asked for the source, as typed when Markdown is off.
            let text = row
                .range
                .clone()
                .and_then(|range| doc.text.get(range))
                .unwrap_or_default()
                .trim_end()
                .to_string();
            div()
                .when(self.shows_source(id), |this| {
                    this.code_font(theme).text_size(theme.mono_font_size)
                })
                .child(self.marked(
                    ("source", id),
                    id,
                    Place::Source(block),
                    text,
                    Ink::new(if thinking {
                        theme.muted_foreground
                    } else {
                        theme.foreground
                    }),
                    cx,
                ))
                .into_any_element()
        } else {
            // The longest code block of the row, at the top level or inside
            // a list item or a quote: the cap and the header apply to every
            // code block the row draws.
            let lines = row.code_lines;
            let uncapped = ui.uncapped.contains(&block);
            let Some(row_view) = row.view.as_ref() else {
                return div().into_any_element();
            };
            let text_view = TextView::new(row_view)
                .style(if thinking {
                    text_style(cx).with_foreground(theme.muted_foreground)
                } else {
                    text_style(cx)
                })
                .selectable(true)
                .document_position(self.document, self.selection_position(Place::Block(block)))
                .image_source(local_images_only)
                .motion(stream_motion(self.settings.stream_fade && !thinking))
                .code_block_actions(|code, _, cx| {
                    let theme = cx.theme();
                    let copy = code.code();
                    h_flex()
                        .gap_1()
                        .pl_1()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .when_some(code.lang(), |this, lang| this.child(lang))
                        .child(Clipboard::new("copy").tooltip("Copy").value(copy))
                })
                .when(!uncapped, |this| this.code_block_max_lines(CODE_LINES));
            let toggle = (lines > CODE_LINES).then(|| {
                let view = view.downgrade();
                Button::new("button")
                    .ghost()
                    .xsmall()
                    .label(if uncapped {
                        format!("Show the first {CODE_LINES} lines")
                    } else {
                        format!("Show all {lines} lines")
                    })
                    .on_click(move |_, _, cx| {
                        view.update(cx, |this, cx| this.toggle_code(id, block, cx))
                            .ok();
                    })
            });
            v_flex()
                .w_full()
                .child(text_view)
                .children(toggle.map(|toggle| {
                    h_flex()
                        .pt_1()
                        .child(div().id("show-all").test_support().child(toggle))
                }))
                .into_any_element()
        };
        if thinking {
            // An open thought, muted and italic in the words' column, with
            // the spinner beside its first block while it streams. A click
            // on it closes it, and a drag selects its words.
            let live = block == 0
                && self
                    .model
                    .position(id)
                    .is_ok_and(|ix| self.model.is_streaming(ix));
            let default = self.thinking_open_by_default();
            let view = view.downgrade();
            return h_flex()
                .id(("thought-text", block_number(id, 0, block, false)))
                .w_full()
                .items_start()
                .gap(rems(GAP))
                .text_color(theme.muted_foreground)
                .text_size(rems(THOUGHT_SIZE))
                .line_height(rems(LINE))
                .italic()
                .cursor_pointer()
                .child(div().flex_none().h(rems(LINE)).child(self.marker(
                    ("live", id),
                    Marker::live(live),
                    cx,
                )))
                .child(div().flex_1().min_w_0().child(body))
                .on_click(move |event, _, cx| {
                    if let ClickEvent::Mouse(click) = event {
                        let moved = click.up.position - click.down.position;
                        let far = f32::from(moved.x).abs() > 3. || f32::from(moved.y).abs() > 3.;
                        if far || click.down.click_count > 1 {
                            return;
                        }
                    }
                    view.update(cx, |this, cx| this.toggle(id, default, cx))
                        .ok();
                })
                .into_any_element();
        }
        div()
            .w_full()
            .children(label)
            .children(from)
            .child(body)
            .into_any_element()
    }

    /// The hover actions under a reply: they always take their space, so
    /// showing them moves nothing.
    fn actions(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let source = self.shows_source(id);
        let render_markdown = self.settings.render_markdown;
        let muted = theme.muted_foreground;
        let view = view.downgrade();
        // Regenerate, under the latest reply. It waits while anything runs.
        let regenerate = self.offers_regenerate(RowKey::Actions(id));
        let busy = self.busy_reason();
        HoverActions {
            id: ("reply-actions", id).into(),
            shown: self.hovered == Some(id),
            build: Box::new(move |shown| {
                let copy_view = view.clone();
                let menu_view = view.clone();
                h_flex()
                    .w_full()
                    .gap_1()
                    .child(
                        div().id(("copy-reply", id)).test_support().child(
                            Clipboard::new("button")
                                .tooltip("Copy")
                                .accessibility_label("Copy message")
                                .text_color(muted)
                                .tab_stop(shown)
                                .value_fn(move |_, cx| {
                                    copy_view
                                        .upgrade()
                                        .and_then(|view| view.read(cx).source_text(id))
                                        .unwrap_or_default()
                                        .into()
                                }),
                        ),
                    )
                    // The source is of use only while Markdown shows
                    // formatted, so the menu has nothing to offer otherwise.
                    .when(render_markdown, |this| {
                        this.child(
                            div().id(("reply-menu", id)).test_support().child(
                                Button::new("button")
                                    .ghost()
                                    .xsmall()
                                    .icon(Icon::new(Lucide::Ellipsis))
                                    .tooltip("More")
                                    .accessibility_label("More actions")
                                    .text_color(muted)
                                    .tab_stop(shown)
                                    .dropdown_menu(move |menu, _, _| {
                                        let view = menu_view.clone();
                                        menu.item(
                                            PopupMenuItem::new(source_label(source)).on_click(
                                                move |_, _, cx| {
                                                    view.update(cx, |this, cx| {
                                                        this.toggle_source(id, cx)
                                                    })
                                                    .ok();
                                                },
                                            ),
                                        )
                                    }),
                            ),
                        )
                    })
                    .when(regenerate, |this| {
                        // Alone at the right end, away from the others, so a
                        // click meant for one of them can't land on it.
                        this.child(div().flex_1().min_w(rems(REGENERATE_GAP)))
                            .child(regenerate_button(
                                id,
                                busy.unwrap_or(REGENERATE_HINT),
                                busy.is_some(),
                                shown,
                                muted,
                                view.clone(),
                            ))
                    })
                    .into_any_element()
            }),
        }
        .into_any_element()
    }

    /// The running indicator of the entry `id`.
    fn spinner(&self, id: u64, cx: &App) -> AnyElement {
        spinner(("running", id), cx)
    }

    /// What stands in a line's markers' column, ▸, ▾, the spinner or
    /// nothing, centred, in the line's font.
    fn marker(&self, id: impl Into<ElementId>, marker: Marker, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let glyph = match marker {
            Marker::Closed => Some("▸"),
            Marker::Open => Some("▾"),
            Marker::None | Marker::Live => None,
        };
        div()
            .flex_none()
            .w(rems(MARKER))
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .code_font(theme)
            .text_color(theme.muted_foreground)
            .when(marker == Marker::Live, |this| this.child(spinner(id, cx)))
            .children(glyph)
            .into_any_element()
    }

    /// "Working…" while the agent waits on the model and no other row says
    /// it's busy. The spinner stands in the markers' column and the words
    /// where a thought's start, so the thought that takes its place starts
    /// where it stood. The spinner alone moves.
    fn working(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .w_full()
            .h(rems(LINE))
            .gap(rems(GAP))
            .text_size(rems(THOUGHT_SIZE))
            .line_height(rems(LINE))
            .text_color(theme.muted_foreground)
            .child(self.marker(("working-spinner", id), Marker::Live, cx))
            .child(WORKING)
            .into_any_element()
    }

    /// Whether thinking shows open unless the reader chose.
    pub(super) fn thinking_open_by_default(&self) -> bool {
        self.settings.expand_thinking && self.settings.view == crate::prefs::TranscriptView::Full
    }

    /// Whether the thought `id` opens, on a rail (`inner`) or not, because
    /// its first line leaves something out, the lines after it or what its
    /// width cuts.
    pub(super) fn thought_opens(&self, id: u64, inner: bool) -> bool {
        let Some(item) = self.item(id) else {
            return false;
        };
        let (first, more) = first_line(&item.text);
        more || self.line_room(inner).is_some_and(|(room, m)| {
            // As drawn, else from the font's average advance.
            let width = self
                .thought_widths
                .borrow()
                .get(&id)
                .filter(|(measured, _)| *measured == first)
                .map(|(_, width)| *width)
                .unwrap_or_else(|| {
                    first.chars().count() as f32 * m.body.advance_at(THOUGHT_SIZE * m.rem)
                });
            width > room
        })
    }

    /// Measures the first line of the thought `id` as its line draws it,
    /// in italic at a thought's size, for [`Self::thought_opens`].
    fn measure_thought(&self, id: u64, window: &mut Window, cx: &App) {
        let Some(item) = self.item(id) else {
            return;
        };
        let (first, _) = first_line(&item.text);
        if self
            .thought_widths
            .borrow()
            .get(&id)
            .is_some_and(|(measured, _)| *measured == first)
        {
            return;
        }
        let theme = cx.theme();
        let font = Font {
            style: FontStyle::Italic,
            ..font(theme.font_family.clone())
        };
        let run = TextRun {
            len: first.len(),
            font,
            color: theme.muted_foreground,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let size = window.rem_size() * THOUGHT_SIZE;
        let width = f32::from(
            window
                .text_system()
                .layout_line(&first, size, &[run], None)
                .width,
        );
        self.thought_widths.borrow_mut().insert(id, (first, width));
    }

    /// A thought's line, its first line muted and italic in the words'
    /// column, with no label or marker, cut to fit. A click or Enter opens
    /// it whole where it leaves something out. While it streams, the
    /// spinner turns in the markers' column.
    fn thinking(&self, id: u64, inner: bool, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Ok(ix) = self.model.position(id) else {
            return div().into_any_element();
        };
        let item = &self.model.entries()[ix].item;
        let live = self.model.is_streaming(ix);
        let (first, more) = first_line(&item.text);
        let words: SharedString = if more {
            format!("{first} …").into()
        } else {
            first.into()
        };
        let line = h_flex()
            .w_full()
            .min_w_0()
            .h_full()
            .gap(rems(GAP))
            .text_size(rems(THOUGHT_SIZE))
            .line_height(rems(LINE))
            .italic()
            .text_color(theme.muted_foreground)
            .child(self.marker(("live", id), Marker::live(live), cx))
            .child(div().flex_1().min_w_0().truncate().child(words.clone()));
        let default = self.thinking_open_by_default();
        let view = view.downgrade();
        div()
            .id(("thinking-row", id))
            .test_support()
            .child(Disclosure {
                id: ("thinking-toggle", id).into(),
                open: false,
                label: words,
                inert: !self.thought_opens(id, inner),
                content: line.into_any_element(),
                on_toggle: Rc::new(move |_, cx| {
                    view.update(cx, |this, cx| this.toggle(id, default, cx))
                        .ok();
                }),
            })
            .into_any_element()
    }

    /// The room, in pixels, that a line's words and what follows them have
    /// in the reading column past the markers' column, and past the rail's
    /// on a rail (`inner`). `None` before the list is laid out.
    fn line_room(&self, inner: bool) -> Option<(f32, &Metrics)> {
        let m = self.metrics.as_ref()?;
        let width = self.list_width.get();
        (width > 0.).then(|| (room(width, inner, m.rem), m))
    }

    /// The widest that what follows a line's words may be where they show
    /// now, on a rail (`inner`) or not, in pixels. The words give way first,
    /// down to about 8 characters, and only then what follows them.
    fn widest_after(&self, inner: bool) -> Option<Pixels> {
        self.line_room(inner)
            .map(|(room, m)| px(after_room(room, m)))
    }

    /// What a call's Full line says after its words, how it went. That is ✓
    /// or ✗ and its outcome, or its job's state, or while it runs its time
    /// and the spinner. Its parts, and how many characters they take.
    fn call_after(
        &self,
        item: &Item,
        job: Option<&JobLine>,
        inner: bool,
        cx: &App,
    ) -> (Vec<AnyElement>, usize) {
        let theme = cx.theme();
        let id = item.id;
        let muted = theme.muted_foreground;
        let widest = self.widest_after(inner);
        let part = |mark: Option<(&'static str, Hsla)>, words: AnyElement| {
            h_flex()
                .id(("outcome", id))
                .test_support()
                .flex_none()
                .min_w_0()
                .when_some(widest, |this, widest| this.max_w(widest))
                .gap(rems(GAP))
                .text_color(muted)
                .child(div().flex_none().child("·"))
                .children(mark.map(|(mark, color)| div().flex_none().text_color(color).child(mark)))
                .child(div().min_w_0().truncate().child(words))
                .into_any_element()
        };
        let outcome = item
            .outcome
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        match (job, item.status) {
            (Some(JobLine::Outcome { text, failed: true }), _) => (
                vec![part(
                    Some(("✗", theme.danger)),
                    text.clone().into_any_element(),
                )],
                text.chars().count() + 4,
            ),
            (Some(job), _) => {
                let words = self.job_words(job);
                let chars = words.chars().count() + 2;
                (vec![part(None, words.into_any_element())], chars)
            }
            (None, Some(CallStatus::Running)) => {
                let time = duration((self.clock)() - item.at);
                let chars = time.chars().count() + 4;
                (
                    vec![
                        part(None, time.into_any_element()),
                        div()
                            .flex_none()
                            .child(self.spinner(id, cx))
                            .into_any_element(),
                    ],
                    chars,
                )
            }
            (None, Some(CallStatus::Ok)) => {
                let chars = outcome.chars().count() + 4;
                (
                    vec![part(Some(("✓", theme.success)), outcome.into_any_element())],
                    chars,
                )
            }
            (None, Some(CallStatus::Failed)) => {
                let chars = outcome.chars().count() + 4;
                let words =
                    self.marked_plain(id, Place::Outcome, outcome, call_line_ink(muted, theme), cx);
                (vec![part(Some(("✗", theme.danger)), words)], chars)
            }
            _ => (Vec::new(), 0),
        }
    }

    /// How many characters what a call's Full line says after its words
    /// takes ([`Self::call_after`]).
    fn call_after_chars(&self, item: &Item, job: Option<&JobLine>) -> usize {
        let outcome = item
            .outcome
            .as_deref()
            .map_or(0, |outcome| outcome.trim().chars().count());
        match (job, item.status) {
            (Some(JobLine::Outcome { text, failed: true }), _) => text.chars().count() + 4,
            (Some(job), _) => self.job_words(job).chars().count() + 2,
            (None, Some(CallStatus::Running)) => {
                duration((self.clock)() - item.at).chars().count() + 6
            }
            (None, Some(_)) => outcome + 4,
            _ => 0,
        }
    }

    /// The width of what a call offers beside its line while it or its job
    /// runs ([`Self::call_action`]), in pixels.
    fn action_width(&self, item: &Item, job: Option<&JobLine>, m: &Metrics) -> f32 {
        let label = match job {
            Some(JobLine::Running { .. } | JobLine::Monitoring { .. }) => "Stop",
            None if item.status == Some(CallStatus::Running) && self.movable.contains(&item.id) => {
                "Move to background"
            }
            _ => return 0.,
        };
        // An extra-small button's padding, and the gap before it.
        label.chars().count() as f32 * m.body.advance_at(0.75 * m.rem) + 1.25 * m.rem
    }

    /// Whether a call's Full line cuts its words in `room` pixels. One that
    /// does opens to them whole.
    pub(super) fn call_cut(&self, item: &Item, room: f32, m: &Metrics) -> bool {
        let job = self.job_line(item);
        let advance = m.mono.advance();
        let after = self.call_after_chars(item, job.as_ref()) as f32 * advance;
        let words = (call_line(item).chars().count() + who_chars(item)) as f32 * advance;
        let actions = self.action_width(item, job.as_ref(), m);
        words + after.min(after_room(room, m)) + actions + 2. * GAP * m.rem > room
    }

    /// Whether the call `id`'s line opens, on a rail (`inner`) or not,
    /// because it holds something or cuts its words.
    pub(super) fn call_opens(&self, id: u64, inner: bool) -> bool {
        self.model.entry(id).is_some_and(|entry| {
            opens(entry)
                || self
                    .line_room(inner)
                    .is_some_and(|(room, m)| self.call_cut(&entry.item, room, m))
        })
    }

    /// Whether the Quiet row `first` opens, because it holds something or
    /// its line cuts its words.
    pub(super) fn group_opens(&self, first: u64) -> bool {
        self.quiet.get(&first).is_some_and(|row| {
            row.opens
                || self
                    .line_room(false)
                    .is_some_and(|(room, m)| self.row_cut(row, room, m))
        })
    }

    /// A call's line as Full writes it, its marker, whose it is (a
    /// worker's, in the main transcript), its words and how it went, with
    /// what it offers while it or its job runs. Open, what it holds shows
    /// under it, on a rail, or on an open row's rail (`inner`) at the
    /// line's words, where its line is muted.
    fn call(&self, id: u64, inner: bool, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let line = call_line(item);
        let job = self.job_line(item);
        let (after, _) = self.call_after(item, job.as_ref(), inner, cx);
        // (A row's line says what its calls offer.)
        let action = (!inner)
            .then(|| self.call_action(item, job.as_ref(), view))
            .flatten();
        let cut = self
            .line_room(inner)
            .is_some_and(|(room, m)| self.call_cut(item, room, m));
        let opens = opens(entry) || cut;
        let open = opens && self.ui.get(&id).and_then(|ui| ui.expanded) == Some(true);
        let color = if inner {
            theme.muted_foreground
        } else {
            theme.foreground
        };
        let words = div().text_color(color).child(self.marked_plain(
            id,
            Place::Text,
            line.clone(),
            call_line_ink(color, theme),
            cx,
        ));
        let who = (!item.is_main()).then(|| who(&item.agent, theme));
        let content = work_line(
            self.marker(("call-mark", id), Marker::of(opens, open), cx),
            who,
            words.into_any_element(),
            after,
            None,
        )
        .code_font(theme)
        .text_size(theme.mono_font_size)
        .into_any_element();
        let header_view = view.downgrade();
        let header = Disclosure {
            id: ("call-toggle", id).into(),
            open,
            label: line.into(),
            inert: !opens,
            content,
            on_toggle: Rc::new(move |_, cx| {
                header_view
                    .update(cx, |this, cx| this.toggle(id, false, cx))
                    .ok();
            }),
        };
        // What the line cut shows first, unless the command's strip says it.
        let whole = (cut && !strip_says_it(item)).then(|| item.text.trim().to_string());
        v_flex()
            .w_full()
            .child(
                h_flex()
                    .id(("call-row", id))
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(header))
                    .children(action.map(|action| div().flex_none().child(action))),
            )
            .children(
                self.thumbnails(item, view, cx)
                    .map(|thumbnails| div().pl(rems(WORDS)).pt_1().child(thumbnails)),
            )
            .when(open, |this| {
                let held = self.call_details(id, whole, cx);
                this.child(if inner {
                    div().pl(rems(WORDS)).pt_1().child(held)
                } else {
                    on_rail(held, theme)
                })
            })
            .into_any_element()
    }

    /// What a call offers to do with itself while it runs (Move to
    /// background) or with its job while that runs (Stop).
    fn call_action(
        &self,
        item: &Item,
        job: Option<&JobLine>,
        view: &Entity<Self>,
    ) -> Option<AnyElement> {
        let id = item.id;
        match job {
            Some(JobLine::Running { job, .. } | JobLine::Monitoring { job, .. }) => {
                let (view, job) = (view.downgrade(), job.clone());
                Some(
                    Button::new(("stop-job", id))
                        .ghost()
                        .xsmall()
                        .label("Stop")
                        .on_click(move |_, _, cx| {
                            view.update(cx, |_, cx| cx.emit(TranscriptEvent::StopJob(job.clone())))
                                .ok();
                        })
                        .into_any_element(),
                )
            }
            None if item.status == Some(CallStatus::Running) && self.movable.contains(&id) => {
                let view = view.downgrade();
                let button = Button::new(("move-call", id))
                    .ghost()
                    .xsmall()
                    .label("Move to background");
                // Ctrl+B moves the main agent's call.
                let button = if item.is_main() {
                    button.tooltip_with_action(
                        "Move to background",
                        &crate::menus::MoveToBackground,
                        None,
                    )
                } else {
                    button
                };
                Some(
                    button
                        .on_click(move |_, _, cx| {
                            view.update(cx, |_, cx| cx.emit(TranscriptEvent::MoveToBackground(id)))
                                .ok();
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    /// A call's details. One that ended shows a card: what its line cut
    /// (a command as a strip at its top, any other detail muted in its
    /// body), then its content, and an edit its diff. One from an old
    /// session shows its parameters and its result under captions. Where
    /// the line cut its words and nothing else says them, `whole` says them
    /// first, muted.
    fn call_details(&self, id: u64, whole: Option<String>, cx: &App) -> AnyElement {
        use super::find::Place;
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let line = item.text.trim().to_string();
        let given = shown_given(item);
        let output = shown_output(entry);
        let diff = self.ui.get(&id).and_then(|ui| ui.diff.clone());
        let details = v_flex().w_full().gap_1().children(whole.map(|whole| {
            div()
                .id(("whole", id))
                .test_support()
                .w_full()
                .code_font(theme)
                .text_size(theme.mono_font_size)
                .text_color(theme.muted_foreground)
                .child(whole)
        }));
        // The first lines of a long text show, and a button opens it all.
        let door = |text: Arc<str>, key: &'static str, title: String| {
            let label = show_all(&text)?;
            let title: SharedString = title.into();
            Some(
                Button::new((key, id))
                    .ghost()
                    .xsmall()
                    .label(label)
                    .on_click(move |_, window, cx| {
                        open_output(title.clone(), text.clone(), window, cx)
                    }),
            )
        };
        if let Some(Given::Parameters(parameters)) = &given {
            let caption = |text: &str| {
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(text.to_string())
            };
            let block = |text: &Arc<str>, key: &'static str, place: Place| {
                let (shown, _) = preview(text);
                div()
                    .w_full()
                    .px_2()
                    .py_1p5()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .code_font(theme)
                    .text_size(theme.mono_font_size)
                    .child(self.marked(
                        (key, id),
                        id,
                        place,
                        shown,
                        Ink::on(theme.foreground, theme.muted),
                        cx,
                    ))
            };
            return details
                .child(caption("Parameters"))
                .child(block(parameters, "parameters", Place::Parameters))
                .children(door(
                    parameters.clone(),
                    "all-parameters",
                    format!("Parameters of {line}"),
                ))
                .when_some(output, |this, output| {
                    this.child(caption("Result"))
                        .child(block(&output, "output", Place::Output))
                        .children(door(output, "all-output", format!("Result of {line}")))
                })
                .into_any_element();
        }
        let detail = match given {
            Some(Given::Detail(detail)) => Some(detail),
            _ => None,
        };
        let (strip, noted) = match detail {
            Some(detail) if item.tool.as_deref() == Some("shell") => (Some(detail), None),
            detail => (None, detail),
        };
        let editor = theme.editor_background();
        let body = (noted.is_some() || output.is_some()).then(|| {
            let (numbers, muted) = output_look(entry);
            let ink = if muted {
                theme.muted_foreground
            } else {
                theme.foreground
            };
            v_flex()
                .w_full()
                .gap_1()
                .px_3()
                .py_1p5()
                .bg(editor)
                .code_font(theme)
                .text_size(theme.mono_font_size)
                .when_some(noted, |this, noted| {
                    this.child(div().w_full().text_color(theme.muted_foreground).child(
                        self.marked(
                            ("detail", id),
                            id,
                            Place::Parameters,
                            noted.to_string(),
                            Ink::on(theme.muted_foreground, editor),
                            cx,
                        ),
                    ))
                })
                .when_some(output.clone(), |this, output| {
                    let (shown, _) = preview(&output);
                    let gutters = numbered_gutters(&shown, numbers);
                    this.child(div().w_full().text_color(ink).child(self.marked_with(
                        ("output", id),
                        id,
                        Place::Output,
                        shown,
                        Ink::on(ink, editor),
                        gutters,
                        theme.muted_foreground,
                        cx,
                    )))
                })
        });
        let has_body = body.is_some();
        let strip = strip.map(|command| {
            h_flex()
                .w_full()
                .items_start()
                .gap_2()
                .px_3()
                .py_1()
                .bg(theme.muted)
                .when(has_body, |this| {
                    this.border_b_1().border_color(theme.border)
                })
                .child(self.timeline(id, &command, cx))
                .child(
                    Clipboard::new(("command-copy", id))
                        .xsmall()
                        .value(command.to_string())
                        .tooltip("Copy the command"),
                )
        });
        let card = (strip.is_some() || has_body).then(|| {
            v_flex()
                .w_full()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .children(strip)
                .children(body)
        });
        let framed = diff.map(|diff| {
            div()
                .w_full()
                .rounded(theme.radius)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden()
                .child(DiffView::new(&diff).max_rows(DIFF_ROWS))
        });
        details
            .children(framed)
            .children(card)
            .children(
                output.and_then(|output| door(output, "all-output", format!("Output of {line}"))),
            )
            .into_any_element()
    }

    /// A command as its card's strip shows it: each command of its chain on
    /// a row of its own after a `$`, and a timeline, a line from each `$` to
    /// the next that keeps clear of them.
    fn timeline(&self, id: u64, command: &Arc<str>, cx: &App) -> AnyElement {
        use super::find::Place;
        let theme = cx.theme();
        let size = f32::from(theme.mono_font_size);
        let (line, advance) = self
            .metrics
            .as_ref()
            .map_or((size * super::height::LINE_HEIGHT, size * 0.6), |m| {
                (m.line_mono, m.mono.advance())
            });
        let parts = chain(command);
        let count = parts.len();
        let rule = theme.muted_foreground.opacity(TIMELINE_ALPHA);
        // 2 px wide, as the `$`'s stem is, and centred under it.
        let x = (advance / 2. - 1.).round();
        let (above, below) = timeline_gaps(line, size);
        let rows: Vec<Div> = parts
            .into_iter()
            .enumerate()
            .map(|(at, part)| {
                let (first, last) = (at == 0, at + 1 == count);
                let gutter = div()
                    .flex_none()
                    .relative()
                    .w(px(2. * advance))
                    .child("$")
                    .when(!first, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left(px(x))
                                .top_0()
                                .w(px(2.))
                                .h(px(above))
                                .bg(rule),
                        )
                    })
                    .when(!last, |this| {
                        this.child(
                            div()
                                .absolute()
                                .left(px(x))
                                .top(px(below))
                                .bottom_0()
                                .w(px(2.))
                                .bg(rule),
                        )
                    });
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_stretch()
                    .child(gutter)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .when(!last, |this| this.pb(px(CHAIN_GAP)))
                            .child(self.marked_part(
                                (SharedString::from(format!("command-{id}")), at),
                                id,
                                Place::Parameters,
                                command,
                                part,
                                Ink::on(theme.muted_foreground, theme.muted),
                                cx,
                            )),
                    )
            })
            .collect();
        v_flex()
            .flex_1()
            .min_w_0()
            .code_font(theme)
            .text_size(theme.mono_font_size)
            .text_color(theme.muted_foreground)
            .children(rows)
            .into_any_element()
    }

    /// What a Quiet row says after its words, whole. That is a test run's
    /// counts, how a job goes or what came, and while a call of it runs that
    /// call's time and the spinner. Its parts, and how many characters they take.
    fn row_after(
        &self,
        row: &QuietRow,
        running: Option<&Item>,
        cx: &App,
    ) -> (Vec<AnyElement>, usize) {
        let theme = cx.theme();
        let mut parts = Vec::new();
        let mut chars = 0;
        let words = &row.words;
        if let Some(news) = &words.news {
            let news = format!("({news})");
            chars += news.chars().count() + 1;
            parts.push(div().flex_none().child(news).into_any_element());
        }
        let mut note: Vec<String> = words.note.iter().cloned().collect();
        if let Some(running) = running {
            note.push(duration((self.clock)() - running.at));
        }
        if !note.is_empty() {
            let note = format!("· {}", note.join(" · "));
            chars += note.chars().count() + 1;
            parts.push(
                div()
                    .flex_none()
                    .min_w_0()
                    .when_some(self.widest_after(false), |this, widest| this.max_w(widest))
                    .truncate()
                    .child(note)
                    .into_any_element(),
            );
        }
        if let Some(running) = running.filter(|_| words.running) {
            chars += 3;
            parts.push(
                div()
                    .flex_none()
                    .child(self.spinner(running.id, cx))
                    .into_any_element(),
            );
        }
        let _ = theme;
        (parts, chars)
    }

    /// The call of a Quiet row that runs, its latest.
    fn running_call(&self, row: &QuietRow) -> Option<&Item> {
        row.members
            .iter()
            .rev()
            .filter_map(|id| self.item(*id))
            .find(|item| item.kind == Kind::Call && item.status == Some(CallStatus::Running))
    }

    /// Whether a Quiet row cuts its words in `room` pixels. One that does
    /// opens to its call as written.
    pub(super) fn row_cut(&self, row: &QuietRow, room: f32, m: &Metrics) -> bool {
        let words = &row.words;
        let advance = m.mono.advance();
        let mut after = 0.;
        if let Some(news) = &words.news {
            after += (news.chars().count() + 2) as f32 * advance + GAP * m.rem;
        }
        let mut note = words
            .note
            .as_deref()
            .map_or(0, |note| note.chars().count() + 2);
        let running = self.running_call(row);
        if let Some(running) = running {
            note += duration((self.clock)() - running.at).chars().count() + 3;
        }
        if note > 0 {
            after += (note as f32 * advance).min(after_room(room, m)) + GAP * m.rem;
        }
        if running.is_some() && words.running {
            after += 0.75 * m.rem + GAP * m.rem;
        }
        if words.failed {
            after += advance + GAP * m.rem;
        }
        for id in &row.members {
            if let Some(item) = self.item(*id).filter(|item| item.kind == Kind::Call) {
                after += self.action_width(item, self.job_line(item).as_ref(), m);
            }
        }
        words.text.chars().count() as f32 * advance + after + GAP * m.rem > room
    }

    /// A Quiet row, its marker and what its calls did in a few words, cut
    /// to fit, and after them what stays whole, its news, how its job goes,
    /// its running call's time, and one ✗ at its right when a call failed. It
    /// offers what its calls offer while they run (Move to background,
    /// Stop), and their images show under it. Open, a row of one call shows
    /// the call's detail and content on its rail, after the call as written
    /// where the line cut its words. A row of several shows its calls and
    /// the thinking between them as rows on its rail.
    fn group(&self, first: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(row) = self.quiet.get(&first) else {
            return div().into_any_element();
        };
        let calls: Vec<&Item> = row
            .members
            .iter()
            .filter_map(|id| self.item(*id))
            .filter(|item| item.kind == Kind::Call)
            .collect();
        let running = self.running_call(row);
        let (after, _) = self.row_after(row, running, cx);
        let cut = self
            .line_room(false)
            .is_some_and(|(room, m)| self.row_cut(row, room, m));
        let opens = row.opens || cut;
        let open = opens && self.group_open(first);
        let words = &row.words;
        let color = if words.muted {
            theme.muted_foreground
        } else {
            theme.foreground
        };
        let failed = words
            .failed
            .then(|| div().text_color(theme.danger).child("✗").into_any_element());
        let content = work_line(
            self.marker(("group-mark", first), Marker::of(opens, open), cx),
            None,
            div()
                .text_color(color)
                .child(words.text.clone())
                .into_any_element(),
            after,
            failed,
        )
        .code_font(theme)
        .text_size(theme.mono_font_size)
        .text_color(theme.muted_foreground)
        .into_any_element();
        // What its calls offer while they or their jobs run.
        let actions: Vec<AnyElement> = calls
            .iter()
            .filter_map(|item| self.call_action(item, self.job_line(item).as_ref(), view))
            .collect();
        let toggle_view = view.downgrade();
        let header = Disclosure {
            id: ("group-toggle", first).into(),
            open,
            label: words.said().into(),
            inert: !opens,
            content,
            on_toggle: Rc::new(move |_, cx| {
                toggle_view
                    .update(cx, |this, cx| this.toggle_group(first, cx))
                    .ok();
            }),
        };
        // The images its calls' tools returned, under its line. (A row of
        // several open shows them under each call.)
        let images = (!(open && row.merged))
            .then(|| {
                let tiles: Vec<AnyElement> = calls
                    .iter()
                    .filter_map(|item| self.thumbnails(item, view, cx))
                    .collect();
                (!tiles.is_empty()).then(|| {
                    v_flex()
                        .pl(rems(WORDS))
                        .pt_1()
                        .gap_2()
                        .children(tiles)
                        .into_any_element()
                })
            })
            .flatten();
        let held = (open && !row.merged).then(|| {
            let item = calls.first().copied();
            let whole = item
                .filter(|item| cut && !strip_says_it(item))
                .map(|item| item.text.trim().to_string());
            on_rail(self.call_details(first, whole, cx), theme)
        });
        v_flex()
            .w_full()
            .child(
                h_flex()
                    .id(("group-row", first))
                    .test_support()
                    .w_full()
                    .min_w_0()
                    .gap_1()
                    .child(div().flex_1().min_w_0().child(header))
                    .children(
                        actions
                            .into_iter()
                            .map(|action| div().flex_none().child(action)),
                    ),
            )
            .children(images)
            .children(held)
            .into_any_element()
    }

    /// A turn's calls in one line (Messages), how many, how many failed and
    /// whether one runs. It opens to the turn's work as Quiet shows it,
    /// under it, from the turn's first thought.
    fn count(&self, first: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let count = self.counts.get(&first).cloned().unwrap_or_default();
        let open = self.counts_open.contains(&first);
        let words = plural(count.calls, "tool call", "tool calls");
        let mut after = Vec::new();
        if count.failed > 0 {
            after.push(
                h_flex()
                    .flex_none()
                    .gap(rems(GAP))
                    .child("·")
                    .child(
                        div()
                            .text_color(theme.danger)
                            .child(format!("{} failed", count.failed)),
                    )
                    .into_any_element(),
            );
        }
        if count.running > 0 {
            after.push(div().flex_none().child("· running").into_any_element());
        }
        let content = work_line(
            self.marker(("count-mark", first), Marker::of(true, open), cx),
            None,
            div()
                .text_color(theme.foreground)
                .child(words.clone())
                .into_any_element(),
            after,
            None,
        )
        .code_font(theme)
        .text_size(theme.mono_font_size)
        .text_color(theme.muted_foreground)
        .into_any_element();
        let view = view.downgrade();
        div()
            .id(("count", first))
            .test_support()
            .child(Disclosure {
                id: ("count-toggle", first).into(),
                open,
                label: words.into(),
                inert: false,
                content,
                on_toggle: Rc::new(move |_, cx| {
                    view.update(cx, |this, cx| this.toggle_count(first, cx))
                        .ok();
                }),
            })
            .into_any_element()
    }

    /// The images an item shows (the user's attachments, what a tool
    /// returned), as tiles of a fixed size, so a row's height doesn't change
    /// when an image is decoded. Each is scaled down on a background thread
    /// through the transcript's image cache.
    fn thumbnails(&self, item: &Item, view: &Entity<Self>, cx: &App) -> Option<AnyElement> {
        if item.images.is_empty() {
            return None;
        }
        let theme = cx.theme();
        let entry = item.id;
        Some(
            h_flex()
                .flex_wrap()
                .gap_2()
                .children(item.images.iter().map(|image| {
                    let view = view.downgrade();
                    let id = image.id;
                    Thumbnail {
                        id: ("image", id).into(),
                        image: self.image(id),
                        cache: self.image_cache.clone(),
                        caption: super::image_caption(&image.label, image.bytes).into(),
                        on_open: Rc::new(move |window, cx| {
                            view.update(cx, |this, cx| this.open_image(entry, id, window, cx))
                                .ok();
                        }),
                    }
                    .into_any_element()
                }))
                .text_color(theme.muted_foreground)
                .into_any_element(),
        )
    }

    /// What a worker's notice is labelled with: the worker's ID, and in the
    /// main transcript the worker that started it, as Arko words it ("r2
    /// (under r1)"). A stop notice says it itself. A worker's own transcript
    /// doesn't repeat who it is.
    pub(super) fn agent_label(&self, item: &Item) -> String {
        let parent = matches!(self.model.filter(), AgentFilter::Main)
            .then(|| {
                item.parent.as_deref().or_else(|| {
                    self.model
                        .worker(&item.agent)
                        .and_then(|worker| worker.parent.as_deref())
                })
            })
            .flatten();
        match parent {
            Some(parent) => format!("{} (under {parent})", item.agent),
            None => item.agent.clone(),
        }
    }

    /// A card of the agent's questions: its line ("Arko asked 2
    /// questions"), which opens the answer panel on those still open, then
    /// the record of each settled one with what became of it. An open
    /// question shows only in the panel, and the answers given in the panel
    /// show in a bubble of the user's under the card.
    fn questions(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let questions = item.questions.as_deref().unwrap_or_default();
        let open = questions.iter().any(|question| question.state.is_open());
        let view = view.downgrade();
        let line = h_flex()
            .id(("question-line", id))
            .test_support()
            .gap_1()
            .text_sm()
            .text_color(theme.muted_foreground)
            .child(Icon::new(Lucide::MessageCircleQuestionMark).xsmall())
            .child(item.text.clone())
            .when(open, |this| {
                this.role(Role::Button)
                    .cursor_pointer()
                    .hover(|style| style.text_color(theme.foreground))
                    .managed_tooltip(|window, cx| {
                        Tooltip::new("Answer")
                            .action(&crate::questions::AnswerQuestions, None)
                            .build(window, cx)
                    })
                    .on_click(move |_, _, cx| {
                        view.update(cx, |_, cx| cx.emit(TranscriptEvent::OpenQuestions(id)))
                            .ok();
                    })
            });
        let muted = theme.muted_foreground;
        let record = questions.iter().enumerate().filter_map(|(n, question)| {
            if question.state.is_open() {
                return None;
            }
            // The user's own answer is in the bubble under the card.
            let outcome = crate::questions::record_outcome(&question.state);
            Some(
                h_flex()
                    .id(("question", id * 8 + n as u64))
                    .test_support()
                    .items_baseline()
                    .gap_1p5()
                    .pt_2()
                    .when_some(question.number, |this, number| {
                        // At least a rem, so the texts line up.
                        this.child(
                            div()
                                .flex_none()
                                .min_w(rems(1.))
                                .text_color(muted)
                                .child(format!("{number}.")),
                        )
                    })
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .id(("question-text", id * 8 + n as u64))
                                    .test_support()
                                    .child(self.marked(
                                        ("question-words", id * 8 + n as u64),
                                        id,
                                        Place::Question(n, QuestionPart::Words),
                                        crate::questions::text(question),
                                        Ink::new(theme.foreground),
                                        cx,
                                    )),
                            )
                            .children(outcome.map(|outcome| {
                                div()
                                    .id(("question-outcome", id * 8 + n as u64))
                                    .test_support()
                                    .text_sm()
                                    .text_color(muted)
                                    .child(self.marked(
                                        ("question-outcome-words", id * 8 + n as u64),
                                        id,
                                        Place::Question(n, QuestionPart::Outcome),
                                        outcome,
                                        Ink::new(muted),
                                        cx,
                                    ))
                            })),
                    ),
            )
        });
        // The answers given in the answer panel are the user's words, so
        // they are a bubble like the user's messages, tagged as a typed
        // answer is, each numbered as its question when the ask numbered
        // them.
        let answers: Vec<AnyElement> = questions
            .iter()
            .enumerate()
            .filter_map(|(n, question)| {
                let answer = crate::questions::answer(&question.state)?;
                Some(
                    h_flex()
                        .id(("answer", id * 8 + n as u64))
                        .test_support()
                        // The number and the words are in the same font, so
                        // their first lines line up.
                        .items_start()
                        .gap_1p5()
                        .when_some(question.number, |this, number| {
                            this.child(
                                div()
                                    .id(("answer-number", id * 8 + n as u64))
                                    .test_support()
                                    .flex_none()
                                    .min_w(rems(1.))
                                    .text_color(muted)
                                    .child(format!("{number}.")),
                            )
                        })
                        .child(
                            div()
                                .id(("answer-text", id * 8 + n as u64))
                                .test_support()
                                .min_w_0()
                                .child(self.marked(
                                    ("answer-words", id * 8 + n as u64),
                                    id,
                                    Place::Answer(n),
                                    answer.to_string(),
                                    Ink::on(theme.foreground, bubble_color(theme)),
                                    cx,
                                )),
                        )
                        .into_any_element(),
                )
            })
            .collect();
        let answers = (!answers.is_empty()).then(|| {
            let bubble = bubble(false, theme)
                .id(("question-answers", id))
                .test_support()
                .child(div().text_xs().text_color(muted).child(ANSWER_TAG))
                .children(answers);
            at_right(bubble, None).pt(rems(ANSWERS_GAP))
        });
        // Once a question has settled, the card is a quiet box like the
        // answer panel's, its line the heading of the record.
        let settled = questions.iter().any(|question| !question.state.is_open());
        let card = v_flex()
            .w_full()
            .when(settled, |this| {
                this.p_3()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius_lg)
            })
            .child(line)
            .children(record);
        v_flex()
            .w_full()
            .child(card)
            .children(answers)
            .into_any_element()
    }

    fn notice(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        // The Messages view hides turns' footers: the notice that says why
        // the turn ended without a reply offers Regenerate instead.
        let regenerate = self.end_line_regenerate(RowKey::Notice(id), view, cx);
        let (color, text_color) = match item.level {
            Some(Level::Error) => (theme.danger, theme.foreground),
            Some(Level::Warn) => (theme.warning, theme.warning),
            _ => (theme.muted_foreground, theme.muted_foreground),
        };
        let icon = notice_icon(item);
        // Its icon in the markers' column, on its first line, and its words
        // after whose it is.
        let line: DefiniteLength = self
            .metrics
            .as_ref()
            .map_or(rems(1.5).into(), |m| px(m.line_sm).into());
        h_flex()
            .relative()
            .w_full()
            .items_start()
            .gap(rems(GAP))
            .text_sm()
            .text_color(text_color)
            .child(
                div()
                    .flex_none()
                    .w(rems(MARKER))
                    .h(line)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon).xsmall().text_color(color)),
            )
            .when(!item.is_main(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .h(line)
                        .flex()
                        .items_center()
                        .text_xs()
                        .code_font(theme)
                        .text_color(theme.muted_foreground)
                        .child(self.agent_label(item)),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .when(regenerate.is_some(), |this| this.pr(rems(REGENERATE_ROOM)))
                    .child(self.marked(
                        ("notice-text", id),
                        id,
                        Place::Text,
                        item.text.clone(),
                        Ink::new(text_color),
                        cx,
                    )),
            )
            .when_some(regenerate, |this, regenerate| {
                this.child(div().absolute().top_0().right_0().child(regenerate))
            })
            .into_any_element()
    }

    fn output(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let border = match item.level {
            Some(Level::Error) => theme.danger,
            Some(Level::Warn) => theme.warning,
            _ => theme.border,
        };
        div()
            .w_full()
            .pl_3()
            .border_l_2()
            .border_color(border)
            .code_font(theme)
            .text_size(theme.mono_font_size)
            .child(self.marked(
                ("output-text", id),
                id,
                Place::Text,
                item.text.clone(),
                Ink::new(theme.foreground),
                cx,
            ))
            .into_any_element()
    }

    fn turn(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let regenerate = self.end_line_regenerate(RowKey::Turn(id), view, cx);
        div()
            .relative()
            .w_full()
            .text_xs()
            .text_color(theme.muted_foreground)
            // At the column's edge, where the reply's text starts.
            .child(
                div()
                    .id(("turn-text", id))
                    .test_support()
                    .truncate()
                    .when(regenerate.is_some(), |this| this.pr(rems(REGENERATE_ROOM)))
                    .child(self.marked_plain(
                        id,
                        Place::Text,
                        item.text.clone(),
                        Ink::new(theme.muted_foreground),
                        cx,
                    )),
            )
            // Over the line, centred on it, so the line keeps its height.
            .when_some(regenerate, |this, regenerate| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .bottom_0()
                        .right_0()
                        .flex()
                        .items_center()
                        .child(regenerate),
                )
            })
            .into_any_element()
    }

    /// Regenerate on the last line of a turn that ended without a reply
    /// (its footer, or the notice that says why), when row `key` is it.
    fn end_line_regenerate(
        &self,
        key: RowKey,
        view: &Entity<Self>,
        cx: &App,
    ) -> Option<AnyElement> {
        if !self.offers_regenerate(key) {
            return None;
        }
        let id = key.entry();
        let muted = cx.theme().muted_foreground;
        let view = view.downgrade();
        Some(
            HoverActions {
                id: ("end-actions", id).into(),
                shown: self.hovered == Some(id),
                build: Box::new(move |shown| {
                    regenerate_button(id, REGENERATE_HINT, false, shown, muted, view)
                        .into_any_element()
                }),
            }
            .into_any_element(),
        )
    }

    /// Whether row `key` offers Regenerate: the latest reply's actions, or
    /// the last line of a turn that ended without a reply.
    pub(super) fn offers_regenerate(&self, key: RowKey) -> bool {
        match key {
            RowKey::Actions(id) => {
                self.model.last_reply() == Some(id)
                    && matches!(self.model.filter(), AgentFilter::Main)
            }
            RowKey::Turn(_) | RowKey::Notice(_) => self.regenerate_end() == Some(key),
            _ => false,
        }
    }

    /// The last line of the turn of the user's latest message, when that
    /// turn ended without a reply and nothing runs: its footer, or in the
    /// Messages view, which hides footers, the notice that says why it
    /// ended.
    pub(super) fn regenerate_end(&self) -> Option<RowKey> {
        if !matches!(self.model.filter(), AgentFilter::Main) {
            return None;
        }
        let message = self.model.regenerable_message()?;
        let turn = self.item(message)?.turn;
        // The message is the latest: its turn's rows are the last ones.
        self.rows
            .iter()
            .rev()
            .map(|row| row.key)
            .take_while(|key| *key != RowKey::User(message))
            .find(|key| {
                matches!(key, RowKey::Turn(_) | RowKey::Notice(_))
                    && self
                        .item(key.entry())
                        .is_some_and(|item| item.is_main() && item.turn == turn)
            })
    }

    fn worker(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        let worker_id = item.worker.clone().unwrap_or_default();
        let summary = self.model.worker(&worker_id);
        let title = summary
            .map(|summary| summary.title.clone())
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| item.text.clone());
        let now = (self.clock)();
        // Its profile, or its model and effort when workers usually run
        // others, once the session has recorded them.
        let setup = summary.and_then(|summary| crate::workers::card_setup(summary, &self.usual));
        // The skills and servers it was narrowed to, in full on hover.
        let narrowed = summary.and_then(crate::workers::narrowed_sentence);
        let state: AnyElement = match summary.map(|summary| summary.state) {
            Some(WorkerState::Running) => {
                let elapsed = summary
                    .and_then(|summary| summary.started_at)
                    .map(|started| duration(now - started))
                    .unwrap_or_default();
                h_flex()
                    .gap_1()
                    .child(self.spinner(id, cx))
                    .child(elapsed)
                    .into_any_element()
            }
            Some(WorkerState::Queued) => "Queued".into_any_element(),
            Some(WorkerState::Finished) => "Finished".into_any_element(),
            Some(WorkerState::Failed) => div()
                .text_color(theme.danger)
                .child("Failed")
                .into_any_element(),
            Some(WorkerState::Stopped) => "Stopped".into_any_element(),
            // No summary yet, or a state a later arko adds.
            _ => div().into_any_element(),
        };
        let open: SharedString = worker_id.clone().into();
        let view = view.downgrade();
        let title = self.marked_plain(id, Place::Text, title, worker_title_ink(theme), cx);
        let card = Button::new(("worker-card", id))
            .outline()
            .small()
            .w_full()
            .tooltip("Open in the workers panel")
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .child(Icon::new(Lucide::Bot).small().flex_none())
                    .child(
                        div()
                            .flex_none()
                            .code_font(theme)
                            .text_size(theme.mono_font_size)
                            .child(worker_id),
                    )
                    .child(div().flex_1().min_w_0().truncate().text_left().child(title))
                    // The title gives way first, and then the setup.
                    .children(setup.map(|setup| {
                        div()
                            .id(("worker-setup", id))
                            .test_support()
                            .flex_shrink(1.)
                            .min_w_0()
                            .max_w(rems(14.))
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .when_some(narrowed.clone(), |this, narrowed| {
                                this.managed_tooltip(move |window, cx| {
                                    Tooltip::new(narrowed.clone()).build(window, cx)
                                })
                            })
                            .child(setup)
                    }))
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(state),
                    ),
            )
            .on_click(move |_, _, cx| {
                let open = open.clone();
                view.update(cx, |this, cx| this.open_worker(open, cx)).ok();
            });
        div()
            .id(("worker-row", id))
            .test_support()
            .child(card)
            .into_any_element()
    }
}

/// Shows a command's whole output in a dialog, as a list that lays out only
/// the lines on screen.
fn open_output(title: SharedString, text: Arc<str>, window: &mut Window, cx: &mut App) {
    open_output_at(title, text, None, window, cx);
}

/// Shows a command's whole output in a dialog, at the bytes `at` (a match
/// of a find), marked.
pub(super) fn open_output_at(
    title: SharedString,
    text: Arc<str>,
    at: Option<std::ops::Range<usize>>,
    window: &mut Window,
    cx: &mut App,
) {
    let viewer = cx.new(|cx| OutputViewer::new(text, at, cx));
    window.open_dialog(cx, move |dialog, _, _| {
        let viewer = viewer.clone();
        dialog
            .title(title.clone())
            .w(px(760.))
            .content(move |content, _, _| content.child(viewer.clone()))
    });
}

/// A long output, one row per line.
struct OutputViewer {
    text: Arc<str>,
    lines: Vec<std::ops::Range<usize>>,
    list: ListState,
    /// The bytes of a match it opened at, marked.
    marked: Option<std::ops::Range<usize>>,
}

impl OutputViewer {
    fn new(text: Arc<str>, at: Option<std::ops::Range<usize>>, _: &mut Context<Self>) -> Self {
        let mut lines = Vec::new();
        let mut start = 0;
        for line in text.split_inclusive('\n') {
            let end = start + line.len();
            lines.push(start..start + line.trim_end_matches(['\n', '\r']).len());
            start = end;
        }
        let list = ListState::new(lines.len(), ListAlignment::Top, px(400.));
        // A few lines above the match show where it is.
        if let Some(at) = &at
            && let Some(line) = lines.iter().position(|line| at.start <= line.end)
        {
            list.scroll_to(ListOffset {
                item_ix: line.saturating_sub(3),
                offset_in_item: px(0.),
            });
        }
        Self {
            text,
            lines,
            list,
            marked: at,
        }
    }

    /// What Copy all copies: the output itself, shared, not a copy of it
    /// made with each frame.
    fn copy_value(&self) -> SharedString {
        self.text.clone().into()
    }
}

impl Render for OutputViewer {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let viewer = cx.entity().downgrade();
        v_flex()
            .gap_2()
            .child(
                div()
                    .h(rems(28.))
                    .w_full()
                    .rounded(theme.radius)
                    .bg(theme.muted)
                    .code_font(theme)
                    .text_size(theme.mono_font_size)
                    .child(
                        list(self.list.clone(), move |ix, _, cx| {
                            let Some(viewer) = viewer.upgrade() else {
                                return div().into_any_element();
                            };
                            let viewer = viewer.read(cx);
                            let range = viewer.lines.get(ix).cloned().unwrap_or_default();
                            let line = viewer
                                .text
                                .get(range.clone())
                                .unwrap_or_default()
                                .to_string();
                            // The match it opened at, on its line.
                            let marked = viewer
                                .marked
                                .clone()
                                .filter(|at| {
                                    at.start < range.end.max(range.start + 1)
                                        && at.end > range.start
                                })
                                .map(|at| {
                                    at.start.max(range.start) - range.start
                                        ..at.end.min(range.end) - range.start
                                });
                            let text = match marked {
                                Some(marked) if line.get(marked.clone()).is_some() => {
                                    let (_, strong) = super::find::match_colors(cx);
                                    StyledText::new(line)
                                        .with_highlights([(
                                            marked,
                                            HighlightStyle {
                                                background_color: Some(strong),
                                                ..HighlightStyle::default()
                                            },
                                        )])
                                        .into_any_element()
                                }
                                _ => SelectableText::new(("output-line", ix as u64), line)
                                    .into_any_element(),
                            };
                            div().px_2().child(text).into_any_element()
                        })
                        .size_full(),
                    ),
            )
            .child(
                h_flex().justify_end().child(
                    Clipboard::new("copy-output")
                        .tooltip("Copy all")
                        .value(self.copy_value()),
                ),
            )
    }
}

/// A notice's icon, by its level. The line that opens a turn Arko started
/// for news (a worker's report, a job that finished, a monitor's alert)
/// has an inbox, so it stands apart from notices about how a turn goes.
fn notice_icon(item: &Item) -> Lucide {
    match item.level {
        Some(Level::Error) => Lucide::CircleX,
        Some(Level::Warn) => Lucide::TriangleAlert,
        _ if item.opens_turn => Lucide::Inbox,
        _ => Lucide::Info,
    }
}

/// A line's toggle, a button and a Tab stop that opens and closes what the
/// line holds and tells assistive technology whether it's open. It is a
/// line of work's height, and its padding stands out past the line's edges,
/// so the line's marker stays in the markers' column.
#[derive(IntoElement)]
struct Disclosure {
    id: ElementId,
    open: bool,
    /// What it's called, read by assistive technology.
    label: SharedString,
    content: AnyElement,
    /// It has nothing to show, so it's its content alone, where it would be.
    inert: bool,
    on_toggle: Callback,
}

impl RenderOnce for Disclosure {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let theme = cx.theme();
        let [_, hover] = disclosure_fills(theme);
        let ring = theme.ring;
        // An extra-small button's padding, its border (the focus ring)
        // taken out of it.
        let inset = window.rem_size() * 0.25;
        let sized = |this: Div| {
            this.h(rems(LINE))
                .mx(-inset)
                .min_w_0()
                .flex()
                .items_center()
                .px(inset - px(1.))
                .text_xs()
                .border_1()
                .border_color(transparent_black())
        };
        if self.inert {
            return sized(div()).child(self.content).into_any_element();
        }
        sized(div())
            .id(self.id)
            .role(Role::Button)
            .aria_expanded(self.open)
            .aria_label(self.label)
            .test_support()
            .track_focus(&focus)
            .rounded(theme.radius)
            .hover(move |this| this.bg(hover))
            .focus_visible(move |this| this.border_color(ring))
            .child(self.content)
            // A click, or Enter or Space while it has the focus.
            .on_click(move |_, window, cx| (self.on_toggle)(window, cx))
            .into_any_element()
    }
}

/// How a call's line and the failure beside it are drawn: `text`, in a
/// [`Disclosure`].
pub(super) fn call_line_ink(text: Hsla, theme: &Theme) -> Ink {
    Ink::in_control(text, disclosure_fills(theme))
}

/// How a worker card's title is drawn: in the label of an outline button.
pub(super) fn worker_title_ink(theme: &Theme) -> Ink {
    Ink::in_control(theme.button_foreground, outline_button_fills(theme))
}

/// What a [`Disclosure`] paints under its content, normally and hovered:
/// nothing, then a ghost button's hover.
pub(super) fn disclosure_fills(theme: &Theme) -> [Hsla; 2] {
    let hover = if theme.is_dark() {
        theme.accent.opacity(0.5)
    } else {
        theme.accent
    };
    [transparent_black(), hover]
}

/// What GPUI Kit's outline button paints under its label, normally and
/// hovered.
pub(super) fn outline_button_fills(theme: &Theme) -> [Hsla; 2] {
    [
        theme.input_background(),
        theme.input.mix_oklab(theme.transparent, 0.5),
    ]
}

/// What the menu item that switches a reply between its formatted text and
/// its Markdown source says, when the reply shows its source (`shown`) or
/// not.
pub(super) fn source_label(shown: bool) -> &'static str {
    if shown {
        "View formatted"
    } else {
        "View source"
    }
}

/// Actions that show while the pointer is over their entry, or while one of
/// them has the focus. Hidden, they take their space (nothing moves when
/// they show) and no Tab stop.
#[derive(IntoElement)]
struct HoverActions {
    id: ElementId,
    /// Whether the pointer is over the entry.
    shown: bool,
    /// Builds the actions, as Tab stops when they show.
    build: Box<dyn FnOnce(bool) -> AnyElement>,
}

impl RenderOnce for HoverActions {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| cx.focus_handle())
            .read(cx)
            .clone();
        let shown = self.shown || focus.contains_focused(window, cx);
        div()
            .id(self.id)
            .test_support()
            .track_focus(&focus)
            .opacity(if shown { 1. } else { 0. })
            .child((self.build)(shown))
    }
}

/// The bubble of the user's words: a message, or the answers given in the
/// answer panel. A note the agent hasn't read (`pending`) is an outline in
/// muted text until it is.
fn bubble(pending: bool, theme: &Theme) -> Div {
    v_flex()
        .min_w_0()
        .gap_1()
        .px_3()
        .py_2()
        .rounded(theme.radius)
        .map(|this| {
            if pending {
                this.border_1()
                    .border_color(theme.border)
                    .text_color(theme.muted_foreground)
            } else {
                this.bg(bubble_color(theme)).text_color(theme.foreground)
            }
        })
}

/// `bubble` at the right of the column, as wide as its words up to half
/// the column, with `below` under it at its right edge. The space at its
/// left gives way first as the column narrows, so a narrow column gives the
/// bubble the width of a comfortable line and a very narrow one all of it.
fn at_right(bubble: impl IntoElement, below: Option<AnyElement>) -> Div {
    h_flex()
        .w_full()
        .items_start()
        .child(
            div()
                .flex_grow(1000.)
                .flex_basis(px(0.))
                .max_w(relative(0.5)),
        )
        .child(
            v_flex()
                .flex_grow(1.)
                .flex_basis(rems(USER_BUBBLE_WIDTH))
                .min_w_0()
                .items_end()
                .gap_1()
                .child(h_flex().w_full().justify_end().child(bubble))
                .children(below),
        )
}

/// The widest a bubble grows in a reading column `column` pixels wide: half
/// the column, or a comfortable line when that's more, and at most the
/// column.
fn bubble_width(column: f32, rem: f32) -> f32 {
    column.min((column * 0.5).max(USER_BUBBLE_WIDTH * rem))
}

/// How far a user's message's bubble moves from the background toward the
/// text: a quiet step that still sets it apart. Light themes take a larger
/// one, as the same step reads weaker on a light background.
const BUBBLE_STEP_DARK: f32 = 0.09;
const BUBBLE_STEP_LIGHT: f32 = 0.13;

/// The surface of a user's message: the theme's background a step toward
/// its text, so every theme gets a bubble in its own tones.
pub(super) fn bubble_color(theme: &Theme) -> Hsla {
    let step = if theme.is_dark() {
        BUBBLE_STEP_DARK
    } else {
        BUBBLE_STEP_LIGHT
    };
    blend(theme.background, theme.foreground, step)
}

/// `from` moved `amount` of the way to `to`, each sRGB channel on its own,
/// which keeps the hue of greys and tints alike.
fn blend(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let (from, to) = (from.to_rgb(), to.to_rgb());
    let mix = |a: f32, b: f32| a + (b - a) * amount;
    Rgba {
        r: mix(from.r, to.r),
        g: mix(from.g, to.g),
        b: mix(from.b, to.b),
        a: from.a,
    }
    .into()
}

/// Regenerate, for the entry `id` it shows on: it sends the user's last
/// message again, after asking.
fn regenerate_button(
    id: u64,
    tooltip: &'static str,
    disabled: bool,
    shown: bool,
    color: Hsla,
    view: WeakEntity<TranscriptView>,
) -> impl IntoElement {
    div().id(("regenerate", id)).test_support().child(
        Button::new("button")
            .ghost()
            .xsmall()
            .icon(Icon::new(Lucide::RotateCcw))
            .text_color(color)
            .tab_stop(shown)
            .disabled(disabled)
            .tooltip(tooltip)
            .accessibility_label("Regenerate")
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.change(TranscriptEvent::Regenerate, cx))
                    .ok();
            }),
    )
}

/// A thumbnail's size, in rem: up to four fit the reading column.
pub(super) const THUMBNAIL: (f32, f32) = (10., 7.5);

/// What a click on a thumbnail or a toggle does.
type Callback = Rc<dyn Fn(&mut Window, &mut App)>;

/// One image of a row, as a tile that opens the image viewer (a click, or
/// Enter when it has the focus, which it gets back when the viewer closes).
#[derive(IntoElement)]
struct Thumbnail {
    id: ElementId,
    /// `None` when the session's store no longer keeps the image.
    image: Option<Arc<Image>>,
    cache: Entity<LruImageCache>,
    caption: SharedString,
    on_open: Callback,
}

impl RenderOnce for Thumbnail {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let focus = window
            .use_keyed_state(self.id.clone(), cx, |_, cx| {
                cx.focus_handle().tab_stop(true)
            })
            .read(cx)
            .clone();
        let theme = cx.theme();
        let caption = self.caption.clone();
        let name: SharedString = format!("Open image {}", self.caption).into();
        let click_focus = focus.clone();
        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(name)
            .test_support()
            .track_focus(&focus)
            .flex_none()
            .w(rems(THUMBNAIL.0))
            .h(rems(THUMBNAIL.1))
            .flex()
            .items_center()
            .justify_center()
            .overflow_hidden()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted)
            // Ringed only while the keyboard is in use.
            .focus_visible(|this| this.border_color(theme.ring))
            .managed_tooltip(move |window, cx| Tooltip::new(caption.clone()).build(window, cx))
            .map(|this| match self.image {
                Some(image) => {
                    // Decoded at the tile's size, which the rem sets (the UI
                    // font size and the zoom); the cache applies the
                    // screen's scale.
                    let rem = window.rem_size();
                    let fit = size(rem * THUMBNAIL.0, rem * THUMBNAIL.1);
                    let source = LruImageCache::source(&self.cache, image, Some(fit));
                    this.child(img(source).size_full().object_fit(ObjectFit::Contain))
                }
                None => this
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Image not kept"),
            })
            // A click, or Enter or Space while it has the focus.
            .on_click(move |_, window, cx| {
                click_focus.focus(window, cx);
                (self.on_open)(window, cx);
            })
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::super::find::shown_prefix;
    use super::{
        AgentFilter, CACHE_REFRESHED, Lucide, OutputViewer, PREVIEW_LINE_CHARS, REGENERATE_HINT,
        apply_diff_patch, code_lines, duration, edit_diff, longest_code, message_label,
        notice_icon, preview, show_all, user_tags,
    };
    use arko::remote::{Delivery, Item, Kind, Level, Source};
    use gpui_kit::AppContext as _;
    use std::sync::Arc;

    /// The line that opens a turn Arko started has an inbox, and other
    /// notices their level's icon.
    #[test]
    fn a_turns_opening_line_has_an_icon_of_its_own() {
        let notice = |text: &str, level: Level| Item::new(Kind::Notice, text).level(level);
        let icon = |item: &Item| notice_icon(item).path();
        assert_eq!(
            icon(&notice("r1 reported", Level::Info).opens_turn()),
            Lucide::Inbox.path()
        );
        assert_eq!(
            icon(&notice("Resumed the session.", Level::Info)),
            Lucide::Info.path()
        );
        assert_eq!(
            icon(&notice("rate limited", Level::Warn)),
            Lucide::TriangleAlert.path()
        );
        assert_eq!(
            icon(&notice("The request failed.", Level::Error)),
            Lucide::CircleX.path()
        );
    }

    #[gpui_kit::test]
    fn the_output_dialogs_copy_shares_the_output(cx: &mut gpui_kit::TestAppContext) {
        let text: Arc<str> = "line\n".repeat(100_000).into();
        let viewer = cx.new(|cx| OutputViewer::new(text.clone(), None, cx));
        let copy = cx.read(|cx| viewer.read(cx).copy_value());
        assert_eq!(copy.as_ptr(), text.as_ptr());
    }

    #[test]
    fn an_edits_diff_is_numbered_from_the_line_it_changed() {
        let mut item = Item::new(Kind::Call, "edit src/a.rs");
        item.tool = Some("edit_file".into());
        item.parameters =
            Some(r#"{"new_string":"b\nc\n","old_string":"b\n","path":"src/a.rs"}"#.into());
        let diff = edit_diff(&item).unwrap();
        assert!(
            diff.to_unified().contains("@@ -1,1 +1,2 @@"),
            "{}",
            diff.to_unified()
        );
        item.output = Some("Edited src/a.rs at lines 40-41: ...".into());
        let diff = edit_diff(&item).unwrap();
        assert!(
            diff.to_unified().contains("@@ -40,1 +40,2 @@"),
            "{}",
            diff.to_unified()
        );
        assert_eq!((diff.added_lines(), diff.removed_lines()), (1, 0));
    }

    #[test]
    fn durations_read_as_people_say_them() {
        assert_eq!(duration(0), "0 s");
        assert_eq!(duration(12_400), "12 s");
        assert_eq!(duration(65_000), "1 m 5 s");
        assert_eq!(duration(7_380_000), "2 h 3 m");
    }

    #[test]
    fn the_longest_code_block_counts_inside_lists_and_quotes() {
        let code: String = (0..50).map(|n| format!("   let x{n} = {n};\n")).collect();
        let list = format!("1. Run this:\n\n   ```rust\n{code}   ```\n2. Done.\n");
        assert_eq!(longest_code(&list), 50);
        let quote = "> ```\n> a\n> b\n> ```\n> text";
        assert_eq!(longest_code(quote), 2);
        // A block still streaming counts what came.
        assert_eq!(longest_code("- step\n\n  ~~~~\n  a\n  ```\n  b\n"), 3);
        assert_eq!(longest_code("no code here"), 0);
    }

    #[test]
    fn a_preview_shows_forty_lines_of_at_most_500_characters() {
        let long = "x".repeat(300_000);
        let (shown, more) = preview(&long);
        assert!(more);
        assert_eq!(shown.chars().count(), 501);
        let many: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let (shown, more) = preview(&many);
        assert_eq!((shown.lines().count(), more), (40, true));
        assert_eq!(show_all(&many).as_deref(), Some("Show all 100 lines"));
    }

    /// A long line ends the preview at its cut, so the lines after it stay
    /// behind Show all with the rest of the line, and the find counts as
    /// shown just what the preview shows.
    #[test]
    fn a_cut_line_ends_the_preview_and_show_all_opens_the_rest() {
        let first = format!("{}tail.", "x".repeat(PREVIEW_LINE_CHARS));
        let text = format!("{first}\n\nSecond paragraph.");
        let (shown, more) = preview(&text);
        assert_eq!(shown, format!("{}…", "x".repeat(PREVIEW_LINE_CHARS)));
        assert!(more);
        assert_eq!(show_all(&text).as_deref(), Some("Show all 3 lines"));
        assert_eq!(&text[..shown_prefix(&text)], shown.trim_end_matches('…'));
        assert_eq!(show_all(&first).as_deref(), Some("Show all"));
        assert_eq!(preview("a\nb"), ("a\nb".to_string(), false));
        assert_eq!(show_all("a\nb"), None);
    }

    #[test]
    fn an_apply_diff_call_shows_its_patch() {
        let mut item = Item::new(Kind::Call, "edit a.rs");
        item.tool = Some("apply_diff".into());
        item.parameters = Some(
            r#"{"diff":"<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE","path":"a.rs"}"#.into(),
        );
        assert_eq!(
            apply_diff_patch(&item).as_deref(),
            Some("<<<<<<< SEARCH\na\n=======\nb\n>>>>>>> REPLACE")
        );
    }

    #[test]
    fn code_lines_leave_out_the_fences() {
        assert_eq!(code_lines("```rust\nfn a() {}\nfn b() {}\n```\n"), 2);
        assert_eq!(code_lines("```rust\nfn a() {}\nfn b() {}"), 2);
        assert_eq!(code_lines("    indented\n    code\n"), 2);
    }

    /// Both Regenerate buttons, the one among a reply's actions and the one
    /// on a turn's last line, say the same.
    #[test]
    fn the_regenerate_buttons_share_their_words() {
        assert_eq!(REGENERATE_HINT, "Send your last message again");
        assert_eq!(CACHE_REFRESHED, "Cache refreshed");
    }

    /// The line over a message says who sent it to an agent, and in the
    /// main transcript whose a worker's message to the user is. The worker's
    /// own transcript and the main agent's messages need none.
    #[test]
    fn a_message_says_whose_it_is() {
        let message = |agent: &str, from: Option<&str>| {
            let mut item = Item::new(Kind::Message, "Found it.").agent(agent);
            item.from = from.map(str::to_string);
            item
        };
        let main = AgentFilter::Main;
        let own = AgentFilter::Worker("r3".into());
        let label = |filter, item| message_label(filter, &item);
        assert_eq!(
            label(&main, message("r3", None)).as_deref(),
            Some("r3 to you")
        );
        assert_eq!(label(&own, message("r3", None)), None);
        assert_eq!(label(&main, message("main", None)), None);
        assert_eq!(
            label(&own, message("r3", Some("main"))).as_deref(),
            Some("From the lead")
        );
        assert_eq!(
            label(&own, message("r3", Some("r1"))).as_deref(),
            Some("From r1")
        );
        let text = Item::new(Kind::Text, "Found it.").agent("r3");
        assert_eq!(label(&main, text), None);
    }

    #[test]
    fn a_message_is_tagged_by_what_became_of_it() {
        let tags = |delivery, pending, source| {
            let mut item = Item::new(Kind::User, "Hello.");
            item.delivery = delivery;
            item.pending = pending;
            item.source = source;
            user_tags(&item)
        };
        // A note the agent hasn't read is "Pending" alone, and a message
        // like any other once it has.
        assert_eq!(tags(Some(Delivery::Note), true, None), ["Pending"]);
        assert!(tags(Some(Delivery::Note), false, None).is_empty());
        assert_eq!(tags(Some(Delivery::Answer), false, None), ["Answer"]);
        assert_eq!(tags(Some(Delivery::Queued), false, None), ["Queued"]);
        // A command shows its slash, and a turn is the usual.
        assert!(tags(Some(Delivery::Command), false, None).is_empty());
        assert!(tags(Some(Delivery::Turn), false, None).is_empty());
        assert_eq!(
            tags(Some(Delivery::Note), true, Some(Source::Web)),
            ["Pending", "From the web page"]
        );
    }
}
