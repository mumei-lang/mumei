use crate::agent;
use crate::pipeline::*;
use mumei_core::hir::lower_atom_to_hir_with_env;
use mumei_core::{parser, resolver, verification};
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) const REPL_EVAL_ATOM: &str = "__repl_eval";

pub(crate) struct ReplContext<'ctx> {
    module_env: verification::ModuleEnv,
    jit_engine: Option<mumei_emit_llvm::jit::JitEngine<'ctx>>,
    extern_blocks: Vec<parser::ExternBlock>,
}

pub(crate) enum ReplAction {
    Continue,
    Quit,
}

enum ReplAgentCommand {
    ValidateSpec {
        input: PathBuf,
        display_input: String,
    },
    ValidateCode {
        input: PathBuf,
        language: String,
        display_input: String,
    },
}

fn repl_print_json_value(value: &serde_json::Value) {
    let rendered = serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    for line in rendered.lines() {
        println!("    {}", line);
    }
}

fn repl_print_json_array(name: &str, values: &[serde_json::Value]) {
    println!("  {}:", name);
    if values.is_empty() {
        println!("    []");
        return;
    }
    for value in values {
        repl_print_json_value(value);
    }
}

fn repl_first_next_step_command(report: &agent::AgentReport) -> Option<&str> {
    report.next_steps.first().and_then(|step| {
        step.get("command")
            .and_then(serde_json::Value::as_str)
            .or_else(|| step.as_str())
    })
}

fn repl_prompt_for_fix(report: &agent::AgentReport) {
    if report.success {
        return;
    }
    print!("  修正しますか？ (y/n) ");
    if let Err(err) = std::io::stdout().flush() {
        eprintln!("  ❌ Prompt error: {}", err);
        return;
    }

    let mut answer = String::new();
    match std::io::stdin().read_line(&mut answer) {
        Ok(_) if answer.trim().eq_ignore_ascii_case("y") => {
            if let Some(command) = repl_first_next_step_command(report) {
                println!("  next_steps[0].command: {}", command);
            } else {
                println!("  next_steps[0].command: <none>");
            }
            println!("  自動実行はしていません。修正後に再検証してください。");
        }
        Ok(_) => {
            println!("  継続します。");
        }
        Err(err) => {
            eprintln!("  ❌ Prompt error: {}", err);
        }
    }
}

fn repl_run_agent(command: ReplAgentCommand) -> Result<agent::AgentReport, String> {
    match command {
        ReplAgentCommand::ValidateSpec {
            input,
            display_input,
        } => {
            println!(
                "  mumei-agent validate-spec --input {} --format json",
                display_input
            );
            agent::validate_spec(&input)
        }
        ReplAgentCommand::ValidateCode {
            input,
            language,
            display_input,
        } => {
            println!(
                "  mumei-agent validate-code --input {} --language {}",
                display_input, language
            );
            agent::validate_code(&input, &language)
        }
    }
}

fn repl_inline_spec_path(input: &str) -> Result<PathBuf, String> {
    let mut path = std::env::temp_dir();
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|err| format!("failed to create inline spec path: {}", err))?
        .as_nanos();
    path.push(format!(
        "mumei-repl-inline-spec-{}-{}.txt",
        std::process::id(),
        now
    ));
    std::fs::write(&path, input)
        .map_err(|err| format!("failed to write inline spec for mumei-agent: {}", err))?;
    Ok(path)
}

fn repl_verify_agent_report(command: ReplAgentCommand, prompt_for_fixes: bool) {
    match repl_run_agent(command) {
        Ok(report) => {
            if report.success {
                println!("  PASS");
            } else {
                println!("  FAIL");
            }
            repl_print_json_array("spec_health_issues", &report.spec_health_issues);
            repl_print_json_array("verification_violations", &report.verification_violations);
            repl_print_json_array("cross_validation_gaps", &report.cross_validation_gaps);
            repl_print_json_array("next_steps", &report.next_steps);
            if prompt_for_fixes {
                repl_prompt_for_fix(&report);
            }
        }
        Err(err) => {
            eprintln!("  ❌ {}", err);
        }
    }
}

