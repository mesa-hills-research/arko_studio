//! Where a list shows each worker and how its figures read.

use arko::ModelChoice;
use arko::remote::{WorkerState, WorkerSummary};
use arko::workers::{MODELS_ABOUT, MODELS_APPLIES};

/// A worker that ended stays among the others this long, then moves to the
/// "Finished" group at the bottom of the list.
pub const ENDED_SHOWS_FOR_MS: i64 = 5 * 60 * 1000;

/// Arko's words in the app's typography: the apostrophes it types straight
/// are typographic here.
pub fn curly(text: &str) -> String {
    text.replace('\'', "’")
}

/// What the "Worker depth" setting does, in Arko's words.
pub fn depth_about() -> String {
    curly(arko::workers::depth_about())
}

/// What a worker depth of `depth` allows now with the models `choice`, in
/// Arko's words. Workers on the session's model start none, whatever the
/// depth.
pub fn depth_effect(depth: u32, choice: ModelChoice) -> String {
    curly(&arko::workers::depth_effect(depth, choice))
}

/// What the "Worker retry cap" setting does, with it `on` or off, in Arko's
/// words.
pub fn retry_cap_about(on: bool) -> String {
    curly(&arko::workers::retry_cap_about(
        on.then_some(arko::workers::DEFAULT_RETRY_CAP),
    ))
}

/// What the "Message copies" setting does, with it `on` or off, in Arko's
/// words.
pub fn copies_about(on: bool) -> String {
    curly(arko::workers::copies_about(on))
}

/// What the "Models for workers" setting does and what `choice` means,
/// in Arko's words.
pub fn models_about(choice: ModelChoice) -> String {
    curly(&format!(
        "{MODELS_ABOUT} {} {MODELS_APPLIES}",
        choice.about()
    ))
}

/// Whether `question` is the main agent's request to save a worker profile,
/// as `arko::profiles::approval_text` words it. It puts each fact on a
/// line of its own and shows the instructions as written, which Markdown
/// would run together and format, so the question card shows it as plain
/// text.
pub fn is_profile_approval(question: &str) -> bool {
    question.starts_with("The main agent wants to save the worker profile ")
}

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

/// A worker's setup in a few words, as the terminal lists it: its model in as
/// few characters as still name it, its effort and its profile, such as
/// "sol · high · 3d-judge". `None` until the session has recorded them.
pub fn setup_label(worker: &WorkerSummary) -> Option<String> {
    let known = |text: &Option<String>| text.clone().filter(|text| !text.is_empty());
    let mut parts: Vec<String> = Vec::new();
    parts.extend(known(&worker.model).map(|model| arko::workers::short_model_name(&model)));
    parts.extend(known(&worker.effort));
    parts.extend(known(&worker.profile));
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// What a worker's model calls cost, as the status bar's figure reads: what
/// it billed, else (work on a plan) what it would cost at API prices, "≈ $0.12".
/// `None` before anything cost.
pub fn cost_label(worker: &WorkerSummary) -> Option<String> {
    if worker.cost_billed >= 0.005 {
        Some(crate::figures::dollars(worker.cost_billed))
    } else if worker.cost_list >= 0.005 {
        Some(format!("≈ {}", crate::figures::dollars(worker.cost_list)))
    } else {
        None
    }
}

/// What a worker and the workers under it cost, as Arko words it: "$0.42 with
/// its workers", or "≈ $0.42 with its workers" where a plan covered calls. It
/// shows once they cost more than the worker alone, so a worker with none
/// under it (or none that cost anything) has no such figure.
pub fn family_cost_label(worker: &WorkerSummary) -> Option<String> {
    let (billed, list) = (worker.family_cost_billed, worker.family_cost_list);
    if list < worker.cost_list + 0.005 {
        return None;
    }
    let figure = if list > billed + 0.005 {
        format!("≈ {}", crate::figures::dollars(list))
    } else {
        crate::figures::dollars(billed)
    };
    Some(format!("{figure} with its workers"))
}

/// The figures of a worker's row: what it cost, then what it cost with its
/// workers.
pub fn costs_label(worker: &WorkerSummary) -> Option<String> {
    let parts: Vec<String> = [cost_label(worker), family_cost_label(worker)]
        .into_iter()
        .flatten()
        .collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The workers that run, as the dialog that asks before stopping them lists
/// them. Each worker is followed by the workers it started, which say whose
/// they are, as in "3 workers (r1, r2 under r1, r3)". `running` is each
/// worker's ID and the ID of the worker that started it.
pub fn running_text(running: &[(String, Option<String>)]) -> String {
    let summaries: Vec<WorkerSummary> = running
        .iter()
        .map(|(id, parent)| {
            let mut worker = WorkerSummary::new(id);
            worker.parent = parent.clone();
            worker
        })
        .collect();
    let names: Vec<String> = arko::remote::family_order(&summaries)
        .into_iter()
        .map(|index| match &running[index] {
            (id, Some(parent)) => format!("{id} under {parent}"),
            (id, None) => id.clone(),
        })
        .collect();
    let count = names.len();
    format!(
        "{count} {} ({})",
        if count == 1 { "worker" } else { "workers" },
        names.join(", ")
    )
}

/// A worker's row as a screen reader names it: the worker, whose it is when
/// another worker started it (as Arko's notices say it), its task and its
/// state, such as "r3 (under r1), Fix the tokenizer tests, Running".
pub fn row_label(worker: &WorkerSummary) -> String {
    let mut label = worker.id.clone();
    if let Some(parent) = &worker.parent {
        label.push_str(&format!(" (under {parent})"));
    }
    if !worker.title.is_empty() {
        label.push_str(&format!(", {}", worker.title));
    }
    label.push_str(&format!(", {}", state_label(worker.state)));
    label
}

/// What Stop in a worker's view says it does. Stopping a worker stops the
/// workers it started, which `with_workers` says it has.
pub fn stop_tip(with_workers: bool) -> &'static str {
    if with_workers {
        "Stop this worker and its workers (Arko can resume them)"
    } else {
        "Stop this worker (Arko can resume it)"
    }
}

/// Where a worker stands in a list that keeps a worker's workers under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    /// The worker's place in the list given.
    pub index: usize,
    /// How many workers it is under: 0 for a worker the main agent started.
    pub level: usize,
    /// The place of the worker its family starts from.
    pub root: usize,
}

