//! The sessions sidebar: every session in Arko's folder, chats first, then
//! one group per folder, most recently used first, with a filter, keyboard
//! navigation, inline rename and a context menu.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use gpui_kit::component::button::{Button, ButtonVariant, ButtonVariants as _};
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::menu::{ContextMenuExt as _, PopupMenuItem};
use gpui_kit::component::tooltip::{ManagedTooltipExt as _, Tooltip};
use gpui_kit::component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, WindowExt as _,
};
use gpui_kit::component::{h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::backend::{BackendError, Mode, SessionEntry, SessionId, SharedBackend};
use crate::layout::relative_time;
use crate::live::{LiveSessions, Phase};
use crate::menus::{DeleteSession, NewSession, RenameSession};
use crate::notifications::{self, NotificationHistory};
use crate::prefs;

/// The key contexts of the list, the filter and a rename in place.
pub const LIST_CONTEXT: &str = "SessionList";
pub const FILTER_CONTEXT: &str = "SessionFilter";
pub const RENAME_CONTEXT: &str = "SessionRename";

/// The group key of the chats.
pub const CHATS: &str = "chats";

actions!(
    session_list,
    [
        SelectPrevious,
        SelectNext,
        SelectFirst,
        SelectLast,
        Collapse,
        Expand,
        OpenSelected,
        FocusList,
        ClearFilter,
        CancelRename,
    ]
);

/// What the sidebar asks of the window.
#[derive(Debug, Clone, PartialEq)]
pub enum SidebarEvent {
    /// Start an agent session in this folder.
    NewSession(PathBuf),
    Open(SessionId),
    OpenConversation(SessionId, u32),
    OpenInNewWindow(SessionId),
    /// Branch the session from its latest message: a new session with its
    /// conversation, which the window opens.
    Branch(SessionId),
    /// A session was renamed; the window updates its title if it shows it.
    Renamed(SessionId, String),
    Deleted(SessionId),
    /// The sessions were listed again.
    Listed,
}

impl EventEmitter<SidebarEvent> for Sidebar {}

/// A row of the list.
#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    Group {
        key: String,
        title: String,
        /// The folder's full path, for the tooltip.
        path: Option<PathBuf>,
        collapsed: bool,
    },
    Session {
        id: SessionId,
        title: String,
        time: String,
        /// The session has several conversations, listed under it.
        conversations: bool,
        expanded: bool,
    },
    Conversation {
        session: SessionId,
        number: u32,
        title: String,
        current: bool,
    },
}

