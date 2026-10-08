//! Contract expression → host-language translation.
//!
//! Shared by the Python/Rust wrapper emitters and the runtime monitor so that
//! every generated runtime check is produced from the parsed contract `Expr`
//! rather than by string surgery on the source text. The previous string
//! replace-based translators could not express `=>` (implication), emitted
//! float division for integer `/`, and mangled `forall(`/`exists(` text left
//! inside larger expressions; this printer handles all three from the AST.
//!
//! Coverage is intentionally small — the expression forms a contract clause can
//! contain after the parser has normalized `!e` (into `IfThenElse`), unary `-x`
//! (into `0 - x` or a negative literal), and comparison chains (into
//! `&&`-joined comparisons). `ArrayAccess`/`FieldAccess` are rejected: pointer
//! indexing and field reads would dereference unvalidated FFI data inside a
//! nominally safe wrapper. `::`-qualified names print with the FFI `::`→`_`
//! symbol mangling (`Vec2::dot` → `Vec2_dot`), never as a host path. Any node
//! outside that set returns `None` so callers can degrade to a comment or an
//! `unchecked` record instead of emitting invalid host code.
//!
//! Notable semantics baked in here:
//! - `=>` prints as `not a or b` (Python) / `!a || b` (Rust). `=>` is
//!   left-associative and binds looser than `||`/`&&`, so both operands are
//!   always parenthesized.
//! - `/` on integer operands prints EUCLIDEAN division matching the verifier's
//!   Z3 `Int` semantics (remainder ≥ 0: `div(7,-2) = -3`, `div(-7,2) = -4`) —
//!   `(_mumei_d := divmod(a, b))[0] + (_mumei_d[1] < 0)` in Python (divmod
//!   evaluates each operand once — chained divisions stay linear),
//!   `a.div_euclid(b)` in Rust. Plain `//` floors and `/` truncates, so
//!   both plain forms can mischeck a verified clause on negative divisors.
//!   Float-typed operands keep `/`.
//! - `forall(v, s, e, c)`/`exists(v, s, e, c)` calls print as
//!   `all(c for v in range(s, e))`/`any(...)` (Python) and
//!   `(s..e).all(|v| c)`/`.any(...)` (Rust); the verifier's quantifier range is
//!   end-exclusive, which matches both host forms.

use std::collections::HashMap;

use crate::hir::HirSignature;
use crate::lowering::{lower, LoweredType};
use crate::parser::{parse_expression_checked, Expr, Op, Stmt};
use crate::verification::ModuleEnv;

/// Host language targeted by a translation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostTarget {
    Python,
    Rust,
}

/// Static knowledge about a contract variable, used to pick the right host
/// operator (Euclidean vs float `/`) and indexing form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractVarKind {
    /// Integer — the default; mumei `/` between ints is integer division.
    Int,
    /// `f32`/`f64` — `/` stays float division.
    Float,
    /// `bool` — at the FFI boundary bools ride as i64, so Rust output coerces
    /// the variable to `(name != 0)`.
    Bool,
    /// Pointer param (`[T]`/`[]<T>`): Rust index becomes `unsafe { *p.add(i) }`.
    Ptr,
    /// Anything else (struct, Str, unknown): treated as `Int` for division and
    /// rejected for Rust `arr[i]` indexing.
    Other,
}

/// Variable-name → kind table consulted while printing a contract clause.
/// Built per atom from `HirSignature` (+ the `result` pseudo-variable).
#[derive(Debug, Clone, Default)]
pub struct ContractVars {
    map: HashMap<String, ContractVarKind>,
}

impl ContractVars {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, name: impl Into<String>, kind: ContractVarKind) {
        self.map.insert(name.into(), kind);
    }

    /// Unknown variables default to `Int`: mumei `/` between ints is integer
    /// division, so untyped or unresolvable names keep the integer semantics.
    pub fn get(&self, name: &str) -> ContractVarKind {
        self.map.get(name).copied().unwrap_or(ContractVarKind::Int)
    }

    /// Build the table from a HIR signature: each param plus `result` (from the
    /// declared or inferred return type).
    pub fn from_signature(sig: &HirSignature, module_env: Option<&ModuleEnv>) -> Self {
        let mut vars = Self::new();
        let mut add = |name: &str, ty: Option<&String>| {
            let ty = ty.map_or("i64", String::as_str);
            let resolved =
                module_env.map_or_else(|| ty.to_string(), |env| env.resolve_base_type(ty));
            vars.insert(name.to_string(), kind_of(&lower(&resolved)));
        };
        for p in &sig.params {
            add(&p.name, p.ty.as_ref());
        }
        let ret = sig
            .return_type
            .as_ref()
            .or(sig.inferred_return_type.as_ref());
        add("result", ret);
        vars
    }

    fn with_bound(&self, var: &str) -> Self {
        let mut vars = self.clone();
        vars.insert(var, ContractVarKind::Int);
        vars
    }
}

