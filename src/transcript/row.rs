//! How each row of the transcript draws.

use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use arko::remote::{
    CallStatus, Delivery, Item, JobKind, JobState, JobSummary, Kind, Level, Source, WorkerState,
};
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::base::image_cache::LruImageCache;
use gpui_kit::base::text::{TextView, TextViewMotion, TextViewStyle};
use gpui_kit::base::{Easing, SelectableText};
use gpui_kit::component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, Theme, WindowExt as _,
    button::{Button, ButtonVariants as _},
    clipboard::Clipboard,
    diff_view::{Diff, DiffView},
    h_flex,
    menu::{DropdownMenu as _, PopupMenuItem},
    shimmer::ShimmerText,
    tooltip::Tooltip,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use super::find::Place;
use crate::fonts::CodeFont as _;

use super::height::Metrics;
use super::model::{Gap, RowKey};
use super::tool::{self, Tool};
use super::{AgentFilter, CODE_LINES, TranscriptEvent, TranscriptView};

/// How often "Thinking…" shimmers, in frames a second.
const SHIMMER_FPS: f32 = 12.;

/// The reading column's widest.
const COLUMN: f32 = 46.;
/// The width, in rem, a user's message may take even when that is more than
/// half the column, the width of a comfortable line.
const USER_BUBBLE_WIDTH: f32 = 17.5;
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

/// The parameters a call's details show: none where its line says them
/// all or its diff does, else its parameters (or, from before the feed kept
/// them, the line's long part: a multi-line command, a worker's task).
pub(super) fn shown_parameters(item: &Item) -> Option<Arc<str>> {
    let line = item.text.trim();
    if parameters_shown_by_line(item) {
        None
    } else if let Some(patch) = apply_diff_patch(item) {
        // An edit from a session saved before edit_file: its patch.
        Some(patch.into())
    } else if let Some(parameters) = item.parameters.clone() {
        Some(parameters)
    } else if line.contains('\n') || item.detail.is_some() {
        Some(item.detail.clone().unwrap_or(line.to_string()).into())
    } else {
        None
    }
}

/// What a call's details show came back: its command's output block, or
/// its tool's result. An edit's diff stands for its result.
pub(super) fn shown_output(entry: &super::model::Entry) -> Option<Arc<str>> {
    let item = &entry.item;
    match (&entry.result, item.output.clone()) {
        (Some(result), _) => result
            .whole
            .clone()
            .or_else(|| result.detail.clone().map(Arc::from)),
        (None, Some(_)) if shows_diff(item) && item.status == Some(CallStatus::Ok) => None,
        (None, Some(output)) if !output.trim().is_empty() => Some(output),
        _ => None,
    }
}

/// Whether a call's details show a diff: an edit's or a write's.
fn shows_diff(item: &Item) -> bool {
    matches!(item.tool.as_deref(), Some("edit_file" | "write_file")) && item.parameters.is_some()
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

/// The first lines of `text` for a preview, each at most
/// [`PREVIEW_LINE_CHARS`] long, and how many lines it has.
pub(super) fn preview(text: &str) -> (String, usize) {
    let total = text.lines().count();
    let shown = text
        .lines()
        .take(OUTPUT_LINES)
        .map(|line| match line.char_indices().nth(PREVIEW_LINE_CHARS) {
            Some((end, _)) => format!("{}…", &line[..end]),
            None => line.to_string(),
        })
        .collect::<Vec<_>>()
        .join("\n");
    (shown, total)
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
    match view.read(cx).rows.get(ix).map(|row| row.key) {
        Some(key @ RowKey::Block { id, block, .. }) => view.update(cx, |this, cx| {
            this.block_view(id, block, cx);
            this.run_handles(key, cx);
        }),
        Some(key) => view.update(cx, |this, cx| this.run_handles(key, cx)),
        None => {}
    }
    let this = view.read(cx);
    let Some(row) = this.rows.get(ix).copied() else {
        return div().into_any_element();
    };
    this.drawing_row.set(ix);
    let content = this.row(row.key, &view, cx);
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
    // The row the keyboard is on shows a ring, laid over it so nothing
    // moves, while the keyboard is in use.
    let cursor = this.is_cursor(row.key, window, cx);
    let ring = (cursor && window.last_input_was_keyboard()).then(|| {
        let theme = cx.theme();
        div()
            .absolute()
            .top_neg_0p5()
            .bottom_neg_0p5()
            .left_2()
            .right_2()
            .border_2()
            .border_color(theme.ring)
            .rounded(theme.radius)
    });
    // To assistive technology, a list item it can move through: the one
    // the keyboard is on is the one in focus.
    let announced = this.row_announced(row.key);
    let label = this.row_label(row.key);
    let expanded = this.row_expanded(row.key);
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
        .pt(rems(gap))
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
                .child(content)
                .children(ring),
        )
        .into_any_element();
    if !run_texts.is_empty() {
        view.update(cx, |this, cx| this.keep_run_texts(run_texts, cx));
    }
    element
}