impl Row {
    /// What stays the same for this row when the list is built again.
    fn key(&self) -> RowKey {
        match self {
            Row::Group { key, .. } => RowKey::Group(key.clone()),
            Row::Session { id, .. } => RowKey::Session(id.clone()),
            Row::Conversation {
                session, number, ..
            } => RowKey::Conversation(session.clone(), *number),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum RowKey {
    Group(String),
    Session(SessionId),
    Conversation(SessionId, u32),
}

/// The group a session belongs to: the chats, or its folder.
pub fn group_key(entry: &SessionEntry) -> String {
    match (&entry.mode, &entry.workspace) {
        (Mode::Agent, Some(folder)) => folder.display().to_string(),
        _ => CHATS.to_string(),
    }
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// The rows to show: the chats first, then each folder, most recently used
/// first, with each group's sessions newest first. A filter keeps the
/// sessions whose title or folder contains it, in open groups.
pub fn rows(
    sessions: &[SessionEntry],
    collapsed: &[String],
    expanded: &HashSet<SessionId>,
    filter: &str,
    now: SystemTime,
) -> Vec<Row> {
    let filter = filter.trim().to_lowercase();
    let mut groups: Vec<(String, Vec<&SessionEntry>)> = Vec::new();
    for entry in sessions {
        let key = group_key(entry);
        match groups.iter_mut().find(|(group, _)| *group == key) {
            Some((_, entries)) => entries.push(entry),
            None => groups.push((key, vec![entry])),
        }
    }
    for (_, entries) in &mut groups {
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.activity()));
    }
    groups.sort_by(|(a_key, a), (b_key, b)| {
        (b_key == CHATS)
            .cmp(&(a_key == CHATS))
            .then_with(|| b[0].activity().cmp(&a[0].activity()))
    });

    let mut rows = Vec::new();
    for (key, entries) in groups {
        let path = entries[0].workspace.clone().filter(|_| key != CHATS);
        let title = match &path {
            Some(path) => folder_name(path),
            None => "Chats".to_string(),
        };
        let group_matches = !filter.is_empty()
            && path
                .as_ref()
                .is_some_and(|path| path.display().to_string().to_lowercase().contains(&filter));
        let shown: Vec<&SessionEntry> = entries
            .into_iter()
            .filter(|entry| {
                filter.is_empty()
                    || group_matches
                    || entry.title.to_lowercase().contains(&filter)
                    || entry
                        .conversations
                        .iter()
                        .any(|conversation| conversation.title.to_lowercase().contains(&filter))
            })
            .collect();
        if shown.is_empty() {
            continue;
        }
        let is_collapsed = filter.is_empty() && collapsed.contains(&key);
        rows.push(Row::Group {
            key,
            title,
            path,
            collapsed: is_collapsed,
        });
        if is_collapsed {
            continue;
        }
        for entry in shown {
            let has_conversations = entry.conversations.len() > 1;
            let is_expanded = has_conversations && expanded.contains(&entry.id);
            rows.push(Row::Session {
                id: entry.id.clone(),
                title: entry.title.clone(),
                time: relative_time(entry.activity(), now),
                conversations: has_conversations,
                expanded: is_expanded,
            });
            if is_expanded {
                rows.extend(
                    entry
                        .conversations
                        .iter()
                        .map(|conversation| Row::Conversation {
                            session: entry.id.clone(),
                            number: conversation.number,
                            title: conversation.title.clone(),
                            current: conversation.current,
                        }),
                );
            }
        }
    }
    rows
}

/// The sessions sidebar of one window.
pub struct Sidebar {
    focus_handle: FocusHandle,
    filter: Entity<InputState>,
    sessions: Vec<SessionEntry>,
    rows: Vec<Row>,
    /// The row the keyboard is on.
    cursor: Option<RowKey>,
    /// The session open in this window.
    active: Option<SessionId>,
    /// Sessions whose conversations are listed.
    expanded: HashSet<SessionId>,
    renaming: Option<(SessionId, Entity<InputState>)>,
    /// The closed groups the rows were built with.
    collapsed: Vec<String>,
    scroll: UniformListScrollHandle,
    loaded: bool,
    /// The sessions running in this app when the list was last read: one
    /// more means a new session to list.
    live_ids: Vec<SessionId>,
    live_titles: std::collections::HashMap<SessionId, SharedString>,
    _subscriptions: Vec<Subscription>,
    _refresh: Option<Task<()>>,
}

impl Sidebar {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Filter sessions")
                .clean_on_escape()
        });
        let subscriptions = vec![
            cx.subscribe_in(&filter, window, Self::on_filter_event),
            // Other preferences change often (the window's place while it
            // moves), so only a change to the closed groups lists again.
            cx.observe_global::<prefs::PrefsStore>(|this, cx| {
                if prefs::get(cx).sidebar.collapsed != this.collapsed {
                    this.rebuild(cx)
                }
            }),
            // The running sessions' indicators. A session created here gets
            // its row.
            // A name an engine gives a session that is open (a model's, a
            // while after the first message) is listed again, whichever
            // session the window shows.
            cx.observe_global::<LiveSessions>(|this, cx| {
                let ids = LiveSessions::ids(cx);
                let titles = LiveSessions::titles(cx);
                let renamed = titles
                    .iter()
                    .any(|(id, title)| this.live_titles.get(id).is_some_and(|was| was != title));
                if renamed || ids.iter().any(|id| !this.live_ids.contains(id)) {
                    this.refresh(cx);
                }
                this.live_ids = ids;
                this.live_titles = titles;
                cx.notify();
            }),
            // Which sessions have news.
            cx.observe_global::<NotificationHistory>(|_, cx| cx.notify()),
        ];
        let mut this = Self {
            focus_handle: cx.focus_handle(),
            filter,
            sessions: Vec::new(),
            rows: Vec::new(),
            cursor: None,
            active: None,
            expanded: HashSet::new(),
            renaming: None,
            collapsed: Vec::new(),
            scroll: UniformListScrollHandle::new(),
            loaded: false,
            live_ids: Vec::new(),
            live_titles: Default::default(),
            _subscriptions: subscriptions,
            _refresh: None,
        };
        this.refresh(cx);
        this
    }

    /// Lists the sessions again, off the UI thread.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let backend = cx.global::<SharedBackend>().clone();
        let listing = cx.background_spawn(async move { backend.sessions() });
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let sessions = listing.await;
            this.update(cx, |this, cx| match sessions {
                Ok(sessions) => this.set_sessions(sessions, cx),
                Err(error) => tracing::error!("couldn't list the sessions ({error:?})"),
            })
            .ok();
        }));
    }

    /// Shows `sessions`, keeping the cursor on the row it was on.
    pub fn set_sessions(&mut self, sessions: Vec<SessionEntry>, cx: &mut Context<Self>) {
        self.sessions = sessions;
        self.loaded = true;
        self.rebuild(cx);
        cx.emit(SidebarEvent::Listed);
    }

    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// Whether a session of the group `key` that this window doesn't show
    /// has news.
    fn group_has_news(&self, key: &str, cx: &App) -> bool {
        self.sessions
            .iter()
            .filter(|entry| group_key(entry) == key)
            .any(|entry| {
                self.active.as_ref() != Some(&entry.id) && notifications::has_news(&entry.id, cx)
            })
    }

    /// Marks the session open in this window.
    pub fn set_active(&mut self, id: Option<SessionId>, cx: &mut Context<Self>) {
        self.active = id;
        cx.notify();
    }

    pub fn focus_filter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.filter.update(cx, |filter, cx| {
            filter.focus(window, cx);
            filter.select_all(window, cx);
        });
    }

    pub fn focus_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cursor.is_none() {
            self.cursor = self
                .active
                .clone()
                .map(RowKey::Session)
                .filter(|key| self.index_of(key).is_some())
                .or_else(|| self.first_session_key());
        }
        self.focus_handle.focus(window, cx);
        self.reveal_cursor();
        cx.notify();
    }

    fn rebuild(&mut self, cx: &mut Context<Self>) {
        let filter = self.filter.read(cx).value().to_string();
        self.collapsed = prefs::get(cx).sidebar.collapsed.clone();
        self.rows = rows(
            &self.sessions,
            &self.collapsed,
            &self.expanded,
            &filter,
            SystemTime::now(),
        );
        if let Some(cursor) = &self.cursor
            && self.index_of(cursor).is_none()
        {
            self.cursor = self.first_session_key();
        }
        cx.notify();
    }

    fn first_session_key(&self) -> Option<RowKey> {
        self.rows
            .iter()
            .find(|row| matches!(row, Row::Session { .. }))
            .or(self.rows.first())
            .map(Row::key)
    }

    fn index_of(&self, key: &RowKey) -> Option<usize> {
        self.rows.iter().position(|row| &row.key() == key)
    }

    fn cursor_index(&self) -> Option<usize> {
        self.cursor.as_ref().and_then(|key| self.index_of(key))
    }

    fn reveal_cursor(&self) {
        if let Some(index) = self.cursor_index() {
            self.scroll.scroll_to_item(index, ScrollStrategy::Nearest);
        }
    }

    fn move_cursor(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(row) = self.rows.get(index) {
            self.cursor = Some(row.key());
            self.reveal_cursor();
            cx.notify();
        }
    }

    fn on_filter_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                self.cursor = None;
                self.rebuild(cx);
                self.cursor = self.first_session_key();
            }
            InputEvent::PressEnter { .. } => {
                if self.cursor.is_none() {
                    self.cursor = self.first_session_key();
                }
                self.open_cursor(window, cx);
            }
            _ => {}
        }
    }

    fn select_previous(&mut self, _: &SelectPrevious, _: &mut Window, cx: &mut Context<Self>) {
        let index = match self.cursor_index() {
            Some(0) | None => self.rows.len().saturating_sub(1),
            Some(index) => index - 1,
        };
        self.move_cursor(index, cx);
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        let index = match self.cursor_index() {
            Some(index) if index + 1 < self.rows.len() => index + 1,
            Some(_) => 0,
            None => 0,
        };
        self.move_cursor(index, cx);
    }

    fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(0, cx);
    }

    fn select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        self.move_cursor(self.rows.len().saturating_sub(1), cx);
    }

    /// ←: closes a group or a session's conversations, or goes up to the
    /// row they're in.
    fn collapse(&mut self, _: &Collapse, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor_index() else {
            return;
        };
        match self.rows[index].clone() {
            Row::Group {
                key,
                collapsed: false,
                ..
            } => self.set_group_collapsed(&key, true, cx),
            Row::Session {
                id, expanded: true, ..
            } => {
                self.expanded.remove(&id);
                self.rebuild(cx);
            }
            Row::Session { .. } => {
                if let Some(group) = self.rows[..index]
                    .iter()
                    .rposition(|row| matches!(row, Row::Group { .. }))
                {
                    self.move_cursor(group, cx);
                }
            }
            Row::Conversation { session, .. } => {
                if let Some(parent) = self.index_of(&RowKey::Session(session)) {
                    self.move_cursor(parent, cx);
                }
            }
            Row::Group { .. } => {}
        }
    }

    /// →: opens a group or a session's conversations.
    fn expand(&mut self, _: &Expand, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor_index() else {
            return;
        };
        match self.rows[index].clone() {
            Row::Group {
                key,
                collapsed: true,
                ..
            } => self.set_group_collapsed(&key, false, cx),
            Row::Session {
                id,
                conversations: true,
                expanded: false,
                ..
            } => {
                self.expanded.insert(id);
                self.rebuild(cx);
            }
            _ => {}
        }
    }

    fn open_selected(&mut self, _: &OpenSelected, window: &mut Window, cx: &mut Context<Self>) {
        self.open_cursor(window, cx);
    }

    fn open_cursor(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.cursor_index() else {
            return;
        };
        self.open_row(index, cx);
    }

    fn open_row(&mut self, index: usize, cx: &mut Context<Self>) {
        match self.rows[index].clone() {
            Row::Group { key, collapsed, .. } => self.set_group_collapsed(&key, !collapsed, cx),
            Row::Session { id, .. } => cx.emit(SidebarEvent::Open(id)),
            Row::Conversation {
                session, number, ..
            } => cx.emit(SidebarEvent::OpenConversation(session, number)),
        }
    }

    fn set_group_collapsed(&mut self, key: &str, collapsed: bool, cx: &mut Context<Self>) {
        let key = key.to_string();
        prefs::update(cx, |prefs| {
            let groups = &mut prefs.sidebar.collapsed;
            groups.retain(|group| *group != key);
            if collapsed {
                groups.push(key.clone());
            }
        });
        // The preferences' observer rebuilds the rows.
    }

    fn toggle_conversations(&mut self, id: &SessionId, cx: &mut Context<Self>) {
        if !self.expanded.remove(id) {
            self.expanded.insert(id.clone());
        }
        self.rebuild(cx);
    }

    fn focus_list_action(&mut self, _: &FocusList, window: &mut Window, cx: &mut Context<Self>) {
        self.focus_list(window, cx);
    }

    fn clear_filter(&mut self, _: &ClearFilter, window: &mut Window, cx: &mut Context<Self>) {
        if self.filter.read(cx).value().is_empty() {
            cx.propagate();
            return;
        }
        self.filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.rebuild(cx);
    }

    /// The session the keyboard is on, or else the one open here.
    fn target(&self) -> Option<SessionId> {
        match self.cursor.as_ref()? {
            RowKey::Session(id) | RowKey::Conversation(id, _) => Some(id.clone()),
            RowKey::Group(_) => None,
        }
    }

    fn entry(&self, id: &SessionId) -> Option<&SessionEntry> {
        self.sessions.iter().find(|entry| &entry.id == id)
    }

    fn rename_action(&mut self, _: &RenameSession, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.target() {
            self.start_rename(id, window, cx);
        }
    }

    /// Turns the session's row into a field holding its name.
    pub fn start_rename(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(title) = self.entry(&id).map(|entry| entry.title.clone()) else {
            return;
        };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(title));
        input.update(cx, |input, cx| {
            input.focus(window, cx);
            input.select_all(window, cx);
        });
        let subscription = cx.subscribe_in(&input, window, {
            let id = id.clone();
            move |this, input, event: &InputEvent, window, cx| match event {
                InputEvent::PressEnter { .. } => {
                    let title = input.read(cx).value().to_string();
                    this.finish_rename(&id, Some(title), window, cx);
                }
                InputEvent::Blur => {
                    let title = input.read(cx).value().to_string();
                    this.finish_rename(&id, Some(title), window, cx);
                }
                _ => {}
            }
        });
        self._subscriptions.push(subscription);
        self.cursor = Some(RowKey::Session(id.clone()));
        if let Some(entry) = self.entry(&id).cloned() {
            // A session in a closed group is shown to be renamed.
            let key = group_key(&entry);
            if prefs::get(cx).sidebar.collapsed.contains(&key) {
                self.set_group_collapsed(&key, false, cx);
            }
        }
        self.renaming = Some((id, input));
        self.reveal_cursor();
        cx.notify();
    }

    fn cancel_rename(&mut self, _: &CancelRename, window: &mut Window, cx: &mut Context<Self>) {
        if let Some((id, _)) = self.renaming.clone() {
            self.finish_rename(&id, None, window, cx);
        }
    }

    /// Ends renaming `id`: saves `title` unless it's `None` or unchanged.
    fn finish_rename(
        &mut self,
        id: &SessionId,
        title: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .renaming
            .as_ref()
            .is_none_or(|(renaming, _)| renaming != id)
        {
            return;
        }
        self.renaming = None;
        // The keyboard goes back to the list, unless a click took it
        // elsewhere, such as to the composer.
        if self.focus_handle.contains_focused(window, cx) {
            self.focus_handle.focus(window, cx);
        }
        cx.notify();
        let Some(title) = title.map(|title| title.trim().to_string()) else {
            return;
        };
        if title.is_empty() || self.entry(id).is_some_and(|entry| entry.title == title) {
            return;
        }
        // Shown at once. The listing that follows the save confirms it.
        if let Some(entry) = self.sessions.iter_mut().find(|entry| &entry.id == id) {
            entry.title = title.clone();
        }
        self.rebuild(cx);
        // A session running here is renamed by its engine, which tells its
        // status too.
        if let Some(session) = LiveSessions::find(id, cx)
            .filter(|session| session.read(cx).link().is_some() && session.read(cx).is_running())
        {
            session.update(cx, |session, cx| session.rename(title.clone(), cx));
            cx.emit(SidebarEvent::Renamed(id.clone(), title));
            return;
        }
        let backend = cx.global::<SharedBackend>().clone();
        let id = id.clone();
        let saving = cx.background_spawn({
            let id = id.clone();
            let title = title.clone();
            async move { backend.rename(&id, &title) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = saving.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => cx.emit(SidebarEvent::Renamed(id, title)),
                    Err(error) => show_error("Couldn’t rename the session.", &error, window, cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    fn delete_action(&mut self, _: &DeleteSession, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.target() {
            self.confirm_delete(id, window, cx);
        }
    }

    /// Asks before deleting a session's saved data, naming what stays: the
    /// folder's files, and its workers' worktrees with changes.
    pub fn confirm_delete(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entry) = self.entry(&id).cloned() else {
            return;
        };
        // Git is asked about the worktrees off the UI thread.
        let backend = cx.global::<SharedBackend>().clone();
        let planning = cx.background_spawn({
            let id = id.clone();
            async move { backend.delete_plan(&id) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let plan = planning.await.unwrap_or_default();
            this.update_in(cx, |this, window, cx| {
                this.ask_to_delete(entry, plan, window, cx)
            })
            .ok();
        })
        .detach();
    }

    fn ask_to_delete(
        &mut self,
        entry: SessionEntry,
        plan: crate::backend::DeletePlan,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut description = match &entry.workspace {
            Some(folder) if entry.mode == Mode::Agent => format!(
                "Its conversation and settings are removed from Arko’s folder. The files in {} stay as they are.",
                folder.display()
            ),
            _ => "Its conversation and settings are removed from Arko’s folder.".to_string(),
        };
        if !plan.kept_worktrees.is_empty() {
            let kept: Vec<String> = plan
                .kept_worktrees
                .iter()
                .map(|(path, branch)| match branch {
                    Some(branch) => format!("{} ({branch})", path.display()),
                    None => path.display().to_string(),
                })
                .collect();
            description.push_str(&format!(
                " Workers’ worktrees with uncommitted changes stay ({}).",
                kept.join(", ")
            ));
        }
        if LiveSessions::find(&entry.id, cx).is_some_and(|session| session.read(cx).is_running()) {
            description.push_str(" It’s open here, so it closes first.");
        }
        let sidebar = cx.entity().downgrade();
        let id = entry.id.clone();
        window.open_alert_dialog(cx, move |alert, _, _| {
            let sidebar = sidebar.clone();
            let id = id.clone();
            alert
                .title(format!("Delete “{}”?", entry.title))
                .description(description.clone())
                .confirm()
                .ok_text("Delete")
                .ok_variant(ButtonVariant::Danger)
                .on_ok(move |_, window, cx| {
                    // After this dialog closes: closing the session may ask
                    // about its work in a dialog of its own.
                    let (sidebar, id) = (sidebar.clone(), id.clone());
                    window.defer(cx, move |window, cx| {
                        sidebar
                            .update(cx, |sidebar, cx| sidebar.delete(id, window, cx))
                            .ok();
                    });
                    true
                })
        });
    }

    fn delete(&mut self, id: SessionId, window: &mut Window, cx: &mut Context<Self>) {
        // A session running here closes first (asking about what runs), and
        // is deleted once it has.
        if let Some(session) =
            LiveSessions::find(&id, cx).filter(|session| session.read(cx).is_running())
        {
            let sidebar = cx.entity().downgrade();
            crate::quit::close_then(vec![session], window, cx, move |window, cx| {
                sidebar
                    .update(cx, |sidebar, cx| sidebar.delete(id, window, cx))
                    .ok();
            });
            return;
        }
        let backend = cx.global::<SharedBackend>().clone();
        let deleting = cx.background_spawn({
            let id = id.clone();
            async move { backend.delete(&id) }
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = deleting.await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok(()) => cx.emit(SidebarEvent::Deleted(id)),
                    Err(error) => show_error("Couldn’t delete the session.", &error, window, cx),
                }
                this.refresh(cx);
            })
            .ok();
        })
        .detach();
    }

    /// The row that shows the keyboard cursor's ring. It shows while the list
    /// has the keyboard, and not while a row's name is being edited, whose
    /// field shows the focus then.
    fn cursor_ring(&self, window: &Window, cx: &App) -> Option<&RowKey> {
        let focused = self.focus_handle.contains_focused(window, cx);
        self.cursor
            .as_ref()
            .filter(|_| focused && self.renaming.is_none())
    }

    fn render_row(&self, index: usize, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[index];
        let theme = cx.theme();
        let is_cursor = self.cursor_ring(window, cx) == Some(&row.key());
        let is_active = match row {
            Row::Session { id, .. } => self.active.as_ref() == Some(id),
            _ => false,
        };
        let renaming = match row {
            Row::Session { id, .. } => self
                .renaming
                .as_ref()
                .filter(|(renaming, _)| renaming == id)
                .map(|(_, input)| input.clone()),
            _ => None,
        };

        let base = h_flex()
            .id(("row", index))
            .test_support()
            .relative()
            .h_7()
            .w_full()
            .px_2()
            .gap_1p5()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.transparent)
            .text_sm()
            .when(is_cursor, |this| this.border_color(theme.ring))
            .when(is_active, |this| {
                this.font_medium()
                    .bg(theme.tokens.sidebar_accent)
                    .text_color(theme.sidebar_accent_foreground)
            })
            .when(!is_active && renaming.is_none(), |this| {
                this.hover(|this| {
                    this.bg(theme.sidebar_accent.opacity(0.8))
                        .text_color(theme.sidebar_accent_foreground)
                })
            });

        match row.clone() {
            Row::Group {
                key,
                title,
                path,
                collapsed,
            } => {
                // A closed group says one of its sessions has news.
                let news = collapsed && self.group_has_news(&key, cx);
                base.role(Role::TreeItem)
                    .aria_label(SharedString::from(row_label(&title, false, false, news)))
                    .aria_expanded(!collapsed)
                    .text_xs()
                    .font_medium()
                    .text_color(theme.muted_foreground)
                    .child(
                        Icon::new(if collapsed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .xsmall(),
                    )
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .child(div().min_w_0().truncate().child(title))
                            .when(news, |this| {
                                this.child(news_dot(("group-news", index), theme.info))
                            }),
                    )
                    .when_some(path.clone(), |this, path| {
                        let path = SharedString::from(path.display().to_string());
                        this.managed_tooltip(move |window, cx| {
                            Tooltip::new(path.clone()).build(window, cx)
                        })
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            this.cursor = this.rows.get(index).map(Row::key);
                            this.focus_handle.focus(window, cx);
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cursor = this.rows.get(index).map(Row::key);
                        this.open_row(index, cx);
                    }))
                    .map(|this| {
                        let Some(folder) = path else {
                            return this.into_any_element();
                        };
                        let sidebar = cx.entity().downgrade();
                        let focus = self.focus_handle.clone();
                        this.context_menu(move |menu, _, _| {
                            let new_folder = folder.clone();
                            let open_folder = folder.clone();
                            let sidebar = sidebar.clone();
                            menu.action_context(focus.clone())
                                .item(PopupMenuItem::new("New Session").on_click(
                                    move |_, _, cx| {
                                        sidebar
                                            .update(cx, |_, cx| {
                                                cx.emit(SidebarEvent::NewSession(
                                                    new_folder.clone(),
                                                ))
                                            })
                                            .ok();
                                    },
                                ))
                                .item(PopupMenuItem::new("Open Folder").on_click(
                                    move |_, _, cx| {
                                        cx.open_with_system(&open_folder);
                                    },
                                ))
                        })
                        .into_any_element()
                    })
            }
            Row::Session {
                id,
                title,
                time,
                conversations,
                expanded,
            } => {
                let chevron = conversations.then(|| {
                    let id = id.clone();
                    Button::new(("conversations", index))
                        .ghost()
                        .xsmall()
                        .compact()
                        .icon(if expanded {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .tooltip(if expanded {
                            "Hide conversations"
                        } else {
                            "Show conversations"
                        })
                        .accessibility_label(if expanded {
                            "Hide conversations"
                        } else {
                            "Show conversations"
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.toggle_conversations(&id, cx);
                        }))
                });
                let menu_id = id.clone();
                let workspace = self
                    .entry(&id)
                    .and_then(|entry| entry.workspace.clone())
                    .filter(|_| {
                        self.entry(&id)
                            .is_some_and(|entry| entry.mode == Mode::Agent)
                    });
                let focus = self.focus_handle.clone();
                let sidebar = cx.entity().downgrade();
                // What the session does here, when it is unusual: working or
                // asking while another is shown, and news (a turn that ended
                // or a question that came while the user looked elsewhere).
                let indicator = LiveSessions::indicator(&id, cx);
                let background = !is_active;
                let working = background && indicator.working && !indicator.question;
                let asking = background && indicator.question;
                let news = background && notifications::has_news(&id, cx);
                let info = theme.info;
                base.role(Role::TreeItem)
                    .aria_label(SharedString::from(row_label(&title, working, asking, news)))
                    .aria_selected(is_active)
                    .when(conversations, |this| this.aria_expanded(expanded))
                    .pl(rems(1.25))
                    // In the space left of the title, so it never takes any
                    // from it, and with the other marks.
                    .when(news, |this| {
                        this.child(
                            news_dot(("row-news", index), info)
                                .absolute()
                                .left(rems(0.5))
                                .top(rems(0.625)),
                        )
                    })
                    .child(
                        div()
                            .w(rems(1.))
                            .flex_none()
                            .flex()
                            .justify_center()
                            .children(chevron),
                    )
                    .map(|this| match renaming {
                        Some(input) => this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .key_context(RENAME_CONTEXT)
                                .on_action(cx.listener(Self::cancel_rename))
                                // Enter in the field confirms the name. The
                                // field passes the key on, and the list's
                                // Enter would open the session too, taking
                                // the keyboard to the composer.
                                .on_action(cx.listener(|_, _: &OpenSelected, _, _| {}))
                                .child(Input::new(&input).xsmall()),
                        ),
                        None => this
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child(title.clone()),
                            )
                            .when(working, |this| {
                                this.child(
                                    div()
                                        .id(("row-working", index))
                                        .test_support()
                                        .flex_none()
                                        .child(
                                            crate::spinner::spinner()
                                                .xsmall()
                                                .color(theme.muted_foreground),
                                        )
                                        .tooltip(|window, cx| {
                                            Tooltip::new("Working").build(window, cx)
                                        }),
                                )
                            })
                            .when(asking, |this| {
                                this.child(
                                    div()
                                        .id(("row-question", index))
                                        .test_support()
                                        .flex_none()
                                        .child(
                                            Icon::new(
                                                gpui_kit::assets::IconName::MessageCircleQuestionMark,
                                            )
                                            .xsmall()
                                            .text_color(theme.warning),
                                        )
                                        .tooltip(|window, cx| {
                                            Tooltip::new("A question waits for you")
                                                .build(window, cx)
                                        }),
                                )
                            })
                            .when(!working && !asking, |this| {
                                this.child(
                                    div()
                                        .flex_none()
                                        .text_xs()
                                        .font_normal()
                                        .text_color(theme.muted_foreground)
                                        .child(time),
                                )
                            }),
                    })
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, _, window, cx| {
                            this.cursor = this.rows.get(index).map(Row::key);
                            this.focus_handle.focus(window, cx);
                            cx.notify();
                        }),
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.cursor = this.rows.get(index).map(Row::key);
                        this.open_row(index, cx);
                    }))
                    .context_menu(move |menu, _, cx| {
                        let id = menu_id.clone();
                        let sidebar = sidebar.clone();
                        let ready = LiveSessions::find(&id, cx)
                            .is_none_or(|session| *session.read(cx).phase() == Phase::Open);
                        let branch = {
                            let id = id.clone();
                            let sidebar = sidebar.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                sidebar
                                    .update(cx, |_, cx| cx.emit(SidebarEvent::Branch(id.clone())))
                                    .ok();
                            }
                        };
                        let open_new = {
                            let id = id.clone();
                            let sidebar = sidebar.clone();
                            move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                                sidebar
                                    .update(cx, |_, cx| {
                                        cx.emit(SidebarEvent::OpenInNewWindow(id.clone()))
                                    })
                                    .ok();
                            }
                        };
                        menu.action_context(focus.clone())
                            .menu("Rename…", Box::new(RenameSession))
                            .item(PopupMenuItem::new("Open in New Window").on_click(open_new))
                            .item(
                                PopupMenuItem::new("Branch from Here")
                                    .disabled(!ready)
                                    .on_click(branch),
                            )
                            .when_some(workspace.clone(), |menu, folder| {
                                menu.item(
                                    PopupMenuItem::new("Reveal Folder")
                                        .on_click(move |_, _, cx| cx.reveal_path(&folder)),
                                )
                            })
                            .separator()
                            .menu("Delete…", Box::new(DeleteSession))
                    })
                    .into_any_element()
            }
            Row::Conversation {
                number,
                title,
                current,
                ..
            } => base
                .role(Role::TreeItem)
                .aria_label(SharedString::from(title.clone()))
                .pl(rems(3.25))
                .text_color(if current {
                    theme.sidebar_foreground
                } else {
                    theme.muted_foreground
                })
                .child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(number.to_string()),
                )
                .child(div().flex_1().min_w_0().truncate().child(title))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.cursor = this.rows.get(index).map(Row::key);
                    this.open_row(index, cx);
                }))
                .into_any_element(),
        }
    }

    fn render_empty(&self, cx: &App) -> impl IntoElement {
        let filter = self.filter.read(cx).value();
        let text = if !filter.is_empty() {
            format!("No sessions match “{filter}”")
        } else {
            "No sessions yet".to_string()
        };
        div()
            .p_3()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .when(self.loaded, |this| this.child(text))
    }
}

