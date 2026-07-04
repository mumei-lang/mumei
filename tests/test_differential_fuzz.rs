//! Differential fuzzing harness: verify vs. native run.
//!
//! Generates small, deterministic `.mm` programs from a seeded PRNG, evaluates
//! them with an in-test reference interpreter, and checks that the two
//! pipelines agree with the reference result:
//!
//! - `mumei verify` must prove `ensures: result == <expected>` (Z3 semantics),
//! - `mumei run` must exit with `<expected>` (lowering + LLVM JIT semantics).
//!
//! A disagreement in either direction is a task_runtime / lowering /
//! verification-encoding bug (e.g. an `f64` silently collapsing to `i64`).
//! Each generated program is also mutated into a *negative* twin whose
//! expected constant is off by one; both pipelines must reject it, proving
//! the oracle actually observes program semantics.
//!
//! The harness is fully deterministic (fixed seeds) so CI failures are
//! reproducible; set `MUMEI_FUZZ_CASES` to fuzz more programs locally.

use std::fmt::Write as _;
use std::process::Command;

// ---------------------------------------------------------------------------
// Deterministic PRNG (xorshift64*) — no external dependencies.
// ---------------------------------------------------------------------------

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// ---------------------------------------------------------------------------
// Expression generator + reference evaluator.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
enum IExpr {
    Const(i64),
    Var(usize),
    Add(Box<IExpr>, Box<IExpr>),
    Sub(Box<IExpr>, Box<IExpr>),
    Mul(Box<IExpr>, Box<IExpr>),
    /// `if <l> <cmp> <r> { <then> } else { <else> }`
    If(Box<IExpr>, Cmp, Box<IExpr>, Box<IExpr>, Box<IExpr>),
}

#[derive(Clone, Copy, Debug)]
enum Cmp {
    Le,
    Ge,
    Eq,
}

impl Cmp {
    fn source(self) -> &'static str {
        match self {
            Cmp::Le => "<=",
            Cmp::Ge => ">=",
            Cmp::Eq => "==",
        }
    }

    fn eval(self, l: i64, r: i64) -> bool {
        match self {
            Cmp::Le => l <= r,
            Cmp::Ge => l >= r,
            Cmp::Eq => l == r,
        }
    }
}

fn gen_iexpr(rng: &mut Rng, depth: u32, num_vars: usize) -> IExpr {
    let leaf = depth == 0 || rng.below(4) == 0;
    if leaf {
        if num_vars > 0 && rng.below(2) == 0 {
            IExpr::Var(rng.below(num_vars as u64) as usize)
        } else {
            IExpr::Const(rng.below(10) as i64)
        }
    } else {
        match rng.below(4) {
            0 => IExpr::Add(
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
            ),
            1 => IExpr::Sub(
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
            ),
            2 => IExpr::Mul(
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                Box::new(gen_iexpr(rng, depth - 1, num_vars)),
            ),
            _ => {
                let cmp = match rng.below(3) {
                    0 => Cmp::Le,
                    1 => Cmp::Ge,
                    _ => Cmp::Eq,
                };
                IExpr::If(
                    Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                    cmp,
                    Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                    Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                    Box::new(gen_iexpr(rng, depth - 1, num_vars)),
                )
            }
        }
    }
}

/// Reference evaluation with an intermediate-value bound. Returns `None` when
/// any intermediate exceeds `MAX_INTERMEDIATE`, so generated programs never
/// approach `i64` overflow — where Z3's unbounded `Int` and the wrapping
/// native runtime would diverge by design rather than by bug.
const MAX_INTERMEDIATE: i64 = 1_000_000_000;

