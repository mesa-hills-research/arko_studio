//! The transcript's rows from the keyboard: Up and Down move a focused row
//! (its hover actions show, and Tab reaches them), Enter opens or closes the
//! row (a call's details, a thinking, a group of calls, a long code block, a
//! worker's card), and Esc gives the keyboard back to the composer. Page
//! Up, Page Down, Home and End scroll the list as they did.

use arko::remote::Kind;
use gpui_kit::component::text::TextBlockContent;
use gpui_kit::*;

use super::model::RowKey;
use super::{AgentFilter, TranscriptEvent, TranscriptView};

actions!(
    transcript,
    [
        /// The row above.
        SelectPrevious,
        /// The row below.
        SelectNext,
        /// Opens or closes the focused row.
        ToggleRow,
        /// Back to the composer.
        Leave,
        /// The keyboard goes to the transcript's rows (or back to the
        /// composer).
        FocusTranscript,
    ]
);

/// The longest a row's label says of its text, in characters.
const LABEL_CHARS: usize = 120;

/// The first line of `text` with words in it, at most [`LABEL_CHARS`]
/// long.
fn first_line(text: &str) -> String {
    let line = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    match line.char_indices().nth(LABEL_CHARS) {
        Some((end, _)) => format!("{}…", &line[..end]),
        None => line.to_string(),
    }
}

impl TranscriptView {
    /// What assistive technology says of row `key`: whose it is or what,
    /// and the start of its text.
    pub(super) fn row_label(&self, key: RowKey) -> SharedString {
        if let RowKey::Working(_) = key {
            return "Working".into();
        }
        let Some(item) = self.model.entry(key.entry()).map(|entry| &entry.item) else {
            return SharedString::default();
        };
        let label = match key {
            RowKey::User(_) if item.pending => {
                format!("You, not read yet, {}", first_line(&item.text))
            }
            RowKey::User(_) => format!("You, {}", first_line(&item.text)),
            RowKey::Block { id, block, .. } => {
                let text = self
                    .ui
                    .get(&id)
                    .and_then(|ui| ui.doc.as_ref())
                    .and_then(|doc| doc.text.get(doc.blocks.get(block)?.range.clone()?))
                    .map(first_line)
                    .unwrap_or_default();
                match (item.kind, block) {
                    (Kind::Thinking, _) => format!("Thinking, {text}"),
                    (_, 0) => format!("Arko, {text}"),
                    _ => text,
                }
            }
            RowKey::Thinking(_) => "Thinking".to_string(),
            RowKey::Call(_) => match item.status {
                Some(arko::remote::CallStatus::Failed) => {
                    format!("{}, failed", first_line(&item.text))
                }
                _ => first_line(&item.text),
            },
            RowKey::Group(first) => {
                let calls = super::model::run_of(&self.model, first, self.settings.view).len();
                format!("{calls} calls")
            }
            RowKey::Count(_) => "Calls".to_string(),
            RowKey::Actions(_) => "Actions".to_string(),
            RowKey::Refresh(_) => "Kept the cache warm".to_string(),
            RowKey::Worker(_) => format!(
                "Worker {}, {}",
                item.worker.as_deref().unwrap_or_default(),
                first_line(&item.text)
            ),
            RowKey::Separator(_) | RowKey::Notice(_) | RowKey::Output(_) | RowKey::Turn(_) => {
                first_line(&item.text)
            }
            RowKey::Working(_) => "Working".to_string(),
        };
        label.into()
    }

    /// Whether assistive technology hears row `key` appear, as it hears
    /// what comes into a log. "Working…" is said once for a turn, when it
    /// follows what started it (the user's message or answer, or nothing
    /// yet), and not again each time the agent waits on the model after
    /// its own output, which would repeat after every call's result.
    pub(super) fn row_announced(&self, key: RowKey) -> bool {
        let RowKey::Working(after) = key else {
            return true;
        };
        after == 0
            || self.model.entry(after).is_some_and(|entry| {
                matches!(
                    entry.item.kind,
                    Kind::User | Kind::Question | Kind::Conversation | Kind::Turn
                )
            })
    }

    /// Whether row `key` is open, for a row that opens and closes.
    pub(super) fn row_expanded(&self, key: RowKey) -> Option<bool> {
        let expanded = |id: u64| self.ui.get(&id).and_then(|ui| ui.expanded);
        match key {
            RowKey::Call(id) => Some(expanded(id).unwrap_or(false)),
            RowKey::Thinking(id) => Some(expanded(id).unwrap_or(self.thinking_open_by_default())),
            RowKey::Group(first) => Some(self.group_open(first)),
            _ => None,
        }
    }

