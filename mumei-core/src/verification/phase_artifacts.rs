use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use z3::ast::Bool;
use z3::{SatResult, Solver};

use super::{phase_contract, PHASE_CONTRACTS};

static SINK: Mutex<Option<State>> = Mutex::new(None);

struct State {
    directory: PathBuf,
    stack: Vec<AtomTrace>,
    write_failures: usize,
    first_write_failure: Option<String>,
    #[cfg(test)]
    owner: std::thread::ThreadId,
}

struct AtomTrace {
    directory: PathBuf,
    atom: String,
    source_file: String,
    outcome: Option<String>,
    error: Option<String>,
    phases: Vec<PhaseTrace>,
    completed_contracts: usize,
    query_count: usize,
    partial: bool,
}

struct PhaseTrace {
    phase: String,
    in_phase_contract: Option<bool>,
    result: String,
    error: Option<String>,
    queries: Vec<QueryTrace>,
}

struct QueryTrace {
    index: usize,
    file: String,
    origin: String,
    result: String,
}

pub fn enable(directory: PathBuf) -> std::io::Result<()> {
    fs::create_dir_all(&directory)?;
    let mut sink = lock_sink();
    *sink = Some(State {
        directory,
        stack: Vec::new(),
        write_failures: 0,
        first_write_failure: None,
        #[cfg(test)]
        owner: std::thread::current().id(),
    });
    Ok(())
}

// Parallel unit tests share the process-global sink: the test that enables
// it runs on one thread while other tests drive the executor on theirs, so
// without a filter their atoms and queries would leak into (and renumber) the
// enabling test's artifacts. Production runs atoms sequentially on the thread
// that called `enable`, so outside tests the sink answers every caller.
fn active(sink: &mut Option<State>) -> Option<&mut State> {
    #[cfg(test)]
    {
        sink.as_mut()
            .filter(|state| state.owner == std::thread::current().id())
    }
    #[cfg(not(test))]
    {
        sink.as_mut()
    }
}

fn active_ref(sink: &Option<State>) -> Option<&State> {
    #[cfg(test)]
    {
        sink.as_ref()
            .filter(|state| state.owner == std::thread::current().id())
    }
    #[cfg(not(test))]
    {
        sink.as_ref()
    }
}

pub fn write_failures() -> Option<(usize, String)> {
    let sink = lock_sink();
    let state = active_ref(&sink)?;
    if state.write_failures == 0 {
        return None;
    }
    Some((state.write_failures, state.first_write_failure.clone()?))
}

pub(crate) fn begin_atom(source_file: &str, atom: &str) {
    let mut sink = lock_sink();
    let Some(state) = active(&mut sink) else {
        return;
    };
    let directory = state
        .directory
        .join(path_component(source_file))
        .join(path_component(atom));
    let _ = fs::remove_dir_all(&directory);
    state.stack.push(AtomTrace {
        directory,
        atom: atom.to_string(),
        source_file: source_file.to_string(),
        outcome: None,
        error: None,
        phases: Vec::new(),
        completed_contracts: 0,
        query_count: 0,
        partial: false,
    });
}

pub(crate) fn finish_atom<T, E: std::fmt::Display>(result: &Result<T, E>) {
    let mut sink = lock_sink();
    let Some(state) = active(&mut sink) else {
        return;
    };
    let Some(mut atom) = state.stack.pop() else {
        return;
    };
    match result {
        Ok(_) => atom.outcome = Some("verified".to_string()),
        Err(error) => {
            let message = first_line(&error.to_string());
            atom.outcome = Some("error".to_string());
            atom.error = Some(message.clone());
            if let Some(record) = atom
                .phases
                .iter_mut()
                .rev()
                .find(|record| record.result == "in_progress")
            {
                record.result = "aborted".to_string();
                record.error = Some(message);
            } else if atom
                .phases
                .last()
                .is_none_or(|phase| phase.result == "passed")
            {
                if let Some(phase) = PHASE_CONTRACTS.get(atom.completed_contracts) {
                    if !atom.phases.iter().any(|record| record.phase == phase.name) {
                        atom.phases.push(PhaseTrace {
                            phase: phase.name.to_string(),
                            in_phase_contract: Some(true),
                            result: "aborted".to_string(),
                            error: Some(message),
                            queries: Vec::new(),
                        });
                    }
                }
            }
        }
    }
    if let Err((path, error)) = write_atom_trace(&atom) {
        record_write_failure(state, &path, &error);
    }
}

