//! The boundary between Arko Studio's interface and arko's engine.
//!
//! The interface asks a [`Backend`] for the sessions in Arko's folder and
//! acts on them. Its calls block (they read and write files, or wait for the
//! engine), so the interface makes them on a background thread and applies
//! the answers on the UI thread. [`FakeBackend`] answers from memory, for
//! tests and for the interface's work before the engine is wired in; the
//! engine's backend wraps arko's library (`arko::sessions`, `arko::Engine`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

/// A session's id: the name of its folder in `<arko>/sessions`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// How a session works: a chat (no tools, no folder) or an agent in a folder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mode {
    Chat,
    Agent,
}

/// A conversation inside a session (the terminal's `/new` starts another,
/// `/resume` goes back to one).
#[derive(Debug, Clone, PartialEq)]
pub struct ConversationEntry {
    /// 1-based, as `/resume N` takes it.
    pub number: u32,
    pub title: String,
    /// The one the session continues.
    pub current: bool,
}

/// Who has a session open. A session is open in one app at a time.
#[derive(Debug, Clone, PartialEq)]
pub struct Holder {
    /// `arko` (the terminal) or `arko-studio`.
    pub app: String,
    pub pid: u32,
    pub host: String,
}

impl Holder {
    /// "arko (process 4121)", with the host when it isn't this machine's.
    pub fn describe(&self, this_host: Option<&str>) -> String {
        let app = match self.app.as_str() {
            "arko" => "arko in a terminal",
            "arko-studio" => "another Arko Studio",
            other => other,
        };
        match this_host {
            Some(host) if host != self.host => {
                format!("{app} (process {} on {})", self.pid, self.host)
            }
            _ => format!("{app} (process {})", self.pid),
        }
    }
}

/// A session as the list shows it, read without loading its conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionEntry {
    pub id: SessionId,
    /// The name the user gave it, else the start of its current
    /// conversation's first message, else "New session".
    pub title: String,
    pub mode: Mode,
    /// The folder an agent works in. Chats have none.
    pub workspace: Option<PathBuf>,
    /// The main agent's model, as arko names it.
    pub model: String,
    pub created: SystemTime,
    /// When it was last worked in; `None` before its first message.
    pub last_active: Option<SystemTime>,
    pub conversations: Vec<ConversationEntry>,
    /// Who has it open, when someone does.
    pub holder: Option<Holder>,
}

impl SessionEntry {
    /// When it was last worked in, or else made: what the list sorts by.
    pub fn activity(&self) -> SystemTime {
        self.last_active.unwrap_or(self.created)
    }
}

/// Why a backend call failed, in words for the user.
#[derive(Debug, Clone, PartialEq)]
pub enum BackendError {
    /// The session is open elsewhere; `None` when an older arko holds it
    /// without saying who it is.
    Locked {
        holder: Option<Holder>,
    },
    NotFound(SessionId),
    /// Anything else, already worded for the user.
    Other(String),
}

impl BackendError {
    pub fn message(&self, this_host: Option<&str>) -> String {
        match self {
            BackendError::Locked {
                holder: Some(holder),
            } => {
                format!("It’s open in {}.", holder.describe(this_host))
            }
            BackendError::Locked { holder: None } => {
                "It’s open in another copy of arko.".to_string()
            }
            BackendError::NotFound(id) => format!("There’s no session {id} any more."),
            BackendError::Other(message) => message.clone(),
        }
    }
}

/// An open session. Holding it keeps the session's lock; dropping it closes
/// the session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionHandle {
    pub entry: SessionEntry,
}

/// What Arko Studio needs from arko's engine to list and manage sessions.
///
/// Every method blocks: call them from a background task.
pub trait Backend: Send + Sync + 'static {
    /// Arko's folder, shared with the terminal app.
    fn data_root(&self) -> &Path;

    /// Every session in Arko's folder, in no particular order.
    fn sessions(&self) -> Result<Vec<SessionEntry>, BackendError>;

    /// Opens a session for this window. Fails with [`BackendError::Locked`]
    /// while another app has it open.
    fn open(&self, id: &SessionId) -> Result<SessionHandle, BackendError>;

    /// Closes a session this app opened (the engine shuts it down and
    /// releases its lock).
    fn close(&self, handle: SessionHandle);

    /// Renames a session. Works while it is open, here or elsewhere.
    fn rename(&self, id: &SessionId, title: &str) -> Result<(), BackendError>;

    /// Deletes a session's saved data (never the files in its folder).
    /// Refuses while the session is open anywhere, this app included.
    fn delete(&self, id: &SessionId) -> Result<(), BackendError>;
}

/// The backend the app runs on, shared by its windows.
#[derive(Clone)]
pub struct SharedBackend(pub Arc<dyn Backend>);

impl gpui_kit::Global for SharedBackend {}

impl std::ops::Deref for SharedBackend {
    type Target = dyn Backend;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// Sessions in memory, for tests and for building the interface before the
/// engine is wired in.
pub struct FakeBackend {
    root: PathBuf,
    state: Mutex<FakeState>,
}

struct FakeState {
    sessions: Vec<SessionEntry>,
    /// Sessions this app has open.
    open: Vec<SessionId>,
}

impl FakeBackend {
    pub fn new(root: impl Into<PathBuf>, sessions: Vec<SessionEntry>) -> Self {
        Self {
            root: root.into(),
            state: Mutex::new(FakeState {
                sessions,
                open: Vec::new(),
            }),
        }
    }

    /// No sessions at all.
    pub fn empty(root: impl Into<PathBuf>) -> Self {
        Self::new(root, Vec::new())
    }