fn repl_verify_spec(input: &str, prompt_for_fixes: bool) {
    let input = input.trim();
    if input.is_empty() {
        eprintln!("  ❌ Usage: :verify-spec <path|inline>");
        return;
    }

    let path = Path::new(input);
    let (input_path, display_input, cleanup_path) = if path.is_file() {
        (path.to_path_buf(), input.to_string(), None)
    } else {
        match repl_inline_spec_path(input) {
            Ok(path) => (path.clone(), "<inline>".to_string(), Some(path)),
            Err(err) => {
                eprintln!("  ❌ {}", err);
                return;
            }
        }
    };

    repl_verify_agent_report(
        ReplAgentCommand::ValidateSpec {
            input: input_path,
            display_input,
        },
        prompt_for_fixes,
    );
    if let Some(path) = cleanup_path {
        let _ = std::fs::remove_file(path);
    }
}

fn repl_verify_code(input: &str, prompt_for_fixes: bool) {
    let input = input.trim();
    if input.is_empty() {
        eprintln!("  ❌ Usage: :verify-code <path>");
        return;
    }
    let path = Path::new(input);
    if !path.is_file() {
        eprintln!("  ❌ Code file not found: {}", input);
        return;
    }
    let language = match agent::infer_code_language(path) {
        Ok(language) => language,
        Err(err) => {
            eprintln!("  ❌ {}", err);
            return;
        }
    };

    repl_verify_agent_report(
        ReplAgentCommand::ValidateCode {
            input: path.to_path_buf(),
            language,
            display_input: input.to_string(),
        },
        prompt_for_fixes,
    );
}

pub(crate) fn repl_register_extern_fn(
    module_env: &mut verification::ModuleEnv,
    ext_fn: &parser::ExternFn,
) {
    let params: Vec<parser::Param> = ext_fn
        .param_types
        .iter()
        .enumerate()
        .map(|(i, ty)| parser::Param {
            name: ext_fn
                .param_names
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("arg{}", i)),
            type_name: Some(ty.clone()),
            type_ref: Some(parser::parse_type_ref(ty)),
            is_ref: false,
            is_ref_mut: false,
            fn_contract_requires: None,
            fn_contract_ensures: None,
        })
        .collect();
    let atom = parser::Atom {
        name: ext_fn.name.clone(),
        type_params: vec![],
        where_bounds: vec![],
        params,
        trace_id: None,
        spec_metadata: std::collections::HashMap::new(),
        requires: ext_fn
            .requires
            .clone()
            .unwrap_or_else(|| "true".to_string()),
        forall_constraints: vec![],
        ensures: ext_fn.ensures.clone().unwrap_or_else(|| "true".to_string()),
        body_expr: String::new(),
        consumed_params: vec![],
        resources: vec![],
        is_async: false,
        trust_level: parser::TrustLevel::Trusted,
        max_unroll: None,
        invariant: None,
        effects: vec![],
        return_type: Some(ext_fn.return_type.clone()),
        span: ext_fn.span.clone(),
        effect_pre: std::collections::HashMap::new(),
        effect_post: std::collections::HashMap::new(),
    };
    module_env.register_atom(&atom);
}

pub(crate) fn repl_register_item(
    ctx: &mut ReplContext<'_>,
    item: &parser::Item,
    compile_atoms: bool,
) -> usize {
    match item {
        parser::Item::Atom(atom) => {
            ctx.module_env.register_atom(atom);
            if compile_atoms {
                repl_verify_and_compile_atom(ctx, atom);
            }
            1
        }
        parser::Item::TypeDef(t) => {
            ctx.module_env.register_type(t);
            1
        }
        parser::Item::StructDef(s) => {
            ctx.module_env.register_struct(s);
            1
        }
        parser::Item::EnumDef(e) => {
            ctx.module_env.register_enum(e);
            1
        }
        parser::Item::TraitDef(t) => {
            ctx.module_env.register_trait(t);
            1
        }
        parser::Item::ImplDef(i) => {
            ctx.module_env.register_impl(i);
            1
        }
        parser::Item::ResourceDef(r) => {
            ctx.module_env.register_resource(r);
            1
        }
        parser::Item::EffectDef(e) => {
            ctx.module_env.register_effect(e);
            1
        }
        parser::Item::ExternBlock(eb) => {
            ctx.extern_blocks.push(eb.clone());
            for ext_fn in &eb.functions {
                repl_register_extern_fn(&mut ctx.module_env, ext_fn);
            }
            eb.functions.len()
        }
        parser::Item::ImplBlock(ib) => {
            for method in &ib.methods {
                let mut qualified = method.clone();
                qualified.name = format!("{}::{}", ib.struct_name, method.name);
                ctx.module_env.register_atom(&qualified);
                if compile_atoms {
                    repl_verify_and_compile_atom(ctx, &qualified);
                }
            }
            ib.methods.len()
        }
        parser::Item::Import(_) => 0,
    }
}