pub(crate) fn complete_phase(name: &str) {
    let mut sink = lock_sink();
    let Some(atom) = active(&mut sink).and_then(|state| state.stack.last_mut()) else {
        return;
    };
    let contract = phase_contract(name);
    let canonical_name = contract.map_or(name, |phase| phase.name);
    let result = name
        .rsplit_once(" (")
        .and_then(|(_, outcome)| outcome.strip_suffix(')'))
        .unwrap_or("passed")
        .to_string();
    let completed_contract = contract.and_then(|contract| {
        PHASE_CONTRACTS
            .iter()
            .position(|phase| phase.name == contract.name)
            .map(|index| index + 1)
    });
    if let Some(record) = atom
        .phases
        .iter_mut()
        .find(|record| record.phase == canonical_name)
    {
        record.result = result;
        if let Some(completed_contract) = completed_contract {
            atom.completed_contracts = atom.completed_contracts.max(completed_contract);
        }
        return;
    }
    atom.phases.push(PhaseTrace {
        phase: canonical_name.to_string(),
        in_phase_contract: (!contract.is_some()).then_some(false),
        result,
        error: None,
        queries: Vec::new(),
    });
    if let Some(completed_contract) = completed_contract {
        atom.completed_contracts = atom.completed_contracts.max(completed_contract);
    }
}

pub fn record_cached_atom(source_file: &str, atom: &str) {
    let mut sink = lock_sink();
    let Some(state) = active(&mut sink) else {
        return;
    };
    let directory = state
        .directory
        .join(path_component(source_file))
        .join(path_component(atom));
    let _ = fs::remove_dir_all(&directory);
    let cached = AtomTrace {
        directory,
        atom: atom.to_string(),
        source_file: source_file.to_string(),
        outcome: Some("cached".to_string()),
        error: None,
        phases: Vec::new(),
        completed_contracts: 0,
        query_count: 0,
        partial: false,
    };
    if let Err((path, error)) = write_atom_trace(&cached) {
        record_write_failure(state, &path, &error);
    }
}

#[track_caller]
pub(crate) fn check(solver: &Solver) -> SatResult {
    let query = capture(solver);
    let result = solver.check();
    query.record(result);
    result
}

#[track_caller]
pub(crate) fn capture(solver: &Solver) -> CapturedQuery {
    capture_query(solver, &[], false)
}

#[track_caller]
fn capture_query(solver: &Solver, assumptions: &[Bool], check_assumptions: bool) -> CapturedQuery {
    let origin = std::panic::Location::caller();
    CapturedQuery(prepare_query(
        solver,
        assumptions,
        check_assumptions,
        origin.file(),
        origin.line(),
    ))
}

#[track_caller]
pub(crate) fn check_assumptions(solver: &Solver, assumptions: &[Bool]) -> SatResult {
    let query = capture_query(solver, assumptions, true);
    let result = solver.check_assumptions(assumptions);
    query.record(result);
    result
}

pub(crate) struct CapturedQuery(Option<PendingQuery>);

impl CapturedQuery {
    pub(crate) fn record(self, result: SatResult) {
        record_query_result(self.0, result);
    }
}

struct PendingQuery {
    file: Option<PathBuf>,
    phase: Option<String>,
    index: usize,
    origin: String,
}

