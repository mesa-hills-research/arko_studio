//! A value loaded off the UI thread for a key, such as the models listed
//! for a folder. What was loaded last stays shown while a newer load runs,
//! and only the load running can change it.

use std::sync::atomic::{AtomicU64, Ordering};

/// Numbers the loads of every [`Loaded`], so a token matches only the
/// loader that made it.
static LOADS: AtomicU64 = AtomicU64::new(1);

/// The value loaded last for a key, and the load running, if one is.
pub struct Loaded<K, V, T = gpui_kit::Task<()>> {
    /// What was loaded last, and the key it was loaded for.
    shown: Option<(K, V)>,
    /// What's shown may be out of date, because what reaches it changed or
    /// a reload of its key was asked for.
    stale: bool,
    running: Option<Running<K, T>>,
}

struct Running<K, T> {
    key: K,
    id: u64,
    /// Dropping it stops the load, since a dropped gpui task is cancelled.
    _task: T,
}

/// One load's token, which its task gives back with the result. Only
/// [`Loaded::start`] makes one, and [`Loaded::finish`] takes it.
#[must_use]
pub struct Load<K> {
    key: K,
    id: u64,
}

impl<K, V, T> Default for Loaded<K, V, T> {
    fn default() -> Self {
        Self {
            shown: None,
            stale: false,
            running: None,
        }
    }
}

impl<K: Clone + PartialEq, V, T> Loaded<K, V, T> {
    /// The value loaded last, whatever its key. It shows while a newer load
    /// runs.
    pub fn shown(&self) -> Option<&V> {
        self.shown.as_ref().map(|(_, value)| value)
    }

    /// Takes `key` as the one wanted now, which stops a load running for
    /// another key. True when a load for `key` should start, because none
    /// runs for it and what's shown isn't for it or is out of date.
    pub fn want(&mut self, key: &K) -> bool {
        if self
            .running
            .as_ref()
            .is_some_and(|running| running.key != *key)
        {
            self.running = None;
        }
        self.running.is_none()
            && (self.stale || self.shown.as_ref().is_none_or(|(shown, _)| shown != key))
    }

    /// Loads `key` with the task `spawn` makes, in place of any load
    /// running. The task gives its result and the token to
    /// [`finish`](Self::finish). Loading the key shown again marks what's
    /// shown out of date until the result comes.
    pub fn start(&mut self, key: K, spawn: impl FnOnce(Load<K>) -> T) {
        let id = LOADS.fetch_add(1, Ordering::Relaxed);
        if self.shown.as_ref().is_some_and(|(shown, _)| *shown == key) {
            self.stale = true;
        }
        self.running = None;
        let task = spawn(Load {
            key: key.clone(),
            id,
        });
        self.running = Some(Running {
            key,
            id,
            _task: task,
        });
    }

    /// The result of `load`. It shows, for the key it was loaded for, if
    /// that load is still the one running, and is dropped otherwise. True
    /// when it shows.
    pub fn finish(&mut self, load: Load<K>, value: V) -> bool {
        if self
            .running
            .as_ref()
            .is_none_or(|running| running.id != load.id)
        {
            return false;
        }
        self.running = None;
        self.shown = Some((load.key, value));
        self.stale = false;
        true
    }