pub(crate) fn repl_verify_and_compile_atom(ctx: &mut ReplContext<'_>, atom: &parser::Atom) -> bool {
    let hir_atom = lower_atom_to_hir_with_env(atom, Some(&ctx.module_env));
    match verification::verify(&hir_atom, Path::new("."), &ctx.module_env) {
        Ok(()) => {
            if let Some(engine) = ctx.jit_engine.as_ref() {
                if let Err(err) =
                    engine.compile_atom(&hir_atom, &ctx.module_env, &ctx.extern_blocks)
                {
                    eprintln!("  ⚠️  JIT compile warning for '{}': {}", atom.name, err);
                }
            }
            println!("  ✅ Verified: {}", atom.name);
            true
        }
        Err(err) => {
            eprintln!("  ❌ Verification failed for '{}': {}", atom.name, err);
            if let verification::MumeiError::VerificationError {
                counterexample: Some(counterexample),
                ..
            } = &err
            {
                eprintln!("  counterexample: {}", counterexample);
            }
            println!("  ℹ️  Atom '{}' registered but not JIT-compiled", atom.name);
            false
        }
    }
}

pub(crate) fn repl_infer_expr_type_name(
    ctx: &ReplContext<'_>,
    expr: &parser::Expr,
) -> Option<String> {
    match expr {
        parser::Expr::Float(_) => Some("f64".to_string()),
        parser::Expr::StringLit(_) => Some("Str".to_string()),
        parser::Expr::Number(_) => Some("i64".to_string()),
        parser::Expr::Variable(name) if name == "true" || name == "false" => {
            Some("bool".to_string())
        }
        parser::Expr::BinaryOp(left, op, right) => match op {
            parser::Op::Eq
            | parser::Op::Neq
            | parser::Op::Gt
            | parser::Op::Lt
            | parser::Op::Ge
            | parser::Op::Le
            | parser::Op::And
            | parser::Op::Or
            | parser::Op::Implies => Some("bool".to_string()),
            parser::Op::Add
            | parser::Op::Sub
            | parser::Op::Mul
            | parser::Op::Pow
            | parser::Op::Div => {
                if repl_infer_expr_type_name(ctx, left)
                    .is_some_and(|ty| ctx.module_env.resolve_base_type(&ty) == "f64")
                    || repl_infer_expr_type_name(ctx, right)
                        .is_some_and(|ty| ctx.module_env.resolve_base_type(&ty) == "f64")
                {
                    Some("f64".to_string())
                } else {
                    Some("i64".to_string())
                }
            }
        },
        parser::Expr::Call(name, _) if name == "sqrt" => Some("f64".to_string()),
        parser::Expr::Call(name, _) => {
            let fqn_name = name.replace('.', "::");
            match ctx
                .module_env
                .get_atom(name)
                .or_else(|| ctx.module_env.get_atom(&fqn_name))
                .and_then(|atom| atom.return_type.as_deref())
            {
                Some(return_type) => Some(return_type.to_string()),
                None => Some("i64".to_string()),
            }
        }
        _ => None,
    }
}

pub(crate) fn repl_wrap_expr(atom_name: &str, expr: &str, return_type: Option<&str>) -> String {
    let return_annotation = return_type.map_or(String::new(), |ty| format!(" -> {ty}"));
    format!(
        "atom {atom_name}(){return_annotation}\n  requires: true;\n  ensures: true;\n  body: {{\n    {expr}\n  }}"
    )
}

pub(crate) fn repl_compile_eval_atom(
    ctx: &mut ReplContext<'_>,
    atom: &parser::Atom,
) -> Option<&'static str> {
    let engine = match ctx.jit_engine.as_ref() {
        Some(engine) => engine,
        None => {
            eprintln!("  ❌ JIT engine not available");
            return None;
        }
    };

    engine.remove_function(REPL_EVAL_ATOM);
    let hir_atom = lower_atom_to_hir_with_env(atom, Some(&ctx.module_env));
    if let Err(err) = engine.compile_atom(&hir_atom, &ctx.module_env, &ctx.extern_blocks) {
        engine.remove_function(REPL_EVAL_ATOM);
        eprintln!("  ❌ JIT compile error: {}", err);
        return None;
    }

    Some(
        if atom
            .return_type
            .as_deref()
            .is_some_and(|return_type| ctx.module_env.resolve_base_type(return_type) == "f64")
        {
            "f64"
        } else {
            "i64"
        },
    )
}

