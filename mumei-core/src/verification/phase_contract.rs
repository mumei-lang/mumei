//! Describes verified data flow between verification phases.
//!
//! Facts are declared only where data actually flows between phases; the
//! static checks in the phase table are independent.

use std::collections::HashSet;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PhaseFact {
    SolverContext,
    BodyResult,
    BodyResultValue,
    EnsuresOutcomes,
    ContextReachability,
    CoverResults,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterexampleFidelity {
    Exact,
    Bounded,
    Approximate,
}

#[derive(Debug, Clone, Copy)]
pub struct PhaseContract {
    pub name: &'static str,
    pub requires: &'static [PhaseFact],
    pub establishes: &'static [PhaseFact],
    pub invalidates: &'static [PhaseFact],
    pub counterexample_fidelity: Option<CounterexampleFidelity>,
}

const NO_FACTS: &[PhaseFact] = &[];
const SOLVER_AND_BODY: &[PhaseFact] = &[
    PhaseFact::SolverContext,
    PhaseFact::BodyResult,
    PhaseFact::BodyResultValue,
];
const ENSURES_REQUIREMENTS: &[PhaseFact] = &[PhaseFact::SolverContext, PhaseFact::EnsuresOutcomes];

pub const ENSURES_PHASE: &str = "Phase 5: ensures verification";
pub const COVER_PHASE: &str = "Phase 7: cover witnesses";

pub const PHASE_CONTRACTS: &[PhaseContract] = &[
    PhaseContract {
        name: "Phase 0-units: unit consistency",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 0-nominal: nominal struct types",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 0a: spec validation",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1a: resource hierarchy",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1f: effect containment",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1f-1: replayability",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1b: BMC resource safety",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: Some(CounterexampleFidelity::Bounded),
    },
    PhaseContract {
        name: "Phase 1c: async recursion depth",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1d: atom invariant",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1e: call graph cycles",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1g: effect params",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1h: MIR move analysis",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1h-2: structured concurrency ownership",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1i: vacuity checking",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 1j: temporal effects",
        requires: NO_FACTS,
        establishes: NO_FACTS,
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: "Phase 4: body evaluation",
        requires: NO_FACTS,
        establishes: SOLVER_AND_BODY,
        invalidates: NO_FACTS,
        counterexample_fidelity: Some(CounterexampleFidelity::Approximate),
    },
    PhaseContract {
        name: ENSURES_PHASE,
        requires: SOLVER_AND_BODY,
        establishes: &[PhaseFact::EnsuresOutcomes],
        invalidates: &[PhaseFact::BodyResult],
        counterexample_fidelity: Some(CounterexampleFidelity::Approximate),
    },
    PhaseContract {
        name: "Phase 6: final Z3 check",
        requires: ENSURES_REQUIREMENTS,
        establishes: &[PhaseFact::ContextReachability],
        invalidates: NO_FACTS,
        counterexample_fidelity: None,
    },
    PhaseContract {
        name: COVER_PHASE,
        requires: &[PhaseFact::SolverContext, PhaseFact::BodyResultValue],
        establishes: &[PhaseFact::CoverResults],
        invalidates: NO_FACTS,
        counterexample_fidelity: Some(CounterexampleFidelity::Exact),
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseOrderError {
    pub phase: &'static str,
    pub missing: PhaseFact,
    pub supplied_by: Option<&'static str>,
}

impl fmt::Display for PhaseOrderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.supplied_by {
            Some(supplied_by) => write!(
                f,
                "phase '{}' requires fact {:?} which is not established before it (established by '{}')",
                self.phase, self.missing, supplied_by
            ),
            None => write!(
                f,
                "phase '{}' requires fact {:?} which is not established before it (no phase establishes it)",
                self.phase, self.missing
            ),
        }
    }
}

impl std::error::Error for PhaseOrderError {}

pub fn check_phase_order(phases: &[PhaseContract]) -> Result<(), PhaseOrderError> {
    let mut live_facts = HashSet::new();

    for phase in phases {
        for &required in phase.requires {
            if !live_facts.contains(&required) {
                let supplied_by = phases
                    .iter()
                    .find(|candidate| candidate.establishes.contains(&required))
                    .map(|candidate| candidate.name);
                return Err(PhaseOrderError {
                    phase: phase.name,
                    missing: required,
                    supplied_by,
                });
            }
        }

        live_facts.extend(phase.establishes.iter().copied());
        for invalidated in phase.invalidates {
            live_facts.remove(invalidated);
        }
    }

    Ok(())
}

pub fn phase_contract(recorded_name: &str) -> Option<&'static PhaseContract> {
    let canonical_name = recorded_name
        .rsplit_once(" (")
        .and_then(|(name, outcome)| outcome.strip_suffix(')').map(|_| name))
        .unwrap_or(recorded_name);
    PHASE_CONTRACTS
        .iter()
        .find(|contract| contract.name == canonical_name)
}