    /// What reaches the value changed. What's shown stays until a new load
    /// of its key ends. A load running stops, since it began before the
    /// change.
    pub fn mark_stale(&mut self) {
        self.running = None;
        self.stale = self.shown.is_some();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Folders = Loaded<&'static str, &'static str, ()>;

    /// Starts a load of `key` and gives back its token.
    fn start(loaded: &mut Folders, key: &'static str) -> Load<&'static str> {
        let mut token = None;
        loaded.start(key, |load| token = Some(load));
        token.unwrap()
    }

    #[test]
    fn a_result_shows_for_the_key_it_was_loaded_for() {
        let mut loaded = Folders::default();
        assert!(loaded.want(&"a"));
        let load = start(&mut loaded, "a");
        assert!(!loaded.want(&"a"), "a load for it runs");
        assert!(loaded.finish(load, "a's"));
        assert_eq!(loaded.shown(), Some(&"a's"));
        assert!(!loaded.want(&"a"), "it's loaded");
        assert!(loaded.want(&"b"), "another key isn't");
    }

    #[test]
    fn the_value_shown_stays_while_another_key_loads() {
        let mut loaded = Folders::default();
        let load = start(&mut loaded, "a");
        loaded.finish(load, "a's");
        assert!(loaded.want(&"b"));
        let load = start(&mut loaded, "b");
        assert_eq!(loaded.shown(), Some(&"a's"));
        assert!(loaded.finish(load, "b's"));
        assert_eq!(loaded.shown(), Some(&"b's"));
    }

    #[test]
    fn results_arriving_out_of_order_show_only_the_latest() {
        let mut loaded = Folders::default();
        let first = start(&mut loaded, "a");
        let second = start(&mut loaded, "b");
        assert!(loaded.finish(second, "b's"));
        assert!(!loaded.finish(first, "a's"));
        assert_eq!(loaded.shown(), Some(&"b's"));
        assert!(loaded.want(&"a"), "a's result was dropped");
    }

    #[test]
    fn going_back_to_the_key_shown_stops_the_load_for_the_other() {
        let mut loaded = Folders::default();
        let load = start(&mut loaded, "a");
        loaded.finish(load, "a's");
        assert!(loaded.want(&"b"));
        let other = start(&mut loaded, "b");
        assert!(!loaded.want(&"a"), "a is shown and up to date");
        assert!(!loaded.finish(other, "b's"), "b's load stopped");
        assert_eq!(loaded.shown(), Some(&"a's"));
    }

    #[test]
    fn a_reload_stopped_by_another_key_runs_again_when_the_key_comes_back() {
        let mut loaded = Folders::default();
        let load = start(&mut loaded, "a");
        loaded.finish(load, "a's");
        // A reload of a, then b before it came.
        let reload = start(&mut loaded, "a");
        assert!(loaded.want(&"b"));
        assert!(!loaded.finish(reload, "a's again"));
        let other = start(&mut loaded, "b");
        // Back to a, whose reload never came.
        assert!(loaded.want(&"a"));
        assert!(!loaded.finish(other, "b's"));
        let reload = start(&mut loaded, "a");
        assert!(loaded.finish(reload, "a's again"));
        assert_eq!(loaded.shown(), Some(&"a's again"));
        assert!(!loaded.want(&"a"));
    }

    #[test]
    fn a_change_during_a_load_stops_it_and_the_next_want_loads_again() {
        let mut loaded = Folders::default();
        let load = start(&mut loaded, "a");
        loaded.finish(load, "a's");
        let reload = start(&mut loaded, "a");
        loaded.mark_stale();
        assert!(!loaded.finish(reload, "from before the change"));
        assert_eq!(loaded.shown(), Some(&"a's"), "the value shown stays");
        assert!(loaded.want(&"a"));
        let reload = start(&mut loaded, "a");
        assert!(loaded.finish(reload, "from after it"));
        assert!(!loaded.want(&"a"));
    }

    #[test]
    fn a_change_with_nothing_shown_stops_the_load_and_marks_nothing() {
        let mut loaded = Folders::default();
        let load = start(&mut loaded, "a");
        loaded.mark_stale();
        assert!(!loaded.finish(load, "a's"));
        assert!(loaded.want(&"a"));
        let load = start(&mut loaded, "a");
        assert!(loaded.finish(load, "a's"));
        assert!(!loaded.want(&"a"));
    }

    #[test]
    fn a_token_from_another_loader_is_ignored() {
        let mut one = Folders::default();
        let mut other = Folders::default();
        let ours = start(&mut one, "a");
        let theirs = start(&mut other, "a");
        assert!(!one.finish(theirs, "theirs"));
        assert_eq!(one.shown(), None);
        assert!(one.finish(ours, "ours"), "its own load still runs");
    }

    #[test]
    fn a_failed_load_shows_its_failure_and_is_not_wanted_again() {
        let mut loaded = Loaded::<&str, Result<&str, &str>, ()>::default();
        let mut token = None;
        loaded.start("a", |load| token = Some(load));
        assert!(loaded.finish(token.unwrap(), Err("unreadable")));
        assert_eq!(loaded.shown(), Some(&Err("unreadable")));
        assert!(!loaded.want(&"a"));
    }
}
