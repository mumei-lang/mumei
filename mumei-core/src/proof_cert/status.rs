/// `z3_check_result`: Z3 proved the obligation (no counter-example).
pub const Z3_UNSAT: &str = "unsat";
/// `z3_check_result`: Z3 found a counter-example.
pub const Z3_SAT: &str = "sat";
/// `z3_check_result`: Z3 could not decide the obligation.
pub const Z3_UNKNOWN: &str = "unknown";
/// `z3_check_result`: the obligation was not checked by Z3.
pub const Z3_SKIPPED: &str = "skipped";
/// `z3_check_result`: discharged by the mumei-lean Lean 4 bridge.
pub const Z3_LEAN_VERIFIED: &str = "lean_verified";
/// `z3_check_result`: Z3 timed out before deciding the obligation.
pub const Z3_TIMEOUT: &str = "timeout";
/// `z3_check_result`: Z3 hit a resource limit.
pub const Z3_RESOURCE_LIMIT: &str = "resource_limit";
/// `z3_check_result`: Z3 produced a spurious candidate.
pub const Z3_SPURIOUS_CANDIDATE: &str = "spurious_candidate";

/// All accepted `z3_check_result` values, in schema order.
pub const Z3_CHECK_RESULTS: [&str; 8] = [
    Z3_UNSAT,
    Z3_SAT,
    Z3_UNKNOWN,
    Z3_SKIPPED,
    Z3_LEAN_VERIFIED,
    Z3_TIMEOUT,
    Z3_RESOURCE_LIMIT,
    Z3_SPURIOUS_CANDIDATE,
];

/// `status`: atom proven.
pub const VERIFIED: &str = "verified";
/// `status`: atom refuted by a counter-example.
pub const FAILED: &str = "failed";
/// `status`: atom not checked.
pub const SKIPPED: &str = "skipped";
/// `status`: atom assumed (e.g. trusted FFI boundary).
pub const TRUSTED: &str = "trusted";
/// `status`: atom pending escalation to the Lean bridge.
pub const ESCALATION_CANDIDATE: &str = "escalation_candidate";

/// All accepted `status` values, in schema order.
pub const VERIFICATION_STATUSES: [&str; 5] =
    [VERIFIED, FAILED, SKIPPED, TRUSTED, ESCALATION_CANDIDATE];

/// `LeanResultMetadata.status`: the Lean bridge discharged the obligation.
///
/// This is the mumei-lean-side result status (distinct from the
/// certificate-level `z3_check_result`/`status` fields above), even though
/// it shares the `"lean_verified"` spelling with [`Z3_LEAN_VERIFIED`].
pub const LEAN_STATUS_VERIFIED: &str = "lean_verified";