    /// The row the keyboard is on, by its index.
    pub(super) fn cursor_index(&self) -> Option<usize> {
        let cursor = self.cursor?;
        self.rows.iter().position(|row| row.key == cursor)
    }

    /// Whether the keyboard is on row `key`, the transcript having the
    /// focus.
    pub(super) fn is_cursor(&self, key: RowKey, window: &Window, cx: &App) -> bool {
        self.cursor == Some(key) && self.focus_handle.contains_focused(window, cx)
    }

    /// Puts the keyboard on row `ix`, scrolling it into view: its hover
    /// actions show.
    fn move_to(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.rows.get(ix) else {
            return;
        };
        let key = row.key;
        self.cursor = Some(key);
        // ("Working…" stands for no entry, so no actions show.)
        self.hovered = (!matches!(key, RowKey::Working(_))).then(|| key.entry());
        self.list.reveal(ix);
        // Arrows from a toggle or an action inside move the rows' focus.
        if !self.focus_handle.is_focused(window) {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
    }

    /// The keyboard comes to the transcript: on the row it was on, else the
    /// last row on screen.
    pub fn focus_rows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ix = self
            .cursor_index()
            .filter(|ix| self.list.shows_whole(*ix))
            .or_else(|| self.list.on_screen().last().copied())
            .or_else(|| self.rows.len().checked_sub(1));
        match ix {
            Some(ix) => self.move_to(ix, window, cx),
            None => self.focus_handle.focus(window, cx),
        }
    }

    /// Up: the row above the focused one, or, when that one is off screen,
    /// the last row on screen.
    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        let ix = match self.cursor_index() {
            Some(ix) if self.list.on_screen().contains(&ix) => ix.saturating_sub(1),
            _ => match self.list.on_screen().last() {
                Some(last) => *last,
                None => self.rows.len().saturating_sub(1),
            },
        };
        self.move_to(ix, window, cx);
    }

    /// Down: the row below the focused one, or, when that one is off
    /// screen, the first row on screen.
    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        let last = self.rows.len().saturating_sub(1);
        let ix = match self.cursor_index() {
            Some(ix) if self.list.on_screen().contains(&ix) => (ix + 1).min(last),
            _ => self.list.on_screen().first().copied().unwrap_or(0),
        };
        self.move_to(ix, window, cx);
    }

    /// Enter: opens or closes what the focused row opens. (A toggle or an
    /// action with the focus takes Enter itself, as a click.)
    fn toggle_row(&mut self, _: &ToggleRow, window: &mut Window, cx: &mut Context<Self>) {
        let Some(key) = self.cursor.filter(|_| self.focus_handle.is_focused(window)) else {
            cx.propagate();
            return;
        };
        match key {
            RowKey::Thinking(id) => {
                let default = self.thinking_open_by_default();
                self.toggle(id, default, cx);
            }
            RowKey::Call(id) | RowKey::Group(id) => self.toggle(id, false, cx),
            RowKey::Count(id) => self.open_run(id, cx),
            RowKey::Worker(id) => {
                if let Some(worker) = self
                    .model
                    .entry(id)
                    .and_then(|entry| entry.item.worker.clone())
                {
                    self.open_worker(worker.into(), cx);
                }
            }
            RowKey::Block { id, block, .. } => {
                let thinking = self
                    .model
                    .entry(id)
                    .is_some_and(|entry| entry.item.kind == Kind::Thinking);
                let code = self
                    .ui
                    .get(&id)
                    .and_then(|ui| ui.doc.as_ref())
                    .and_then(|doc| doc.blocks.get(block))
                    .is_some_and(|row| matches!(row.content, TextBlockContent::CodeBlock(_)));
                if thinking {
                    // The thought folds back into its row.
                    let default = self.thinking_open_by_default();
                    self.toggle(id, default, cx);
                    self.cursor = Some(RowKey::Thinking(id));
                } else if code {
                    self.toggle_code(id, block, cx);
                }
            }
            _ => {}
        }
    }

    /// Esc: the keyboard goes back to the composer (in a worker's view, to
    /// the list of workers).
    fn leave(&mut self, _: &Leave, _: &mut Window, cx: &mut Context<Self>) {
        // The actions the keyboard showed go, unless the pointer is there.
        if self.hovered_rows.is_empty() && self.hovered.take().is_some() {
            cx.notify();
        }
        if matches!(self.model.filter(), AgentFilter::Worker(_)) {
            cx.propagate();
            return;
        }
        cx.emit(TranscriptEvent::Leave);
    }

    /// Listens to the rows' keys.
    pub(super) fn on_row_keys<E: InteractiveElement>(this: E, cx: &mut Context<Self>) -> E {
        this.on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::toggle_row))
            .on_action(cx.listener(Self::leave))
    }
}

#[cfg(test)]
#[path = "nav_tests.rs"]
mod tests;