fn prepare_query(
    solver: &Solver,
    assumptions: &[Bool],
    check_assumptions: bool,
    source: &str,
    line: u32,
) -> Option<PendingQuery> {
    let mut sink = lock_sink();
    let state = active(&mut sink)?;
    let (pending, failure) = {
        let atom = state.stack.last_mut()?;
        let phase_name = PHASE_CONTRACTS
            .get(atom.completed_contracts)
            .or_else(|| PHASE_CONTRACTS.last())?
            .name;
        let index = atom.query_count.checked_add(1)?;
        if !atom
            .phases
            .iter_mut()
            .any(|record| record.phase == phase_name)
        {
            let known = phase_contract(phase_name).is_some();
            atom.phases.push(PhaseTrace {
                phase: phase_name.to_string(),
                in_phase_contract: (!known).then_some(false),
                result: "in_progress".to_string(),
                error: None,
                queries: Vec::new(),
            });
        }
        let slug = phase_slug(phase_name);
        let file_name = format!("{index:04}-{slug}.smt2");
        let file_path = atom.directory.join(&file_name);
        let assumptions_text = assumptions
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(" ");
        let command = if check_assumptions {
            format!("(check-sat-assuming ({assumptions_text}))")
        } else {
            "(check-sat)".to_string()
        };
        let query_text = format!(
            "; atom: {}\n; phase: {}\n; origin: {}:{}\n{}\n{}\n",
            atom.atom, phase_name, source, line, solver, command
        );
        let write_result = match fs::create_dir_all(&atom.directory) {
            Ok(()) => fs::write(&file_path, query_text).map_err(|error| (file_path.clone(), error)),
            Err(error) => Err((atom.directory.clone(), error)),
        };
        match write_result {
            Ok(()) => {
                atom.query_count = index;
                (
                    Some(PendingQuery {
                        file: Some(file_path),
                        phase: Some(phase_name.to_string()),
                        index,
                        origin: format!("{source}:{line}"),
                    }),
                    None,
                )
            }
            Err((path, error)) => {
                atom.partial = true;
                (None, Some((path, error)))
            }
        }
    };
    if let Some((path, error)) = failure {
        record_write_failure(state, &path, &error);
    }
    pending
}

fn record_query_result(query: Option<PendingQuery>, result: SatResult) {
    let Some(query) = query else {
        return;
    };
    let mut sink = lock_sink();
    let Some(atom) = active(&mut sink).and_then(|state| state.stack.last_mut()) else {
        return;
    };
    let (Some(file), Some(phase)) = (query.file, query.phase) else {
        return;
    };
    let Some(phase_record) = atom.phases.iter_mut().find(|record| record.phase == phase) else {
        return;
    };
    phase_record.queries.push(QueryTrace {
        index: query.index,
        file: file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string(),
        origin: query.origin,
        result: sat_result_name(result).to_string(),
    });
}

fn sat_result_name(result: SatResult) -> &'static str {
    match result {
        SatResult::Sat => "sat",
        SatResult::Unsat => "unsat",
        SatResult::Unknown => "unknown",
    }
}

fn write_atom_trace(atom: &AtomTrace) -> Result<(), (PathBuf, std::io::Error)> {
    if let Err(error) = fs::create_dir_all(&atom.directory) {
        return Err((atom.directory.clone(), error));
    }
    let phases = atom
        .phases
        .iter()
        .map(|phase| {
            let mut value = serde_json::json!({
                "phase": phase.phase,
                "result": phase.result,
                "queries": phase.queries.iter().map(|query| serde_json::json!({
                    "index": query.index,
                    "file": query.file,
                    "origin": query.origin,
                    "result": query.result,
                })).collect::<Vec<_>>(),
            });
            if let Some(in_phase_contract) = phase.in_phase_contract {
                value["in_phase_contract"] = serde_json::json!(in_phase_contract);
            }
            if let Some(error) = &phase.error {
                value["error"] = serde_json::json!(error);
            }
            value
        })
        .collect::<Vec<_>>();
    let document = serde_json::json!({
        "version": 1,
        "atom": atom.atom,
        "source_file": atom.source_file,
        "outcome": atom.outcome.as_deref().unwrap_or("error"),
        "error": atom.error,
        "partial": atom.partial,
        "phases": phases,
    });
    let path = atom.directory.join("phases.json");
    fs::write(
        &path,
        serde_json::to_vec_pretty(&document).unwrap_or_default(),
    )
    .map_err(|error| (path, error))
}