pub(crate) fn repl_execute_eval_atom(ctx: &mut ReplContext<'_>, return_type: &str) {
    let Some(engine) = ctx.jit_engine.as_ref() else {
        eprintln!("  ❌ JIT engine not available");
        return;
    };
    let result = if return_type == "f64" {
        engine
            .execute_f64(REPL_EVAL_ATOM)
            .map(|value| value.to_string())
    } else {
        engine
            .execute_i64(REPL_EVAL_ATOM)
            .map(|value| value.to_string())
    };
    match result {
        Ok(value) => println!("  = {}", value),
        Err(err) => eprintln!("  ❌ Execution error: {}", err),
    }
    engine.remove_function(REPL_EVAL_ATOM);
}

pub(crate) fn repl_eval_expr(ctx: &mut ReplContext<'_>, expr: &str, verify_first: bool) {
    let parsed_expr = parser::parse_expression(expr);
    let expr_type = repl_infer_expr_type_name(ctx, &parsed_expr);
    let eval_return_type = expr_type
        .as_deref()
        .filter(|ty| ctx.module_env.resolve_base_type(ty) == "f64");
    let wrapped = repl_wrap_expr(REPL_EVAL_ATOM, expr, eval_return_type);
    let items = parser::parse_module(&wrapped);
    let Some(parser::Item::Atom(atom)) = items.first() else {
        eprintln!("  ❌ Syntax error: could not parse expression");
        return;
    };

    if verify_first {
        let hir_atom = lower_atom_to_hir_with_env(atom, Some(&ctx.module_env));
        match verification::verify(&hir_atom, Path::new("."), &ctx.module_env) {
            Ok(()) => {}
            Err(err) => {
                eprintln!("  ❌ Verification failed: {}", err);
                if let verification::MumeiError::VerificationError {
                    counterexample: Some(counterexample),
                    ..
                } = &err
                {
                    eprintln!("  counterexample: {}", counterexample);
                }
                return;
            }
        }
    }

    if let Some(return_type) = repl_compile_eval_atom(ctx, atom) {
        repl_execute_eval_atom(ctx, return_type);
    }
}

pub(crate) fn repl_type_expr(ctx: &ReplContext<'_>, expr: &str) {
    let parsed = parser::parse_expression(expr);
    match repl_infer_expr_type_name(ctx, &parsed) {
        Some(return_type) => println!("  : {}", return_type),
        None => println!("  : unknown"),
    }
}

pub(crate) fn repl_load_single_file(ctx: &mut ReplContext<'_>, file: &Path) -> Option<usize> {
    println!("  Loading '{}'...", file.display());
    let source = match read_source_file(file) {
        Ok(source) => source,
        Err(err) => {
            eprintln!("  ❌ Failed to read '{}': {}", file.display(), err);
            return None;
        }
    };
    let items = parser::parse_module(&source);
    if items.is_empty() {
        eprintln!(
            "  ❌ Syntax error: no items parsed from '{}'",
            file.display()
        );
        return None;
    }
    let mut count = 0;
    for item in &items {
        count += repl_register_item(ctx, item, true);
    }
    println!(
        "  ✅ Loaded {} definition(s) from '{}'",
        count,
        file.display()
    );
    Some(count)
}

pub(crate) fn repl_load_file(ctx: &mut ReplContext<'_>, file: &str) {
    let path = Path::new(file);
    if path.is_dir() {
        let mut files = collect_mm_files(path);
        files.sort();

        if files.is_empty() {
            eprintln!("  ⚠️  No .mm files found in '{}'", file);
            return;
        }

        let mut total_count = 0;
        let mut loaded_files = 0;
        for mm_file in &files {
            if let Some(count) = repl_load_single_file(ctx, mm_file) {
                total_count += count;
                loaded_files += 1;
            }
        }

        println!(
            "  ✅ Total: {} definition(s) from {} file(s) in '{}'",
            total_count, loaded_files, file
        );
        return;
    }

    repl_load_single_file(ctx, path);
}

pub(crate) fn repl_verify_named_atom(ctx: &mut ReplContext<'_>, atom_name: &str) {
    match ctx.module_env.get_atom(atom_name).cloned() {
        Some(atom) => {
            repl_verify_and_compile_atom(ctx, &atom);
        }
        None => eprintln!("  ❌ Unknown atom '{}'", atom_name),
    }
}