fn kind_of(ty: &LoweredType) -> ContractVarKind {
    match ty {
        LoweredType::I64 | LoweredType::I32 | LoweredType::U64 | LoweredType::U32 => {
            ContractVarKind::Int
        }
        LoweredType::Bool => ContractVarKind::Bool,
        LoweredType::F64 | LoweredType::F32 => ContractVarKind::Float,
        LoweredType::Array(_) => ContractVarKind::Ptr,
        LoweredType::Str | LoweredType::Other(_) => ContractVarKind::Other,
    }
}

/// Parse `text` (a contract clause or quantifier fragment) and print it as a
/// host-language expression. `None` when the text does not parse cleanly or
/// uses a form with no host counterpart — callers must degrade, never emit the
/// raw source.
pub fn contract_text_to_host(
    text: &str,
    vars: &ContractVars,
    target: HostTarget,
) -> Option<String> {
    let expr = parse_expression_checked(text).ok()?;
    let expr = simplify(expr);
    emit(&expr, vars, target, 0).or_else(|| emit_translatable_conjuncts(&expr, vars, target))
}

/// Partial-clause fallback: when the whole conjunction cannot print (one
/// conjunct uses an unsupported form like `arr[i]`), emit each translatable
/// top-level conjunct instead of dropping the entire clause — a requires of
/// `n > 0 && arr[i] > 0` still enforces `n > 0` at runtime. Conjuncts that
/// cannot print are skipped (the assert message keeps the full clause text,
/// so the skipped part remains documented); when nothing prints, or the
/// expression is not a top-level conjunction, the caller degrades as before.
fn emit_translatable_conjuncts(e: &Expr, vars: &ContractVars, t: HostTarget) -> Option<String> {
    let mut conjuncts = Vec::new();
    flatten_and(e, &mut conjuncts);
    if conjuncts.len() < 2 {
        return None;
    }
    let emitted: Vec<String> = conjuncts
        .iter()
        .filter_map(|c| emit(c, vars, t, 0))
        .collect();
    if emitted.is_empty() {
        return None;
    }
    let sep = if t == HostTarget::Python {
        " and "
    } else {
        " && "
    };
    Some(emitted.join(sep))
}

fn flatten_and<'a>(e: &'a Expr, out: &mut Vec<&'a Expr>) {
    if let Expr::BinaryOp(l, Op::And, r) = e {
        flatten_and(l, out);
        flatten_and(r, out);
    } else {
        out.push(e);
    }
}

/// Escape a decoded string literal for embedding in a `"..."` literal in either
/// host language.
fn escape_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

fn is_bool_var(e: &Expr, value: bool) -> bool {
    let name = if value { "true" } else { "false" };
    matches!(e, Expr::Variable(v) if v == name)
}

/// Fold trivial `x && true` / `x || false` remnants left by the parser's
/// quantifier stripping (`requires: n >= 0 && forall(...)` keeps `n >= 0 &&
/// true`). Runs bottom-up; conservative — only literal-identity folds.
fn simplify(e: Expr) -> Expr {
    match e {
        Expr::BinaryOp(l, op, r) => {
            let l = Box::new(simplify(*l));
            let r = Box::new(simplify(*r));
            match op {
                Op::And if is_bool_var(&l, true) => *r,
                Op::And if is_bool_var(&r, true) => *l,
                Op::And if is_bool_var(&l, false) || is_bool_var(&r, false) => {
                    Expr::Variable("false".into())
                }
                Op::Or if is_bool_var(&l, false) => *r,
                Op::Or if is_bool_var(&r, false) => *l,
                Op::Or if is_bool_var(&l, true) || is_bool_var(&r, true) => {
                    Expr::Variable("true".into())
                }
                Op::Implies if is_bool_var(&l, true) => *r,
                Op::Implies if is_bool_var(&l, false) || is_bool_var(&r, true) => {
                    Expr::Variable("true".into())
                }
                _ => Expr::BinaryOp(l, op, r),
            }
        }
        Expr::ArrayAccess(name, idx) => Expr::ArrayAccess(name, Box::new(simplify(*idx))),
        Expr::ArrayLit(elems) => Expr::ArrayLit(elems.into_iter().map(simplify).collect()),
        Expr::Call(name, args) => Expr::Call(name, args.into_iter().map(simplify).collect()),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => Expr::IfThenElse {
            cond: Box::new(simplify(*cond)),
            then_branch,
            else_branch,
        },
        other => other,
    }
}