/// A session row as a screen reader names it: the title, and the state the
/// row's marks show. The dot for news isn't read, the label says it.
fn row_label(title: &str, working: bool, asking: bool, news: bool) -> String {
    let mut label = title.to_string();
    if working {
        label.push_str(", working");
    }
    if asking {
        label.push_str(", a question waits");
    }
    if news {
        label.push_str(", new activity");
    }
    label
}

/// The dot that says a session, or a closed group of them, has news. It
/// has no name or role of its own, so a screen reader leaves it out.
fn news_dot(
    id: impl Into<ElementId>,
    color: Hsla,
) -> gpui_kit::base::ObservedElement<Stateful<Div>> {
    div()
        .id(id)
        .test_support()
        .flex_none()
        .size_2()
        .rounded_full()
        .bg(color)
}

/// Tells the user why a backend call failed.
fn show_error(what: &str, error: &BackendError, window: &mut Window, cx: &mut App) {
    window.push_notification(
        gpui_kit::component::notification::Notification::error(format!(
            "{what} {}",
            error.message()
        )),
        cx,
    );
}

impl Focusable for Sidebar {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Sidebar {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        crate::testing::rendered("Sidebar");
        let theme = cx.theme();
        let rows = self.rows.len();
        v_flex()
            .id("sidebar")
            .size_full()
            .bg(theme.sidebar)
            .text_color(theme.sidebar_foreground)
            .child(
                v_flex()
                    .flex_none()
                    .p_2()
                    .gap_2()
                    // Laid out as a row of the list, its icon in the
                    // chevrons' place and its label where group names start.
                    .child(
                        h_flex()
                            .id("new-session")
                            .test_support()
                            .role(Role::Button)
                            .aria_label(SharedString::from("New session"))
                            .h_7()
                            .w_full()
                            .px_2()
                            .gap_1p5()
                            .rounded(theme.radius)
                            .border_1()
                            .border_color(theme.transparent)
                            .text_sm()
                            .hover(|this| {
                                this.bg(theme.sidebar_accent.opacity(0.8))
                                    .text_color(theme.sidebar_accent_foreground)
                            })
                            .child(Icon::new(IconName::Plus).xsmall())
                            .child("New session")
                            .tooltip(|window, cx| {
                                Tooltip::new("New session")
                                    .action(&NewSession, None)
                                    .build(window, cx)
                            })
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(NewSession), cx)
                            }),
                    )
                    .child(
                        div()
                            .key_context(FILTER_CONTEXT)
                            .on_action(cx.listener(Self::focus_list_action))
                            .on_action(cx.listener(Self::clear_filter))
                            .child(Input::new(&self.filter).small().cleanable(true).prefix(
                                Icon::new(IconName::Search).text_color(theme.muted_foreground),
                            )),
                    ),
            )
            .child(
                div()
                    .id("session-list")
                    .test_support()
                    .key_context(LIST_CONTEXT)
                    .track_focus(&self.focus_handle)
                    .role(Role::Tree)
                    .aria_label(SharedString::from("Sessions"))
                    .on_action(cx.listener(Self::select_previous))
                    .on_action(cx.listener(Self::select_next))
                    .on_action(cx.listener(Self::select_first))
                    .on_action(cx.listener(Self::select_last))
                    .on_action(cx.listener(Self::collapse))
                    .on_action(cx.listener(Self::expand))
                    .on_action(cx.listener(Self::open_selected))
                    .on_action(cx.listener(Self::rename_action))
                    .on_action(cx.listener(Self::delete_action))
                    .flex_1()
                    .min_h_0()
                    .px_2()
                    .pb_2()
                    .map(|this| {
                        if rows == 0 {
                            this.child(self.render_empty(cx))
                        } else {
                            this.child(
                                uniform_list(
                                    "session-rows",
                                    rows,
                                    cx.processor(
                                        |this, range: std::ops::Range<usize>, window, cx| {
                                            range
                                                .map(|index| this.render_row(index, window, cx))
                                                .collect()
                                        },
                                    ),
                                )
                                .size_full()
                                .track_scroll(&self.scroll),
                            )
                        }
                    }),
            )
    }
}