pub(crate) fn repl_print_env(ctx: &ReplContext<'_>) {
    println!(
        "  --- Registered Atoms ({}) ---",
        ctx.module_env.atoms.len()
    );
    let mut names: Vec<&String> = ctx.module_env.atoms.keys().collect();
    names.sort();
    for name in names {
        if let Some(atom) = ctx.module_env.atoms.get(name) {
            let params_str: Vec<String> = atom
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name, p.type_name.as_deref().unwrap_or("?")))
                .collect();
            println!(
                "    {} atom {}({}) [{:?}]",
                if atom.is_async { "async" } else { "" },
                name,
                params_str.join(", "),
                atom.trust_level
            );
        }
    }
    println!(
        "  --- Registered Types ({}) ---",
        ctx.module_env.types.len()
    );
    for name in ctx.module_env.types.keys() {
        println!("    type {}", name);
    }
    println!(
        "  --- Registered Structs ({}) ---",
        ctx.module_env.structs.len()
    );
    for name in ctx.module_env.structs.keys() {
        println!("    struct {}", name);
    }
    println!(
        "  --- Registered Enums ({}) ---",
        ctx.module_env.enums.len()
    );
    for name in ctx.module_env.enums.keys() {
        println!("    enum {}", name);
    }
}

pub(crate) fn repl_help() {
    println!("  :help          — Show this help");
    println!("  :quit/:exit    — Exit the REPL");
    println!("  :load <file|dir> — Load atoms and extern declarations from .mm files");
    println!("  :type <expr>   — Infer a simple expression type");
    println!("  :verify <atom> — Verify and JIT-compile a registered atom");
    println!("  :verify-spec <path|inline> — Validate a natural-language spec with mumei-agent");
    println!("  :verify-code <path> — Validate foreign-language code with mumei-agent");
    println!("  :eval <expr>   — JIT compile and execute an expression without verification");
    println!("  :check <expr>  — Parse and type-check an expression");
    println!("  :env           — Show registered atoms and types");
}

pub(crate) fn repl_brace_balance(input: &str) -> i32 {
    let mut balance = 0;
    let mut in_string = false;
    let mut escaped = false;
    for ch in input.chars() {
        if in_string {
            if ch == '"' && !escaped {
                in_string = false;
            }
            escaped = ch == '\\' && !escaped;
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => balance += 1,
            '}' => balance -= 1,
            _ => {}
        }
    }
    balance
}

pub(crate) fn repl_input_incomplete(input: &str) -> bool {
    let trimmed = input.trim_start();
    let looks_like_atom = trimmed.starts_with("atom ")
        || trimmed.starts_with("trusted atom ")
        || trimmed.starts_with("unverified atom ")
        || trimmed.starts_with("async atom ");
    if looks_like_atom {
        return !input.contains("body:") || repl_brace_balance(input) > 0;
    }
    trimmed.starts_with("extern ") && (!input.contains('}') || repl_brace_balance(input) > 0)
}

