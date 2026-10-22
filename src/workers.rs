//! Where a list shows each worker and how its figures read.

use arko::remote::{WorkerState, WorkerSummary};

/// A worker that ended stays among the others this long, then moves to the
/// "Finished" group at the bottom of the list.
pub const ENDED_SHOWS_FOR_MS: i64 = 5 * 60 * 1000;

/// How long the worker has worked, including runs before the session
/// reopened. A worker waiting for its first run has no time yet.
pub fn run_time_ms(worker: &WorkerSummary, now: i64) -> Option<i64> {
    (worker.state != WorkerState::Queued || worker.ran_ms > 0)
        .then(|| i64::try_from(worker.run_time_ms(now)).unwrap_or(i64::MAX))
}

/// Where a list of workers shows a worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    /// Running or waiting, or ended less than [`ENDED_SHOWS_FOR_MS`] ago.
    Current,
    /// Ended a while ago, or before the session opened: in the collapsed
    /// "Finished" group.
    Finished,
}

/// Where `worker` shows at `now`. A failure stays in sight until the
/// user has `seen` it (opened it in the panel).
pub fn section(worker: &WorkerSummary, now: i64, seen: bool) -> Section {
    if worker.state.is_active() || (worker.state == WorkerState::Failed && !seen) {
        return Section::Current;
    }
    // A worker restored with the session has no start: it ended before.
    match (worker.started_at, worker.ended_at) {
        (Some(_), Some(ended)) if now - ended < ENDED_SHOWS_FOR_MS => Section::Current,
        _ => Section::Finished,
    }
}

/// When the next worker of `workers` moves to the "Finished" group, after
/// `now`, if one will. `seen` says which ones the user has opened.
pub fn next_move(workers: &[WorkerSummary], now: i64, seen: impl Fn(&str) -> bool) -> Option<i64> {
    workers
        .iter()
        .filter(|worker| {
            section(worker, now, seen(&worker.id)) == Section::Current
                && !worker.state.is_active()
                && (worker.state != WorkerState::Failed || seen(&worker.id))
        })
        .filter_map(|worker| worker.ended_at)
        .map(|ended| ended + ENDED_SHOWS_FOR_MS)
        .min()
}

/// A worker's time as a list shows it: "12 s", "4 m", "1 h 05 m". The
/// minutes take two figures once there are hours, so right-aligned times
/// line up in tabular figures.
pub fn elapsed_label(ms: i64) -> String {
    let seconds = ms.max(0) / 1000;
    match seconds {
        0..60 => format!("{seconds} s"),
        60..3600 => format!("{} m", seconds / 60),
        _ => format!("{} h {:02} m", seconds / 3600, seconds % 3600 / 60),
    }
}

/// A context size as the status figures show it: "8.2k", "142k", "1.02M",
/// "1M".
pub fn tokens_label(tokens: usize) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        // Cut, not rounded: 9,990 reads "9.9k", never "10.0k".
        1_000..10_000 => format!("{:.1}k", (tokens / 100) as f64 / 10.),
        10_000..1_000_000 => format!("{}k", tokens / 1_000),
        _ => {
            let millions = format!("{:.2}", (tokens / 10_000) as f64 / 100.);
            format!("{}M", millions.trim_end_matches('0').trim_end_matches('.'))
        }
    }
}

/// The state as a word, for tooltips and the panel's header.
pub fn state_label(state: WorkerState) -> &'static str {
    match state {
        WorkerState::Queued => "Waiting",
        WorkerState::Running => "Running",
        WorkerState::Finished => "Finished",
        WorkerState::Failed => "Failed",
        WorkerState::Stopped => "Stopped",
        // A state a later arko adds.
        _ => "Ended",
    }
}

