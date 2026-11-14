//! A meter of how the transcript's lines move while an agent works, in the
//! window as the app draws it, with the session's column, the composer and
//! what opens above it, in the app's fonts. A scripted session takes every
//! live step of a turn many times, in the Full, Quiet and Messages views, at
//! several window sizes, with the reader following the end and with the
//! reader scrolled up. Between the turns' steps the approval tray and the
//! question chip open and close, the composer grows and shrinks, the command
//! menu opens and closes, and the window gets shorter and taller again.
//!
//! Every word the session says is a word no other line has (`w` and a
//! number), so each line of text on the screen is found again in the next
//! frame by its first such word, on the device pixel row its glyphs stand on
//! ([`gpui_kit::Window::painted_text`]). Following, a line may move up and
//! never down. Scrolled up, no line moves. Three moves are allowed. The
//! lines above a fold the reader toggles move by at most a line, as the
//! space around an open item comes or goes, and the clicked line stays.
//! "Working…" and the notes waiting at the end go down as what comes above
//! them takes their place, into the blank space below the end. A taller
//! view moves the lines down by what a shorter one moved them up. The
//! reader's own moves (a scroll, another view) aren't counted.
//!
//! `SHIFT_METER_SEEDS` plays that many sessions at more sizes, and
//! `SHIFT_METER_OUT` names a file for the tables of every step and view.

use std::cell::Cell;
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::rc::Rc;
use std::time::Duration;

use arko::remote::{Delivery, Feed, Level, Reading, Source};
use gpui_kit::test::TestWindowExt as _;
use gpui_kit::{
    AnyWindowHandle, AppContext as _, Bounds, ElementId, Entity, HeadlessAppContext,
    InputEvent as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    Point, ScrollDelta, ScrollWheelEvent, WindowBounds, WindowOptions, point, px, size,
};

use super::TranscriptView;
use super::height_tests::real;
use super::model::RowKey;
use super::tests::{call, result, say, think};
use crate::live::LiveSession;
use crate::prefs::{self, TranscriptView as View};
use crate::workspace::Workspace;

/// The views, in the order the tables list them.
const VIEWS: [View; 3] = [View::Full, View::Quiet, View::Messages];

fn view_name(view: usize) -> &'static str {
    ["full", "quiet", "messages"][view]
}

/// Numbers from a seed, so a session plays the same each time.
struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0 % n.max(1)
    }

    /// True `percent` times in a hundred.
    fn chance(&mut self, percent: u64) -> bool {
        self.below(100) < percent
    }
}

/// A step of a turn, or of what happens around it, which the frames it
/// makes are counted under.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Step {
    Asked,
    WorkingCall,
    WorkingThought,
    ThoughtGrows,
    ThoughtCall,
    Result,
    CallReply,
    ThoughtReply,
    ReplyGrows,
    ReplyWorking,
    WorkingReply,
    JobEnds,
    Notice,
    Waits,
    Read,
    FoldOpens,
    FoldCloses,
    TurnEnds,
    TrayOpens,
    TrayCloses,
    ChipOpens,
    ChipCloses,
    ComposerGrows,
    ComposerShrinks,
    MenuOpens,
    MenuCloses,
    Shorter,
    Taller,
    ViewAway,
    ViewBack,
}

impl Step {
    /// What makes the transcript's view shorter.
    fn takes(self) -> bool {
        matches!(
            self,
            Step::TrayOpens
                | Step::ChipOpens
                | Step::ComposerGrows
                | Step::MenuOpens
                | Step::Shorter
        )
    }

    /// What makes it taller again.
    fn gives_back(self) -> bool {
        matches!(
            self,
            Step::TrayCloses
                | Step::ChipCloses
                | Step::ComposerShrinks
                | Step::MenuCloses
                | Step::Taller
        )
    }

    const ALL: [Step; 30] = [
        Step::Asked,
        Step::WorkingCall,
        Step::WorkingThought,
        Step::ThoughtGrows,
        Step::ThoughtCall,
        Step::Result,
        Step::CallReply,
        Step::ThoughtReply,
        Step::ReplyGrows,
        Step::ReplyWorking,
        Step::WorkingReply,
        Step::JobEnds,
        Step::Notice,
        Step::Waits,
        Step::Read,
        Step::FoldOpens,
        Step::FoldCloses,
        Step::TurnEnds,
        Step::TrayOpens,
        Step::TrayCloses,
        Step::ChipOpens,
        Step::ChipCloses,
        Step::ComposerGrows,
        Step::ComposerShrinks,
        Step::MenuOpens,
        Step::MenuCloses,
        Step::Shorter,
        Step::Taller,
        Step::ViewAway,
        Step::ViewBack,
    ];

    fn name(self) -> &'static str {
        match self {
            Step::Asked => "message → Working…",
            Step::WorkingCall => "Working… → call",
            Step::WorkingThought => "Working… → thought",
            Step::ThoughtGrows => "thought streams",
            Step::ThoughtCall => "thought → call",
            Step::Result => "call → its result",
            Step::CallReply => "result → reply",
            Step::ThoughtReply => "thought → reply",
            Step::ReplyGrows => "reply streams",
            Step::ReplyWorking => "reply pauses → Working…",
            Step::WorkingReply => "Working… → reply goes on",
            Step::JobEnds => "job running → finished",
            Step::Notice => "notice",
            Step::Waits => "message waits",
            Step::Read => "waiting message read",
            Step::FoldOpens => "fold opens near the end",
            Step::FoldCloses => "fold closes near the end",
            Step::TurnEnds => "turn ends",
            Step::TrayOpens => "approval tray opens",
            Step::TrayCloses => "approval tray closes",
            Step::ChipOpens => "question chip shows",
            Step::ChipCloses => "question chip goes",
            Step::ComposerGrows => "composer grows",
            Step::ComposerShrinks => "composer shrinks",
            Step::MenuOpens => "command menu opens",
            Step::MenuCloses => "command menu closes",
            Step::Shorter => "window shorter",
            Step::Taller => "window taller again",
            Step::ViewAway => "another view",
            Step::ViewBack => "back to the view",
        }
    }
}

/// The frames of a step, how many moved a line, and by how many device
/// pixels at most and in all (each frame's largest move).
#[derive(Debug, Default, Clone, Copy)]
struct Tally {
    frames: u32,
    shifted: u32,
    max: u32,
    total: u32,
}

impl Tally {
    fn add(&mut self, moved: u32) {
        self.frames += 1;
        if moved > 0 {
            self.shifted += 1;
            self.max = self.max.max(moved);
            self.total += moved;
        }
    }

    fn sum(&mut self, other: &Tally) {
        self.frames += other.frames;
        self.shifted += other.shifted;
        self.max = self.max.max(other.max);
        self.total += other.total;
    }

    fn cell(&self) -> String {
        if self.frames == 0 {
            "–".into()
        } else if self.shifted == 0 {
            format!("{} · 0", self.frames)
        } else {
            format!(
                "{} · **{}** · {} / {}",
                self.frames, self.shifted, self.max, self.total
            )
        }
    }
}

/// Where the reader is, for the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Reader {
    Following,
    Up,
}

/// How a frame moved its lines, which sorts the moves by cause.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum How {
    /// The reader toggled a fold, and the clicked line moved, or a line
    /// above it moved by more than a line.
    Toggled,
    /// Following, every line on the screen moved down (the end came down
    /// to the bottom).
    AllDown,
    /// Following, lines below a change moved down, and the transcript was
    /// shorter than the screen, its start at the top.
    Short,
    /// Following, lines below a change moved down, with blank space below
    /// the end.
    Gap,
    /// Following, lines below a change moved down, with the end at the
    /// bottom.
    AtEnd,
    /// Scrolled up, a line moved.
    Up,
}

