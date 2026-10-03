use z3::SatResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextReachability {
    Reachable,
    Unreachable,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ClauseOutcome {
    Proved,
    Vacuous,
    AlwaysFalse,
    FailsOnSomeInputs,
    Fails,
    Unknown,
    Skipped,
}

pub fn classify(
    ctx: ContextReachability,
    refute: SatResult,
    holds: Option<SatResult>,
) -> ClauseOutcome {
    if ctx == ContextReachability::Unreachable {
        return ClauseOutcome::Vacuous;
    }

    match refute {
        SatResult::Unsat => ClauseOutcome::Proved,
        SatResult::Unknown => ClauseOutcome::Unknown,
        SatResult::Sat => match holds {
            Some(SatResult::Unsat) => ClauseOutcome::AlwaysFalse,
            Some(SatResult::Sat) => ClauseOutcome::FailsOnSomeInputs,
            Some(SatResult::Unknown) | None => ClauseOutcome::Fails,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::{classify, ClauseOutcome, ContextReachability};
    use z3::SatResult;

    #[test]
    fn classify_all_context_and_solver_result_combinations() {
        let contexts = [
            ContextReachability::Reachable,
            ContextReachability::Unreachable,
            ContextReachability::Unknown,
        ];
        let solver_results = [SatResult::Sat, SatResult::Unsat, SatResult::Unknown];
        let holds_results = [
            None,
            Some(SatResult::Sat),
            Some(SatResult::Unsat),
            Some(SatResult::Unknown),
        ];

        for context in contexts {
            for refute in solver_results {
                for holds in holds_results {
                    let expected = if context == ContextReachability::Unreachable {
                        ClauseOutcome::Vacuous
                    } else {
                        match refute {
                            SatResult::Unsat => ClauseOutcome::Proved,
                            SatResult::Unknown => ClauseOutcome::Unknown,
                            SatResult::Sat => match holds {
                                Some(SatResult::Unsat) => ClauseOutcome::AlwaysFalse,
                                Some(SatResult::Sat) => ClauseOutcome::FailsOnSomeInputs,
                                Some(SatResult::Unknown) | None => ClauseOutcome::Fails,
                            },
                        }
                    };
                    assert_eq!(
                        classify(context, refute, holds),
                        expected,
                        "context={context:?}, refute={refute:?}, holds={holds:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn clause_outcomes_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_value(ClauseOutcome::FailsOnSomeInputs).unwrap(),
            serde_json::json!("fails_on_some_inputs")
        );
        assert_eq!(
            serde_json::to_value(ContextReachability::Unreachable).unwrap(),
            serde_json::json!("unreachable")
        );
        assert_eq!(
            serde_json::to_value(ClauseOutcome::Skipped).unwrap(),
            serde_json::json!("skipped")
        );
    }
}