#[cfg(test)]
mod tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{CHATS, Row, row_label, rows};
    use crate::backend::Backend as _;
    use crate::backend::{ConversationEntry, FakeBackend, Mode, SessionEntry, SessionId};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    #[test]
    fn a_rows_name_carries_its_marks() {
        assert_eq!(
            row_label("Fix the parser", false, false, false),
            "Fix the parser"
        );
        assert_eq!(
            row_label("Fix the parser", true, false, true),
            "Fix the parser, working, new activity"
        );
        assert_eq!(
            row_label("Fix the parser", false, true, false),
            "Fix the parser, a question waits"
        );
    }

    fn sample() -> (Vec<SessionEntry>, SystemTime) {
        let now = SystemTime::now();
        let sessions = FakeBackend::sample("/arko", now).sessions().unwrap();
        (sessions, now)
    }

    fn titles(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Group { title, .. } => format!("# {title}"),
                Row::Session { title, .. } => title.clone(),
                Row::Conversation { number, title, .. } => format!("  {number} {title}"),
            })
            .collect()
    }

    #[test]
    fn chats_come_first_then_folders_by_their_latest_session() {
        let (sessions, now) = sample();
        let rows = rows(&sessions, &[], &HashSet::new(), "", now);
        assert_eq!(
            titles(&rows),
            [
                "# Chats",
                "Variable fonts and the wght axis",
                "Naming a desktop app",
                "# arko",
                "Split the engine out of main.rs",
                "Fix the handover size for workers",
                "# gpui_kit",
                "Golden screenshots on Linux",
            ]
        );
    }

    #[test]
    fn a_collapsed_group_keeps_only_its_header() {
        let (sessions, now) = sample();
        let rows = rows(
            &sessions,
            &["/home/ada/src/arko".to_string()],
            &HashSet::new(),
            "",
            now,
        );
        assert!(rows.contains(&Row::Group {
            key: "/home/ada/src/arko".into(),
            title: "arko".into(),
            path: Some(PathBuf::from("/home/ada/src/arko")),
            collapsed: true,
        }));
        assert!(!titles(&rows).contains(&"Split the engine out of main.rs".to_string()));
    }

    #[test]
    fn the_filter_matches_titles_folders_and_conversations_in_open_groups() {
        let (sessions, now) = sample();
        let collapsed = vec![CHATS.to_string(), "/home/ada/src/arko".to_string()];
        let found = rows(&sessions, &collapsed, &HashSet::new(), "FONTS", now);
        assert_eq!(
            titles(&found),
            ["# Chats", "Variable fonts and the wght axis"]
        );
        let found = rows(&sessions, &collapsed, &HashSet::new(), "gpui_kit", now);
        assert_eq!(
            titles(&found),
            ["# gpui_kit", "Golden screenshots on Linux"]
        );
        let found = rows(&sessions, &collapsed, &HashSet::new(), "950k", now);
        assert_eq!(
            titles(&found),
            ["# arko", "Fix the handover size for workers"]
        );
        assert!(rows(&sessions, &[], &HashSet::new(), "nothing like it", now).is_empty());
    }

    /// A session is named by the first words of its first message, and the
    /// filter still finds the rest of that message in its conversation.
    #[test]
    fn a_session_shows_its_short_name_and_is_found_by_its_whole_first_line() {
        let now = SystemTime::now();
        let line = "Fix the flaky scroll test in the sidebar before the release";
        let entry = SessionEntry {
            id: SessionId("s".into()),
            title: arko::session_names::short_name(line),
            mode: Mode::Chat,
            workspace: None,
            model: String::new(),
            created: now - Duration::from_secs(60),
            last_active: None,
            conversations: vec![ConversationEntry {
                number: 1,
                title: line.into(),
                current: true,
            }],
            holder: None,
            had_workers: false,
        };
        let shown = rows(std::slice::from_ref(&entry), &[], &HashSet::new(), "", now);
        assert_eq!(titles(&shown), ["# Chats", "Fix the flaky scroll test in…"]);
        for words in ["before the release", "SIDEBAR BEFORE", "scroll test in"] {
            let found = rows(
                std::slice::from_ref(&entry),
                &[],
                &HashSet::new(),
                words,
                now,
            );
            assert_eq!(titles(&found).len(), 2, "{words}");
        }
        assert!(rows(&[entry], &[], &HashSet::new(), "after the release", now).is_empty());
    }

    #[test]
    fn an_expanded_session_lists_its_conversations() {
        let (sessions, now) = sample();
        let id = SessionId("20261015-171000-c3d4".into());
        let rows = rows(&sessions, &[], &HashSet::from([id.clone()]), "", now);
        let at = rows
            .iter()
            .position(|row| matches!(row, Row::Session { id: row_id, .. } if *row_id == id))
            .unwrap();
        assert!(matches!(
            &rows[at],
            Row::Session {
                conversations: true,
                expanded: true,
                ..
            }
        ));
        assert_eq!(
            titles(&rows[at + 1..at + 3]),
            [
                "  1 Fix the handover size for workers",
                "  2 Check /handover-at 950k"
            ]
        );
    }

    #[test]
    fn a_session_with_one_conversation_has_no_disclosure() {
        let now = SystemTime::now();
        let entry = SessionEntry {
            id: SessionId("s".into()),
            title: "One".into(),
            mode: Mode::Chat,
            workspace: None,
            model: String::new(),
            created: now - Duration::from_secs(60),
            last_active: None,
            conversations: vec![ConversationEntry {
                number: 1,
                title: "One".into(),
                current: true,
            }],
            holder: None,
            had_workers: false,
        };
        let rows = rows(&[entry], &[], &HashSet::new(), "", now);
        assert!(
            matches!(&rows[1], Row::Session { conversations: false, time, .. } if time == "1m")
        );
    }
}