    /// A handful of sessions in two folders and two chats, one of them open
    /// in the terminal, relative to `now`.
    pub fn sample(root: impl Into<PathBuf>, now: SystemTime) -> Self {
        let ago = |minutes: u64| now - Duration::from_secs(minutes * 60);
        let agent = |id: &str, title: &str, folder: &str, minutes: u64| SessionEntry {
            id: SessionId(id.into()),
            title: title.into(),
            mode: Mode::Agent,
            workspace: Some(PathBuf::from(folder)),
            model: "claude-opus-5-5".into(),
            created: ago(minutes + 30),
            last_active: Some(ago(minutes)),
            conversations: Vec::new(),
            holder: None,
        };
        let chat = |id: &str, title: &str, minutes: u64| SessionEntry {
            mode: Mode::Chat,
            workspace: None,
            ..agent(id, title, "", minutes)
        };
        let mut sessions = vec![
            agent(
                "20261016-091200-a1b2",
                "Split the engine out of main.rs",
                "/home/ada/src/arko",
                4,
            ),
            agent(
                "20261015-171000-c3d4",
                "Fix the handover size for workers",
                "/home/ada/src/arko",
                60 * 20,
            ),
            agent(
                "20261014-110500-e5f6",
                "Golden screenshots on Linux",
                "/home/ada/src/gpui_kit",
                60 * 47,
            ),
            chat(
                "20261016-080000-a7b8",
                "Variable fonts and the wght axis",
                90,
            ),
            chat("20261009-120000-c9d0", "Naming a desktop app", 60 * 24 * 7),
        ];
        sessions[1].conversations = vec![
            ConversationEntry {
                number: 1,
                title: "Fix the handover size for workers".into(),
                current: false,
            },
            ConversationEntry {
                number: 2,
                title: "Check /handover-at 950k".into(),
                current: true,
            },
        ];
        sessions[2].holder = Some(Holder {
            app: "arko".into(),
            pid: 4121,
            host: "studio".into(),
        });
        Self::new(root, sessions)
    }

    fn with_state<R>(&self, f: impl FnOnce(&mut FakeState) -> R) -> R {
        f(&mut self
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()))
    }
}

impl Backend for FakeBackend {
    fn data_root(&self) -> &Path {
        &self.root
    }

    fn sessions(&self) -> Result<Vec<SessionEntry>, BackendError> {
        Ok(self.with_state(|state| state.sessions.clone()))
    }

    fn open(&self, id: &SessionId) -> Result<SessionHandle, BackendError> {
        self.with_state(|state| {
            let entry = state
                .sessions
                .iter()
                .find(|entry| &entry.id == id)
                .cloned()
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            if let Some(holder) = &entry.holder {
                return Err(BackendError::Locked {
                    holder: Some(holder.clone()),
                });
            }
            if !state.open.contains(id) {
                state.open.push(id.clone());
            }
            Ok(SessionHandle { entry })
        })
    }

    fn close(&self, handle: SessionHandle) {
        let id = handle.entry.id;
        self.with_state(|state| state.open.retain(|open| *open != id));
    }

    fn rename(&self, id: &SessionId, title: &str) -> Result<(), BackendError> {
        let title = title.trim();
        if title.is_empty() {
            return Err(BackendError::Other(
                "A session’s name can’t be empty.".into(),
            ));
        }
        self.with_state(|state| {
            let entry = state
                .sessions
                .iter_mut()
                .find(|entry| &entry.id == id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            entry.title = title.to_string();
            Ok(())
        })
    }

    fn delete(&self, id: &SessionId) -> Result<(), BackendError> {
        self.with_state(|state| {
            let index = state
                .sessions
                .iter()
                .position(|entry| &entry.id == id)
                .ok_or_else(|| BackendError::NotFound(id.clone()))?;
            if let Some(holder) = &state.sessions[index].holder {
                return Err(BackendError::Locked {
                    holder: Some(holder.clone()),
                });
            }
            if state.open.contains(id) {
                return Err(BackendError::Locked {
                    holder: Some(Holder {
                        app: "arko-studio".into(),
                        pid: std::process::id(),
                        host: String::new(),
                    }),
                });
            }
            state.sessions.remove(index);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> FakeBackend {
        FakeBackend::sample("/arko", SystemTime::now())
    }

    #[test]
    fn a_session_open_elsewhere_says_who_has_it() {
        let backend = backend();
        let error = backend
            .open(&SessionId("20261014-110500-e5f6".into()))
            .unwrap_err();
        assert_eq!(
            error.message(Some("studio")),
            "It’s open in arko in a terminal (process 4121)."
        );
        assert_eq!(
            error.message(Some("laptop")),
            "It’s open in arko in a terminal (process 4121 on studio)."
        );
    }

    #[test]
    fn rename_trims_and_refuses_an_empty_name() {
        let backend = backend();
        let id = SessionId("20261016-080000-a7b8".into());
        backend.rename(&id, "  Fonts  ").unwrap();
        let sessions = backend.sessions().unwrap();
        assert_eq!(
            sessions.iter().find(|entry| entry.id == id).unwrap().title,
            "Fonts"
        );
        assert!(backend.rename(&id, "   ").is_err());
    }

    #[test]
    fn delete_refuses_an_open_session_and_removes_a_closed_one() {
        let backend = backend();
        let id = SessionId("20261016-091200-a1b2".into());
        let handle = backend.open(&id).unwrap();
        assert!(matches!(
            backend.delete(&id),
            Err(BackendError::Locked { .. })
        ));
        backend.close(handle);
        backend.delete(&id).unwrap();
        assert!(
            backend
                .sessions()
                .unwrap()
                .iter()
                .all(|entry| entry.id != id)
        );
        assert_eq!(backend.delete(&id), Err(BackendError::NotFound(id.clone())));
    }
}