/// The line that moved most.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum What {
    /// A message waiting to be read, at the end.
    Note,
    /// The "Working…" line.
    Working,
    Other,
}

type Cause = (How, What);

/// A window's width and height, and its pixel ratio in hundredths.
type WindowSize = (u32, u32, u32);

/// A move the meter allows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum Allowed {
    /// The reader toggled a fold, and the lines above it moved by at most
    /// a line.
    Toggle,
    /// "Working…" or a note waiting at the end went down into the blank
    /// space below the end, as what came above them took their place.
    Room,
    /// A taller view gave back what a shorter one took.
    GiveBack,
}

/// What the sessions measured.
#[derive(Default)]
struct Meter {
    /// By step, view and where the reader was.
    tallies: BTreeMap<(Step, usize, Reader), Tally>,
    /// By the window's size and pixel ratio (in hundredths), view and where
    /// the reader was.
    sizes: BTreeMap<(WindowSize, usize, Reader), Tally>,
    /// The frames that moved a line, by cause, step and view.
    causes: BTreeMap<(Cause, Step, usize), Tally>,
    /// The moves the meter allows, by step and view.
    allowed: BTreeMap<(Allowed, Step, usize), Tally>,
    /// The largest move of each cause in each step and view, with the
    /// screens before and after.
    examples: BTreeMap<(Cause, Step, usize), (u32, String)>,
    /// Round trips (a panel, the composer, the menu, the window) that left
    /// a line off its row, by step and view, with the largest move.
    trips: BTreeMap<(Step, usize), Tally>,
    trip_examples: BTreeMap<(Step, usize), (u32, String)>,
    /// The frames whose lines moved sideways on their rows, by step and
    /// view, with the largest move.
    sideways: BTreeMap<(Step, usize), Tally>,
    side_examples: BTreeMap<(Step, usize), (u32, String)>,
}

impl Meter {
    fn shifted(&self) -> u32 {
        self.tallies.values().map(|tally| tally.shifted).sum()
    }

    fn frames(&self) -> u32 {
        self.tallies.values().map(|tally| tally.frames).sum()
    }

    fn trips_off(&self) -> u32 {
        self.trips.values().map(|tally| tally.shifted).sum()
    }

    /// A row for each step and a column for each view and where the reader
    /// was, each cell "frames · shifted · max / total px".
    fn table(&self) -> String {
        let mut out = String::new();
        let columns: Vec<String> = (0..VIEWS.len())
            .flat_map(|view| {
                [
                    format!("{}, following", view_name(view)),
                    format!("{}, scrolled up", view_name(view)),
                ]
            })
            .collect();
        let _ = writeln!(out, "| Step | {} |", columns.join(" | "));
        let _ = writeln!(out, "|---|{}", "---|".repeat(columns.len()));
        let mut all = vec![Tally::default(); columns.len()];
        for step in Step::ALL {
            let mut cells = Vec::new();
            for view in 0..VIEWS.len() {
                for (at, reader) in [Reader::Following, Reader::Up].into_iter().enumerate() {
                    let tally = self
                        .tallies
                        .get(&(step, view, reader))
                        .copied()
                        .unwrap_or_default();
                    all[view * 2 + at].sum(&tally);
                    cells.push(tally.cell());
                }
            }
            let _ = writeln!(out, "| {} | {} |", step.name(), cells.join(" | "));
        }
        let cells: Vec<String> = all.iter().map(Tally::cell).collect();
        let _ = writeln!(out, "| All | {} |", cells.join(" | "));
        out
    }

    fn size_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(
            out,
            "| Window | Pixel ratio | View | Following | Scrolled up |"
        );
        let _ = writeln!(out, "|---|---|---|---|---|");
        let mut sizes: Vec<WindowSize> = self.sizes.keys().map(|(size, ..)| *size).collect();
        sizes.dedup();
        for size in sizes {
            for view in 0..VIEWS.len() {
                let cell = |reader| {
                    self.sizes
                        .get(&(size, view, reader))
                        .copied()
                        .unwrap_or_default()
                        .cell()
                };
                let _ = writeln!(
                    out,
                    "| {}×{} | {} | {} | {} | {} |",
                    size.0,
                    size.1,
                    size.2 as f32 / 100.,
                    view_name(view),
                    cell(Reader::Following),
                    cell(Reader::Up)
                );
            }
        }
        out
    }

    fn cause_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "| Cause | Step | full | quiet | messages |");
        let _ = writeln!(out, "|---|---|---|---|---|");
        let mut keys: Vec<(Cause, Step)> = self.causes.keys().map(|(c, s, _)| (*c, *s)).collect();
        keys.dedup();
        for (cause, step) in keys {
            let cells: Vec<String> = (0..VIEWS.len())
                .map(|view| match self.causes.get(&(cause, step, view)) {
                    Some(tally) => format!("{} · {} / {}", tally.shifted, tally.max, tally.total),
                    None => "–".into(),
                })
                .collect();
            let _ = writeln!(
                out,
                "| {:?}, {:?} | {} | {} |",
                cause.0,
                cause.1,
                step.name(),
                cells.join(" | ")
            );
        }
        out
    }

    fn allowed_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "| Move | Step | full | quiet | messages |");
        let _ = writeln!(out, "|---|---|---|---|---|");
        let mut keys: Vec<(Allowed, Step)> = self
            .allowed
            .keys()
            .map(|(kind, step, _)| (*kind, *step))
            .collect();
        keys.dedup();
        for (kind, step) in keys {
            let cells: Vec<String> = (0..VIEWS.len())
                .map(|view| match self.allowed.get(&(kind, step, view)) {
                    Some(tally) => format!("{} · {} / {}", tally.frames, tally.max, tally.total),
                    None => "–".into(),
                })
                .collect();
            let kind = match kind {
                Allowed::Toggle => "the reader's toggle",
                Allowed::Room => "the end makes room",
                Allowed::GiveBack => "a taller view gives back",
            };
            let _ = writeln!(out, "| {kind} | {} | {} |", step.name(), cells.join(" | "));
        }
        out
    }

    fn trip_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "| Round trip | full | quiet | messages |");
        let _ = writeln!(out, "|---|---|---|---|");
        let mut steps: Vec<Step> = self.trips.keys().map(|(step, _)| *step).collect();
        steps.dedup();
        for step in steps {
            let cells: Vec<String> = (0..VIEWS.len())
                .map(|view| {
                    self.trips
                        .get(&(step, view))
                        .copied()
                        .unwrap_or_default()
                        .cell()
                })
                .collect();
            let _ = writeln!(out, "| {} | {} |", step.name(), cells.join(" | "));
        }
        out
    }

    fn side_table(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "| Step | full | quiet | messages |");
        let _ = writeln!(out, "|---|---|---|---|");
        let mut steps: Vec<Step> = self.sideways.keys().map(|(step, _)| *step).collect();
        steps.dedup();
        for step in steps {
            let cells: Vec<String> = (0..VIEWS.len())
                .map(|view| match self.sideways.get(&(step, view)) {
                    Some(tally) => format!("{} · {} / {}", tally.frames, tally.max, tally.total),
                    None => "–".into(),
                })
                .collect();
            let _ = writeln!(out, "| {} | {} |", step.name(), cells.join(" | "));
        }
        out
    }

    fn examples(&self) -> String {
        let mut out = String::new();
        for ((step, view), (_, example)) in &self.side_examples {
            let _ = writeln!(
                out,
                "### Sideways, {}, {}\n\n```text\n{example}```\n",
                step.name(),
                view_name(*view)
            );
        }
        for ((cause, step, view), (_, example)) in &self.examples {
            let _ = writeln!(
                out,
                "### {:?} {:?}, {}, {}\n\n```text\n{example}```\n",
                cause.0,
                cause.1,
                step.name(),
                view_name(*view)
            );
        }
        for ((step, view), (_, example)) in &self.trip_examples {
            let _ = writeln!(
                out,
                "### Round trip, {}, {}\n\n```text\n{example}```\n",
                step.name(),
                view_name(*view)
            );
        }
        out
    }
}