pub fn counterexample_fidelity(
    phase: &PhaseContract,
    replay_status: Option<&str>,
) -> CounterexampleFidelity {
    if replay_status == Some("validated") {
        CounterexampleFidelity::Exact
    } else {
        phase
            .counterexample_fidelity
            .unwrap_or(CounterexampleFidelity::Approximate)
    }
}

pub fn raised_counterexample_fidelity(
    phase: &PhaseContract,
    replay_status: Option<&str>,
    complete: bool,
) -> CounterexampleFidelity {
    if complete {
        counterexample_fidelity(phase, replay_status)
    } else {
        CounterexampleFidelity::Approximate
    }
}

#[cfg(test)]
mod tests {
    use super::{
        check_phase_order, counterexample_fidelity, phase_contract, raised_counterexample_fidelity,
        CounterexampleFidelity, PhaseContract, PhaseFact, PHASE_CONTRACTS,
    };

    #[test]
    fn declared_phase_order_is_valid() {
        assert!(check_phase_order(PHASE_CONTRACTS).is_ok());
    }

    #[test]
    fn phase_five_before_phase_four_reports_the_missing_fact() {
        let mut phases = PHASE_CONTRACTS.to_vec();
        phases.swap(15, 16);

        let error = check_phase_order(&phases).unwrap_err();

        assert_eq!(error.phase, "Phase 5: ensures verification");
        assert_eq!(error.missing, PhaseFact::SolverContext);
        assert_eq!(error.supplied_by, Some("Phase 4: body evaluation"));
        assert_eq!(
            error.to_string(),
            "phase 'Phase 5: ensures verification' requires fact SolverContext which is not established before it (established by 'Phase 4: body evaluation')"
        );
    }

    #[test]
    fn phase_five_invalidates_body_result() {
        let consumer = PhaseContract {
            name: "body result consumer",
            requires: &[PhaseFact::BodyResult],
            establishes: &[],
            invalidates: &[],
            counterexample_fidelity: None,
        };
        let mut phases = PHASE_CONTRACTS[..17].to_vec();
        phases.push(consumer);

        let error = check_phase_order(&phases).unwrap_err();

        assert_eq!(error.phase, "body result consumer");
        assert_eq!(error.missing, PhaseFact::BodyResult);
        assert_eq!(error.supplied_by, Some("Phase 4: body evaluation"));
    }

    #[test]
    fn phase_order_error_displays_when_no_phase_supplies_a_fact() {
        let phase = PhaseContract {
            name: "needs context reachability",
            requires: &[PhaseFact::ContextReachability],
            establishes: &[],
            invalidates: &[],
            counterexample_fidelity: None,
        };

        let error = check_phase_order(&[phase]).unwrap_err();

        assert_eq!(error.supplied_by, None);
        assert_eq!(
            error.to_string(),
            "phase 'needs context reachability' requires fact ContextReachability which is not established before it (no phase establishes it)"
        );
    }

    #[test]
    fn phase_contract_strips_recorded_outcome_suffix() {
        assert_eq!(
            phase_contract("Phase 5: ensures verification (failed)").map(|contract| contract.name),
            Some("Phase 5: ensures verification")
        );
    }

    #[test]
    fn counterexample_fidelity_uses_replay_status_then_phase_default() {
        let ensures = phase_contract("Phase 5: ensures verification").unwrap();
        let bounded = phase_contract("Phase 1b: BMC resource safety").unwrap();
        let unspecified = phase_contract("Phase 0a: spec validation").unwrap();

        assert_eq!(
            counterexample_fidelity(ensures, Some("validated")),
            CounterexampleFidelity::Exact
        );
        assert_eq!(
            counterexample_fidelity(ensures, Some("inconclusive")),
            CounterexampleFidelity::Approximate
        );
        assert_eq!(
            counterexample_fidelity(bounded, None),
            CounterexampleFidelity::Bounded
        );
        assert_eq!(
            counterexample_fidelity(unspecified, None),
            CounterexampleFidelity::Approximate
        );
    }

    #[test]
    fn incomplete_raised_counterexample_is_approximate() {
        let ensures = phase_contract("Phase 5: ensures verification").unwrap();

        assert_eq!(
            raised_counterexample_fidelity(ensures, Some("validated"), false),
            CounterexampleFidelity::Approximate
        );
        assert_eq!(
            raised_counterexample_fidelity(ensures, Some("validated"), true),
            CounterexampleFidelity::Exact
        );
    }
}