/// What a worker did last, in a line: its report's first line once it
/// finished, else its latest call or why it stopped.
pub fn last_line(worker: &WorkerSummary) -> Option<String> {
    let report = (worker.state == WorkerState::Finished)
        .then_some(worker.report.as_deref())
        .flatten()
        .and_then(|report| report.lines().find(|line| !line.trim().is_empty()));
    report
        .or(worker.last.as_deref())
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(
        id: &str,
        state: WorkerState,
        started: Option<i64>,
        ended: Option<i64>,
    ) -> WorkerSummary {
        let mut summary = WorkerSummary::new(id);
        summary.title = format!("Task of {id}");
        summary.state = state;
        summary.tool_calls = 3;
        summary.started_at = started;
        summary.ended_at = ended;
        summary.ran_ms = started.zip(ended).map_or(0, |(s, e)| (e - s).max(0) as u64);
        summary.running_since = started.filter(|_| state == WorkerState::Running);
        summary
    }

    #[test]
    fn times_include_runs_before_the_session_reopened() {
        let queued = worker("r1", WorkerState::Queued, None, None);
        assert_eq!(run_time_ms(&queued, 5_000), None);
        let mut restored = worker("r1", WorkerState::Finished, None, None);
        restored.ran_ms = 60_000;
        assert_eq!(run_time_ms(&restored, 100_000), Some(60_000));
        restored.state = WorkerState::Running;
        restored.running_since = Some(200_000);
        assert_eq!(run_time_ms(&restored, 230_000), Some(90_000));
        // The clock may move backwards, but the earlier runs remain.
        assert_eq!(run_time_ms(&restored, 190_000), Some(60_000));
        restored.state = WorkerState::Queued;
        restored.running_since = None;
        assert_eq!(run_time_ms(&restored, 300_000), Some(60_000));
    }

    #[test]
    fn ended_workers_stay_five_minutes_then_go_to_finished() {
        let now = 1_000_000;
        let running = worker("r1", WorkerState::Running, Some(0), None);
        let just_done = worker("r2", WorkerState::Finished, Some(0), Some(now - 60_000));
        let long_done = worker(
            "r3",
            WorkerState::Failed,
            Some(0),
            Some(now - ENDED_SHOWS_FOR_MS),
        );
        let restored = worker("r4", WorkerState::Finished, None, Some(now));
        assert_eq!(section(&running, now, true), Section::Current);
        assert_eq!(section(&just_done, now, true), Section::Current);
        assert_eq!(section(&long_done, now, true), Section::Finished);
        assert_eq!(section(&restored, now, true), Section::Finished);
        // A failure stays in sight until the user has seen it.
        assert_eq!(section(&long_done, now, false), Section::Current);
        assert_eq!(
            next_move(std::slice::from_ref(&long_done), now, |_| false),
            None
        );
        let all = [running, just_done, long_done, restored];
        assert_eq!(
            next_move(&all, now, |_| true),
            Some(now - 60_000 + ENDED_SHOWS_FOR_MS)
        );
        assert_eq!(next_move(&all[..1], now, |_| true), None);
    }

    #[test]
    fn times_read_short_and_line_up_right_aligned() {
        assert_eq!(elapsed_label(12_400), "12 s");
        assert_eq!(elapsed_label(4 * 60_000 + 59_000), "4 m");
        assert_eq!(elapsed_label(59 * 60_000), "59 m");
        assert_eq!(elapsed_label(3_600_000 + 5 * 60_000), "1 h 05 m");
        assert_eq!(elapsed_label(9 * 3_600_000 + 59 * 60_000), "9 h 59 m");
        assert_eq!(elapsed_label(-5), "0 s");
        assert_eq!(tokens_label(820), "820");
        assert_eq!(tokens_label(8_240), "8.2k");
        assert_eq!(tokens_label(9_990), "9.9k");
        assert_eq!(tokens_label(88_000), "88k");
        assert_eq!(tokens_label(1_020_000), "1.02M");
        assert_eq!(tokens_label(1_000_000), "1M");
        assert_eq!(tokens_label(1_500_000), "1.5M");
    }

    #[test]
    fn the_last_line_is_the_reports_once_finished() {
        let mut done = worker("r1", WorkerState::Finished, Some(0), Some(1));
        done.last = Some("read src/a.rs".into());
        done.report = Some("\nFixed the race in setup.\nMore.".into());
        assert_eq!(
            last_line(&done).as_deref(),
            Some("Fixed the race in setup.")
        );
        let mut running = worker("r2", WorkerState::Running, Some(0), None);
        assert_eq!(last_line(&running), None);
        running.last = Some("$ cargo test".into());
        assert_eq!(last_line(&running).as_deref(), Some("$ cargo test"));
    }
}