/// The words of `text` the session made, `w` and a number, past a word
/// cut short.
fn words(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = None;
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    for (at, &(index, c)) in chars.iter().enumerate() {
        let word = c.is_ascii_alphanumeric();
        match (start, word) {
            (None, true) => start = Some(index),
            (Some(from), false) => {
                if c != '…' {
                    out.push(&text[from..index]);
                }
                start = None;
            }
            _ => {}
        }
        if at + 1 == chars.len()
            && let Some(from) = start
        {
            out.push(&text[from..]);
        }
    }
    out.retain(|word| {
        word.len() > 1 && word.starts_with('w') && word[1..].bytes().all(|b| b.is_ascii_digit())
    });
    out
}

/// What "Working…" says.
const WORKING: &str = "Working…";

/// A line of text on the screen.
#[derive(Clone, Debug, PartialEq)]
struct Line {
    /// The device pixel row its glyphs stand on.
    row: i32,
    /// The device pixel column its first glyph starts at.
    x: i32,
    text: String,
}

/// Each line of `lines` by what tells it from every other line there, its
/// first word no other line has, or "Working…", with its index in `lines`.
/// Lines that share all their words, and lines of no word of the session,
/// are left out.
fn ids(lines: &[Line]) -> HashMap<String, usize> {
    let mut seen: HashMap<&str, usize> = HashMap::new();
    let all: Vec<Vec<&str>> = lines.iter().map(|line| words(&line.text)).collect();
    for words in &all {
        let mut mine = words.clone();
        mine.sort_unstable();
        mine.dedup();
        for word in mine {
            *seen.entry(word).or_default() += 1;
        }
    }
    let working = lines
        .iter()
        .filter(|line| line.text.trim() == WORKING)
        .count();
    let mut out = HashMap::new();
    for (at, (line, words)) in lines.iter().zip(&all).enumerate() {
        let id = match words.iter().find(|word| seen[*word] == 1) {
            Some(word) => format!("w:{word}"),
            None if line.text.trim() == WORKING && working == 1 => format!("t:{WORKING}"),
            None => continue,
        };
        out.insert(id, at);
    }
    out
}

/// How the lines of a frame moved from the frame before.
#[derive(Default)]
struct Moved {
    /// The most device pixels a line moved that no rule allows, down while
    /// following and at all while reading.
    px: u32,
    /// The line that moved most, from where to where.
    line: Option<(String, i32, i32)>,
    /// The highest line both frames show moved down too, so every line did.
    all: bool,
    /// The most a line below what changed moved, down into the blank below
    /// the end while following, either way while reading (rule 10).
    room: u32,
    /// The most any line moved up.
    up: u32,
    /// The most a line whose text stayed moved sideways, and which, from
    /// what column to what column.
    side: u32,
    side_line: Option<(String, i32, i32)>,
}

/// The device row in `before` from which the lines are below what changed
/// in `after`. That is the first line that went or says something else, or
/// the first line that came, at the line above it.
fn changed_from(before: &[Line], after: &[Line]) -> i32 {
    let (was, now) = (ids(before), ids(after));
    let id_of = |ids: &HashMap<String, usize>, at: usize| {
        ids.iter()
            .find(|(_, ix)| **ix == at)
            .map(|(id, _)| id.clone())
    };
    let mut from = i32::MAX;
    for (at, line) in before.iter().enumerate() {
        let Some(id) = id_of(&was, at) else {
            continue;
        };
        match now.get(&id) {
            Some(&to) if after[to].text == line.text => {}
            _ => {
                from = from.min(line.row);
                break;
            }
        }
    }
    let mut above = i32::MIN;
    for (at, _) in after.iter().enumerate() {
        let Some(id) = id_of(&now, at) else {
            continue;
        };
        match was.get(&id) {
            Some(&ix) => above = before[ix].row,
            None => {
                from = from.min(above.saturating_add(1));
                break;
            }
        }
    }
    from
}

/// How the lines of `before` that `after` shows again moved. Lines of
/// `before` below the row `below` are left out (the fold a click toggled
/// there moves them). `blank` is the blank space below the end before, and
/// from the rows `ends` on (before, after) the frames show "Working…" and
/// the notes waiting.
fn moves(
    before: &[Line],
    after: &[Line],
    following: bool,
    below: Option<i32>,
    blank: i32,
    ends: (i32, i32),
) -> Moved {
    let (was, now) = (ids(before), ids(after));
    let change = changed_from(before, after);
    let mut moved = Moved::default();
    let mut highest: Option<(i32, i32)> = None;
    for (id, &ix) in &was {
        let (from, x) = (before[ix].row, before[ix].x);
        if below.is_some_and(|row| from > row) {
            continue;
        }
        let Some(&to_ix) = now.get(id) else {
            continue;
        };
        let to = after[to_ix].row;
        if after[to_ix].text == before[ix].text
            && after[to_ix].row == from
            && after[to_ix].x.abs_diff(x) > moved.side
        {
            moved.side = after[to_ix].x.abs_diff(x);
            moved.side_line = Some((id.clone(), x, after[to_ix].x));
        }
        if to < from {
            moved.up = moved.up.max(from.abs_diff(to));
        }
        let px = if to > from || !following {
            to.abs_diff(from)
        } else {
            0
        };
        // (Output that grows on the screen pushes what is below it into the
        // blank, and while the reader reads, past the bottom edge. "Working…"
        // and the notes waiting ride on the end.)
        let rides = (to >= ends.1 && to > from) || (from >= ends.0 && !following);
        if px > 0 && (rides || (from >= change && (!following || px as i32 <= blank + 1))) {
            moved.room = moved.room.max(px);
            continue;
        }
        if highest.is_none_or(|(at, _)| from < at) {
            highest = Some((from, to));
        }
        if px > moved.px {
            moved.px = px;
            moved.line = Some((id.clone(), from, to));
        }
    }
    moved.all = highest.is_some_and(|(from, to)| to > from);
    moved
}

/// A frame's transcript as it was painted, and how the view stood.
#[derive(Clone)]
struct Shot {
    /// Each line of text in the transcript, top to bottom, its pieces on
    /// one row joined.
    lines: Vec<Line>,
    following: bool,
    /// The frame followed the end.
    followed: bool,
    /// Blank space below the end, in device pixels.
    blank: i32,
    /// The transcript's start shows at its top.
    start: bool,
    /// The device row "Working…" and the notes waiting start at, or the
    /// bottom of the screen.
    end: i32,
    /// The device rows the transcript shows, from and to.
    top: i32,
    bottom: i32,
}

impl Shot {
    fn render(&self, other: &Shot) -> String {
        let mut out = String::new();
        let rows: Vec<i32> = {
            let mut rows: Vec<i32> = self
                .lines
                .iter()
                .chain(&other.lines)
                .map(|line| line.row)
                .collect();
            rows.sort_unstable();
            rows.dedup();
            rows
        };
        let text = |shot: &Shot, y: i32| {
            shot.lines
                .iter()
                .find(|line| line.row == y)
                .map_or(String::new(), |line| line.text.chars().take(60).collect())
        };
        for y in rows {
            let _ = writeln!(out, "{y:>5} {:<60} │ {}", text(self, y), text(other, y));
        }
        out
    }
}