/// Does this expression text contain a float operand, so Python `/` should stay
/// float division instead of `//`?
fn expr_is_floaty(e: &Expr, vars: &ContractVars) -> bool {
    match e {
        Expr::Float(_) => true,
        Expr::Variable(name) => vars.get(name) == ContractVarKind::Float,
        Expr::BinaryOp(l, Op::Add | Op::Sub | Op::Mul | Op::Div | Op::Pow, r) => {
            expr_is_floaty(l, vars) || expr_is_floaty(r, vars)
        }
        _ => false,
    }
}

/// Binding power of a printed node in the target language, for minimal (but
/// semantics-preserving) parenthesization. Binary operators use the target's
/// relative precedence — mumei and both hosts agree on the ordering of all
/// shared ops (`||`/`or` < `&&`/`and` < comparisons < `|` < `^` < `&` < shifts <
/// `+`/`-` < `*`/`/`), with `=>` emitted at `or`-level. Compound-but-atomic
/// forms (calls, indexing, field access, if-expressions, literals) sit above
/// every operator.
fn node_bp(e: &Expr, t: HostTarget) -> u8 {
    const ATOM: u8 = 100;
    match e {
        Expr::BinaryOp(_, op, _) => match op {
            Op::Or | Op::Implies => 1,
            Op::And => 2,
            Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le => {
                if t == HostTarget::Python {
                    4
                } else {
                    3
                }
            }
            Op::BitOr => {
                if t == HostTarget::Python {
                    5
                } else {
                    4
                }
            }
            Op::BitXor => {
                if t == HostTarget::Python {
                    6
                } else {
                    5
                }
            }
            Op::BitAnd => {
                if t == HostTarget::Python {
                    7
                } else {
                    6
                }
            }
            Op::Shl | Op::Shr => {
                if t == HostTarget::Python {
                    8
                } else {
                    7
                }
            }
            Op::Add | Op::Sub => {
                if t == HostTarget::Python {
                    9
                } else {
                    8
                }
            }
            Op::Mul | Op::Div => {
                if t == HostTarget::Python {
                    10
                } else {
                    9
                }
            }
            // Python `**`; never printed for Rust.
            Op::Pow => 12,
        },
        // `not`/`!` desugar form, generic if-expression, calls, indexing, field
        // access — all either self-parenthesized or postfix-tight.
        _ => ATOM,
    }
}

fn emit(e: &Expr, vars: &ContractVars, t: HostTarget, ctx_bp: u8) -> Option<String> {
    let s = emit_raw(e, vars, t)?;
    if node_bp(e, t) < ctx_bp {
        Some(format!("({s})"))
    } else {
        Some(s)
    }
}

/// `then`/`else` arms decode to `false`/`true` exactly when the parser built
/// the `IfThenElse` to encode `!e`.
fn is_not_desugar(then_branch: &Stmt, else_branch: &Stmt) -> bool {
    fn bool_tail(stmt: &Stmt) -> Option<bool> {
        if let Stmt::Expr(Expr::Variable(v), _) = stmt {
            match v.as_str() {
                "true" => return Some(true),
                "false" => return Some(false),
                _ => {}
            }
        }
        None
    }
    bool_tail(then_branch) == Some(false) && bool_tail(else_branch) == Some(true)
}

/// Extract the tail expression of a then/else branch when it is a bare
/// expression statement (the only form a contract-level `if` produces a value
/// in without effects).
fn stmt_tail_expr(stmt: &Stmt) -> Option<&Expr> {
    match stmt {
        Stmt::Expr(e, _) => Some(e),
        _ => None,
    }
}