pub(crate) fn repl_handle_line(
    ctx: &mut ReplContext<'_>,
    input: &str,
    prompt_for_fixes: bool,
) -> ReplAction {
    match input {
        ":quit" | ":q" | ":exit" => {
            println!("Goodbye! 🗡️");
            ReplAction::Quit
        }
        ":help" | ":h" => {
            repl_help();
            ReplAction::Continue
        }
        ":env" => {
            repl_print_env(ctx);
            ReplAction::Continue
        }
        _ if input.starts_with(":load ") => {
            repl_load_file(ctx, input.strip_prefix(":load ").unwrap().trim());
            ReplAction::Continue
        }
        _ if input.starts_with(":type ") => {
            repl_type_expr(ctx, input.strip_prefix(":type ").unwrap().trim());
            ReplAction::Continue
        }
        _ if input.starts_with(":verify ") => {
            let target = input.strip_prefix(":verify ").unwrap().trim();
            if ctx.module_env.get_atom(target).is_some() {
                repl_verify_named_atom(ctx, target);
            } else {
                repl_eval_expr(ctx, target, true);
            }
            ReplAction::Continue
        }
        ":verify-spec" => {
            eprintln!("  ❌ Usage: :verify-spec <path|inline>");
            ReplAction::Continue
        }
        _ if input.starts_with(":verify-spec ") => {
            repl_verify_spec(
                input.strip_prefix(":verify-spec ").unwrap().trim(),
                prompt_for_fixes,
            );
            ReplAction::Continue
        }
        ":verify-code" => {
            eprintln!("  ❌ Usage: :verify-code <path>");
            ReplAction::Continue
        }
        _ if input.starts_with(":verify-code ") => {
            repl_verify_code(
                input.strip_prefix(":verify-code ").unwrap().trim(),
                prompt_for_fixes,
            );
            ReplAction::Continue
        }
        _ if input.starts_with(":check ") => {
            let expr = input.strip_prefix(":check ").unwrap().trim();
            let wrapped = repl_wrap_expr(REPL_EVAL_ATOM, expr, None);
            if parser::parse_module(&wrapped).is_empty() {
                eprintln!("  ❌ Syntax error: could not parse expression");
            } else {
                println!("  ✅ Parsed expression");
            }
            ReplAction::Continue
        }
        _ if input.starts_with(":eval ") => {
            repl_eval_expr(ctx, input.strip_prefix(":eval ").unwrap().trim(), false);
            ReplAction::Continue
        }
        _ => {
            let items = parser::parse_module(input);
            if items.is_empty() {
                repl_eval_expr(ctx, input, true);
            } else {
                for item in &items {
                    match item {
                        parser::Item::Atom(atom) => {
                            repl_register_item(ctx, item, false);
                            repl_verify_and_compile_atom(ctx, atom);
                        }
                        parser::Item::TypeDef(t) => {
                            repl_register_item(ctx, item, false);
                            println!("  ✅ Registered type '{}'", t.name);
                        }
                        parser::Item::StructDef(s) => {
                            repl_register_item(ctx, item, false);
                            println!("  ✅ Registered struct '{}'", s.name);
                        }
                        parser::Item::EnumDef(e) => {
                            repl_register_item(ctx, item, false);
                            println!("  ✅ Registered enum '{}'", e.name);
                        }
                        parser::Item::ExternBlock(eb) => {
                            repl_register_item(ctx, item, false);
                            println!("  ✅ Registered {} extern function(s)", eb.functions.len());
                        }
                        _ => {
                            repl_register_item(ctx, item, false);
                            println!("  ✅ Processed");
                        }
                    }
                }
            }
            ReplAction::Continue
        }
    }
}

pub(crate) fn cmd_repl() {
    println!(
        "🗡️  Mumei REPL v{} (JIT enabled)",
        env!("CARGO_PKG_VERSION")
    );
    println!("  Type expressions or atom definitions to evaluate.");
    println!("  Commands: :help, :type <expr>, :verify <atom>, :verify-spec <path|inline>, :verify-code <path>, :load <file|dir>, :quit");
    println!();

    let mut module_env = verification::ModuleEnv::new();
    verification::register_builtin_traits(&mut module_env);
    verification::register_builtin_effects(&mut module_env);

    // std/prelude を自動ロード
    if let Ok(cwd) = std::env::current_dir() {
        if let Err(e) = resolver::resolve_prelude(&cwd, &mut module_env) {
            eprintln!("  ⚠️  Prelude load warning: {}", e);
        }
    }

    // P7-A: Initialize JIT execution engine
    let jit_context = mumei_emit_llvm::LlvmContext::create();
    let jit_engine = match mumei_emit_llvm::jit::JitEngine::new(&jit_context) {
        Ok(engine) => Some(engine),
        Err(e) => {
            eprintln!("  ⚠️  JIT engine unavailable: {}. Execution disabled.", e);
            None
        }
    };
    let mut ctx = ReplContext {
        module_env,
        jit_engine,
        extern_blocks: Vec::new(),
    };

    let stdin = std::io::stdin();
    let prompt_for_fixes = stdin.is_terminal();
    let mut line_buf = String::new();
    let mut pending_input = String::new();

    loop {
        if pending_input.is_empty() {
            eprint!("mumei> ");
        } else {
            eprint!("....> ");
        }
        line_buf.clear();
        match stdin.read_line(&mut line_buf) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("  ❌ Read error: {}", e);
                break;
            }
        }

        let input = line_buf.trim_end();
        if input.is_empty() {
            continue;
        }

        if pending_input.is_empty() {
            pending_input.push_str(input);
        } else {
            pending_input.push('\n');
            pending_input.push_str(input);
        }

        if repl_input_incomplete(&pending_input) {
            continue;
        }

        let input = std::mem::take(&mut pending_input);
        if matches!(
            repl_handle_line(&mut ctx, input.trim(), prompt_for_fixes),
            ReplAction::Quit
        ) {
            break;
        }
    }
}

// =============================================================================
// mumei infer-effects — Effect inference (JSON output for MCP)
// =============================================================================