/// What is live at the end of the transcript, which names the next step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Live {
    Working,
    Thought,
    Reply,
    Result,
}

/// A session, played into the app's window a change at a time, each frame
/// drawn and measured.
struct Run<'m> {
    cx: HeadlessAppContext,
    window: AnyWindowHandle,
    workspace: Entity<Workspace>,
    session: Entity<LiveSession>,
    feed: Feed,
    clock: Rc<Cell<i64>>,
    rng: Rng,
    word: u64,
    view: usize,
    size: (f32, f32),
    /// The window's device pixels to a logical one.
    scale: f32,
    meter: &'m mut Meter,
    last: Option<Shot>,
    /// The reader keeps scrolled up.
    scrolled: bool,
    live: Live,
    /// The fold the reader opened, to close again.
    opened: Option<(RowKey, ElementId)>,
    /// A job that runs, by its id.
    job: Option<uuid::Uuid>,
    /// A message waiting to be read.
    waiting: Option<String>,
    notes: u64,
    /// What shorter views took since the reader last started following, in
    /// device pixels, which taller ones may give back.
    taken: i32,
}

/// An app in a headless window with the app's fonts, `size` logical pixels,
/// showing a session that follows `feed`.
fn open(
    size: (f32, f32),
    scale: f32,
    view: View,
    thinking: bool,
) -> (
    HeadlessAppContext,
    AnyWindowHandle,
    Entity<Workspace>,
    Entity<LiveSession>,
    Feed,
) {
    let mut cx = real::app();
    cx.update(|cx| {
        prefs::update(cx, |prefs| {
            prefs.view = view;
            prefs.appearance.expand_thinking = thinking;
        })
    });
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds {
            origin: point(px(0.), px(0.)),
            size: gpui_kit::size(px(size.0), px(size.1)),
        })),
        focus: false,
        show: false,
        ..Default::default()
    };
    let (window, workspace) = cx
        .update(|cx| crate::workspace::open_window(options, cx))
        .unwrap();
    cx.simulate_window_scale_factor_change(window, scale)
        .unwrap();
    cx.run_until_parked();
    let feed = Feed::new("main");
    let session = cx.update(|cx| cx.new(|cx| LiveSession::for_tests("s1", cx)));
    cx.update(|cx| crate::live::LiveSessions::add_for_tests(session.clone(), cx));
    cx.update(|cx| session.update(cx, |session, cx| session.follow_feed(feed.clone(), cx)));
    cx.update_window(window, |_, window, cx| {
        workspace.update(cx, |workspace, cx| {
            workspace.show_session(session.clone(), window, cx)
        })
    })
    .unwrap();
    real::settle(&mut cx, window);
    (cx, window, workspace, session, feed)
}

impl<'m> Run<'m> {
    fn new(
        seed: u64,
        view: usize,
        (size, scale): ((f32, f32), f32),
        scrolled: bool,
        thinking: bool,
        meter: &'m mut Meter,
    ) -> Self {
        let mut run = Self::bare(seed, view, (size, scale), scrolled, thinking, meter);
        // A history the turns come after, so the transcript fills the
        // screen in some sessions and not in others.
        let earlier = run.rng.below(4);
        run.history(earlier);
        run
    }

    /// A session's window with nothing in it yet.
    fn bare(
        seed: u64,
        view: usize,
        (size, scale): ((f32, f32), f32),
        scrolled: bool,
        thinking: bool,
        meter: &'m mut Meter,
    ) -> Self {
        let (cx, window, workspace, session, feed) = open(size, scale, VIEWS[view], thinking);
        Self {
            cx,
            window,
            workspace,
            session,
            feed,
            clock: Rc::new(Cell::new(1_000_000)),
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            word: 0,
            view,
            size,
            scale,
            meter,
            last: None,
            scrolled,
            live: Live::Working,
            opened: None,
            job: None,
            waiting: None,
            notes: 0,
            taken: 0,
        }
    }

    /// A session with `earlier` turns of history and nothing else.
    fn with_history(
        view: usize,
        (size, scale): ((f32, f32), f32),
        earlier: u64,
        meter: &'m mut Meter,
    ) -> Self {
        let mut run = Self::bare(1, view, (size, scale), false, false, meter);
        run.history(earlier);
        run
    }

    /// The session's history, `earlier` turns, drawn and settled.
    fn history(&mut self, earlier: u64) {
        let run = self;
        for _ in 0..earlier {
            let text = format!("Earlier {}", run.words(4));
            run.feed.user(&text, Source::App, Delivery::Turn);
            run.feed.turn_started();
            let reply = run.some(10, 30);
            run.feed.reply(&reply);
            run.feed.turn_ended("3 s · $0.01", None);
        }
        run.settle();
        let clock = run.clock.clone();
        run.with_transcript(|transcript, cx| {
            transcript.set_clock(move || clock.get(), cx);
        });
        run.settle();
        let shot = run.shot();
        run.last = Some(shot);
    }

    /// `n` words no other text has.
    fn words(&mut self, n: u64) -> String {
        let words: Vec<String> = (0..n.max(1))
            .map(|_| {
                self.word += 1;
                format!("w{}", self.word)
            })
            .collect();
        words.join(" ")
    }

    /// A name several lines may have, which the meter doesn't find a line
    /// by.
    fn name(&mut self, kind: char) -> String {
        self.word += 1;
        format!("{kind}{}", self.word)
    }

    fn some(&mut self, least: u64, more: u64) -> String {
        let n = least + self.rng.below(more);
        self.words(n)
    }

    fn transcript(&mut self) -> Option<Entity<TranscriptView>> {
        let workspace = self.workspace.clone();
        self.cx.update(|cx| {
            let view = workspace.read(cx).session_view().clone();
            view.read(cx).transcript(cx).cloned()
        })
    }

    fn with_transcript(
        &mut self,
        f: impl FnOnce(&mut TranscriptView, &mut gpui_kit::Context<TranscriptView>),
    ) {
        if let Some(transcript) = self.transcript() {
            self.cx.update(|cx| transcript.update(cx, f));
        }
    }

    /// Moves the clocks on by `ms`.
    fn wait(&mut self, ms: u64) {
        self.clock.set(self.clock.get() + ms as i64);
        self.cx.advance_clock(Duration::from_millis(ms));
    }

    /// Draws until nothing waits, without measuring.
    fn settle(&mut self) {
        real::settle(&mut self.cx, self.window);
    }

    /// Lets through what waits and draws a frame.
    fn draw(&mut self) {
        self.wait(20);
        self.cx.run_until_parked();
        real::frame(&mut self.cx, self.window);
    }