fn emit_raw(e: &Expr, vars: &ContractVars, t: HostTarget) -> Option<String> {
    Some(match e {
        // Negative literals always print parenthesized: bare `-2` binds
        // wrongly in Python (`-2 ** 2` is `-(2 ** 2)`) and in Rust method
        // calls (`-2.abs()` is `-(2.abs())`).
        Expr::Number(n) => {
            if *n < 0 {
                format!("({n})")
            } else {
                n.to_string()
            }
        }
        Expr::Float(f) => {
            if !f.is_finite() {
                return None;
            }
            if *f < 0.0 {
                format!("({f:?})")
            } else {
                format!("{f:?}")
            }
        }
        Expr::StringLit(s) => format!("\"{}\"", escape_str(s)),
        Expr::Variable(name) => match name.as_str() {
            "true" => {
                if t == HostTarget::Python {
                    "True".into()
                } else {
                    "true".into()
                }
            }
            "false" => {
                if t == HostTarget::Python {
                    "False".into()
                } else {
                    "false".into()
                }
            }
            // A bare `forall`/`exists` identifier or a parser-internal marker
            // variable (recovery placeholder) is never translatable.
            "forall" | "exists" => return None,
            _ if name.starts_with("__mumei_") => return None,
            _ => {
                // `::`-qualified names are mumei module paths, not host
                // paths — print them with the same `::`→`_` mangling the FFI
                // symbol uses (`Vec2::dot` → `Vec2_dot`) instead of emitting a
                // Rust path expression like `std::process::exit`.
                let name = name.replace("::", "_");
                if t == HostTarget::Rust && vars.get(&name) == ContractVarKind::Bool {
                    // FFI bools are i64; coerce to a real Rust bool.
                    format!("({name} != 0)")
                } else {
                    name
                }
            }
        },
        Expr::ArrayLit(elems) => {
            let parts: Option<Vec<String>> = elems.iter().map(|e| emit(e, vars, t, 0)).collect();
            format!("[{}]", parts?.join(", "))
        }
        // Pointer indexing is not a safe runtime check: `arr[i]` on a ctypes
        // pointer or `unsafe { *arr.add(i) }` dereferences a caller-controlled
        // raw pointer with no bounds validation inside a nominally safe
        // wrapper, and `FieldAccess` on an FFI-typed param (i64/c_void_p) is
        // never valid host code. Degrade rather than emit unsound/broken code.
        Expr::ArrayAccess(..) | Expr::FieldAccess(..) => return None,
        Expr::BinaryOp(l, op, r) => return emit_binop(l, op, r, e, vars, t),
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            if is_not_desugar(then_branch, else_branch) {
                // Python `not` binds looser than comparisons but tighter than
                // and/or: operands at comparison level stay bare, boolean
                // combinators get parens.
                let not_ctx = if t == HostTarget::Python { 4 } else { 50 };
                let c = emit(cond, vars, t, not_ctx)?;
                if t == HostTarget::Python {
                    format!("not {c}")
                } else {
                    format!("!{c}")
                }
            } else {
                let cond = emit(cond, vars, t, 0)?;
                let then_e = emit(stmt_tail_expr(then_branch)?, vars, t, 0)?;
                let else_e = emit(stmt_tail_expr(else_branch)?, vars, t, 0)?;
                if t == HostTarget::Python {
                    format!("({then_e} if {cond} else {else_e})")
                } else {
                    format!("(if {cond} {{ {then_e} }} else {{ {else_e} }})")
                }
            }
        }
        Expr::Call(name, args) => return emit_call(name, args, vars, t),
        // Match, StructInit, Block, Lambda, AtomRef, CallRef, Async/Await,
        // Perform, ChanSend/ChanRecv, and anything else — no runtime host
        // counterpart.
        _ => return None,
    })
}