/// The space above a row, in rem.
fn gap_rems(gap: Gap) -> f32 {
    match gap {
        Gap::None => 0.,
        Gap::Tight => 0.25,
        Gap::Small => 0.5,
        Gap::Section => 1.,
        Gap::Turn => 1.75,
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
    let measured;
    let metrics = match &this.metrics {
        Some(metrics) => metrics,
        None => {
            measured = Metrics::current(None, window, cx);
            &measured
        }
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
        // The reading column, inside its side padding.
        let column = width.min(COLUMN * rem) - 2. * rem;
        let body = match row.key {
            RowKey::Separator(_) | RowKey::Refresh(_) | RowKey::Turn(_) => m.line_xs,
            // Extra-small buttons and toggles.
            RowKey::Working(_)
            | RowKey::Actions(_)
            | RowKey::Thinking(_)
            | RowKey::Group(_)
            | RowKey::Count(_) => 1.25 * rem,
            // A small button.
            RowKey::Worker(_) => 1.5 * rem,
            RowKey::User(id) => self.user_height(id, column, m),
            RowKey::Call(id) => self.call_height(id, column, m, cx),
            RowKey::Notice(id) => self.notice_height(id, column, m),
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
        // The bubble is at most half the column, or a comfortable line when
        // that's more, and at most the column.
        let bubble = column.min((column * 0.5).max(USER_BUBBLE_WIDTH * rem));
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

    fn call_height(&self, id: u64, column: f32, m: &Metrics, cx: &App) -> f32 {
        let rem = m.rem;
        let Some(ui) = self.ui.get(&id) else {
            return 0.;
        };
        let mut height = 1.25 * rem;
        // The thumbnails and the details are indented past the icon.
        let inside = column - 1.5 * rem;
        if ui.images > 0 {
            height += 0.25 * rem + thumbnails_height(ui.images, inside, rem);
        }
        if ui.expanded == Some(true)
            && let Some(entry) = self.model.entry(id)
        {
            height += self.details_height(entry, inside, m, cx);
        }
        height
    }

    /// The height of a call's details, as [`Self::call_details`] lays them
    /// out in `width`.
    fn details_height(
        &self,
        entry: &super::model::Entry,
        width: f32,
        m: &Metrics,
        cx: &App,
    ) -> f32 {
        let rem = m.rem;
        let item = &entry.item;
        // The first lines of a text in the code font, and "Show all".
        let lines = |text: &str| {
            let (shown, total) = preview(text);
            let lines = super::height::hard_lines(&shown);
            let mut height = 0.75 * rem + m.mono.text_lines(&lines, width - rem) * m.line_mono;
            if total > OUTPUT_LINES {
                height += 0.25 * rem + 1.25 * rem;
            }
            height
        };
        let caption = m.line_xs;
        let mut parts = Vec::new();
        let diff = self.ui.get(&item.id).and_then(|ui| ui.diff.clone());
        let has_diff = diff.is_some();
        if let Some(diff) = &diff {
            let rows = diff.read(cx).row_count().clamp(1, DIFF_ROWS);
            parts.push(f32::from(DiffView::row_height(cx)) * rows as f32);
        } else if let Some(parameters) = shown_parameters(item) {
            parts.push(caption);
            parts.push(lines(&parameters));
        }
        match (&entry.result, item.output.as_deref()) {
            (Some(_), _) => {
                parts.push(caption);
                if let Some(body) = shown_output(entry) {
                    parts.push(lines(&body));
                }
            }
            (None, Some(_)) if has_diff && item.status == Some(CallStatus::Ok) => {}
            (None, Some(output)) if !output.trim().is_empty() => {
                parts.push(caption);
                parts.push(lines(output));
            }
            (None, _) => {
                if item.outcome.is_some() {
                    parts.push(caption);
                }
            }
        }
        // Their top padding, and the gaps between them.
        0.25 * rem + parts.iter().sum::<f32>() + 0.375 * rem * parts.len().saturating_sub(1) as f32
    }

    fn notice_height(&self, id: u64, column: f32, m: &Metrics) -> f32 {
        let rem = m.rem;
        let Some(item) = self.item(id) else {
            return 0.;
        };
        // Inside its padding, past its icon.
        let mut width = column - rem - 0.75 * rem - 0.375 * rem;
        if !item.is_main() {
            let agent = item.agent.chars().count() as f32;
            width -= agent * m.mono.advance_at(0.75 * rem) + 0.375 * rem;
        }
        if self.offers_regenerate(RowKey::Notice(id)) {
            width -= REGENERATE_ROOM * rem;
        }
        let lines = self.ui.get(&id).map_or(&[][..], |ui| &ui.lines);
        m.body_at(0.875).text_lines(lines, width) * m.line_sm
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
        // A thinking's text is small.
        let (body, line) = if ui.small {
            (m.body_at(0.875), m.line_sm)
        } else {
            (m.body, m.line)
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
                    // Inside its padding, long lines wrapped.
                    1.5 * rem + m.mono.text_lines(&lines[..shown], column - 1.5 * rem) * m.line_mono
                }
                BlockFacts::Table(rows) => *rows as f32 * (line + 0.5 * rem + 1.) + 1. + rem,
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
        RowKey::Working(id) => ("working", id),
    };
    (name, id).into()
}

/// The running indicator, turning when `turns` (and the system allows
/// motion). It is the kit's spinner, which turns evenly in each frame the
/// display shows and turns as it paints, so the transcript isn't drawn again
/// for each step.
fn spinner(id: impl Into<ElementId>, turns: bool, cx: &App) -> AnyElement {
    if !turns || cx.reduce_motion() {
        return Icon::new(IconName::Loader).xsmall().into_any_element();
    }
    div()
        .id(id)
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
                && ["monitoring", "ended", "stopped", "its command exited"]
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
/// alerts", "Its command exited (exit 1)".
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
        JobState::Finished => (format!("Its command exited{alerts}"), false),
        JobState::Failed => (
            match &job.outcome {
                Some(how) => format!("Its command exited ({how}){alerts}"),
                None => format!("Its command exited{alerts}"),
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

/// The muted words on the first line of a user's message, for what its
/// place doesn't already show. A note the agent hasn't read is "Pending", and
/// once it has, it is a message like any other. A slash command reads as one
/// by itself.
fn user_tags(item: &Item) -> Vec<String> {
    let mut tags = Vec::new();
    match item.delivery {
        Some(Delivery::Note) if item.pending => tags.push("Pending".into()),
        Some(Delivery::Answer) => tags.push("Answer".into()),
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

/// How the rows' Markdown looks: the theme's text style, with tables that
/// scroll sideways inside their row rather than squeeze their columns.
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
        .with_code_background(theme.muted)
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
    fn row(&self, key: RowKey, view: &Entity<Self>, cx: &App) -> AnyElement {
        match key {
            RowKey::Separator(id) => self.separator(id, cx),
            RowKey::User(id) => self.user(id, view, cx),
            RowKey::Refresh(_) => self.refresh(cx),
            RowKey::Block {
                id, block, source, ..
            } => self.block(id, block, source, view, cx),
            RowKey::Actions(id) => self.actions(id, view, cx),
            RowKey::Thinking(id) => self.thinking(id, view, cx),
            RowKey::Call(id) => self.call(id, view, cx),
            RowKey::Group(id) => self.group(id, view, cx),
            RowKey::Count(id) => self.count(id, view, cx),
            RowKey::Notice(id) => self.notice(id, view, cx),
            RowKey::Output(id) => self.output(id, cx),
            RowKey::Turn(id) => self.turn(id, view, cx),
            RowKey::Worker(id) => self.worker(id, view, cx),
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

    /// The quiet line of a message that only kept the prompt cache warm.
    fn refresh(&self, cx: &App) -> AnyElement {
        let theme = cx.theme();
        div()
            .w_full()
            .px_2()
            .text_xs()
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
                            Button::new("button")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .tooltip("Copy")
                                .accessibility_label("Copy message")
                                .text_color(muted)
                                .tab_stop(shown)
                                .on_click(move |_, _, cx| {
                                    copy_view
                                        .update(cx, |this, cx| this.copy_entry(id, cx))
                                        .ok();
                                }),
                        ),
                    )
                    .into_any_element()
            }),
        };
        // The message is a bubble at the right of the column, as wide as its
        // words up to half the column. The space at its left gives way first
        // as the column narrows, so a narrow column gives the bubble the
        // width of a comfortable line and a very narrow one all of it. Its
        // actions keep their room under it, shown or not, so showing them
        // moves nothing.
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
                    .child(
                        h_flex().w_full().justify_end().child(
                            v_flex()
                                .id(("user-message", id))
                                .test_support()
                                .min_w_0()
                                .gap_1()
                                .px_3()
                                .py_2()
                                .rounded(theme.radius)
                                .map(|this| {
                                    // A note the agent hasn't read is an
                                    // outline in muted text until it is.
                                    if item.pending {
                                        this.border_1().border_color(theme.border).text_color(muted)
                                    } else {
                                        this.bg(bubble_color(theme)).text_color(theme.foreground)
                                    }
                                })
                                .when(!tags.is_empty(), |this| {
                                    this.child(
                                        div().text_xs().text_color(muted).child(tags.join(" · ")),
                                    )
                                })
                                .children(self.thumbnails(item, view, cx))
                                // As wide as its words on one line, up to the
                                // bubble's widest, where it wraps.
                                .child(self.marked(("user-text", id), id, Place::Text, text, cx)),
                        ),
                    )
                    .child(actions),
            )
            .into_any_element()
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
                .child("Question")
        });
        let from = (block == 0 && item.kind == Kind::Message)
            .then(|| item.from.clone())
            .flatten()
            .map(|from| {
                div()
                    .pb_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(if from == arko::remote::MAIN_AGENT {
                        "From the lead".to_string()
                    } else {
                        format!("From {from}")
                    })
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
                .child(self.marked(("source", id), id, Place::Source(block), text, cx))
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
        div()
            .w_full()
            .when(thinking, |this| {
                this.text_color(theme.muted_foreground).text_sm()
            })
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
                            Button::new("button")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Copy)
                                .tooltip("Copy")
                                .accessibility_label("Copy message")
                                .text_color(muted)
                                .tab_stop(shown)
                                .on_click(move |_, _, cx| {
                                    copy_view
                                        .update(cx, |this, cx| this.copy_entry(id, cx))
                                        .ok();
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

    /// The running indicator of the entry `id`: it turns only for the
    /// latest thing that runs, and stands still for the others.
    fn spinner(&self, id: u64, cx: &App) -> AnyElement {
        spinner(("running", id), self.animated == Some(id), cx)
    }

    /// "Working…" while the agent waits on the model and no other row says
    /// it's busy. It's the latest thing that runs, so its indicator turns.
    fn working(&self, id: u64, cx: &App) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .h_5()
            .px_1()
            .gap_1()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(spinner(("working-spinner", id), true, cx))
            .child("Working…")
            .into_any_element()
    }

    /// Whether thinking shows open unless the reader chose.
    pub(super) fn thinking_open_by_default(&self) -> bool {
        self.settings.expand_thinking
            && (self.settings.view == crate::prefs::TranscriptView::Full
                || matches!(self.model.filter(), AgentFilter::Worker(_)))
    }

    fn thinking(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Ok(ix) = self.model.position(id) else {
            return div().into_any_element();
        };
        let default = self.thinking_open_by_default();
        let open = self
            .ui
            .get(&id)
            .and_then(|ui| ui.expanded)
            .unwrap_or(default);
        let streaming = self.model.is_streaming(ix);
        let name: SharedString = if streaming {
            "Thinking…".into()
        } else {
            match self.model.thought_for(ix) {
                Some(ms) => format!("Thought for {}", duration(ms)).into(),
                None => "Thought".into(),
            }
        };
        let label: AnyElement = if streaming && self.animated == Some(id) && !cx.reduce_motion() {
            ShimmerText::new("Thinking…")
                .id(("thinking-shimmer", id))
                .max_fps(SHIMMER_FPS)
                .into_any_element()
        } else if streaming {
            "Thinking…".into_any_element()
        } else {
            match self.model.thought_for(ix) {
                Some(ms) => format!("Thought for {}", duration(ms)),
                None => "Thought".to_string(),
            }
            .into_any_element()
        };
        let view = view.downgrade();
        h_flex()
            .w_full()
            .child(
                div()
                    .id(("thinking-row", id))
                    .test_support()
                    .child(Disclosure {
                        id: ("thinking-toggle", id).into(),
                        open,
                        label: name,
                        full_width: false,
                        content: h_flex()
                            .gap_1()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(Icon::new(chevron(open)).xsmall())
                            .child(label)
                            .into_any_element(),
                        on_toggle: Rc::new(move |_, cx| {
                            view.update(cx, |this, cx| this.toggle(id, default, cx))
                                .ok();
                        }),
                    }),
            )
            .into_any_element()
    }

    fn call(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let open = self.ui.get(&id).and_then(|ui| ui.expanded).unwrap_or(false);
        let line = item.text.lines().next().unwrap_or_default().to_string();
        let tool = Tool::of(item);
        let job = self.job_line(item);
        let muted = |text: String| {
            div()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
                .into_any_element()
        };
        let trailing: Option<AnyElement> = match (&job, item.status) {
            (Some(JobLine::Outcome { text, failed: true }), _) => Some(
                h_flex()
                    .flex_none()
                    .min_w_0()
                    .max_w(rems(18.))
                    .gap_1()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(Icon::new(Lucide::CircleX).xsmall())
                    .child(div().min_w_0().truncate().child(text.clone()))
                    .into_any_element(),
            ),
            (Some(job), _) => Some(muted(self.job_words(job))),
            (None, Some(CallStatus::Running)) => Some(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(self.spinner(id, cx))
                    .child(duration((self.clock)() - item.at))
                    .into_any_element(),
            ),
            (None, Some(CallStatus::Failed)) => Some(
                h_flex()
                    .flex_none()
                    .min_w_0()
                    .max_w(rems(18.))
                    .gap_1()
                    .text_xs()
                    .text_color(theme.danger)
                    .child(Icon::new(Lucide::CircleX).xsmall())
                    .when_some(item.outcome.clone(), |this, outcome| {
                        this.child(div().min_w_0().truncate().child(self.marked_plain(
                            id,
                            Place::Outcome,
                            outcome,
                            cx,
                        )))
                    })
                    .into_any_element(),
            ),
            _ => None,
        };
        let action = self.call_action(item, job.as_ref(), view);
        let header_view = view.downgrade();
        let header = Disclosure {
            id: ("call-toggle", id).into(),
            open,
            label: line.clone().into(),
            full_width: true,
            content: h_flex()
                .w_full()
                .min_w_0()
                .gap_1p5()
                .text_color(theme.muted_foreground)
                .when(!item.is_main(), |this| {
                    this.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .code_font(theme)
                            .child(item.agent.clone()),
                    )
                })
                .child(Icon::new(tool.icon()).xsmall().flex_none())
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_left()
                        .child(self.marked_plain(id, Place::Text, line.clone(), cx)),
                )
                .children(trailing)
                .into_any_element(),
            on_toggle: Rc::new(move |_, cx| {
                header_view
                    .update(cx, |this, cx| this.toggle(id, false, cx))
                    .ok();
            }),
        };
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
                    .map(|thumbnails| div().pl_6().pt_1().child(thumbnails)),
            )
            .when(open, |this| this.child(self.call_details(id, cx)))
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

    /// A call's details: its parameters in full, its output, its diff.
    fn call_details(&self, id: u64, cx: &App) -> AnyElement {
        use super::find::Place;
        let theme = cx.theme();
        let Some(entry) = self.model.entry(id) else {
            return div().into_any_element();
        };
        let item = &entry.item;
        let mono = |text: String, key: &'static str, place: Place| {
            div()
                .w_full()
                .px_2()
                .py_1p5()
                .rounded(theme.radius)
                .bg(theme.muted)
                .code_font(theme)
                .text_size(theme.mono_font_size)
                .child(self.marked((key, id), id, place, text, cx))
        };
        let caption = |text: String| {
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(text)
        };
        // The first lines of a long text, and a button that shows it all.
        let lines = |text: Arc<str>, key: &'static str, place: Place, title: String| {
            let (shown, total) = preview(&text);
            let title: SharedString = title.into();
            v_flex()
                .w_full()
                .gap_1()
                .child(mono(shown, key, place))
                .when(total > OUTPUT_LINES, |this| {
                    this.child(
                        Button::new((key, id))
                            .ghost()
                            .xsmall()
                            .label(format!("Show all {total} lines"))
                            .on_click(move |_, window, cx| {
                                open_output(title.clone(), text.clone(), window, cx)
                            }),
                    )
                })
        };
        let line = item.text.trim().to_string();
        let mut details = v_flex().w_full().gap_1p5().pt_1().pl_6();
        // What the call was given: an edit's changes as a diff, else the
        // parameters (or, from before the feed kept them, the line's long
        // part: a multi-line command, a worker's task).
        let diff = self.ui.get(&id).and_then(|ui| ui.diff.clone());
        let has_diff = diff.is_some();
        let parameters = shown_parameters(item);
        if let Some(diff) = diff {
            details = details.child(DiffView::new(&diff).max_rows(DIFF_ROWS));
        } else if let Some(parameters) = parameters {
            details = details.child(caption("Parameters".into())).child(lines(
                parameters,
                "parameters",
                Place::Parameters,
                format!("Parameters of {line}"),
            ));
        }
        // What came back: a command's output block, or the tool's result.
        match (&entry.result, item.output.clone()) {
            (Some(result), _) => {
                details = details.child(caption(result.text.clone()));
                if let Some(body) = shown_output(entry) {
                    details = details.child(lines(
                        body,
                        "output",
                        Place::Output,
                        format!("Output of {line}"),
                    ));
                }
            }
            // An edit's diff says what its result would.
            (None, Some(_)) if has_diff && item.status == Some(CallStatus::Ok) => {}
            (None, Some(output)) if !output.trim().is_empty() => {
                details = details.child(caption("Result".into())).child(lines(
                    output,
                    "output",
                    Place::Output,
                    format!("Result of {line}"),
                ));
            }
            (None, _) => {
                if let Some(outcome) = &item.outcome {
                    details = details.child(caption(outcome.clone()));
                }
            }
        }
        details.into_any_element()
    }

    fn group(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let members = super::model::run_of(&self.model, id, self.settings.view);
        let entries = self.model.entries();
        let calls: Vec<&Item> = members
            .iter()
            .map(|ix| &entries[*ix].item)
            .filter(|item| item.kind == Kind::Call)
            .collect();
        let running = calls
            .iter()
            .rev()
            .find(|item| item.status == Some(CallStatus::Running))
            .copied();
        let failed = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Failed))
            .count();
        let open = self.group_open(id);
        // A folded group's header names the call that runs, or else the
        // latest whose job runs, and offers what that call's own row does
        // (Move to background, Stop).
        let in_background = |item: &&&Item| {
            matches!(
                self.job_line(item),
                Some(JobLine::Running { .. } | JobLine::Monitoring { .. })
            )
        };
        let active = running.or_else(|| match open {
            true => None,
            false => calls.iter().rev().find(in_background).copied(),
        });
        let action = match open {
            true => None,
            false => {
                active.and_then(|item| self.call_action(item, self.job_line(item).as_ref(), view))
            }
        };
        let view = view.downgrade();
        let summary = tool::summary(calls.iter().copied());
        let header =
            Disclosure {
                id: ("group-toggle", id).into(),
                open,
                label: summary.clone().into(),
                full_width: true,
                content: h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_1p5()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(chevron(open)).xsmall())
                    .child(div().min_w_0().truncate().child(summary))
                    .when_some(active, |this, active| {
                        this.child(
                            h_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .when(active.status == Some(CallStatus::Running), |this| {
                                    this.child(self.spinner(active.id, cx))
                                })
                                .child(div().min_w_0().truncate().child(
                                    active.text.lines().next().unwrap_or_default().to_string(),
                                )),
                        )
                    })
                    .when(active.is_none(), |this| this.child(div().flex_1()))
                    .when(failed > 0, |this| {
                        this.child(
                            h_flex()
                                .flex_none()
                                .gap_1()
                                .text_color(theme.danger)
                                .child(Icon::new(Lucide::CircleX).xsmall())
                                .child(format!("{failed} failed")),
                        )
                    })
                    .into_any_element(),
                on_toggle: Rc::new(move |_, cx| {
                    view.update(cx, |this, cx| this.toggle_group(id, cx)).ok();
                }),
            };
        let Some(action) = action else {
            return header.into_any_element();
        };
        h_flex()
            .w_full()
            .min_w_0()
            .gap_1()
            .child(div().flex_1().min_w_0().child(header))
            .child(div().flex_none().child(action))
            .into_any_element()
    }

    fn count(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let members = super::model::run_of(&self.model, id, self.settings.view);
        let entries = self.model.entries();
        let calls: Vec<&Item> = members.iter().map(|ix| &entries[*ix].item).collect();
        let failed = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Failed))
            .count();
        let running = calls
            .iter()
            .filter(|item| item.status == Some(CallStatus::Running))
            .count();
        let mut text = tool::summary(calls.iter().copied());
        if running > 0 {
            text.push_str(&format!(" · {running} running"));
        }
        let view = view.downgrade();
        Button::new(("count", id))
            .ghost()
            .xsmall()
            .text_color(theme.muted_foreground)
            .tooltip("Show in the full transcript")
            .child(text)
            .when(failed > 0, |this| {
                this.child(
                    h_flex()
                        .gap_1()
                        .text_color(theme.danger)
                        .child(Icon::new(Lucide::CircleX).xsmall())
                        .child(format!("{failed} failed")),
                )
            })
            .on_click(move |_, _, cx| {
                view.update(cx, |this, cx| this.open_run(id, cx)).ok();
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

    fn notice(&self, id: u64, view: &Entity<Self>, cx: &App) -> AnyElement {
        let theme = cx.theme();
        let Some(item) = self.item(id) else {
            return div().into_any_element();
        };
        // The Messages view hides turns' footers: the notice that says why
        // the turn ended without a reply offers Regenerate instead.
        let regenerate = self.end_line_regenerate(RowKey::Notice(id), view, cx);
        let (icon, color, text_color) = match item.level {
            Some(Level::Error) => (Lucide::CircleX, theme.danger, theme.foreground),
            Some(Level::Warn) => (Lucide::TriangleAlert, theme.warning, theme.foreground),
            _ => (Lucide::Info, theme.muted_foreground, theme.muted_foreground),
        };
        h_flex()
            .relative()
            .w_full()
            .items_start()
            .gap_1p5()
            .px_2()
            .text_sm()
            .text_color(text_color)
            .when(!item.is_main(), |this| {
                this.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .code_font(theme)
                        .text_color(theme.muted_foreground)
                        .child(item.agent.clone()),
                )
            })
            .child(
                div()
                    .flex_none()
                    .pt_0p5()
                    .child(Icon::new(icon).xsmall().text_color(color)),
            )
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
            .child(self.marked(("output-text", id), id, Place::Text, item.text.clone(), cx))
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
            .px_2()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(
                div()
                    .truncate()
                    .when(regenerate.is_some(), |this| this.pr(rems(REGENERATE_ROOM)))
                    .child(self.marked_plain(id, Place::Text, item.text.clone(), cx)),
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
        // Its model, effort and profile, once the session has recorded them.
        let setup = summary.and_then(crate::workers::setup_label);
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
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_left()
                            .child(self.marked_plain(id, Place::Text, title, cx)),
                    )
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

/// The chevron of a disclosure, open or closed.
fn chevron(open: bool) -> IconName {
    if open {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    }
}

/// A row's toggle: a button, a Tab stop, that opens and closes the row's
/// details and tells assistive technology whether they're open.
#[derive(IntoElement)]
struct Disclosure {
    id: ElementId,
    open: bool,
    /// What it's called, read by assistive technology.
    label: SharedString,
    content: AnyElement,
    full_width: bool,
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
        // A ghost button's hover.
        let hover = if theme.is_dark() {
            theme.accent.opacity(0.5)
        } else {
            theme.accent
        };
        let ring = theme.ring;
        // An extra-small button's size and padding, its border (the focus
        // ring) taken out of the padding.
        let padding = window.rem_size() * 0.25 - px(1.);
        div()
            .id(self.id)
            .role(Role::Button)
            .aria_expanded(self.open)
            .aria_label(self.label)
            .test_support()
            .track_focus(&focus)
            .when(self.full_width, |this| this.w_full())
            .min_w_0()
            .h_5()
            .flex()
            .items_center()
            .px(padding)
            .text_xs()
            .rounded(theme.radius)
            .border_1()
            .border_color(transparent_black())
            .hover(move |this| this.bg(hover))
            .focus_visible(move |this| this.border_color(ring))
            .child(self.content)
            // A click, or Enter or Space while it has the focus.
            .on_click(move |_, window, cx| (self.on_toggle)(window, cx))
    }
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
            .focus(|this| this.border_color(theme.ring))
            .tooltip(move |window, cx| Tooltip::new(caption.clone()).build(window, cx))
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
    use super::{
        CACHE_REFRESHED, OutputViewer, REGENERATE_HINT, apply_diff_patch, code_lines, duration,
        edit_diff, longest_code, preview, user_tags,
    };
    use arko::remote::{Delivery, Item, Kind, Source};
    use gpui_kit::AppContext as _;
    use std::sync::Arc;

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
        let (shown, total) = preview(&long);
        assert_eq!(total, 1);
        assert_eq!(shown.chars().count(), 501);
        let many: String = (0..100).map(|n| format!("line {n}\n")).collect();
        let (shown, total) = preview(&many);
        assert_eq!((shown.lines().count(), total), (40, 100));
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