    /// The transcript as the frame just drawn painted it.
    fn shot(&mut self) -> Shot {
        let transcript = self.transcript();
        let window = self.window;
        // (Text that floats over the transcript isn't its.)
        let (lines, scale, over, rem) = self
            .cx
            .update_window(window, |_, window, _| {
                let over: Vec<Bounds<Pixels>> = ["answer-panel", "slash-menu"]
                    .into_iter()
                    .filter_map(|id| window.try_find(id).map(|found| found.bounds()))
                    .collect();
                (
                    window.painted_text(),
                    window.scale_factor(),
                    over,
                    window.rem_size(),
                )
            })
            .unwrap();
        let Some(transcript) = transcript else {
            return Shot {
                lines: Vec::new(),
                following: true,
                followed: true,
                blank: 0,
                start: true,
                end: i32::MAX,
                top: 0,
                bottom: 0,
            };
        };
        let device = |y: Pixels| (f32::from(y) * scale).round() as i32;
        self.cx.update(|cx| {
            let view = transcript.read(cx);
            let state = view.list.handle();
            let viewport = state.viewport_bounds();
            let (top, bottom) = (device(viewport.top()), device(viewport.bottom()));
            let mut pieces: Vec<(i32, i32, String)> = lines
                .into_iter()
                .filter_map(|line| {
                    let y = line.baseline.0 as i32;
                    let inside = line.bounds.left() >= viewport.left() - px(1.)
                        && line.mask.top() >= viewport.top() - px(1.)
                        && line.mask.bottom() <= viewport.bottom() + px(1.)
                        && !over.iter().any(|over| over.contains(&line.bounds.origin))
                        && y > top
                        && y <= bottom;
                    (inside && !line.text.trim().is_empty())
                        .then(|| (y, line.x.0 as i32, line.text.to_string()))
                })
                .collect();
            pieces.sort();
            // (Pieces of one line in other fonts stand a pixel or two apart.)
            let mut rows: Vec<Vec<(i32, i32, String)>> = Vec::new();
            for piece in pieces {
                match rows.last_mut() {
                    Some(row) if piece.0 - row[0].0 <= 3 => row.push(piece),
                    _ => rows.push(vec![piece]),
                }
            }
            let lines: Vec<Line> = rows
                .into_iter()
                .map(|mut row| {
                    row.sort_by_key(|(_, x, _)| *x);
                    let (y, x) = (row[0].0, row[0].1);
                    let text: Vec<String> = row.into_iter().map(|(_, _, text)| text).collect();
                    Line {
                        row: y,
                        x,
                        text: text.join(" "),
                    }
                })
                .collect();
            // (The list's bounds for a row leave out the space above the
            // first, and the space below the last is the transcript's own.)
            let last = view.rows.len().checked_sub(1);
            let end_bottom = last
                .and_then(|ix| state.bounds_for_item(ix))
                .map(|bounds| device(bounds.bottom() + rem * super::list::PADDING * 2.));
            let blank = end_bottom.map_or(0, |end| (bottom - end).max(0));
            let start = state.logical_scroll_top().item_ix == 0
                && f32::from(state.logical_scroll_top().offset_in_item) <= 0.;
            let end = if view.core_rows < view.rows.len() {
                state
                    .bounds_for_item(view.core_rows)
                    .map_or(i32::MAX, |bounds| device(bounds.top()))
            } else {
                i32::MAX
            };
            Shot {
                lines,
                following: view.is_following(cx),
                followed: view.list.followed(),
                blank,
                start,
                end,
                top,
                bottom,
            }
        })
    }

    /// Draws the frames a change makes, until they stop changing, and
    /// counts each under `step`. Lines below the row `below` of the frame
    /// before aren't counted in the first (a fold toggled on that row moves
    /// them), and no line above it may move by more than `toggle`.
    fn frames_below(&mut self, step: Step, below: Option<(i32, i32)>) {
        let mut below = below;
        for _ in 0..4 {
            self.draw();
            let shot = self.shot();
            let same = self
                .last
                .as_ref()
                .is_some_and(|last| last.lines == shot.lines);
            self.measure(step, &shot, below);
            below = None;
            self.last = Some(shot);
            if same {
                break;
            }
        }
    }

    fn frames(&mut self, step: Step) {
        self.frames_below(step, None);
    }

    /// Counts how the lines of `shot` moved from the last frame's.
    fn measure(&mut self, step: Step, shot: &Shot, below: Option<(i32, i32)>) {
        let Some(last) = &self.last else {
            return;
        };
        if std::env::var("SHIFT_METER_DEBUG").is_ok() {
            eprintln!(
                "{step:?} below {below:?} following {} lines {} top {} bottom {} end {} blank {}\n{}",
                shot.following,
                shot.lines.len(),
                shot.top,
                shot.bottom,
                shot.end,
                shot.blank,
                last.render(shot)
            );
        }
        let following = shot.following && last.following;
        let moved = moves(
            &last.lines,
            &shot.lines,
            following,
            below.map(|(row, _)| row),
            last.blank,
            (last.end, shot.end),
        );
        if moved.room > 0 {
            self.meter
                .allowed
                .entry((Allowed::Room, step, self.view))
                .or_default()
                .add(moved.room);
        }
        let mut px = moved.px;
        // The reader's own toggle keeps the clicked line still, and the lines
        // above it move by at most a line.
        if let Some((row, line)) = below {
            let clicked = moved.line.as_ref().is_some_and(|(_, from, _)| *from == row);
            let clicked_moved = {
                let (was, now) = (ids(&last.lines), ids(&shot.lines));
                was.iter()
                    .filter(|(_, ix)| last.lines[**ix].row == row)
                    .any(|(id, _)| now.get(id).is_some_and(|to| shot.lines[*to].row != row))
            };
            if !clicked && !clicked_moved && moved.px as i32 <= line {
                if moved.px > 0 {
                    self.meter
                        .allowed
                        .entry((Allowed::Toggle, step, self.view))
                        .or_default()
                        .add(moved.px);
                }
                px = 0;
            }
        }
        // A line whose text stays keeps its column.
        let side = moved.side;
        // A shorter view takes, and a taller one gives back what shorter ones
        // took, while following.
        if following && step.takes() {
            self.taken += moved.up as i32;
        }
        if step.gives_back() && following && px > 0 && px as i32 <= self.taken {
            self.meter
                .allowed
                .entry((Allowed::GiveBack, step, self.view))
                .or_default()
                .add(px);
            self.taken -= px as i32;
            px = 0;
        }
        if !last.following && shot.following {
            self.taken = 0;
        }
        // (Sideways moves are counted on their own.)
        if side > 0 {
            self.meter
                .sideways
                .entry((step, self.view))
                .or_default()
                .add(side);
            if let Some((id, from, to)) = moved.side_line.clone() {
                let key = (step, self.view);
                if self
                    .meter
                    .side_examples
                    .get(&key)
                    .is_none_or(|(most, _)| side > *most)
                {
                    let example = format!(
                        "{}×{} at {}: {id} went from column {from} to {to} (device px)\n{}",
                        self.size.0,
                        self.size.1,
                        self.scale,
                        last.render(shot)
                    );
                    self.meter.side_examples.insert(key, (side, example));
                }
            }
        }
        let reader = if following {
            Reader::Following
        } else {
            Reader::Up
        };
        let size = (
            self.size.0 as u32,
            self.size.1 as u32,
            (self.scale * 100.).round() as u32,
        );
        self.meter
            .tallies
            .entry((step, self.view, reader))
            .or_default()
            .add(px);
        self.meter
            .sizes
            .entry((size, self.view, reader))
            .or_default()
            .add(px);
        if let Some((id, from, to)) = moved.line.clone().filter(|_| px > 0) {
            let text = last
                .lines
                .iter()
                .find(|line| line.row == from)
                .map_or("", |line| line.text.as_str());
            let what = if text.trim() == WORKING {
                What::Working
            } else if text.starts_with("Also") {
                What::Note
            } else {
                What::Other
            };
            let how = match () {
                _ if below.is_some() => How::Toggled,
                _ if !following => How::Up,
                _ if moved.all => How::AllDown,
                _ if last.start && shot.start => How::Short,
                _ if last.blank > 0 => How::Gap,
                _ => How::AtEnd,
            };
            let cause = (how, what);
            let key = (cause, step, self.view);
            self.meter.causes.entry(key).or_default().add(px);
            if self
                .meter
                .examples
                .get(&key)
                .is_none_or(|(most, _)| px > *most)
            {
                let example = format!(
                    "{}×{} at {}: {id} went from {from} to {to} (device px), {} px blank below the end, then {}{}\n{}",
                    size.0,
                    size.1,
                    self.scale,
                    last.blank,
                    shot.blank,
                    if last.start {
                        ", the start at the top"
                    } else {
                        ""
                    },
                    last.render(shot)
                );
                self.meter.examples.insert(key, (px, example));
            }
        }
    }