fn emit_binop(
    l_expr: &Expr,
    op: &Op,
    r_expr: &Expr,
    whole: &Expr,
    vars: &ContractVars,
    t: HostTarget,
) -> Option<String> {
    // A string literal can never be an operand of a host comparison or
    // arithmetic (`text == "ok"` on a pointer/int param is a type error in
    // Rust, and `==` on `c_char_p` is never the intended check). Degrade the
    // whole clause instead of emitting wrong code.
    if matches!(l_expr, Expr::StringLit(_)) || matches!(r_expr, Expr::StringLit(_)) {
        return None;
    }
    if *op == Op::Implies {
        // `=>` is left-associative and binds looser than `||`/`&&`; both
        // operands are always parenthesized so nesting and chaining print
        // unambiguously: `a => b` → `not (a) or (b)` / `!(a) || (b)`.
        let l = emit(l_expr, vars, t, 0)?;
        let r = emit(r_expr, vars, t, 0)?;
        return Some(if t == HostTarget::Python {
            format!("not ({l}) or ({r})")
        } else {
            format!("!({l}) || ({r})")
        });
    }
    if *op == Op::Pow && t == HostTarget::Rust {
        // Rust integers have no `**` operator; `.pow(u32)` cannot express
        // negative exponents — degrade instead of emitting wrong code.
        return None;
    }
    if *op == Op::Div && !expr_is_floaty(whole, vars) {
        // Z3 `Int` division — what `verify` proves `Op::Div` against — is
        // EUCLIDEAN: `mod` is always ≥ 0, so the quotient floors for positive
        // divisors and ceils for negative ones (verified against Z3 4.14.1:
        // `div(7,-2) = -3`, `div(-7,-2) = 4`, `div(-7,2) = -4`). Neither
        // Python `//` (always floors: `7 // -2 == -4`) nor Rust `/`
        // (truncates: `-7 / 2 == -3`) matches it for all sign combinations.
        // Emit Euclidean division so a clause proven by `verify` can never
        // mischeck in the host:
        //   Python `divmod(l, r)[0] + (divmod(l, r)[1] < 0)` — adds 1 exactly
        //   when the floored remainder went negative (only possible for a
        //   negative divisor); Rust `a.div_euclid(b)` — the stdlib Euclidean
        //   primitive. Operands emit inside call parens (ctx 0).
        let l = emit(l_expr, vars, t, 0)?;
        let r = emit(r_expr, vars, t, 0)?;
        return Some(if t == HostTarget::Python {
            // `divmod` evaluates the operands ONCE — plain `//`/`%` would
            // duplicate each printed operand, doubling text and evaluations
            // per chained division. The walrus reads quotient and remainder
            // from a single divmod call.
            format!("((_mumei_d := divmod({l}, {r}))[0] + (_mumei_d[1] < 0))")
        } else {
            // `as i64` pins the receiver type: a literal like `100` is an
            // ambiguous `{integer}` that cannot receive a method call, and
            // every integer at the FFI boundary is i64 anyway.
            format!("(({l}) as i64).div_euclid({r})")
        });
    }
    // `x == true` on an FFI bool/int param: the literal normalizes to `1`/`0`
    // when the other operand does not itself emit a bool, keeping the
    // comparison type-correct (`i64 == bool` does not compile).
    let normalize_bool_literal = |e: &Expr, text: &mut String, other_bool: bool| {
        if t == HostTarget::Rust
            && matches!(*op, Op::Eq | Op::Neq)
            && !other_bool
            && matches!(e, Expr::Variable(v) if v == "true" || v == "false")
        {
            *text = if is_bool_var(e, true) { "1" } else { "0" }.to_string();
        }
    };
    let sym: &str = match op {
        Op::Add => "+",
        Op::Sub => "-",
        Op::Mul => "*",
        Op::Div => "/",
        Op::Pow => "**",
        Op::Eq => "==",
        Op::Neq => "!=",
        Op::Gt => ">",
        Op::Lt => "<",
        Op::Ge => ">=",
        Op::Le => "<=",
        Op::And => {
            if t == HostTarget::Python {
                " and "
            } else {
                " && "
            }
        }
        Op::Or => {
            if t == HostTarget::Python {
                " or "
            } else {
                " || "
            }
        }
        Op::BitAnd => "&",
        Op::BitOr => "|",
        Op::BitXor => "^",
        Op::Shl => "<<",
        Op::Shr => ">>",
        Op::Implies => unreachable!(),
    };
    let bp = node_bp(whole, t);
    let (lctx, rctx) = match *op {
        // Comparisons are non-associative (the parser already flattened chains
        // into `&&`), so a nested comparison on either side needs parens.
        Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le => (bp + 1, bp + 1),
        // `**` is right-associative.
        Op::Pow => (bp + 1, bp),
        _ => (bp, bp + 1),
    };
    let mut l = emit(l_expr, vars, t, lctx)?;
    let mut r = emit(r_expr, vars, t, rctx)?;
    normalize_bool_literal(l_expr, &mut l, emits_bool(r_expr, vars));
    normalize_bool_literal(r_expr, &mut r, emits_bool(l_expr, vars));
    let s = if sym.starts_with(' ') {
        format!("{l}{sym}{r}")
    } else {
        format!("{l} {sym} {r}")
    };
    Some(s)
}

