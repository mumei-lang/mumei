use super::module_env::ModuleEnv;
use super::translator::{
    apply_refinement_constraint, expr_to_z3, param_z3_value, stmt_to_z3, VCtx,
    DEFAULT_CONSTRAINT_BUDGET,
};
use super::types::Env;
use super::{
    parse_body_expr, parse_expression, Atom, Bool, Config, Dynamic, Int, SatResult, Solver,
};
use crate::parser::{Expr, Op, Param, RefinedType, Stmt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};

const DEFAULT_INTEGER_BOUND: i64 = 100;
const MAX_ARRAY_LEN: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyBasedTestConfig {
    pub test_count: usize,
    pub max_shrink_steps: usize,
    pub seed: u64,
    pub include_boundary_values: bool,
}

impl Default for PropertyBasedTestConfig {
    fn default() -> Self {
        Self {
            test_count: 100,
            max_shrink_steps: 64,
            seed: 0x004d_554d_4549,
            include_boundary_values: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PropertyBasedTestResult {
    pub status: String,
    pub tests_run: usize,
    pub failures_found: usize,
    pub shrink_steps: usize,
    pub shrunk_counterexample: Option<BTreeMap<String, GeneratedValue>>,
    pub diagnostics: Vec<String>,
}

impl PropertyBasedTestResult {
    pub fn passed(tests_run: usize, diagnostics: Vec<String>) -> Self {
        Self {
            status: "passed".to_string(),
            tests_run,
            failures_found: 0,
            shrink_steps: 0,
            shrunk_counterexample: None,
            diagnostics,
        }
    }

    fn failed(
        tests_run: usize,
        shrink_steps: usize,
        counterexample: HashMap<String, GeneratedValue>,
        diagnostic: String,
    ) -> Self {
        Self {
            status: "failed".to_string(),
            tests_run,
            failures_found: 1,
            shrink_steps,
            shrunk_counterexample: Some(sorted_assignment(counterexample)),
            diagnostics: vec![diagnostic],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum GeneratedValue {
    Int(i64),
    Bool(bool),
    Array(Vec<GeneratedValue>),
}

impl GeneratedValue {
    fn size(&self) -> i64 {
        match self {
            GeneratedValue::Int(value) => value.abs(),
            GeneratedValue::Bool(value) => i64::from(*value),
            GeneratedValue::Array(values) => {
                values.len() as i64 + values.iter().map(GeneratedValue::size).sum::<i64>()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenerationContext<'a> {
    pub module_env: &'a ModuleEnv,
    #[allow(dead_code)]
    pub config: &'a PropertyBasedTestConfig,
}

pub trait InputGenerator {
    fn generate(&self, rng: &mut DeterministicRng) -> GeneratedValue;
    fn boundary_values(&self) -> Vec<GeneratedValue>;
}

pub trait Shrinker {
    fn shrink_candidates(&self, value: &GeneratedValue) -> Vec<GeneratedValue>;
}

#[derive(Debug, Clone)]
pub struct IntRangeGenerator {
    pub min: i64,
    pub max: i64,
}

impl IntRangeGenerator {
    pub fn new(min: i64, max: i64) -> Self {
        if min <= max {
            Self { min, max }
        } else {
            Self { min: max, max: min }
        }
    }
}

impl InputGenerator for IntRangeGenerator {
    fn generate(&self, rng: &mut DeterministicRng) -> GeneratedValue {
        GeneratedValue::Int(rng.gen_range(self.min, self.max))
    }

    fn boundary_values(&self) -> Vec<GeneratedValue> {
        let mut values = vec![self.min, self.max, 0];
        values.push(self.min.saturating_add(1).min(self.max));
        values.push(self.max.saturating_sub(1).max(self.min));
        values.retain(|value| *value >= self.min && *value <= self.max);
        values.sort_unstable();
        values.dedup();
        values.into_iter().map(GeneratedValue::Int).collect()
    }
}

impl Shrinker for IntRangeGenerator {
    fn shrink_candidates(&self, value: &GeneratedValue) -> Vec<GeneratedValue> {
        let GeneratedValue::Int(current) = value else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        for target in [0, self.min, self.max] {
            if target != *current && target >= self.min && target <= self.max {
                candidates.push(GeneratedValue::Int(target));
            }
        }

        let mut cursor = *current;
        for _ in 0..32 {
            let next = if cursor > 0 {
                cursor / 2
            } else {
                (cursor + 1) / 2
            };
            if next == cursor {
                break;
            }
            if next >= self.min
                && next <= self.max
                && next != *current
                && next.signum() == current.signum()
            {
                candidates.push(GeneratedValue::Int(next));
            }
            cursor = next;
        }
        candidates.sort_by_key(GeneratedValue::size);
        candidates.dedup();
        candidates
    }
}

#[derive(Debug, Clone)]
pub struct BoolGenerator;

impl InputGenerator for BoolGenerator {
    fn generate(&self, rng: &mut DeterministicRng) -> GeneratedValue {
        GeneratedValue::Bool(rng.next_u64().is_multiple_of(2))
    }

    fn boundary_values(&self) -> Vec<GeneratedValue> {
        vec![GeneratedValue::Bool(false), GeneratedValue::Bool(true)]
    }
}

impl Shrinker for BoolGenerator {
    fn shrink_candidates(&self, value: &GeneratedValue) -> Vec<GeneratedValue> {
        match value {
            GeneratedValue::Bool(true) => vec![GeneratedValue::Bool(false)],
            _ => Vec::new(),
        }
    }
}

pub struct ArrayGenerator {
    element: Box<dyn GeneratorShrink>,
    max_len: usize,
}

impl ArrayGenerator {
    pub fn new(element: Box<dyn GeneratorShrink>, max_len: usize) -> Self {
        Self { element, max_len }
    }
}

impl std::fmt::Debug for ArrayGenerator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArrayGenerator")
            .field("max_len", &self.max_len)
            .finish_non_exhaustive()
    }
}

impl InputGenerator for ArrayGenerator {
    fn generate(&self, rng: &mut DeterministicRng) -> GeneratedValue {
        let len = rng.gen_usize(self.max_len);
        GeneratedValue::Array((0..len).map(|_| self.element.generate(rng)).collect())
    }

    fn boundary_values(&self) -> Vec<GeneratedValue> {
        let element_boundaries = self.element.boundary_values();
        let mut arrays = vec![GeneratedValue::Array(Vec::new())];
        if let Some(first) = element_boundaries.first() {
            arrays.push(GeneratedValue::Array(vec![first.clone()]));
        }
        if !element_boundaries.is_empty() {
            arrays.push(GeneratedValue::Array(element_boundaries));
        }
        arrays
    }
}

impl Shrinker for ArrayGenerator {
    fn shrink_candidates(&self, value: &GeneratedValue) -> Vec<GeneratedValue> {
        let GeneratedValue::Array(values) = value else {
            return Vec::new();
        };
        let mut candidates = Vec::new();
        if !values.is_empty() {
            candidates.push(GeneratedValue::Array(Vec::new()));
            candidates.push(GeneratedValue::Array(values[..values.len() / 2].to_vec()));
            candidates.push(GeneratedValue::Array(values[..values.len() - 1].to_vec()));
        }
        for index in 0..values.len() {
            for shrunk in self.element.shrink_candidates(&values[index]) {
                let mut next = values.clone();
                next[index] = shrunk;
                candidates.push(GeneratedValue::Array(next));
            }
        }
        candidates.sort_by_key(GeneratedValue::size);
        candidates.dedup();
        candidates
    }
}

pub trait GeneratorShrink: InputGenerator + Shrinker {}

impl<T> GeneratorShrink for T where T: InputGenerator + Shrinker {}

pub struct GeneratedInputs {
    generators: Vec<(String, Box<dyn GeneratorShrink>)>,
}

impl GeneratedInputs {
    fn new(generators: Vec<(String, Box<dyn GeneratorShrink>)>) -> Self {
        Self { generators }
    }

    pub fn is_empty(&self) -> bool {
        self.generators.is_empty()
    }

    pub fn boundary_assignments(&self) -> Vec<HashMap<String, GeneratedValue>> {
        boundary_assignments(self)
    }

    pub fn random_assignment(&self, rng: &mut DeterministicRng) -> HashMap<String, GeneratedValue> {
        random_assignment(self, rng)
    }

    fn iter(&self) -> impl Iterator<Item = &(String, Box<dyn GeneratorShrink>)> {
        self.generators.iter()
    }
}

impl std::fmt::Debug for GeneratedInputs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedInputs")
            .field("len", &self.generators.len())
            .finish()
    }
}

#[derive(Debug, Clone)]
pub struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    pub fn new(seed: u64) -> Self {
        Self { state: seed.max(1) }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn gen_range(&mut self, min: i64, max: i64) -> i64 {
        if min >= max {
            return min;
        }
        let width = (max as i128 - min as i128 + 1) as u64;
        min.saturating_add((self.next_u64() % width) as i64)
    }

    fn gen_usize(&mut self, max: usize) -> usize {
        if max == 0 {
            0
        } else {
            (self.next_u64() as usize) % (max + 1)
        }
    }
}

pub fn synthesize_input_generator(
    type_name: Option<&str>,
    ctx: &GenerationContext<'_>,
) -> Box<dyn GeneratorShrink> {
    match type_name {
        Some(name) if is_array_type(name) => {
            let element_type = array_element_type(name);
            Box::new(ArrayGenerator::new(
                synthesize_input_generator(Some(element_type), ctx),
                MAX_ARRAY_LEN,
            ))
        }
        Some("bool") | Some("Bool") => Box::new(BoolGenerator),
        Some(type_name) => {
            let base_type = ctx.module_env.resolve_base_type(type_name);
            if is_array_type(&base_type) {
                let element_type = array_element_type(&base_type);
                return Box::new(ArrayGenerator::new(
                    synthesize_input_generator(Some(element_type), ctx),
                    MAX_ARRAY_LEN,
                ));
            }
            if let Some(refined) = ctx.module_env.get_type(type_name) {
                return Box::new(generator_from_refinement(refined));
            }
            Box::new(generator_from_type_and_predicates(
                Some(type_name),
                &[],
                &[],
            ))
        }
        None => Box::new(IntRangeGenerator::new(
            -DEFAULT_INTEGER_BOUND,
            DEFAULT_INTEGER_BOUND,
        )),
    }
}

pub fn generator_from_refinement(refined: &RefinedType) -> IntRangeGenerator {
    let parsed = parse_expression(&refined.predicate_raw);
    let mut bounds = Bounds::default();
    infer_bounds_from_expr(&parsed, &refined.operand, &mut bounds);
    IntRangeGenerator::new(bounds.min, bounds.max)
}

pub fn synthesize_input_generators(atom: &Atom, module_env: &ModuleEnv) -> GeneratedInputs {
    let config = PropertyBasedTestConfig::default();
    let ctx = GenerationContext {
        module_env,
        config: &config,
    };
    build_generators(atom, module_env, &ctx)
}

pub fn check_generated_assignment(
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
) -> Option<String> {
    check_assignment(atom, module_env, assignment)
}

pub fn shrink_counterexample(
    atom: &Atom,
    module_env: &ModuleEnv,
    generators: &GeneratedInputs,
    initial: HashMap<String, GeneratedValue>,
    config: &PropertyBasedTestConfig,
) -> (HashMap<String, GeneratedValue>, usize) {
    shrink_assignment(atom, module_env, generators, initial, config)
}

pub fn run_property_based_test(
    atom: &Atom,
    module_env: &ModuleEnv,
    config: &PropertyBasedTestConfig,
) -> PropertyBasedTestResult {
    if !atom.type_params.is_empty() {
        return PropertyBasedTestResult::passed(
            0,
            vec!["property-based validation skipped for generic atom".to_string()],
        );
    }

    let ctx = GenerationContext { module_env, config };
    let generators = build_generators(atom, module_env, &ctx);
    if generators.is_empty() {
        return PropertyBasedTestResult::passed(
            0,
            vec!["property-based validation skipped because no supported inputs were found".into()],
        );
    }

    let mut rng = DeterministicRng::new(config.seed);
    let mut tests_run = 0usize;
    let boundary_cases = if config.include_boundary_values {
        boundary_assignments(&generators)
    } else {
        Vec::new()
    };

    for assignment in boundary_cases {
        tests_run += 1;
        if let Some(diagnostic) = check_assignment(atom, module_env, &assignment) {
            let (shrunk, shrink_steps) =
                shrink_assignment(atom, module_env, &generators, assignment, config);
            return PropertyBasedTestResult::failed(tests_run, shrink_steps, shrunk, diagnostic);
        }
    }

    for _ in 0..config.test_count {
        tests_run += 1;
        let assignment = random_assignment(&generators, &mut rng);
        if let Some(diagnostic) = check_assignment(atom, module_env, &assignment) {
            let (shrunk, shrink_steps) =
                shrink_assignment(atom, module_env, &generators, assignment, config);
            return PropertyBasedTestResult::failed(tests_run, shrink_steps, shrunk, diagnostic);
        }
    }

    PropertyBasedTestResult::passed(
        tests_run,
        vec![format!(
            "property-based validation passed {} generated input(s)",
            tests_run
        )],
    )
}

fn build_generators(
    atom: &Atom,
    module_env: &ModuleEnv,
    ctx: &GenerationContext<'_>,
) -> GeneratedInputs {
    GeneratedInputs::new(
        atom.params
            .iter()
            .map(|param| {
                let generator = generator_for_param(param, atom, module_env, ctx);
                (param.name.clone(), generator)
            })
            .collect(),
    )
}

fn generator_for_param(
    param: &Param,
    atom: &Atom,
    module_env: &ModuleEnv,
    ctx: &GenerationContext<'_>,
) -> Box<dyn GeneratorShrink> {
    if let Some(type_name) = param.type_name.as_deref() {
        if let Some(refined) = module_env.get_type(type_name) {
            return Box::new(generator_from_refinement(refined));
        }
    }
    let predicates = [atom.requires.as_str(), atom.ensures.as_str()];
    let generator: Box<dyn GeneratorShrink> = Box::new(generator_from_type_and_predicates(
        param.type_name.as_deref(),
        &predicates,
        &[param.name.as_str()],
    ));
    generator.or_else_array(param, ctx)
}

trait ArrayFallback {
    fn or_else_array(self, param: &Param, ctx: &GenerationContext<'_>) -> Box<dyn GeneratorShrink>;
}

impl ArrayFallback for Box<dyn GeneratorShrink> {
    fn or_else_array(self, param: &Param, ctx: &GenerationContext<'_>) -> Box<dyn GeneratorShrink> {
        let type_name = param.type_name.as_deref();
        if type_name.is_some_and(is_array_type) {
            synthesize_input_generator(type_name, ctx)
        } else {
            self
        }
    }
}

fn generator_from_type_and_predicates(
    type_name: Option<&str>,
    predicates: &[&str],
    variable_names: &[&str],
) -> IntRangeGenerator {
    let mut bounds = Bounds::for_type(type_name);
    for predicate in predicates {
        let parsed = parse_expression(predicate);
        for variable_name in variable_names {
            infer_bounds_from_expr(&parsed, variable_name, &mut bounds);
        }
    }
    IntRangeGenerator::new(bounds.min, bounds.max)
}

#[derive(Debug, Clone, Copy)]
struct Bounds {
    min: i64,
    max: i64,
}

impl Default for Bounds {
    fn default() -> Self {
        Self {
            min: -DEFAULT_INTEGER_BOUND,
            max: DEFAULT_INTEGER_BOUND,
        }
    }
}

impl Bounds {
    fn for_type(type_name: Option<&str>) -> Self {
        match type_name {
            Some("u64") | Some("usize") => Self {
                min: 0,
                max: DEFAULT_INTEGER_BOUND,
            },
            _ => Self::default(),
        }
    }
}

fn infer_bounds_from_expr(expr: &Expr, variable_name: &str, bounds: &mut Bounds) {
    match expr {
        Expr::BinaryOp(left, Op::And, right) => {
            infer_bounds_from_expr(left, variable_name, bounds);
            infer_bounds_from_expr(right, variable_name, bounds);
        }
        Expr::BinaryOp(left, Op::Or, right) => {
            let mut left_bounds = *bounds;
            let mut right_bounds = *bounds;
            infer_bounds_from_expr(left, variable_name, &mut left_bounds);
            infer_bounds_from_expr(right, variable_name, &mut right_bounds);
            bounds.min = bounds.min.max(left_bounds.min.min(right_bounds.min));
            bounds.max = bounds.max.min(left_bounds.max.max(right_bounds.max));
        }
        Expr::BinaryOp(left, op, right) => {
            apply_comparison_bound(left, op.clone(), right, variable_name, bounds)
        }
        Expr::Call(_, args) => {
            for arg in args {
                infer_bounds_from_expr(arg, variable_name, bounds);
            }
        }
        Expr::IfThenElse {
            cond,
            then_branch,
            else_branch,
        } => {
            infer_bounds_from_expr(cond, variable_name, bounds);
            infer_bounds_from_stmt(then_branch, variable_name, bounds);
            infer_bounds_from_stmt(else_branch, variable_name, bounds);
        }
        _ => {}
    }
}

fn infer_bounds_from_stmt(stmt: &Stmt, variable_name: &str, bounds: &mut Bounds) {
    match stmt {
        Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
            infer_bounds_from_expr(value, variable_name, bounds)
        }
        Stmt::Expr(value, _) => infer_bounds_from_expr(value, variable_name, bounds),
        Stmt::ArrayStore { index, value, .. } => {
            infer_bounds_from_expr(index, variable_name, bounds);
            infer_bounds_from_expr(value, variable_name, bounds);
        }
        Stmt::Block(stmts, _)
        | Stmt::TaskGroup {
            children: stmts, ..
        } => {
            for stmt in stmts {
                infer_bounds_from_stmt(stmt, variable_name, bounds);
            }
        }
        Stmt::While {
            cond,
            invariant,
            body,
            ..
        } => {
            infer_bounds_from_expr(cond, variable_name, bounds);
            infer_bounds_from_expr(invariant, variable_name, bounds);
            infer_bounds_from_stmt(body, variable_name, bounds);
        }
        Stmt::Acquire { body, .. } | Stmt::Task { body, .. } => {
            infer_bounds_from_stmt(body, variable_name, bounds);
        }
        Stmt::Cancel { .. } => {}
    }
}

fn apply_comparison_bound(
    left: &Expr,
    op: Op,
    right: &Expr,
    variable_name: &str,
    bounds: &mut Bounds,
) {
    if let Expr::Variable(name) = left {
        if name == variable_name {
            if let Some(value) = literal_or_named_bound(right) {
                update_bound(bounds, op, value, true);
                return;
            }
        }
    }
    if let Expr::Variable(name) = right {
        if name == variable_name {
            if let Some(value) = literal_or_named_bound(left) {
                update_bound(bounds, op, value, false);
            }
        }
    }
}

fn update_bound(bounds: &mut Bounds, op: Op, value: i64, variable_on_left: bool) {
    match (op, variable_on_left) {
        (Op::Ge, true) | (Op::Le, false) => bounds.min = bounds.min.max(value),
        (Op::Gt, true) | (Op::Lt, false) => bounds.min = bounds.min.max(value.saturating_add(1)),
        (Op::Le, true) | (Op::Ge, false) => bounds.max = bounds.max.min(value),
        (Op::Lt, true) | (Op::Gt, false) => bounds.max = bounds.max.min(value.saturating_sub(1)),
        (Op::Eq, _) => {
            bounds.min = bounds.min.max(value);
            bounds.max = bounds.max.min(value);
        }
        _ => {}
    }
}

fn literal_or_named_bound(expr: &Expr) -> Option<i64> {
    match expr {
        Expr::Number(value) => Some(*value),
        Expr::Variable(name) => match name.as_str() {
            "MIN" => Some(-DEFAULT_INTEGER_BOUND),
            "MAX" | "LIMIT" => Some(DEFAULT_INTEGER_BOUND),
            _ => None,
        },
        _ => None,
    }
}

fn boundary_assignments(generators: &GeneratedInputs) -> Vec<HashMap<String, GeneratedValue>> {
    let mut assignments = Vec::new();
    let mut base = HashMap::new();
    for (name, generator) in generators.iter() {
        let value = generator
            .boundary_values()
            .into_iter()
            .next()
            .unwrap_or(GeneratedValue::Int(0));
        base.insert(name.clone(), value);
    }
    assignments.push(base.clone());

    for (name, generator) in generators.iter() {
        for value in generator.boundary_values() {
            let mut assignment = base.clone();
            assignment.insert(name.clone(), value);
            assignments.push(assignment);
        }
    }
    assignments
}

fn random_assignment(
    generators: &GeneratedInputs,
    rng: &mut DeterministicRng,
) -> HashMap<String, GeneratedValue> {
    generators
        .iter()
        .map(|(name, generator)| (name.clone(), generator.generate(rng)))
        .collect()
}

fn check_assignment(
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
) -> Option<String> {
    if !assignment_satisfies_preconditions(atom, module_env, assignment) {
        return None;
    }
    let result = evaluate_body(atom, module_env, assignment)?;
    if assignment_satisfies_ensures(atom, module_env, assignment, &result) {
        None
    } else {
        Some(format!(
            "property-based validation found counterexample for '{}': inputs={:?}, result={:?}",
            atom.name, assignment, result
        ))
    }
}

fn assignment_satisfies_preconditions(
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
) -> bool {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(1000);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(&ctx, module_env, atom);
    let mut env = seed_concrete_env(&ctx, atom, module_env, assignment, None);
    if assert_parameter_refinements(&vc, &solver, atom, module_env, &mut env).is_err() {
        return false;
    }
    if assert_clause(&vc, &solver, &mut env, &atom.requires).is_err() {
        return false;
    }
    matches!(solver.check(), SatResult::Sat)
}

fn assignment_satisfies_ensures(
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
    result: &GeneratedValue,
) -> bool {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(1000);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(&ctx, module_env, atom);
    let mut env = seed_concrete_env(&ctx, atom, module_env, assignment, Some(result));
    if assert_clause(&vc, &solver, &mut env, &atom.ensures).is_err() {
        return true;
    }
    matches!(solver.check(), SatResult::Sat)
}

fn evaluate_body(
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
) -> Option<GeneratedValue> {
    let mut cfg = Config::new();
    cfg.set_timeout_msec(1000);
    let ctx = super::Context::new(&cfg);
    let solver = Solver::new(&ctx);
    let vc = validation_ctx(&ctx, module_env, atom);
    let mut env = seed_concrete_env(&ctx, atom, module_env, assignment, None);
    let body = parse_body_expr(&atom.body_expr);
    let value = stmt_to_z3(&vc, &body, &mut env, Some(&solver)).ok()?;
    if !matches!(solver.check(), SatResult::Sat) {
        return None;
    }
    dynamic_to_generated(&value, &solver)
}

fn dynamic_to_generated(value: &Dynamic<'_>, solver: &Solver<'_>) -> Option<GeneratedValue> {
    if let Some(int_value) = value.as_int() {
        let model = solver.get_model()?;
        let evaluated = model.eval(&int_value, true)?;
        return evaluated.as_i64().map(GeneratedValue::Int);
    }
    if let Some(bool_value) = value.as_bool() {
        let model = solver.get_model()?;
        let evaluated = model.eval(&bool_value, true)?;
        return evaluated.as_bool().map(GeneratedValue::Bool);
    }
    None
}

fn shrink_assignment(
    atom: &Atom,
    module_env: &ModuleEnv,
    generators: &GeneratedInputs,
    initial: HashMap<String, GeneratedValue>,
    config: &PropertyBasedTestConfig,
) -> (HashMap<String, GeneratedValue>, usize) {
    let mut current = initial;
    let mut steps = 0usize;
    let mut changed = true;
    while changed && steps < config.max_shrink_steps {
        changed = false;
        for (name, generator) in generators.iter() {
            let Some(value) = current.get(name).cloned() else {
                continue;
            };
            for candidate in generator.shrink_candidates(&value) {
                if candidate.size() >= value.size() {
                    continue;
                }
                let mut next = current.clone();
                next.insert(name.clone(), candidate);
                steps += 1;
                if check_assignment(atom, module_env, &next).is_some() {
                    current = next;
                    changed = true;
                    break;
                }
                if steps >= config.max_shrink_steps {
                    break;
                }
            }
            if changed || steps >= config.max_shrink_steps {
                break;
            }
        }
    }
    (current, steps)
}

fn sorted_assignment(
    assignment: HashMap<String, GeneratedValue>,
) -> BTreeMap<String, GeneratedValue> {
    assignment.into_iter().collect()
}

fn validation_ctx<'a>(
    ctx: &'a super::Context,
    module_env: &'a ModuleEnv,
    atom: &'a Atom,
) -> VCtx<'a> {
    VCtx {
        ctx,
        module_env,
        current_atom: Some(atom),
        linearity_ctx: None,
        effect_ctx: None,
        constraint_count: None,
        constraint_budget: DEFAULT_CONSTRAINT_BUDGET,
        has_string_constraints: None,
        path_cond_stack: std::cell::RefCell::new(Vec::new()),
        profiler: None,
        ieee754_f64: false,
    }
}

fn seed_concrete_env<'a>(
    ctx: &'a super::Context,
    atom: &Atom,
    module_env: &ModuleEnv,
    assignment: &HashMap<String, GeneratedValue>,
    result: Option<&GeneratedValue>,
) -> Env<'a> {
    let mut env: Env<'a> = HashMap::new();
    env.insert("true".to_string(), Bool::from_bool(ctx, true).into());
    env.insert("false".to_string(), Bool::from_bool(ctx, false).into());
    for param in &atom.params {
        let value = assignment
            .get(&param.name)
            .map(|value| value_to_dynamic(ctx, &param.name, value))
            .unwrap_or_else(|| {
                param_z3_value(
                    ctx,
                    &param.name,
                    param.type_name.as_deref(),
                    module_env,
                    false,
                )
            });
        env.insert(param.name.clone(), value.clone());
        if let Some(GeneratedValue::Array(values)) = assignment.get(&param.name) {
            env.insert(format!("__z3_arr_{}", param.name), value);
            env.insert(
                format!("len_{}", param.name),
                Int::from_i64(ctx, values.len() as i64).into(),
            );
        }
    }
    let result_value = result
        .map(|value| value_to_dynamic(ctx, "result", value))
        .unwrap_or_else(|| {
            param_z3_value(
                ctx,
                "result",
                atom.return_type.as_deref(),
                module_env,
                false,
            )
        });
    env.insert("result".to_string(), result_value);
    env
}

fn value_to_dynamic<'a>(
    ctx: &'a super::Context,
    name: &str,
    value: &GeneratedValue,
) -> Dynamic<'a> {
    match value {
        GeneratedValue::Int(value) => Int::from_i64(ctx, *value).into(),
        GeneratedValue::Bool(value) => Bool::from_bool(ctx, *value).into(),
        GeneratedValue::Array(values) => {
            let array = z3::ast::Array::new_const(
                ctx,
                format!("{}_concrete", name),
                &z3::Sort::int(ctx),
                &z3::Sort::int(ctx),
            );
            let mut concrete = array;
            for (index, value) in values.iter().enumerate() {
                if let GeneratedValue::Int(element) = value {
                    concrete = concrete.store(
                        &Int::from_i64(ctx, index as i64),
                        &Int::from_i64(ctx, *element),
                    );
                }
            }
            concrete.into()
        }
    }
}