#[cfg(test)]
mod focus_tests {
    // Named, not `super::*`: GPUI's glob brings a `test` macro that would
    // shadow Rust's.
    use super::{RowKey, Sidebar};
    use crate::backend::SessionId;
    use crate::tests::{act, open};
    use crate::{Startup, menus};
    use gpui_kit::test::TestWindowExt as _;
    use gpui_kit::{AnyWindowHandle, AppContext as _, Entity, TestAppContext};
    use std::sync::Arc;
    use std::time::SystemTime;

    fn start(cx: &mut TestAppContext) {
        let backend = Arc::new(crate::backend::FakeBackend::sample(
            "/arko",
            SystemTime::now(),
        ));
        cx.update(|cx| crate::init(Startup::for_tests(backend), cx));
    }

    /// The row that shows the cursor's ring, whether a name is being edited
    /// and whether the keyboard is in the sidebar.
    fn focus(
        sidebar: &Entity<Sidebar>,
        window: AnyWindowHandle,
        cx: &mut TestAppContext,
    ) -> (Option<RowKey>, bool, bool) {
        act(cx, window, |_, _| {});
        cx.update_window(window, |_, window, cx| {
            let sidebar = sidebar.read(cx);
            (
                sidebar.cursor_ring(window, cx).cloned(),
                sidebar.renaming.is_some(),
                sidebar.focus_handle.contains_focused(window, cx),
            )
        })
        .unwrap()
    }

