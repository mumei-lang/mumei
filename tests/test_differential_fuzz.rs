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
//!
//! A second generator (`gen_array_program`) targets the stale-read class of
//! soundness bugs: programs that write a local `[i64]` through `a[i] = v`,
//! `while` loops, calls into storing / pure atoms, and captured-array
//! lambdas. The reference interpreter tracks both the concrete array and the
//! set of elements the verifier is expected to still know after each
//! havoc point, so positive programs only read provable elements while
//! every havoc event yields a *stale twin* that claims the pre-write value
//! and must be rejected by the verifier.

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
    /// `a[k]` — a read of the program's local array (array programs only).
    ArrRead(usize),
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
    gen_iexpr_with_arr(rng, depth, num_vars, &[])
}

/// Like `gen_iexpr`, but leaves may also read `a[k]` for any `k` in
/// `known_idx` (the elements whose value the verifier is expected to know).
/// With an empty `known_idx` the PRNG stream is consumed exactly as before,
/// so the scalar generator's seeds stay stable.
fn gen_iexpr_with_arr(rng: &mut Rng, depth: u32, num_vars: usize, known_idx: &[usize]) -> IExpr {
    let leaf = depth == 0 || rng.below(4) == 0;
    if leaf {
        if !known_idx.is_empty() && rng.below(3) == 0 {
            IExpr::ArrRead(known_idx[rng.below(known_idx.len() as u64) as usize])
        } else if num_vars > 0 && rng.below(2) == 0 {
            IExpr::Var(rng.below(num_vars as u64) as usize)
        } else {
            IExpr::Const(rng.below(10) as i64)
        }
    } else {
        match rng.below(4) {
            0 => IExpr::Add(
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
            ),
            1 => IExpr::Sub(
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
            ),
            2 => IExpr::Mul(
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
            ),
            _ => {
                let cmp = match rng.below(3) {
                    0 => Cmp::Le,
                    1 => Cmp::Ge,
                    _ => Cmp::Eq,
                };
                IExpr::If(
                    Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                    cmp,
                    Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                    Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
                    Box::new(gen_iexpr_with_arr(rng, depth - 1, num_vars, known_idx)),
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
    eval_iexpr_with_arr(expr, vars, &[])
}

fn eval_iexpr_with_arr(expr: &IExpr, vars: &[i64], arr: &[i64]) -> Option<i64> {
    let bounded = |v: i64| {
        if v.abs() <= MAX_INTERMEDIATE {
            Some(v)
        } else {
            None
        }
    };
    let ev = |e: &IExpr| eval_iexpr_with_arr(e, vars, arr);
    match expr {
        IExpr::Const(c) => Some(*c),
        IExpr::Var(i) => Some(vars[*i]),
        IExpr::ArrRead(k) => Some(arr[*k]),
        IExpr::Add(l, r) => bounded(ev(l)?.checked_add(ev(r)?)?),
        IExpr::Sub(l, r) => bounded(ev(l)?.checked_sub(ev(r)?)?),
        IExpr::Mul(l, r) => bounded(ev(l)?.checked_mul(ev(r)?)?),
        IExpr::If(l, cmp, r, t, e) => {
            if cmp.eval(ev(l)?, ev(r)?) {
                ev(t)
            } else {
                ev(e)
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
        IExpr::ArrRead(k) => {
            let _ = write!(out, "a[{}]", k);
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
// Array / loop / call / lambda program generator (stale-read soundness net).
// ---------------------------------------------------------------------------

const ARR_LEN: usize = 3;

/// One statement of an array program. Every variant that writes the array
/// through something other than a direct `a[k] = v` is a *havoc point*: the
/// verifier forgets the whole array afterwards (except what a loop invariant
/// re-establishes), so the reference tracks `known` alongside the values.
#[derive(Clone, Debug)]
enum ArrStmt {
    /// `a[idx] = <val>;`
    Store { idx: usize, val: IExpr },
    /// `let x<acc> = <init>; while i < trips { a[k] = v ...; x<acc> = x<acc> + step; i = i + 1 }`
    /// with an invariant that carries every fixed-index store and the
    /// untouched known elements across the havoc.
    Loop {
        acc: usize,
        init: IExpr,
        /// Concrete value of `init` (the invariant states it as a constant).
        s0: i64,
        step: i64,
        trips: i64,
        stores: Vec<(usize, i64)>,
        /// Elements known before the loop and not stored by it — restated in
        /// the invariant so they survive the post-loop havoc.
        carried: Vec<(usize, i64)>,
    },
    /// `while i < ARR_LEN invariant: forall(j, 0, i, a[j] == val) { a[i] = val; i = i + 1 }`
    Sweep { val: i64, trips_var: usize },
    /// `let x<res> = store_<idx>(a, <val>);` — callee does `a[idx] = v`.
    CallStore { idx: usize, val: i64, res: usize },
    /// `let x<res> = read_<idx>(a);` — pure callee with `ensures: result == a[idx] + off`.
    CallPure { idx: usize, off: i64, res: usize },
    /// `while i < trips { let t = store_<idx>(a, <val>); i = i + 1 }`
    LoopCallStore { idx: usize, val: i64, trips: i64 },
    /// `let g<lam> = |v| { a[idx] = v; 0 }; let x<res> = call(g<lam>, <val>);`
    LambdaStore {
        idx: usize,
        val: i64,
        lam: usize,
        res: usize,
    },
    /// `let g<lam> = |v| { a[idx] = v; 0 }; while i < trips { let t = call(g<lam>, <val>); i = i + 1 }`
    LoopLambdaStore {
        idx: usize,
        val: i64,
        lam: usize,
        trips: i64,
    },
}

/// Which havoc mechanism a statement exercises (for coverage accounting).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
enum HavocKind {
    LoopStore,
    LoopSweep,
    CallStore,
    LoopCallStore,
    LambdaStore,
    LoopLambdaStore,
}

const ALL_HAVOC_KINDS: [HavocKind; 6] = [
    HavocKind::LoopStore,
    HavocKind::LoopSweep,
    HavocKind::CallStore,
    HavocKind::LoopCallStore,
    HavocKind::LambdaStore,
    HavocKind::LoopLambdaStore,
];

impl ArrStmt {
    fn havoc_kind(&self) -> Option<HavocKind> {
        match self {
            ArrStmt::Store { .. } | ArrStmt::CallPure { .. } => None,
            ArrStmt::Loop { .. } => Some(HavocKind::LoopStore),
            ArrStmt::Sweep { .. } => Some(HavocKind::LoopSweep),
            ArrStmt::CallStore { .. } => Some(HavocKind::CallStore),
            ArrStmt::LoopCallStore { .. } => Some(HavocKind::LoopCallStore),
            ArrStmt::LambdaStore { .. } => Some(HavocKind::LambdaStore),
            ArrStmt::LoopLambdaStore { .. } => Some(HavocKind::LoopLambdaStore),
        }
    }
}

/// A stale-read probe: after statement `after_stmt` executed, `a[idx]` holds
/// `new_val`, but the verifier knew `old_val` before that statement. A
/// program that reads `a[idx]` there and claims `old_val` is a stale read
/// and must be rejected; claiming `new_val` (native semantics) must exit 1
/// when compared against `old_val`.
#[derive(Clone, Debug)]
struct StaleProbe {
    after_stmt: usize,
    idx: usize,
    old_val: i64,
    new_val: i64,
    kind: HavocKind,
}

struct ArrProgram {
    init: [i64; ARR_LEN],
    /// Scalar `let x<i>` bindings evaluated before the statements.
    bindings: Vec<IExpr>,
    stmts: Vec<ArrStmt>,
    final_expr: IExpr,
    expected: i64,
    /// Callee atoms referenced by `CallStore` / `CallPure` (idx, val-offset).
    store_callees: Vec<usize>,
    pure_callees: Vec<(usize, i64)>,
    probes: Vec<StaleProbe>,
    /// Concrete array after every statement ran.
    final_arr: [i64; ARR_LEN],
}

/// Interpreter + verifier-knowledge state threaded through generation.
struct ArrState {
    arr: [i64; ARR_LEN],
    /// The verifier's expected knowledge of each element (`None` = havoced).
    known: [Option<i64>; ARR_LEN],
    vars: Vec<i64>,
}

impl ArrState {
    fn known_idx(&self) -> Vec<usize> {
        (0..ARR_LEN).filter(|k| self.known[*k].is_some()).collect()
    }

    fn havoc_all(&mut self) {
        self.known = [None; ARR_LEN];
    }
}

fn small_val(rng: &mut Rng) -> i64 {
    rng.below(9) as i64 + 1
}

/// Generates one statement, updating `state` (reference + knowledge) and
/// recording the callees / probes it introduces. Returns `None` when an
/// intermediate value leaves the bounded range.
fn gen_arr_stmt(
    rng: &mut Rng,
    state: &mut ArrState,
    prog: &mut ArrProgram,
    lambda_count: &mut usize,
) -> Option<ArrStmt> {
    let stmt_index = prog.stmts.len();
    let push_probe = |state: &ArrState,
                      idx: usize,
                      new_val: i64,
                      kind: HavocKind,
                      probes: &mut Vec<StaleProbe>| {
        if let Some(old_val) = state.known[idx] {
            if old_val != new_val {
                probes.push(StaleProbe {
                    after_stmt: stmt_index,
                    idx,
                    old_val,
                    new_val,
                    kind,
                });
            }
        }
    };
    let choice = rng.below(8);
    let stmt = match choice {
        0 => {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let known = state.known_idx();
            let val = gen_iexpr_with_arr(rng, 1, state.vars.len(), &known);
            let v = eval_iexpr_with_arr(&val, &state.vars, &state.arr)?;
            state.arr[idx] = v;
            state.known[idx] = Some(v);
            ArrStmt::Store { idx, val }
        }
        1 => {
            let trips = rng.below(3) as i64 + 1;
            let step = rng.below(7) as i64 - 3;
            let known = state.known_idx();
            let init = gen_iexpr_with_arr(rng, 1, state.vars.len(), &known);
            let s0 = eval_iexpr_with_arr(&init, &state.vars, &state.arr)?;
            let acc = state.vars.len();
            state.vars.push(s0 + step * trips);
            let n_stores = rng.below(ARR_LEN as u64) as usize + 1;
            let mut stores: Vec<(usize, i64)> = Vec::new();
            for _ in 0..n_stores {
                let idx = rng.below(ARR_LEN as u64) as usize;
                if stores.iter().any(|(k, _)| *k == idx) {
                    continue;
                }
                stores.push((idx, small_val(rng)));
            }
            let carried: Vec<(usize, i64)> = (0..ARR_LEN)
                .filter(|k| !stores.iter().any(|(s, _)| s == k))
                .filter_map(|k| state.known[k].map(|v| (k, v)))
                .collect();
            for (idx, v) in &stores {
                push_probe(state, *idx, *v, HavocKind::LoopStore, &mut prog.probes);
                state.arr[*idx] = *v;
            }
            state.havoc_all();
            for (idx, v) in stores.iter().chain(carried.iter()) {
                state.known[*idx] = Some(*v);
            }
            ArrStmt::Loop {
                acc,
                init,
                s0,
                step,
                trips,
                stores,
                carried,
            }
        }
        2 => {
            let val = small_val(rng);
            for idx in 0..ARR_LEN {
                push_probe(state, idx, val, HavocKind::LoopSweep, &mut prog.probes);
                state.arr[idx] = val;
            }
            state.known = [Some(val); ARR_LEN];
            let trips_var = state.vars.len();
            state.vars.push(ARR_LEN as i64);
            ArrStmt::Sweep { val, trips_var }
        }
        3 => {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let val = small_val(rng);
            push_probe(state, idx, val, HavocKind::CallStore, &mut prog.probes);
            state.arr[idx] = val;
            state.havoc_all();
            if !prog.store_callees.contains(&idx) {
                prog.store_callees.push(idx);
            }
            let res = state.vars.len();
            state.vars.push(0);
            ArrStmt::CallStore { idx, val, res }
        }
        4 => {
            let known = state.known_idx();
            if known.is_empty() {
                return gen_arr_stmt(rng, state, prog, lambda_count);
            }
            let idx = known[rng.below(known.len() as u64) as usize];
            let off = rng.below(5) as i64;
            if !prog.pure_callees.contains(&(idx, off)) {
                prog.pure_callees.push((idx, off));
            }
            let res = state.vars.len();
            state.vars.push(state.arr[idx] + off);
            ArrStmt::CallPure { idx, off, res }
        }
        5 => {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let val = small_val(rng);
            let trips = rng.below(3) as i64 + 1;
            push_probe(state, idx, val, HavocKind::LoopCallStore, &mut prog.probes);
            state.arr[idx] = val;
            state.havoc_all();
            if !prog.store_callees.contains(&idx) {
                prog.store_callees.push(idx);
            }
            ArrStmt::LoopCallStore { idx, val, trips }
        }
        6 => {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let val = small_val(rng);
            push_probe(state, idx, val, HavocKind::LambdaStore, &mut prog.probes);
            state.arr[idx] = val;
            state.havoc_all();
            let lam = *lambda_count;
            *lambda_count += 1;
            let res = state.vars.len();
            state.vars.push(0);
            ArrStmt::LambdaStore { idx, val, lam, res }
        }
        _ => {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let val = small_val(rng);
            let trips = rng.below(3) as i64 + 1;
            push_probe(
                state,
                idx,
                val,
                HavocKind::LoopLambdaStore,
                &mut prog.probes,
            );
            state.arr[idx] = val;
            state.havoc_all();
            let lam = *lambda_count;
            *lambda_count += 1;
            ArrStmt::LoopLambdaStore {
                idx,
                val,
                lam,
                trips,
            }
        }
    };
    Some(stmt)
}

fn gen_array_program(seed: u64) -> ArrProgram {
    for attempt in 0.. {
        let mut rng = Rng::new(seed.wrapping_add(attempt));
        let mut init = [0i64; ARR_LEN];
        for slot in init.iter_mut() {
            *slot = rng.below(10) as i64;
        }
        let mut state = ArrState {
            arr: init,
            known: init.map(Some),
            vars: Vec::new(),
        };
        let mut prog = ArrProgram {
            init,
            bindings: Vec::new(),
            stmts: Vec::new(),
            final_expr: IExpr::Const(0),
            expected: 0,
            store_callees: Vec::new(),
            pure_callees: Vec::new(),
            probes: Vec::new(),
            final_arr: init,
        };
        let num_vars = rng.below(3) as usize;
        let mut ok = true;
        for i in 0..num_vars {
            let expr = gen_iexpr_with_arr(&mut rng, 2, i, &state.known_idx());
            match eval_iexpr_with_arr(&expr, &state.vars, &state.arr) {
                Some(v) => state.vars.push(v),
                None => {
                    ok = false;
                    break;
                }
            }
            prog.bindings.push(expr);
        }
        if !ok {
            continue;
        }
        let num_stmts = 2 + rng.below(4) as usize;
        let mut lambda_count = 0;
        for _ in 0..num_stmts {
            match gen_arr_stmt(&mut rng, &mut state, &mut prog, &mut lambda_count) {
                Some(stmt) => prog.stmts.push(stmt),
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        // The final expression must read the array, so make sure at least one
        // element is provable after the last havoc.
        if state.known_idx().is_empty() {
            let idx = rng.below(ARR_LEN as u64) as usize;
            let v = small_val(&mut rng);
            state.arr[idx] = v;
            state.known[idx] = Some(v);
            prog.stmts.push(ArrStmt::Store {
                idx,
                val: IExpr::Const(v),
            });
        }
        let known = state.known_idx();
        let read = IExpr::ArrRead(known[rng.below(known.len() as u64) as usize]);
        let rest = gen_iexpr_with_arr(&mut rng, 2, state.vars.len(), &known);
        let final_expr = IExpr::Add(Box::new(rest), Box::new(read));
        if let Some(expected) = eval_iexpr_with_arr(&final_expr, &state.vars, &state.arr) {
            prog.final_expr = final_expr;
            prog.expected = expected;
            prog.final_arr = state.arr;
            return prog;
        }
    }
    unreachable!("bounded array program generation always terminates")
}

fn const_source(c: i64) -> String {
    if c < 0 {
        format!("(0 - {})", -c)
    } else {
        format!("{}", c)
    }
}

fn arr_stmt_source(stmt: &ArrStmt, pos: usize, out: &mut String) {
    let i = format!("i{pos}");
    match stmt {
        ArrStmt::Store { idx, val } => {
            let _ = write!(out, "    a[{idx}] = ");
            iexpr_source(val, out);
            out.push_str(";\n");
        }
        ArrStmt::Loop {
            acc,
            init,
            s0,
            step,
            trips,
            stores,
            carried,
        } => {
            let _ = write!(out, "    let x{acc} = ");
            iexpr_source(init, out);
            out.push_str(";\n");
            let mut inv = format!(
                "{i} >= 0 && {i} <= {trips} && x{acc} == {} + {i} * {}",
                const_source(*s0),
                const_source(*step)
            );
            for (k, v) in carried {
                let _ = write!(inv, " && a[{k}] == {}", const_source(*v));
            }
            if !stores.is_empty() {
                let _ = write!(inv, " && ({i} == 0 || (");
                let parts: Vec<String> = stores
                    .iter()
                    .map(|(k, v)| format!("a[{k}] == {}", const_source(*v)))
                    .collect();
                inv.push_str(&parts.join(" && "));
                inv.push_str("))");
            }
            let _ = writeln!(
                out,
                "    let {i} = 0;\n    while {i} < {trips}\n    invariant: {inv}\n    decreases: {trips} - {i}\n    {{"
            );
            for (k, v) in stores {
                let _ = writeln!(out, "        a[{k}] = {v};");
            }
            let _ = writeln!(
                out,
                "        x{acc} = x{acc} + {};\n        {i} = {i} + 1\n    }};",
                const_source(*step)
            );
        }
        ArrStmt::Sweep { val, trips_var } => {
            let _ = writeln!(
                out,
                "    let x{trips_var} = len(a);\n    let {i} = 0;\n    while {i} < {ARR_LEN}\n    invariant: {i} >= 0 && {i} <= {ARR_LEN} && forall(j, 0, {i}, a[j] == {val})\n    decreases: {ARR_LEN} - {i}\n    {{\n        a[{i}] = {val};\n        {i} = {i} + 1\n    }};"
            );
        }
        ArrStmt::CallStore { idx, val, res } => {
            let _ = writeln!(out, "    let x{res} = store_{idx}(a, {val});");
        }
        ArrStmt::CallPure { idx, off, res } => {
            let _ = writeln!(out, "    let x{res} = read_{idx}_{off}(a);");
        }
        ArrStmt::LoopCallStore { idx, val, trips } => {
            let _ = writeln!(
                out,
                "    let {i} = 0;\n    while {i} < {trips}\n    invariant: {i} >= 0 && {i} <= {trips}\n    decreases: {trips} - {i}\n    {{\n        let t = store_{idx}(a, {val});\n        {i} = {i} + 1\n    }};"
            );
        }
        ArrStmt::LambdaStore { idx, val, lam, res } => {
            let _ = writeln!(
                out,
                "    let g{lam} = |v| {{ a[{idx}] = v; 0 }};\n    let x{res} = call(g{lam}, {val});"
            );
        }
        ArrStmt::LoopLambdaStore {
            idx,
            val,
            lam,
            trips,
        } => {
            let _ = writeln!(
                out,
                "    let g{lam} = |v| {{ a[{idx}] = v; 0 }};\n    let {i} = 0;\n    while {i} < {trips}\n    invariant: {i} >= 0 && {i} <= {trips}\n    decreases: {trips} - {i}\n    {{\n        let t = call(g{lam}, {val});\n        {i} = {i} + 1\n    }};"
            );
        }
    }
}

fn arr_callees_source(prog: &ArrProgram) -> String {
    let mut out = String::new();
    for idx in &prog.store_callees {
        let _ = writeln!(
            out,
            "atom store_{idx}(a: [i64], v: i64) -> i64\nrequires: len(a) >= {ARR_LEN};\nensures: result == 0;\nbody: {{\n    a[{idx}] = v;\n    0\n}};\n"
        );
    }
    for (idx, off) in &prog.pure_callees {
        let _ = writeln!(
            out,
            "atom read_{idx}_{off}(a: [i64]) -> i64\nrequires: len(a) >= {ARR_LEN};\nensures: result == a[{idx}] + {off};\nbody: {{\n    a[{idx}] + {off}\n}};\n"
        );
    }
    out
}

/// Renders the `main` prefix shared by the positive program and its stale
/// probes: the array literal, scalar bindings and `stmts[..=upto]`.
fn arr_body_prefix(prog: &ArrProgram, upto: usize) -> String {
    let mut body = String::new();
    let elems: Vec<String> = prog.init.iter().map(|c| c.to_string()).collect();
    let _ = writeln!(body, "    let a = [{}];", elems.join(", "));
    for (i, binding) in prog.bindings.iter().enumerate() {
        let _ = write!(body, "    let x{} = ", i);
        iexpr_source(binding, &mut body);
        body.push_str(";\n");
    }
    for (pos, stmt) in prog.stmts[..upto].iter().enumerate() {
        arr_stmt_source(stmt, pos, &mut body);
    }
    body
}

/// Same probe shape as `program_source`: the atom returns 0 when the final
/// expression equals `expected_eq`, else 1.
fn array_program_source(prog: &ArrProgram, expected_eq: i64, want_match: bool) -> String {
    let mut body = arr_body_prefix(prog, prog.stmts.len());
    let mut final_src = String::new();
    iexpr_source(&prog.final_expr, &mut final_src);
    let _ = writeln!(
        body,
        "    if {} == {} {{ 0 }} else {{ 1 }}",
        final_src,
        const_source(expected_eq)
    );
    let ensured = if want_match { 0 } else { 1 };
    format!(
        "{}atom main()\nrequires: true;\nensures: result == {ensured};\nbody: {{\n{body}}};\n",
        arr_callees_source(prog)
    )
}

/// Stale-read twin: run the program up to and including the havoc statement,
/// then compare `a[idx]` against the *pre-write* value. With
/// `ensures: result == 0` this is a stale read the verifier must reject;
/// with `ensures: result >= 0` it verifies trivially and the native exit code
/// (1, the values differ) shows the runtime observed the write.
fn stale_probe_source(prog: &ArrProgram, probe: &StaleProbe, claim_stale: bool) -> String {
    stale_probe_source_at(prog, probe, probe.after_stmt + 1, claim_stale)
}

/// Like `stale_probe_source`, but runs `stmts[..upto]` before the read, so a
/// stale belief can also be probed after *later* statements (pure calls,
/// other loops) that must not resurrect the pre-write value.
fn stale_probe_source_at(
    prog: &ArrProgram,
    probe: &StaleProbe,
    upto: usize,
    claim_stale: bool,
) -> String {
    let mut body = arr_body_prefix(prog, upto);
    let _ = writeln!(
        body,
        "    if a[{}] == {} {{ 0 }} else {{ 1 }}",
        probe.idx,
        const_source(probe.old_val)
    );
    let ensures = if claim_stale {
        "result == 0"
    } else {
        "result >= 0"
    };
    format!(
        "{}atom main()\nrequires: true;\nensures: {ensures};\nbody: {{\n{body}}};\n",
        arr_callees_source(prog)
    )
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
    // Scale with MUMEI_FUZZ_CASES like the positive test, but keep the
    // default CI cost lower (each seed drives three pipeline invocations).
    for seed in 1..=fuzz_cases().div_ceil(3) {
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

// ---------------------------------------------------------------------------
// Array / loop / call / lambda soundness net.
// ---------------------------------------------------------------------------

fn assert_verifies(source: &str, what: &str) {
    let fixture = write_fixture(what, source);
    let verify = mumei(&["verify"], &fixture);
    assert!(
        verify.status.success(),
        "{what}: verifier disagrees with reference\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&verify.stdout),
        String::from_utf8_lossy(&verify.stderr)
    );
    cleanup(&fixture);
}

fn assert_rejected(source: &str, what: &str) {
    let fixture = write_fixture(what, source);
    let verify = mumei(&["verify"], &fixture);
    assert!(
        !verify.status.success(),
        "{what}: verifier accepted a program whose result differs from its ensures\nprogram:\n{source}"
    );
    cleanup(&fixture);
}

fn assert_runs_with(source: &str, code: i32, what: &str) {
    let fixture = write_fixture(what, source);
    let run = mumei(&["run"], &fixture);
    assert_eq!(
        run.status.code(),
        Some(code),
        "{what}: native run disagrees with reference\nprogram:\n{source}\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&run.stdout),
        String::from_utf8_lossy(&run.stderr)
    );
    cleanup(&fixture);
}

const ARRAY_SEED_MIX: u64 = 0xD1B54A32D192ED03;

/// Positive differential over array programs: the verifier must prove the
/// reference result through every store / loop / call / lambda havoc (using
/// only elements it can still know), and the native runtime must agree.
#[test]
fn array_programs_agree_across_verify_and_run() {
    for seed in 1..=fuzz_cases() {
        let prog = gen_array_program(seed.wrapping_mul(ARRAY_SEED_MIX));
        let source = array_program_source(&prog, prog.expected, true);
        let what = format!("arr_pos_{seed}");
        assert_verifies(&source, &what);
        assert_runs_with(&source, 0, &what);
    }
}

/// Off-by-one twin of every array program: the verifier must reject the
/// wrong constant, and the corrected mutant (`result == 1`) must verify and
/// exit 1 natively.
#[test]
fn mutated_array_programs_are_rejected_by_both_pipelines() {
    for seed in 1..=fuzz_cases().div_ceil(3) {
        let prog = gen_array_program(seed.wrapping_mul(ARRAY_SEED_MIX));
        let wrong = prog.expected.wrapping_add(1);
        assert_rejected(
            &array_program_source(&prog, wrong, true),
            &format!("arr_neg_{seed}"),
        );
        let corrected = array_program_source(&prog, wrong, false);
        let what = format!("arr_neg_run_{seed}");
        assert_verifies(&corrected, &what);
        assert_runs_with(&corrected, 1, &what);
    }
}

/// Stale-read twins: for every havoc point whose write changes an element the
/// verifier knew, reading that element right after and claiming the
/// pre-write value must be rejected. The `result >= 0` form of the same
/// program verifies trivially and must exit 1 natively (the runtime saw the
/// new value). Seeds are scanned until `fuzz_cases()` probes ran *and* every
/// havoc kind (loop store, loop sweep, call, call-in-loop, lambda,
/// lambda-in-loop) was exercised at least once, so the combination space is
/// covered regardless of the case budget.
#[test]
fn stale_reads_after_every_havoc_kind_are_rejected() {
    let budget = fuzz_cases();
    let mut ran = 0u64;
    let mut covered: std::collections::BTreeSet<HavocKind> = Default::default();
    let mut seed = 1u64;
    while ran < budget || covered.len() < ALL_HAVOC_KINDS.len() {
        assert!(
            ran < budget || seed <= 512,
            "no seed below 512 exercised every havoc kind; covered {covered:?}"
        );
        let prog = gen_array_program(seed.wrapping_mul(ARRAY_SEED_MIX));
        for (n, probe) in prog.probes.iter().enumerate() {
            let fresh_kind = !covered.contains(&probe.kind);
            if ran >= budget && !fresh_kind {
                continue;
            }
            let what = format!("arr_stale_{seed}_{n}_{:?}", probe.kind);
            assert_rejected(&stale_probe_source(&prog, probe, true), &what);
            let observed = stale_probe_source(&prog, probe, false);
            assert_verifies(&observed, &what);
            assert_ne!(probe.old_val, probe.new_val);
            assert_runs_with(&observed, 1, &what);
            covered.insert(probe.kind);
            ran += 1;
        }
        seed += 1;
    }
}

/// Late stale reads: a pre-write value the verifier forgot at a havoc point
/// must not be resurrected by the statements that follow (pure calls, other
/// loops' invariants, unrelated stores). For every probe whose element still
/// differs from `old_val` at the end of the program, the full program plus
/// `a[idx] == old_val` must be rejected and must exit 1 natively.
#[test]
fn late_stale_reads_are_rejected() {
    let mut ran = 0u64;
    for seed in 1..=fuzz_cases() {
        let prog = gen_array_program(seed.wrapping_mul(ARRAY_SEED_MIX));
        for (n, probe) in prog.probes.iter().enumerate() {
            if probe.after_stmt + 1 == prog.stmts.len()
                || prog.final_arr[probe.idx] == probe.old_val
            {
                continue;
            }
            let what = format!("arr_late_{seed}_{n}_{:?}", probe.kind);
            let upto = prog.stmts.len();
            assert_rejected(&stale_probe_source_at(&prog, probe, upto, true), &what);
            let observed = stale_probe_source_at(&prog, probe, upto, false);
            assert_verifies(&observed, &what);
            assert_runs_with(&observed, 1, &what);
            ran += 1;
        }
    }
    assert!(ran > 0, "no seed produced a late stale-read probe");
}

/// Every generated array program keeps the array and stores into it, and
/// across the first 64 seeds the generator reaches every havoc kind — so the
/// negative twins are not re-testing a single fixture shape.
#[test]
fn array_generator_covers_havoc_kind_space() {
    let mut kinds: std::collections::BTreeSet<HavocKind> = Default::default();
    let mut probes = 0usize;
    for seed in 1..=64u64 {
        let prog = gen_array_program(seed.wrapping_mul(ARRAY_SEED_MIX));
        kinds.extend(prog.stmts.iter().filter_map(ArrStmt::havoc_kind));
        probes += prog.probes.len();
        let source = array_program_source(&prog, prog.expected, true);
        assert!(
            source.contains("let a = ["),
            "seed {seed} lost the array:\n{source}"
        );
        assert!(
            source.contains("a[") && source.contains("] = "),
            "seed {seed} never stores into the array:\n{source}"
        );
    }
    assert_eq!(
        kinds.len(),
        ALL_HAVOC_KINDS.len(),
        "generator never produced some havoc kind: {kinds:?}"
    );
    assert!(probes > 0, "generator produced no stale-read probes");
}

/// The `call(|x| { x[k] = v; 0 }, a)` shape (a lambda storing through its
/// *parameter*) has no native lowering yet, so it is pinned verify-only: the
/// stale pre-call claim must be rejected both straight-line and inside a
/// `while` loop, for every index.
#[test]
fn param_lambda_store_stale_reads_are_rejected() {
    for idx in 0..ARR_LEN {
        let init = [7, 1, 2];
        let old = init[idx];
        let new = old + 5;
        let elems = "7, 1, 2";
        let straight = format!(
            "atom main()\nrequires: true;\nensures: result == 0;\nbody: {{\n    let a = [{elems}];\n    let t = call(|x| {{ x[{idx}] = {new}; 0 }}, a);\n    if a[{idx}] == {old} {{ 0 }} else {{ 1 }}\n}};\n"
        );
        assert_rejected(&straight, &format!("param_lambda_straight_{idx}"));
        let looped = format!(
            "atom main()\nrequires: true;\nensures: result == 0;\nbody: {{\n    let a = [{elems}];\n    let i = 0;\n    while i < 2\n    invariant: i >= 0 && i <= 2\n    decreases: 2 - i\n    {{\n        let t = call(|x| {{ x[{idx}] = {new}; 0 }}, a);\n        i = i + 1\n    }};\n    if a[{idx}] == {old} {{ 0 }} else {{ 1 }}\n}};\n"
        );
        assert_rejected(&looped, &format!("param_lambda_loop_{idx}"));
        // The untouched siblings keep their entry facts only when the lambda
        // is pure — a storing lambda havocs the whole arg, so a sibling claim
        // is likewise not provable (no wrong-verify in either direction).
        let pure = format!(
            "atom main()\nrequires: true;\nensures: result == 0;\nbody: {{\n    let a = [{elems}];\n    let t = call(|x| {{ x[{idx}] }}, a);\n    if a[{idx}] == {old} {{ 0 }} else {{ 1 }}\n}};\n"
        );
        assert_verifies(&pure, &format!("param_lambda_pure_{idx}"));
    }
}