fn record_write_failure(state: &mut State, path: &std::path::Path, error: &std::io::Error) {
    state.write_failures += 1;
    if state.first_write_failure.is_none() {
        state.first_write_failure = Some(format!("{}: {error}", path.display()));
    }
}

fn sanitize(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.is_empty() {
        "_".to_string()
    } else {
        sanitized
    }
}

fn path_component(value: &str) -> String {
    let hash = format!("{:x}", Sha256::digest(value.as_bytes()));
    format!("{}-{}", sanitize(value), &hash[..8])
}

fn phase_slug(phase: &str) -> String {
    let mut slug = String::new();
    let mut previous_separator = false;
    for character in phase.chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            slug.push(character);
            previous_separator = false;
        } else if !previous_separator && !slug.is_empty() {
            slug.push('-');
            previous_separator = true;
        }
    }
    slug.trim_matches('-').to_string()
}

fn first_line(message: &str) -> String {
    message
        .lines()
        .next()
        .unwrap_or("verification failed")
        .to_string()
}

fn lock_sink() -> std::sync::MutexGuard<'static, Option<State>> {
    SINK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
fn disable() {
    *lock_sink() = None;
}

#[cfg(test)]
mod tests {
    use super::{
        begin_atom, capture, complete_phase, disable, enable, finish_atom, path_component,
        phase_slug, sanitize, write_failures,
    };
    use serde_json::Value;
    use std::fs;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    use z3::ast::Bool;
    use z3::{Config, Context, Solver};