fn eval_iexpr(expr: &IExpr, vars: &[i64]) -> Option<i64> {
    let bounded = |v: i64| {
        if v.abs() <= MAX_INTERMEDIATE {
            Some(v)
        } else {
            None
        }
    };
    match expr {
        IExpr::Const(c) => Some(*c),
        IExpr::Var(i) => Some(vars[*i]),
        IExpr::Add(l, r) => bounded(eval_iexpr(l, vars)?.checked_add(eval_iexpr(r, vars)?)?),
        IExpr::Sub(l, r) => bounded(eval_iexpr(l, vars)?.checked_sub(eval_iexpr(r, vars)?)?),
        IExpr::Mul(l, r) => bounded(eval_iexpr(l, vars)?.checked_mul(eval_iexpr(r, vars)?)?),
        IExpr::If(l, cmp, r, t, e) => {
            if cmp.eval(eval_iexpr(l, vars)?, eval_iexpr(r, vars)?) {
                eval_iexpr(t, vars)
            } else {
                eval_iexpr(e, vars)
            }
        }
    }
}

/// Render as Mumei source. Negative constants are written `(0 - n)` because
/// the grammar has no unary minus.
fn iexpr_source(expr: &IExpr, out: &mut String) {
    match expr {
        IExpr::Const(c) if *c < 0 => {
            let _ = write!(out, "(0 - {})", -c);
        }
        IExpr::Const(c) => {
            let _ = write!(out, "{}", c);
        }
        IExpr::Var(i) => {
            let _ = write!(out, "x{}", i);
        }
        IExpr::Add(l, r) | IExpr::Sub(l, r) | IExpr::Mul(l, r) => {
            let op = match expr {
                IExpr::Add(..) => "+",
                IExpr::Sub(..) => "-",
                _ => "*",
            };
            out.push('(');
            iexpr_source(l, out);
            let _ = write!(out, " {} ", op);
            iexpr_source(r, out);
            out.push(')');
        }
        IExpr::If(l, cmp, r, t, e) => {
            out.push_str("(if ");
            iexpr_source(l, out);
            let _ = write!(out, " {} ", cmp.source());
            iexpr_source(r, out);
            out.push_str(" { ");
            iexpr_source(t, out);
            out.push_str(" } else { ");
            iexpr_source(e, out);
            out.push_str(" })");
        }
    }
}

/// A generated program: `let` bindings followed by a final expression, plus
/// the reference-evaluated result.
struct GenProgram {
    bindings: Vec<IExpr>,
    final_expr: IExpr,
    expected: i64,
}

fn gen_program(seed: u64) -> GenProgram {
    // Retry with derived seeds until every intermediate stays within bounds.
    for attempt in 0.. {
        let mut rng = Rng::new(seed.wrapping_add(attempt));
        let num_vars = 1 + rng.below(3) as usize;
        let mut bindings = Vec::new();
        let mut values = Vec::new();
        let mut ok = true;
        for i in 0..num_vars {
            let expr = gen_iexpr(&mut rng, 2, i);
            match eval_iexpr(&expr, &values) {
                Some(v) => values.push(v),
                None => {
                    ok = false;
                    break;
                }
            }
            bindings.push(expr);
        }
        if !ok {
            continue;
        }
        let final_expr = gen_iexpr(&mut rng, 3, num_vars);
        if let Some(expected) = eval_iexpr(&final_expr, &values) {
            return GenProgram {
                bindings,
                final_expr,
                expected,
            };
        }
    }
    unreachable!("bounded program generation always terminates")
}

/// Render the differential probe. `expected_eq` is the constant compared
/// against the program result; the atom returns 0 when they match, 1 when
/// they don't, so `mumei run`'s exit code is the oracle.
fn program_source(prog: &GenProgram, expected_eq: i64, want_match: bool) -> String {
    let mut body = String::new();
    for (i, binding) in prog.bindings.iter().enumerate() {
        let _ = write!(body, "    let x{} = ", i);
        iexpr_source(binding, &mut body);
        body.push_str(";\n");
    }
    body.push_str("    if ");
    let mut final_src = String::new();
    iexpr_source(&prog.final_expr, &mut final_src);
    let eq_src = if expected_eq < 0 {
        format!("(0 - {})", -expected_eq)
    } else {
        format!("{}", expected_eq)
    };
    let _ = writeln!(body, "{} == {} {{ 0 }} else {{ 1 }}", final_src, eq_src);
    let ensured = if want_match { 0 } else { 1 };
    format!("atom main()\nrequires: true;\nensures: result == {ensured};\nbody: {{\n{body}}};\n")
}

