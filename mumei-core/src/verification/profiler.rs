use serde::{Deserialize, Serialize};
use std::cmp::Reverse;
use std::time::Instant;
use z3::{Context, Solver, Statistics, StatisticsValue};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConstraintProfile {
    pub constraint_id: String,
    pub rlimit_consumed: u64,
    pub time_ms: u64,
    pub source_location: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolverHeatmap {
    pub atom_name: String,
    pub total_time_ms: u64,
    pub total_rlimit: u64,
    pub constraints: Vec<ConstraintProfile>,
    pub timeout_reason: String,
}

pub struct IncrementalProfiler<'a> {
    solver: &'a Solver<'a>,
    _ctx: &'a Context,
    constraints: Vec<ConstraintProfile>,
    base_rlimit: u64,
    last_rlimit: u64,
    started_at: Instant,
}

impl<'a> IncrementalProfiler<'a> {
    pub fn new(solver: &'a Solver<'a>, ctx: &'a Context) -> Self {
        let base_rlimit = extract_rlimit_from_stats(&solver.get_statistics());
        Self {
            solver,
            _ctx: ctx,
            constraints: Vec::new(),
            base_rlimit,
            last_rlimit: base_rlimit,
            started_at: Instant::now(),
        }
    }

    pub fn record_assertion(&mut self, constraint_id: &str, source_location: Option<String>) {
        let before = self.last_rlimit;
        let before_time = self.started_at.elapsed();
        let after = self.get_current_rlimit();
        let consumed = after.saturating_sub(before);
        self.last_rlimit = after;

        self.constraints.push(ConstraintProfile {
            constraint_id: constraint_id.to_string(),
            rlimit_consumed: consumed,
            time_ms: self
                .started_at
                .elapsed()
                .saturating_sub(before_time)
                .as_millis() as u64,
            source_location,
        });
    }

    pub fn profile_assertion(&mut self, constraint_id: &str, source_location: Option<String>) {
        self.record_assertion(constraint_id, source_location);
    }

    pub fn begin_check(&mut self) -> usize {
        self.constraints.len()
    }

    pub fn end_check(&mut self, start_index: usize) {
        let after = self.get_current_rlimit();
        let consumed = after.saturating_sub(self.last_rlimit);
        self.last_rlimit = after;

        if consumed == 0 || start_index >= self.constraints.len() {
            return;
        }

        let share = consumed / (self.constraints.len() - start_index) as u64;
        let mut remainder = consumed % (self.constraints.len() - start_index) as u64;
        for constraint in self.constraints.iter_mut().skip(start_index) {
            constraint.rlimit_consumed = constraint.rlimit_consumed.saturating_add(share);
            if remainder > 0 {
                constraint.rlimit_consumed = constraint.rlimit_consumed.saturating_add(1);
                remainder -= 1;
            }
        }
    }

    fn get_current_rlimit(&self) -> u64 {
        extract_rlimit_from_stats(&self.solver.get_statistics())
    }

    pub fn build_heatmap(&self, atom_name: &str, timeout_reason: &str) -> SolverHeatmap {
        let current_rlimit = self.get_current_rlimit();
        let total_rlimit = current_rlimit
            .saturating_sub(self.base_rlimit)
            .max(self.constraints.iter().map(|c| c.rlimit_consumed).sum());

        SolverHeatmap {
            atom_name: atom_name.to_string(),
            total_time_ms: self.started_at.elapsed().as_millis() as u64,
            total_rlimit,
            constraints: self.constraints.clone(),
            timeout_reason: timeout_reason.to_string(),
        }
    }
}

fn extract_rlimit_from_stats(stats: &Statistics<'_>) -> u64 {
    for key in ["rlimit count", ":rlimit-count", "rlimit-count", "rlimit"] {
        if let Some(value) = stats.value(key) {
            return statistics_value_to_u64(value);
        }
    }

    stats
        .entries()
        .find(|entry| entry.key.contains("rlimit"))
        .map(|entry| statistics_value_to_u64(entry.value))
        .unwrap_or(0)
}

fn statistics_value_to_u64(value: StatisticsValue) -> u64 {
    match value {
        StatisticsValue::UInt(value) => u64::from(value),
        StatisticsValue::Double(value) if value.is_finite() && value >= 0.0 => value as u64,
        StatisticsValue::Double(_) => 0,
    }
}

pub(crate) fn top_consumers_summary(constraints: &[ConstraintProfile], top_n: usize) -> String {
    let mut sorted: Vec<_> = constraints.iter().collect();
    sorted.sort_by_key(|constraint| Reverse(constraint.rlimit_consumed));

    let summary = sorted
        .iter()
        .take(top_n)
        .map(|constraint| {
            format!(
                "{} ({} rlimit)",
                constraint.constraint_id, constraint.rlimit_consumed
            )
        })
        .collect::<Vec<_>>()
        .join(", ");

    if summary.is_empty() {
        "none".to_string()
    } else {
        summary
    }
}