fn assert_parameter_refinements<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    atom: &Atom,
    module_env: &ModuleEnv,
    env: &mut Env<'a>,
) -> Result<(), String> {
    for param in &atom.params {
        let Some(type_name) = param.type_name.as_deref() else {
            continue;
        };
        let Some(refined) = module_env.get_type(type_name) else {
            continue;
        };
        apply_refinement_constraint(vc, solver, &param.name, refined, env)
            .map_err(|err| err.to_string())?;
    }
    Ok(())
}

fn assert_clause<'a>(
    vc: &VCtx<'a>,
    solver: &Solver<'a>,
    env: &mut Env<'a>,
    clause: &str,
) -> Result<(), String> {
    let trimmed = clause.trim();
    if trimmed.is_empty() || trimmed == "true" {
        return Ok(());
    }
    let clause_ast = parse_expression(trimmed);
    let clause_z3 = expr_to_z3(vc, &clause_ast, env, None).map_err(|err| err.to_string())?;
    let Some(clause_bool) = clause_z3.as_bool() else {
        return Err(format!("clause '{trimmed}' did not evaluate to bool"));
    };
    solver.assert(&clause_bool);
    Ok(())
}

fn is_array_type(type_name: &str) -> bool {
    type_name.starts_with('[') && type_name.ends_with(']')
}

fn array_element_type(type_name: &str) -> &str {
    type_name
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .map(str::trim)
        .unwrap_or("i64")
}