/// Does the printed form of `e` have host type `bool`? At the FFI boundary
/// mumei `bool` vars and `bool` results are i64, so only a genuinely boolean
/// printed expression counts: comparisons/`&&`/`||`/`=>`, `!e`, `true`/`false`,
/// and `bool`-kind variables (which print as `(name != 0)` in Rust).
fn emits_bool(e: &Expr, vars: &ContractVars) -> bool {
    match e {
        Expr::Variable(v) => v == "true" || v == "false" || vars.get(v) == ContractVarKind::Bool,
        Expr::BinaryOp(_, op, _) => matches!(
            op,
            Op::And | Op::Or | Op::Implies | Op::Eq | Op::Neq | Op::Gt | Op::Lt | Op::Ge | Op::Le
        ),
        Expr::IfThenElse {
            then_branch,
            else_branch,
            ..
        } => {
            is_not_desugar(then_branch, else_branch)
                || (stmt_tail_expr(then_branch).is_some_and(|e| emits_bool(e, vars))
                    && stmt_tail_expr(else_branch).is_some_and(|e| emits_bool(e, vars)))
        }
        _ => false,
    }
}

fn emit_call(name: &str, args: &[Expr], vars: &ContractVars, t: HostTarget) -> Option<String> {
    // Quantifiers that stayed inside an expression (under `||`, `!`, `=>`, an
    // `if`, a `match`, or inside an `ensures`) reach us as `forall(v, s, e, c)`
    // calls. The verifier's range is `[s, e)` which matches `range`/`..`.
    if matches!(name, "forall" | "exists") {
        let (var, start, end, cond) = match args {
            [Expr::Variable(v), start, end, cond] => (v, start, end, cond),
            _ => return None,
        };
        let inner = vars.with_bound(var);
        let start = emit(start, vars, t, 0)?;
        let end = emit(end, vars, t, 0)?;
        let cond = emit(cond, &inner, t, 0)?;
        return Some(match (name, t) {
            ("forall", HostTarget::Python) => {
                format!("all({cond} for {var} in range({start}, {end}))")
            }
            ("exists", HostTarget::Python) => {
                format!("any({cond} for {var} in range({start}, {end}))")
            }
            ("forall", HostTarget::Rust) => {
                format!("(({start})..({end})).all(|{var}| {cond})")
            }
            ("exists", HostTarget::Rust) => {
                format!("(({start})..({end})).any(|{var}| {cond})")
            }
            _ => return None,
        });
    }
    let parts: Option<Vec<String>> = args.iter().map(|a| emit(a, vars, t, 0)).collect();
    let parts = parts?;
    // `min`/`max` appear in quantifier bounds; Rust exposes them as methods.
    if matches!(name, "min" | "max") && t == HostTarget::Rust && parts.len() == 2 {
        return Some(format!("({}).{}({})", parts[0], name, parts[1]));
    }
    let args = parts.join(", ");
    // Same `::`→`_` mangling as variables: `std::process::exit` becomes the
    // (nonexistent) symbol `std_process_exit`, never a host path call.
    Some(match t {
        HostTarget::Python | HostTarget::Rust => {
            format!("{}({})", name.replace("::", "_"), args)
        }
    })
}

/// Reconstruct the canonical `forall(v, s, e, c)` / `exists(...)` source text of
/// a hoisted quantifier — used both as the assert/check message and as the
/// input re-parsed by `contract_text_to_host`.
pub fn quantifier_source_text(q: &crate::hir::HirQuantifier) -> String {
    let name = match q.kind {
        crate::hir::HirQuantifierKind::ForAll => "forall",
        crate::hir::HirQuantifierKind::Exists => "exists",
    };
    format!(
        "{}({}, {}, {}, {})",
        name, q.var, q.start, q.end, q.condition
    )
}

/// Translate a hoisted contract quantifier into a host-language check, or
/// `None` when any of its parts (var/start/end/condition) is untranslatable.
pub fn quantifier_to_host(
    q: &crate::hir::HirQuantifier,
    vars: &ContractVars,
    target: HostTarget,
) -> Option<String> {
    contract_text_to_host(&quantifier_source_text(q), vars, target)
}