    // Tests that call `enable` overwrite the shared sink, so they must not
    // overlap with each other.
    static ENABLE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn artifact_path_components_are_sanitized_and_distinct() {
        assert_eq!(sanitize("S::m"), "S__m");
        assert_eq!(sanitize(""), "_");
        for (value, prefix) in [("S::m", "S__m"), ("", "_")] {
            let component = path_component(value);
            let hash = component.strip_prefix(&format!("{prefix}-")).unwrap();
            assert_eq!(hash.len(), 8);
            assert!(hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));
        }
        assert_ne!(path_component("S::m"), path_component("S__m"));
        assert_eq!(
            phase_slug("Phase 5: ensures verification"),
            "phase-5-ensures-verification"
        );
    }

    #[test]
    fn errors_without_an_active_phase_abort_the_inferred_phase() {
        let _guard = ENABLE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "mumei-phase-artifacts-state-{}-{unique}",
            std::process::id()
        ));
        enable(directory.clone()).unwrap();

        begin_atom("source.mm", "captured_query");
        let context = Context::new(&Config::new());
        let solver = Solver::new(&context);
        solver.assert(&Bool::from_bool(&context, true));
        let query = capture(&solver);
        let query_path = directory
            .join(path_component("source.mm"))
            .join(path_component("captured_query"))
            .join("0001-phase-0-units-unit-consistency.smt2");
        let query_text = fs::read_to_string(&query_path).unwrap();
        assert!(query_text.contains("(assert true)"));
        assert!(query_text.ends_with("(check-sat)\n"));
        let result = solver.check();
        query.record(result);
        complete_phase("Phase 0-units: unit consistency");
        let verified: Result<(), &str> = Ok(());
        finish_atom(&verified);
        let captured: Value = serde_json::from_slice(
            &fs::read(
                directory
                    .join(path_component("source.mm"))
                    .join(path_component("captured_query"))
                    .join("phases.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(captured["partial"], false);
        assert_eq!(captured["phases"][0]["queries"][0]["result"], "sat");

        begin_atom("source.mm", "early_error");
        let early_error: Result<(), &str> = Err("early failure");
        finish_atom(&early_error);
        let early: Value = serde_json::from_slice(
            &fs::read(
                directory
                    .join(path_component("source.mm"))
                    .join(path_component("early_error"))
                    .join("phases.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            early["phases"][0]["phase"],
            "Phase 0-units: unit consistency"
        );
        assert_eq!(early["phases"][0]["result"], "aborted");

        begin_atom("source.mm", "after_body");
        complete_phase("Phase 4: body evaluation");
        let body_error: Result<(), &str> = Err("ensures failed");
        finish_atom(&body_error);
        let after_body: Value = serde_json::from_slice(
            &fs::read(
                directory
                    .join(path_component("source.mm"))
                    .join(path_component("after_body"))
                    .join("phases.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(after_body["phases"][0]["phase"], "Phase 4: body evaluation");
        assert_eq!(after_body["phases"][0]["result"], "passed");
        assert_eq!(
            after_body["phases"][1]["phase"],
            "Phase 5: ensures verification"
        );
        assert_eq!(after_body["phases"][1]["result"], "aborted");

        begin_atom("blocked.mm", "failed_write");
        let blocked_query_path = directory
            .join(path_component("blocked.mm"))
            .join(path_component("failed_write"))
            .join("0001-phase-0-units-unit-consistency.smt2");
        fs::create_dir_all(&blocked_query_path).unwrap();
        let query = capture(&solver);
        query.record(solver.check());
        complete_phase("Phase 0-units: unit consistency");
        finish_atom(&verified);
        let partial: Value = serde_json::from_slice(
            &fs::read(
                directory
                    .join(path_component("blocked.mm"))
                    .join(path_component("failed_write"))
                    .join("phases.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(partial["partial"], true);
        assert_eq!(partial["phases"][0]["queries"], serde_json::json!([]));
        let (count, first) = write_failures().unwrap();
        assert!(count >= 1);
        assert!(first.contains(&blocked_query_path.display().to_string()));

        let verified: Result<(), &str> = Ok(());
        begin_atom("a/b.mm", "shared");
        finish_atom(&verified);
        begin_atom("a_b.mm", "shared");
        finish_atom(&verified);
        let first_directory = directory
            .join(path_component("a/b.mm"))
            .join(path_component("shared"));
        let second_directory = directory
            .join(path_component("a_b.mm"))
            .join(path_component("shared"));
        assert_ne!(first_directory, second_directory);
        assert!(first_directory.join("phases.json").exists());
        assert!(second_directory.join("phases.json").exists());

        let preserved_directory = directory.join("preserved_after_enable_failure");
        enable(preserved_directory.clone()).unwrap();
        let regular_file_parent = directory.join("regular_file_parent");
        fs::write(&regular_file_parent, "not a directory").unwrap();
        assert!(enable(regular_file_parent.join("artifacts")).is_err());
        assert_eq!(write_failures(), None);
        begin_atom("source.mm", "preserved");
        finish_atom(&verified);
        assert!(preserved_directory
            .join(path_component("source.mm"))
            .join(path_component("preserved"))
            .join("phases.json")
            .exists());

        disable();
        fs::remove_file(regular_file_parent).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn sink_ignores_atoms_and_queries_from_other_threads() {
        let _guard = ENABLE_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "mumei-phase-artifacts-thread-{}-{unique}",
            std::process::id()
        ));
        enable(directory.clone()).unwrap();

        begin_atom("owner.mm", "owned");
        std::thread::spawn(|| {
            begin_atom("intruder.mm", "intruder");
            let context = Context::new(&Config::new());
            let solver = Solver::new(&context);
            solver.assert(&Bool::from_bool(&context, true));
            let query = capture(&solver);
            query.record(solver.check());
            let intruder_ok: Result<(), &str> = Ok(());
            finish_atom(&intruder_ok);
        })
        .join()
        .unwrap();

        let verified: Result<(), &str> = Ok(());
        finish_atom(&verified);
        assert!(!directory.join(path_component("intruder.mm")).exists());
        assert!(directory
            .join(path_component("owner.mm"))
            .join(path_component("owned"))
            .join("phases.json")
            .exists());

        disable();
        fs::remove_dir_all(directory).unwrap();
    }
}