    /// A frame the reader's own move made, which nothing is counted for.
    fn moved(&mut self) {
        for _ in 0..3 {
            self.draw();
        }
        let shot = self.shot();
        self.last = Some(shot);
    }

    /// The wheel over the transcript, up by `dy` when positive.
    fn wheel(&mut self, dy: f32) {
        let Some(last) = &self.last else {
            return;
        };
        let middle = (last.top + last.bottom) as f32 / 2. / self.scale;
        let position = point(px(self.size.0 * 0.6), px(middle));
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    ScrollWheelEvent {
                        position,
                        delta: ScrollDelta::Pixels(point(px(0.), px(dy))),
                        ..Default::default()
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
    }

    /// A reader who keeps scrolled up and follows again scrolls up a few
    /// lines.
    fn scroll_up(&mut self) {
        let following = self.last.as_ref().is_some_and(|last| last.following);
        if self.scrolled && following {
            let lines = 2 + self.rng.below(6);
            self.wheel(lines as f32 * 20.);
            self.moved();
            self.taken = 0;
        }
    }

    /// Now and then, what may come at any time (a notice, a job's end, a
    /// message the user sends meanwhile, the reader toggling a fold, a panel
    /// or the composer or the menu that opens and closes, or the window
    /// getting shorter and taller again). Returns whether a notice came, after
    /// which the agent's text goes on in an item of its own.
    fn meanwhile(&mut self) -> bool {
        let notice = self.rng.chance(6);
        if notice {
            let text = self.some(3, 12);
            let level = if self.rng.chance(50) {
                Level::Warn
            } else {
                Level::Info
            };
            self.feed.notice(level, &text);
            self.frames(Step::Notice);
        }
        if let Some(job) = self.job
            && self.rng.chance(5)
        {
            self.job = None;
            self.feed.job_ended(
                job,
                arko::remote::JobState::Finished,
                Some("finished".into()),
            );
            self.frames(Step::JobEnds);
        }
        if self.waiting.is_none() && self.rng.chance(2) {
            let text = format!("Also {}", self.some(2, 8));
            self.notes += 1;
            self.feed.note(&text, Source::App, self.notes, Vec::new());
            self.waiting = Some(text);
            self.frames(Step::Waits);
        }
        if self.rng.chance(8) {
            self.toggle();
        }
        if self.rng.chance(4) {
            self.round_trip();
        }
        notice
    }

    /// The agent reads the message waiting.
    fn read_waiting(&mut self) {
        if let Some(text) = self.waiting.take() {
            self.feed.notes_sent(&[text]);
            self.frames(Step::Read);
        }
    }

    /// One of the panels, the composer, the menu or the window's height
    /// changes, now and then with output meanwhile, and changes back. With
    /// nothing between, every line is back on its row.
    fn round_trip(&mut self) {
        let before = self.last.clone();
        let output = self.rng.chance(25);
        let kind = self.rng.below(6);
        let (open, close) = match kind {
            0 => (Step::TrayOpens, Step::TrayCloses),
            1 => (Step::ChipOpens, Step::ChipCloses),
            2 => (Step::ComposerGrows, Step::ComposerShrinks),
            3 => (Step::MenuOpens, Step::MenuCloses),
            4 => (Step::Shorter, Step::Taller),
            _ => (Step::ViewAway, Step::ViewBack),
        };
        self.change(kind, true);
        if open == Step::ViewAway {
            // (Another view is the reader's own move.)
            self.moved();
        } else {
            self.frames(open);
        }
        if output {
            let text = self.some(3, 10);
            self.feed.notice(Level::Info, &text);
            self.frames(Step::Notice);
        }
        self.change(kind, false);
        if close == Step::ViewBack {
            self.moved();
        } else {
            self.frames(close);
        }
        // (The chip comes with the agent's questions, which are output.
        // Another view closes a follower's blank, as End does, and one that
        // shows the end of a follower whose end a fold took off the screen
        // follows there, so the way back is at the end.)
        let closes = kind == 5
            && before
                .as_ref()
                .is_some_and(|b| b.following && (b.blank > 0 || !b.followed));
        if !output
            && kind != 1
            && !closes
            && let (Some(before), Some(after)) = (before, self.last.clone())
        {
            let (was, now) = (ids(&before.lines), ids(&after.lines));
            let off = was
                .iter()
                .filter_map(|(id, from)| {
                    let (from, to) = (&before.lines[*from], &after.lines[*now.get(id)?]);
                    Some(to.row.abs_diff(from.row).max(to.x.abs_diff(from.x)))
                })
                .max()
                .unwrap_or(0);
            self.meter
                .trips
                .entry((open, self.view))
                .or_default()
                .add(off);
            let key = (open, self.view);
            if off > 0
                && self
                    .meter
                    .trip_examples
                    .get(&key)
                    .is_none_or(|(most, _)| off > *most)
            {
                let example = format!(
                    "{}×{} at {}: a line ended {off} device px off its place\n{}",
                    self.size.0,
                    self.size.1,
                    self.scale,
                    before.render(&after)
                );
                self.meter.trip_examples.insert(key, (off, example));
            }
        }
    }

    /// Opens (`on`) or closes what `kind` names.
    fn change(&mut self, kind: u64, on: bool) {
        let window = self.window;
        let workspace = self.workspace.clone();
        let session = self.session.clone();
        match kind {
            0 => {
                let reading = Reading {
                    approvals: if on {
                        vec![crate::approvals_tests::profile("a1", "reviewer")]
                    } else {
                        Vec::new()
                    },
                    ..Reading::default()
                };
                self.cx.update(|cx| {
                    session.update(cx, |session, cx| {
                        session.set_status(reading, arko::remote::Activity::default(), cx)
                    })
                });
            }
            1 => {
                let questions = if on {
                    let text = format!("Which {}?", self.words(2));
                    crate::questions::ask(&self.feed, &[("q1", &text)], true);
                    self.feed.open_questions()
                } else {
                    self.feed.set_question_state(
                        "q1",
                        arko::questions::QuestionState::Answered {
                            answer: "Yes".into(),
                        },
                    );
                    Vec::new()
                };
                let reading = Reading {
                    questions,
                    ..Reading::default()
                };
                self.cx.update(|cx| {
                    session.update(cx, |session, cx| {
                        session.set_status(reading, arko::remote::Activity::default(), cx)
                    })
                });
            }
            2 | 3 => {
                let text = match (kind, on) {
                    (2, true) => "One\ntwo\nthree\nfour".to_string(),
                    (3, true) => "/".to_string(),
                    _ => String::new(),
                };
                self.cx
                    .update_window(window, |_, window, cx| {
                        let view = workspace.read(cx).session_view().clone();
                        let composer = view.read(cx).composer().clone();
                        composer.update(cx, |composer, cx| composer.set_text(&text, window, cx));
                    })
                    .unwrap();
            }
            5 => {
                let view = if on {
                    VIEWS[(self.view + 1 + self.rng.below(2) as usize) % VIEWS.len()]
                } else {
                    VIEWS[self.view]
                };
                self.cx
                    .update(|cx| prefs::update(cx, |prefs| prefs.view = view));
            }
            _ => {
                let height = if on { self.size.1 - 120. } else { self.size.1 };
                let width = self.size.0;
                self.cx
                    .update_window(window, |_, window, _| {
                        window.resize(size(px(width), px(height)));
                    })
                    .unwrap();
            }
        }
    }

    /// Clicks at `position`, pressed and let go.
    fn click(&mut self, position: Point<Pixels>) {
        self.cx
            .update_window(self.window, |_, window, cx| {
                window.dispatch_event(
                    MouseMoveEvent {
                        position,
                        pressed_button: None,
                        modifiers: Modifiers::default(),
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    MouseDownEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                        first_mouse: false,
                    }
                    .to_platform_input(),
                    cx,
                );
                window.dispatch_event(
                    MouseUpEvent {
                        button: MouseButton::Left,
                        position,
                        modifiers: Modifiers::default(),
                        click_count: 1,
                    }
                    .to_platform_input(),
                    cx,
                );
            })
            .unwrap();
        // (A click on an open thought closes it once it proves single.)
        self.wait(600);
    }

    /// The folds on the screen, bottom first, by their rows' keys and their
    /// toggles' ids.
    fn folds(&mut self) -> Vec<(RowKey, ElementId)> {
        let Some(transcript) = self.transcript() else {
            return Vec::new();
        };
        let keys: Vec<RowKey> = self.cx.update(|cx| {
            let view = transcript.read(cx);
            view.list
                .on_screen()
                .into_iter()
                .rev()
                .filter_map(|ix| view.rows.get(ix).map(|row| row.key))
                .collect()
        });
        keys.into_iter()
            .filter_map(|key| {
                let id = match key {
                    RowKey::Call(id) => ElementId::from(("call-toggle", id)),
                    RowKey::Thinking(id) => ElementId::from(("thinking-toggle", id)),
                    RowKey::Group(first) => ElementId::from(("group-toggle", first)),
                    RowKey::Count(first) => ElementId::from(("count-toggle", first)),
                    _ => return None,
                };
                Some((key, id))
            })
            .collect()
    }

    /// The height of the row `key` as last laid out, if it was.
    fn row_height(&mut self, key: RowKey) -> Option<f32> {
        let transcript = self.transcript()?;
        self.cx.update(|cx| {
            let view = transcript.read(cx);
            let ix = view.rows.iter().position(|row| row.key == key)?;
            view.list
                .handle()
                .bounds_for_item(ix)
                .map(|bounds| f32::from(bounds.size.height))
        })
    }

    /// Where to click the toggle `id`, on its first line, if it shows wholly
    /// inside the transcript.
    fn toggle_at(&mut self, id: &ElementId) -> Option<Point<Pixels>> {
        let last = self.last.clone()?;
        let id = id.clone();
        let bounds = self
            .cx
            .update_window(self.window, |_, window, _| {
                window
                    .try_find(id)
                    .filter(|found| found.visible())
                    .map(|found| found.bounds())
            })
            .ok()??;
        let (top, bottom) = (f32::from(bounds.top()), f32::from(bounds.bottom()));
        if top * self.scale < last.top as f32 || (top + 30.) * self.scale > last.bottom as f32 {
            return None;
        }
        let width = f32::from(bounds.size.width);
        Some(point(
            bounds.left() + px(24f32.min(width / 2.)),
            bounds.top() + px(12f32.min((bottom - top) / 2.).max(2.)),
        ))
    }

    /// Toggles the lowest fold among the screen's last rows, or toggles
    /// back the one the reader toggled.
    fn toggle(&mut self) {
        let candidates = match self.opened.take() {
            Some(fold) => vec![fold],
            None => self.folds().into_iter().take(6).collect(),
        };
        let Some((key, id, at)) = candidates
            .into_iter()
            .find_map(|(key, id)| self.toggle_at(&id).map(|at| (key, id, at)))
        else {
            return;
        };
        let Some(last) = self.last.clone() else {
            return;
        };
        // The line clicked, and how tall a line is.
        let y = (f32::from(at.y) * self.scale).round() as i32;
        let line = last
            .lines
            .windows(2)
            .map(|pair| pair[1].row - pair[0].row)
            .filter(|gap| *gap > 0)
            .min()
            .unwrap_or(24)
            .max((16. * self.scale) as i32)
            + 2;
        let Some(row) = last
            .lines
            .iter()
            .map(|line| line.row)
            .filter(|row| (row - y).abs() <= line)
            .min_by_key(|row| (row - y).abs())
        else {
            return;
        };
        let height = self.row_height(key);
        self.click(at);
        self.draw();
        let shot = self.shot();
        let opened = match (height, self.row_height(key)) {
            (Some(before), Some(after)) => after > before,
            _ => shot.lines.len() > last.lines.len(),
        };
        let step = if opened {
            Step::FoldOpens
        } else {
            Step::FoldCloses
        };
        if opened {
            self.opened = Some((key, id));
        }
        // The frames the toggle makes keep the clicked line on its row, and
        // the lines above it move by a line at most in all.
        let mut shot = shot;
        let mut row = Some((row, line));
        for _ in 0..4 {
            let same = self
                .last
                .as_ref()
                .is_some_and(|last| last.lines == shot.lines);
            self.measure(step, &shot, row);
            row = row.and_then(|(clicked, line)| {
                let was = ids(&self.last.as_ref()?.lines);
                let now = ids(&shot.lines);
                let last = &self.last.as_ref()?.lines;
                let id = was.iter().find(|(_, at)| last[**at].row == clicked)?.0;
                Some((shot.lines[*now.get(id)?].row, line))
            });
            self.last = Some(shot);
            if same {
                break;
            }
            self.draw();
            shot = self.shot();
        }
        self.scroll_up();
    }

    /// A thought streams, from what was live.
    fn think(&mut self) {
        if self.live == Live::Reply {
            return;
        }
        let text = self.some(2, 4);
        think(&self.feed, &text);
        self.live = Live::Thought;
        self.frames(Step::WorkingThought);
        for _ in 0..self.rng.below(5) {
            let n = 2 + self.rng.below(10);
            let more = match self.rng.below(6) {
                0 => format!("\n\n{}", self.words(n)),
                _ => format!(" {}", self.words(n)),
            };
            think(&self.feed, &more);
            self.frames(Step::ThoughtGrows);
            self.meanwhile();
        }
    }

    /// A call, which comes after what is live, its result, and Working…
    /// after it.
    fn call(&mut self) {
        let step = if self.live == Live::Thought {
            Step::ThoughtCall
        } else {
            Step::WorkingCall
        };
        let file = format!("src/{}/{}.rs", self.name('d'), self.words(1));
        let lines = 1 + self.rng.below(6);
        let (tool, parameters, output) = match self.rng.below(10) {
            0..=4 => {
                let text: Vec<String> = (0..lines)
                    .map(|n| format!("{} | {}", n + 1, self.some(2, 6)))
                    .collect();
                (
                    "read_file",
                    vec![("path", serde_json::Value::from(file.clone()))],
                    text.join("\n"),
                )
            }
            5..=6 => {
                let old = self.some(2, 3);
                let new = self.some(2, 3);
                (
                    "edit_file",
                    vec![
                        ("path", serde_json::Value::from(file.clone())),
                        ("old_string", old.clone().into()),
                        ("new_string", new.clone().into()),
                    ],
                    format!("Edited {file} at line 3:\n3 | {new}"),
                )
            }
            7..=8 => {
                let command = format!("cargo test {}", self.words(2));
                let text: Vec<String> = (0..lines)
                    .map(|_| format!("{} ok", self.some(2, 5)))
                    .collect();
                (
                    "shell",
                    vec![("command", serde_json::Value::from(command))],
                    format!("exit code 0, 1.0 s\n{}", text.join("\n")),
                )
            }
            _ => {
                let command = format!("npm run {}", self.words(2));
                (
                    "terminal_start",
                    vec![("command", serde_json::Value::from(command.clone()))],
                    "Started a job.".to_string(),
                )
            }
        };
        let id = call(&self.feed, "main", tool, &parameters);
        self.frames(step);
        self.meanwhile();
        result(&self.feed, "main", id, tool, &output);
        if tool == "terminal_start" && self.job.is_none() {
            let job = uuid::Uuid::new_v4();
            self.feed.job_started(
                arko::remote::JobStart::new(job, "main", arko::remote::JobKind::Command, "npm run")
                    .made_by(id),
            );
            self.job = Some(job);
        }
        self.live = Live::Result;
        self.frames(Step::Result);
        self.meanwhile();
    }

    /// The reply streams, from what was live, in pieces, and now and then
    /// a pause, after which Working… shows and a call may come.
    fn reply(&mut self) {
        let step = match self.live {
            Live::Result => Step::CallReply,
            Live::Thought => Step::ThoughtReply,
            _ => Step::WorkingReply,
        };
        let first = self.some(1, 4);
        say(&self.feed, "main", &first);
        self.live = Live::Reply;
        self.frames(step);
        let mut bold = false;
        for _ in 0..6 + self.rng.below(20) {
            let words = self.some(1, 6);
            let piece = match self.rng.below(100) {
                0..=9 => format!("\n\n{words}"),
                10..=19 if bold => {
                    bold = false;
                    format!(" {words}**")
                }
                10..=19 => {
                    bold = true;
                    format!(" **{words}")
                }
                20..=25 if !bold => format!("\n- {words}"),
                26..=28 if !bold => format!("\n\n## {words}\n\n"),
                29..=32 => format!(" `{words}`"),
                _ => format!(" {words}"),
            };
            say(&self.feed, "main", &piece);
            self.frames(Step::ReplyGrows);
            self.meanwhile();
            if self.rng.chance(8) {
                if bold {
                    say(&self.feed, "main", "**");
                    bold = false;
                }
                // It pauses, and Working… shows after a while.
                self.wait(2_500);
                self.live = Live::Working;
                self.frames(Step::ReplyWorking);
                if self.rng.chance(50) {
                    self.call();
                    let more = self.some(1, 4);
                    say(&self.feed, "main", &more);
                    self.live = Live::Reply;
                    self.frames(Step::CallReply);
                } else {
                    let more = format!(" {}", self.some(1, 4));
                    say(&self.feed, "main", &more);
                    self.live = Live::Reply;
                    self.frames(Step::WorkingReply);
                }
            }
        }
        if bold {
            say(&self.feed, "main", "**");
            self.frames(Step::ReplyGrows);
        }
    }

    /// A turn, from the user's message to its end.
    fn turn(&mut self) {
        self.scroll_up();
        let text = format!("Please {}", self.some(3, 12));
        self.feed.user(&text, Source::App, Delivery::Turn);
        self.feed.turn_started();
        self.live = Live::Working;
        self.frames(Step::Asked);
        self.meanwhile();
        for _ in 0..1 + self.rng.below(3) {
            if self.rng.chance(60) {
                self.think();
            }
            for _ in 0..self.rng.below(5) {
                if self.rng.chance(30) {
                    self.think();
                }
                self.call();
            }
            if self.rng.chance(30) {
                self.read_waiting();
            }
            self.reply();
        }
        self.read_waiting();
        let ended = format!("{} s · $0.01", self.words(1));
        self.feed.turn_ended(&ended, None);
        self.frames(Step::TurnEnds);
        if self.opened.is_some() {
            self.toggle();
        }
    }
}

/// The pixel ratios the meter plays at, by turns.
const SCALES: [f32; 4] = [1., 1.25, 1.5, 2.];

/// The window sizes the meter plays at, the first three in the test's own
/// run.
const SIZES: [(f32, f32); 6] = [
    (900., 640.),
    (1300., 900.),
    (700., 480.),
    (1100., 560.),
    (640., 820.),
    (1500., 1000.),
];

/// Plays sessions in every view, with the reader following and with the
/// reader scrolled up, the agent's thinking folded and shown whole:
/// `seeds` of them at every size, else one at each of the first three
/// sizes, its thinking folded or whole by turns.
fn measure(seeds: Option<u64>) -> Meter {
    let mut meter = Meter::default();
    let (sizes, turns) = match seeds {
        Some(_) => (&SIZES[..], 4),
        None => (&SIZES[..3], 2),
    };
    for seed in 0..seeds.unwrap_or(1) {
        for (at, size) in sizes.iter().enumerate() {
            for view in 0..VIEWS.len() {
                let whole = match seeds {
                    Some(_) => vec![false, true],
                    None => vec![(at + view) % 2 == 1],
                };
                for thinking in whole {
                    for scrolled in [false, true] {
                        let seed = 1
                            + seed * 1000
                            + at as u64 * 20
                            + view as u64 * 4
                            + u64::from(thinking) * 2
                            + u64::from(scrolled);
                        let scale = SCALES[(at + view + seed as usize) % SCALES.len()];
                        let mut run =
                            Run::new(seed, view, (*size, scale), scrolled, thinking, &mut meter);
                        for _ in 0..turns {
                            run.turn();
                        }
                    }
                }
            }
        }
    }
    meter
}

/// Following a session, no line moves down from one frame to the next,
/// and scrolled up none moves at all, through every live step of a turn
/// and every panel, menu and resize around it, in every view, but for the
/// moves the meter allows. What opens and closes again puts every line
/// back on its row.
#[path = "following_tests.rs"]
mod following;

#[test]
fn no_line_moves_down_as_an_agent_works() {
    let seeds = std::env::var("SHIFT_METER_SEEDS")
        .ok()
        .and_then(|seeds| seeds.parse().ok());
    let meter = measure(seeds);
    let table = format!(
        "{} frames, {} moved a line, and {} round trips left a line off its row. Each cell is frames · frames that moved a line · the most device px / all device px.\n\n{}\n{}\n{}\n{}",
        meter.frames(),
        meter.shifted(),
        meter.trips_off(),
        meter.table(),
        meter.size_table(),
        meter.cause_table(),
        meter.trip_table(),
    );
    let table = format!(
        "{table}\nLines that moved sideways on their rows, which the test reports and doesn't fail on: a thought shown whole stands a device pixel right of the lines beside it at a pixel ratio of 1.5, and the lines take each other's column as it ends. Each cell is frames · the most device px / all device px.\n\n{}",
        meter.side_table()
    );
    let table = format!(
        "{table}\nThe moves the test allows. The reader's toggle moves the lines above the fold by at most a line, as the space around an open item comes or goes. \"Working…\" and a note waiting at the end go down into the blank space below the end as what comes above them takes their place. A taller view gives back what a shorter one took. Each cell is frames · the most device px / all device px.\n\n{}",
        meter.allowed_table()
    );
    if let Ok(path) = std::env::var("SHIFT_METER_OUT") {
        std::fs::write(
            &path,
            format!(
                "{table}\n## The largest move of each cause, step and view\n\n{}",
                meter.examples()
            ),
        )
        .unwrap();
    }
    assert!(meter.frames() > 500, "{} frames", meter.frames());
    assert_eq!(
        meter.shifted() + meter.trips_off(),
        0,
        "\n{table}\n{}",
        meter.examples()
    );
}