// ---------------------------------------------------------------------------
// Pipeline drivers.
// ---------------------------------------------------------------------------

fn write_fixture(tag: &str, source: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("mumei_diff_fuzz_{}_{}", std::process::id(), tag));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale fuzz dir");
    }
    std::fs::create_dir_all(&dir).expect("create fuzz dir");
    let path = dir.join("main.mm");
    std::fs::write(&path, source).expect("write fuzz fixture");
    path
}

fn mumei(args: &[&str], fixture: &std::path::Path) -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let mut cmd = Command::new(bin);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.arg(fixture).current_dir(env!("CARGO_MANIFEST_DIR"));
    cmd.output().expect("failed to spawn mumei")
}

fn cleanup(fixture: &std::path::Path) {
    std::fs::remove_dir_all(fixture.parent().unwrap()).ok();
}

fn fuzz_cases() -> u64 {
    std::env::var("MUMEI_FUZZ_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(6)
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// Positive differential: for each seed, the reference result must be agreed
/// on by both the verifier (Z3) and the native runtime (exit code).
#[test]
fn generated_programs_agree_across_verify_and_run() {
    for seed in 1..=fuzz_cases() {
        let prog = gen_program(seed.wrapping_mul(0x9E3779B97F4A7C15));
        let source = program_source(&prog, prog.expected, true);
        let fixture = write_fixture(&format!("pos_{seed}"), &source);

        let verify = mumei(&["verify"], &fixture);
        assert!(
            verify.status.success(),
            "seed {seed}: verifier disagrees with reference (expected {})\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
            prog.expected,
            String::from_utf8_lossy(&verify.stdout),
            String::from_utf8_lossy(&verify.stderr)
        );

        let run = mumei(&["run"], &fixture);
        assert_eq!(
            run.status.code(),
            Some(0),
            "seed {seed}: native run disagrees with reference (expected {})\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
            prog.expected,
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );

        cleanup(&fixture);
    }
}

/// Negative differential: an off-by-one expected constant must be rejected by
/// the verifier and observed by the runtime (exit code 1). This proves the
/// oracle is sensitive — both pipelines actually compute the program result.
#[test]
fn mutated_programs_are_rejected_by_both_pipelines() {
    for seed in [3u64, 5] {
        let prog = gen_program(seed.wrapping_mul(0x9E3779B97F4A7C15));
        let wrong = prog.expected.wrapping_add(1);
        // want_match=true asserts `result == 0`, but the comparison constant is
        // wrong, so the program actually evaluates to 1: verify must fail.
        let source = program_source(&prog, wrong, true);
        let fixture = write_fixture(&format!("neg_{seed}"), &source);

        let verify = mumei(&["verify"], &fixture);
        assert!(
            !verify.status.success(),
            "seed {seed}: verifier accepted a program whose result differs from its ensures\nprogram:\n{source}"
        );
        cleanup(&fixture);

        // want_match=false asserts `result == 1`, matching the actual
        // behavior, so verification and the native exit code must both agree.
        let source = program_source(&prog, wrong, false);
        let fixture = write_fixture(&format!("neg_run_{seed}"), &source);
        let verify = mumei(&["verify"], &fixture);
        assert!(
            verify.status.success(),
            "seed {seed}: verifier rejected the corrected mutant\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&verify.stdout),
            String::from_utf8_lossy(&verify.stderr)
        );
        let run = mumei(&["run"], &fixture);
        assert_eq!(
            run.status.code(),
            Some(1),
            "seed {seed}: native run disagrees with reference on mutant\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        );
        cleanup(&fixture);
    }
}