/// The workers in the order a list shows them, which is Arko's own
/// ([`arko::remote::family_order`]): each worker followed by the workers it
/// started, depth first, in the order they started. A worker whose parent
/// isn't in the list starts a family of its own.
pub fn tree(workers: &[WorkerSummary]) -> Vec<Placed> {
    let index_of: std::collections::HashMap<&str, usize> = workers
        .iter()
        .enumerate()
        .map(|(index, worker)| (worker.id.as_str(), index))
        .collect();
    // Each worker's level and family, set when it is reached: its parent
    // comes before it, unless the parents form a loop, which starts a family.
    let mut reached: Vec<Option<(usize, usize)>> = vec![None; workers.len()];
    arko::remote::family_order(workers)
        .into_iter()
        .map(|index| {
            let above = workers[index]
                .parent
                .as_deref()
                .and_then(|parent| index_of.get(parent).copied())
                .filter(|parent| *parent != index)
                .and_then(|parent| reached[parent]);
            let (level, root) = match above {
                Some((level, root)) => (level + 1, root),
                None => (0, index),
            };
            reached[index] = Some((level, root));
            Placed { index, level, root }
        })
        .collect()
}

/// Where each worker (by its place in `workers`) shows at `now`. A family
/// moves to the "Finished" group together, when all its workers would, so
/// a worker that finished doesn't sit alone below a parent that still
/// shows. `seen` says which failures the user has opened.
pub fn family_sections(
    workers: &[WorkerSummary],
    tree: &[Placed],
    now: i64,
    seen: impl Fn(&str) -> bool,
) -> Vec<Section> {
    let mut current = vec![false; workers.len()];
    for placed in tree {
        let worker = &workers[placed.index];
        if section(worker, now, seen(&worker.id)) == Section::Current {
            current[placed.root] = true;
        }
    }
    let mut sections = vec![Section::Finished; workers.len()];
    for placed in tree {
        if current[placed.root] {
            sections[placed.index] = Section::Current;
        }
    }
    sections
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

/// Why a worker failed, whole, when it did and Arko says.
pub fn failure(worker: &WorkerSummary) -> Option<&str> {
    (worker.state == WorkerState::Failed)
        .then_some(worker.error.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|error| !error.is_empty())
}

/// What a worker did last, in a line: its report's first line once it
/// finished, the first line of its error once it failed (the summary's
/// `last` cuts the error short), else its latest call or why it stopped.
pub fn last_line(worker: &WorkerSummary) -> Option<String> {
    fn first(text: &str) -> Option<&str> {
        text.lines().find(|line| !line.trim().is_empty())
    }
    let report = (worker.state == WorkerState::Finished)
        .then_some(worker.report.as_deref())
        .flatten()
        .and_then(first);
    let error = failure(worker).and_then(first);
    report
        .or(error)
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

    /// The question the agent's save of a profile asks is recognized in all
    /// three places, and an ordinary question isn't.
    #[test]
    fn the_agents_request_to_save_a_profile_is_recognized() {
        use arko::profiles::{Place, Profile};
        let mut profile = Profile::new("3d-judge", "Judges 3D results");
        profile.instructions = "Compare the render.\nBe brief.".into();
        for place in Place::ALL {
            let question = arko::profiles::approval_text(&profile, place);
            assert!(is_profile_approval(&question), "{question}");
        }
        assert!(!is_profile_approval("Tabs or spaces?"));
        assert!(!is_profile_approval("Shall I save the worker profile?"));
    }

    #[test]
    fn the_models_setting_reads_in_arkos_words() {
        for choice in ModelChoice::ALL {
            let about = models_about(choice);
            assert!(about.starts_with("Who picks a worker’s model. "), "{about}");
            assert!(about.ends_with("A change applies to the next worker started."));
            assert!(!about.contains('\''), "{about}");
        }
        assert!(
            models_about(ModelChoice::Session)
                .contains("Every worker runs the session’s worker model, and none starts workers.")
        );
        assert_eq!(curly("Session's worker model"), "Session’s worker model");
    }

    /// The depth setting says what it does as soon as it is set, with no
    /// word about taking effect later, and nothing is left in Arko's straight
    /// apostrophes.
    #[test]
    fn the_depth_setting_reads_in_arkos_words() {
        let about = depth_about();
        assert!(about.starts_with("How many levels of workers there can be, from 1 to 4."));
        assert!(
            about.contains("a worker can start workers of its own"),
            "{about}"
        );
        let agent = ModelChoice::Agent;
        assert_eq!(
            depth_effect(1, agent),
            "Only the main agent starts workers."
        );
        let three = depth_effect(3, agent);
        assert_eq!(
            three,
            "Workers can go 3 levels deep, and a worker can start workers of its own when the agent that starts it says so."
        );
        // Every worker on the session's model starts none, so the depth
        // row can't promise nesting beside a models row that rules it out.
        let session = depth_effect(3, ModelChoice::Session);
        assert_eq!(
            session,
            "No worker starts workers, because every worker runs the session’s worker model."
        );
        for text in [
            about,
            depth_effect(2, agent),
            three,
            depth_effect(1, agent),
            session,
        ] {
            assert!(!text.contains('\''), "{text}");
            assert!(!text.contains("takes effect"), "{text}");
        }
    }

    /// The dialog that asks before stopping lists each running worker with
    /// the workers it started after it, and says whose a worker is.
    #[test]
    fn the_stop_dialog_lists_running_workers_by_family() {
        let list = |workers: &[(&str, Option<&str>)]| {
            let workers: Vec<_> = workers
                .iter()
                .map(|(id, parent)| (id.to_string(), parent.map(str::to_string)))
                .collect();
            running_text(&workers)
        };
        assert_eq!(list(&[("r1", None)]), "1 worker (r1)");
        assert_eq!(
            list(&[
                ("r1", None),
                ("r2", None),
                ("r3", Some("r1")),
                ("r4", Some("r3"))
            ]),
            "4 workers (r1, r3 under r1, r4 under r3, r2)"
        );
        // A worker whose starter isn't running still says whose it is.
        assert_eq!(list(&[("r5", Some("r2"))]), "1 worker (r5 under r2)");
    }

    /// The message copies setting says what it does with it on or off, in
    /// Arko's words with the app's apostrophes.
    #[test]
    fn the_message_copies_setting_reads_in_arkos_words() {
        use arko::workers::{COPIES_NAME, copies_command};
        assert_eq!(COPIES_NAME, "Message copies");
        assert!(copies_about(true).starts_with("The agent that started a worker gets a copy"));
        assert!(copies_about(false).contains("doesn’t see the messages you send"));
        for on in [true, false] {
            assert!(!copies_about(on).contains('\''));
            assert!(arko::commands::find(&copies_command(on)).is_some());
        }
        assert_eq!(copies_command(false), "/message-copies off");
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

    /// The model's short name, the effort and the profile, as the terminal
    /// lists them.
    #[test]
    fn a_workers_setup_reads_in_a_few_words() {
        let mut worker = worker("r1", WorkerState::Running, Some(0), None);
        assert_eq!(setup_label(&worker), None);
        worker.model = Some("gpt-6.1-sol".into());
        assert_eq!(setup_label(&worker).as_deref(), Some("sol"));
        worker.effort = Some("high".into());
        worker.profile = Some("3d-judge".into());
        assert_eq!(
            setup_label(&worker).as_deref(),
            Some("sol · high · 3d-judge")
        );
        worker.model = Some("cc:claude-opus-5-5".into());
        worker.effort = None;
        worker.profile = None;
        assert_eq!(setup_label(&worker).as_deref(), Some("cc:opus"));
        // A model no alias names stays as it is, cut when it is long.
        worker.model = Some("openrouter:vendor/some-model-with-a-very-long-slug".into());
        let label = setup_label(&worker).unwrap();
        assert!(
            label.ends_with('…') && label.chars().count() <= 28,
            "{label}"
        );
        worker.model = None;
        worker.profile = Some("reviewer".into());
        assert_eq!(setup_label(&worker).as_deref(), Some("reviewer"));
    }

    #[test]
    fn a_workers_cost_is_what_it_billed_else_the_api_price() {
        let mut worker = worker("r1", WorkerState::Running, Some(0), None);
        assert_eq!(cost_label(&worker), None);
        worker.cost_billed = 0.0004;
        worker.cost_list = 0.003;
        assert_eq!(cost_label(&worker), None, "under half a cent");
        worker.cost_list = 0.12;
        assert_eq!(cost_label(&worker).as_deref(), Some("≈ $0.12"));
        worker.cost_billed = 0.34;
        assert_eq!(cost_label(&worker).as_deref(), Some("$0.34"));
    }

    #[test]
    fn stopping_a_worker_says_it_stops_the_workers_it_started() {
        assert_eq!(stop_tip(false), "Stop this worker (Arko can resume it)");
        assert!(stop_tip(true).starts_with("Stop this worker and its workers"));
    }

    /// A family's cost reads as Arko words it, once it is more than the
    /// worker's own.
    #[test]
    fn a_family_cost_reads_as_arkos_cards_do() {
        let mut worker = worker("r1", WorkerState::Running, Some(0), None);
        assert_eq!(family_cost_label(&worker), None);
        worker.cost_billed = 0.12;
        worker.cost_list = 0.12;
        worker.family_cost_billed = 0.12;
        worker.family_cost_list = 0.12;
        assert_eq!(
            family_cost_label(&worker),
            None,
            "nothing more than its own"
        );
        worker.family_cost_billed = 0.42;
        worker.family_cost_list = 0.42;
        assert_eq!(
            family_cost_label(&worker).as_deref(),
            Some("$0.42 with its workers")
        );
        assert_eq!(
            costs_label(&worker).as_deref(),
            Some("$0.12 · $0.42 with its workers")
        );
        // Calls a plan covered cost nothing billed, and show at API prices.
        worker.family_cost_billed = 0.12;
        worker.family_cost_list = 0.9;
        assert_eq!(
            family_cost_label(&worker).as_deref(),
            Some("≈ $0.90 with its workers")
        );
        // A worker that cost nothing itself still shows what its workers did.
        worker.cost_billed = 0.;
        worker.cost_list = 0.;
        assert_eq!(
            costs_label(&worker).as_deref(),
            Some("≈ $0.90 with its workers")
        );
    }

    fn child(id: &str, parent: &str) -> WorkerSummary {
        let mut child = worker(id, WorkerState::Running, Some(0), None);
        child.parent = Some(parent.into());
        child
    }

    /// The workers of a worker follow it, in the order they were given, and
    /// the families keep the order of their first workers.
    #[test]
    fn workers_stand_under_the_worker_that_started_them() {
        let workers = [
            worker("r1", WorkerState::Running, Some(0), None),
            worker("r2", WorkerState::Running, Some(0), None),
            child("r3", "r1"),
            child("r4", "r1"),
            child("r5", "r3"),
            child("r6", "r2"),
        ];
        let at = |placed: &[Placed]| -> Vec<(usize, usize)> {
            placed
                .iter()
                .map(|placed| (placed.index, placed.level))
                .collect()
        };
        let placed = tree(&workers);
        // r1, r3, r5, r4, r2, r6.
        assert_eq!(
            at(&placed),
            [(0, 0), (2, 1), (4, 2), (3, 1), (1, 0), (5, 1)]
        );
        assert_eq!(
            placed.iter().map(|placed| placed.root).collect::<Vec<_>>(),
            [0, 0, 0, 0, 1, 1]
        );
        // Without parents, the order given.
        let flat: Vec<WorkerSummary> = (1..=3)
            .map(|n| worker(&format!("r{n}"), WorkerState::Running, Some(0), None))
            .collect();
        assert_eq!(at(&tree(&flat)), [(0, 0), (1, 0), (2, 0)]);
    }

    /// The list follows Arko's own order of a family, in the arrangements
    /// that make it matter: a family that started interleaved with another,
    /// a worker whose parent comes later in the list, a parent that isn't
    /// listed and parents in a loop.
    #[test]
    fn the_list_goes_by_arkos_order_of_a_family() {
        let arrangements: [Vec<WorkerSummary>; 4] = [
            vec![
                worker("r1", WorkerState::Running, Some(0), None),
                worker("r2", WorkerState::Running, Some(0), None),
                child("r3", "r1"),
                child("r4", "r2"),
                child("r5", "r1"),
                child("r6", "r5"),
            ],
            // A worker listed before the worker that started it.
            vec![
                child("r2", "r1"),
                worker("r1", WorkerState::Running, Some(0), None),
            ],
            vec![child("r1", "r9"), child("r2", "r1"), child("r3", "r2")],
            vec![child("r1", "r2"), child("r2", "r1"), child("r3", "r2")],
        ];
        for workers in arrangements {
            let ours: Vec<usize> = tree(&workers).iter().map(|placed| placed.index).collect();
            assert_eq!(ours, arko::remote::family_order(&workers), "{workers:?}");
        }
    }

    /// A worker whose parent isn't listed, one that is its own parent and
    /// two that are each other's still show, once each.
    #[test]
    fn a_parent_that_is_missing_or_a_loop_loses_no_worker() {
        let workers = [
            child("r1", "r9"),
            child("r2", "r2"),
            child("r3", "r4"),
            child("r4", "r3"),
        ];
        let placed = tree(&workers);
        let mut indexes: Vec<usize> = placed.iter().map(|placed| placed.index).collect();
        indexes.sort();
        assert_eq!(indexes, [0, 1, 2, 3]);
        assert_eq!(placed[0].level, 0);
        assert_eq!(placed[1].level, 0);
    }

    /// A family moves to "Finished" together: a parent and its worker that
    /// ended a while ago go, but not while any of them still shows.
    #[test]
    fn a_family_moves_to_finished_together() {
        let now = 10_000_000;
        let long_ago = now - ENDED_SHOWS_FOR_MS - 1;
        let ended = |id: &str, parent: Option<&str>, at: i64| {
            let mut worker = worker(id, WorkerState::Finished, Some(0), Some(at));
            worker.parent = parent.map(str::to_string);
            worker
        };
        let sections = |workers: &[WorkerSummary], seen: bool| {
            family_sections(workers, &tree(workers), now, |_| seen)
        };
        // All ended long ago.
        let family = [
            ended("r1", None, long_ago),
            ended("r2", Some("r1"), long_ago - 1_000),
        ];
        assert_eq!(sections(&family, true), [Section::Finished; 2]);
        // A worker that ended just now keeps its family in sight.
        let family = [
            ended("r1", None, long_ago),
            ended("r2", Some("r1"), now - 1_000),
        ];
        assert_eq!(sections(&family, true), [Section::Current; 2]);
        // So does one still running, and an unseen failure.
        let mut running = [
            ended("r1", None, long_ago),
            worker("r2", WorkerState::Running, Some(0), None),
        ];
        running[1].parent = Some("r1".into());
        assert_eq!(sections(&running, true), [Section::Current; 2]);
        let mut failed = [
            ended("r1", None, long_ago),
            ended("r2", Some("r1"), long_ago),
        ];
        failed[1].state = WorkerState::Failed;
        assert_eq!(sections(&failed, false), [Section::Current; 2]);
        assert_eq!(sections(&failed, true), [Section::Finished; 2]);
        // Another family is its own.
        let two = [ended("r1", None, long_ago), ended("r2", None, now - 1_000)];
        assert_eq!(sections(&two, true), [Section::Finished, Section::Current]);
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

    /// A worker that failed shows the first line of its whole error, not
    /// the summary's `last`, which Arko cuts short.
    #[test]
    fn the_last_line_of_a_failed_worker_is_its_error() {
        let mut failed = worker("r1", WorkerState::Failed, Some(0), Some(1));
        failed.last = Some("the model failed after 3 tr".into());
        assert_eq!(
            last_line(&failed).as_deref(),
            Some("the model failed after 3 tr")
        );
        failed.error = Some("\nthe model failed after 3 tries\nThe last was a 429.".into());
        assert_eq!(
            last_line(&failed).as_deref(),
            Some("the model failed after 3 tries")
        );
        assert_eq!(
            failure(&failed),
            Some("the model failed after 3 tries\nThe last was a 429.")
        );
        // An error left over from a run that went on isn't shown.
        failed.state = WorkerState::Running;
        assert_eq!(failure(&failed), None);
    }
}