    /// While a session's name is edited, only its field shows focus. Enter
    /// and Escape give the keyboard and the ring back to its row, and a click
    /// into the composer leaves the keyboard there.
    #[gpui_kit::test]
    fn the_name_field_alone_shows_focus_while_renaming(cx: &mut TestAppContext) {
        start(cx);
        let (window, workspace) = open(cx, 1300.);
        // Focus moving away is told only to an active window's elements.
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        let sidebar = cx.read(|cx| workspace.read(cx).sidebar().clone());
        let fonts = RowKey::Session(SessionId("20261016-080000-a7b8".into()));
        act(cx, window, |window, cx| {
            window.dispatch_action(Box::new(menus::FocusSessionFilter), cx)
        });
        act(cx, window, |window, cx| window.press("down", cx));
        assert_eq!(
            focus(&sidebar, window, cx),
            (Some(fonts.clone()), false, true)
        );

        for end in ["enter", "escape"] {
            act(cx, window, |window, cx| window.press("f2", cx));
            assert_eq!(focus(&sidebar, window, cx), (None, true, true), "{end}");
            act(cx, window, |window, cx| window.press(end, cx));
            assert_eq!(
                focus(&sidebar, window, cx),
                (Some(fonts.clone()), false, true),
                "{end}"
            );
        }

        act(cx, window, |window, cx| window.press("f2", cx));
        let view = cx.read(|cx| workspace.read(cx).session_view().clone());
        act(cx, window, |window, cx| {
            view.update(cx, |view, cx| view.focus_composer(window, cx))
        });
        assert_eq!(focus(&sidebar, window, cx), (None, false, false));
    }
}
