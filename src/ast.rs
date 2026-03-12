// src/ast.rs
// Generics 基盤: 型参照（型引数付き）の共通表現

/// 型参照: `i64`, `Stack<i64>`, `Map<String, List<i64>>` などを表現する。
/// パーサー・検証器・コード生成の全レイヤーで共通に使用する。
#[derive(Debug, Clone, PartialEq)]
pub struct TypeRef {
    /// 型名（例: "i64", "Stack", "T"）
    pub name: String,
    /// 型引数リスト（例: Stack<i64> → [TypeRef("i64")]）。
    /// 非ジェネリック型の場合は空。
    pub type_args: Vec<TypeRef>,
    /// エフェクトセット（関数型に付与されるエフェクト情報）
    /// 例: atom_ref(i64) -> i64 with [FileWrite] → Some(vec!["FileWrite"])
    /// 例: atom_ref(i64) -> i64 with E → Some(vec!["E"])
    /// 非関数型や with なしの場合は None
    pub effect_set: Option<Vec<String>>,
}

impl TypeRef {
    /// 型引数なしの単純な型参照を作成する
    pub fn simple(name: &str) -> Self {
        TypeRef {
            name: name.to_string(),
            type_args: vec![],
            effect_set: None,
        }
    }

    /// 型引数付きの型参照を作成する
    pub fn generic(name: &str, args: Vec<TypeRef>) -> Self {
        TypeRef {
            name: name.to_string(),
            type_args: args,
            effect_set: None,
        }
    }

    /// 表示用の正規化名を返す（例: "Stack<i64>", "atom_ref(i64) -> i64"）
    pub fn display_name(&self) -> String {
        let base = if self.is_fn_type() {
            // 関数型: atom_ref(param_types...) -> return_type
            let param_types: Vec<String> = self.type_args[..self.type_args.len() - 1]
                .iter()
                .map(|a| a.display_name())
                .collect();
            let return_type = self.type_args.last().unwrap().display_name();
            format!("atom_ref({}) -> {}", param_types.join(", "), return_type)
        } else if self.type_args.is_empty() {
            self.name.clone()
        } else {
            let args: Vec<String> = self.type_args.iter().map(|a| a.display_name()).collect();
            format!("{}<{}>", self.name, args.join(", "))
        };
        if let Some(ref effects) = self.effect_set {
            if effects.len() == 1 {
                format!("{} with {}", base, effects[0])
            } else {
                format!("{} with [{}]", base, effects.join(", "))
            }
        } else {
            base
        }
    }

    /// 型パラメータ（型変数）かどうかを判定する。
    /// 大文字1文字（T, U, V など）を型パラメータとして扱う。
    pub fn is_type_param(&self) -> bool {
        self.type_args.is_empty()
            && self.name.len() == 1
            && self.name.chars().next().map_or(false, |c| c.is_uppercase())
    }

    /// 関数型を作成する: atom_ref(param_types...) -> return_type
    /// TypeRef の構造を再利用し、name="atom_ref" で関数型を表現する。
    /// type_args の最後の要素が戻り値型、それ以外がパラメータ型。
    pub fn fn_type(param_types: Vec<TypeRef>, return_type: TypeRef) -> Self {
        let mut type_args = param_types;
        type_args.push(return_type);
        TypeRef {
            name: "atom_ref".to_string(),
            type_args,
            effect_set: None,
        }
    }

    /// 関数型かどうかを判定する
    pub fn is_fn_type(&self) -> bool {
        self.name == "atom_ref" && !self.type_args.is_empty()
    }

    /// 関数型のパラメータ型を返す（最後の要素＝戻り値型を除く）
    #[allow(dead_code)]
    pub fn fn_param_types(&self) -> Option<Vec<&TypeRef>> {
        if self.is_fn_type() && self.type_args.len() >= 2 {
            Some(self.type_args[..self.type_args.len() - 1].iter().collect())
        } else {
            None
        }
    }

    /// 関数型の戻り値型を返す（type_args の最後の要素）
    #[allow(dead_code)]
    pub fn fn_return_type(&self) -> Option<&TypeRef> {
        if self.is_fn_type() {
            self.type_args.last()
        } else {
            None
        }
    }

    /// 型変数の置換: type_map に従って型パラメータを具体型に置き換える
    pub fn substitute(&self, type_map: &std::collections::HashMap<String, TypeRef>) -> TypeRef {
        if let Some(replacement) = type_map.get(&self.name) {
            // 型パラメータが具体型にマッピングされている場合、置換する
            // 置換先にもさらに型引数がある場合は再帰的に処理
            let mut result = replacement.clone();
            if !self.type_args.is_empty() {
                // 例: T<U> のような場合（通常は発生しないが安全のため）
                result.type_args = self
                    .type_args
                    .iter()
                    .map(|a| a.substitute(type_map))
                    .collect();
            }
            // エフェクトセットの置換
            result.effect_set = self.effect_set.as_ref().map(|effects| {
                effects
                    .iter()
                    .map(|eff| {
                        if let Some(concrete) = type_map.get(eff) {
                            concrete.name.clone()
                        } else {
                            eff.clone()
                        }
                    })
                    .collect()
            });
            result
        } else {
            // 型パラメータでない場合、型引数のみ再帰的に置換
            let mut result = TypeRef {
                name: self.name.clone(),
                type_args: self
                    .type_args
                    .iter()
                    .map(|a| a.substitute(type_map))
                    .collect(),
                effect_set: None,
            };
            // エフェクトセットの置換
            result.effect_set = self.effect_set.as_ref().map(|effects| {
                effects
                    .iter()
                    .map(|eff| {
                        if let Some(concrete) = type_map.get(eff) {
                            concrete.name.clone()
                        } else {
                            eff.clone()
                        }
                    })
                    .collect()
            });
            result
        }
    }
}

impl std::fmt::Display for TypeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

// =============================================================================
// 単相化 (Monomorphization) エンジン
// =============================================================================
//
// ジェネリック定義（`struct Stack<T>`, `atom push<T>`, `enum Option<T>` 等）を
// 使用箇所から収集した具体的な型引数で展開し、型パラメータのない具体的な定義を生成する。
//
// Rust と同様の「単相化」方式を採用:
// - コンパイル時に Stack<i64>, Stack<f64> など使用されている型ごとにコードを複製
// - 実行時の型消去やオーバーヘッドがない

use crate::parser::{
    parse_body_expr, parse_type_ref, Atom, EnumDef, EnumVariant, Expr, Item, Param, Stmt,
    StructDef, StructField,
};
use std::collections::{HashMap, HashSet};

/// 単相化コンテキスト: ジェネリック定義と使用インスタンスを管理する
#[derive(Debug, Default)]
pub struct Monomorphizer {
    /// ジェネリック Struct 定義: 名前 → 定義
    generic_structs: HashMap<String, StructDef>,
    /// ジェネリック Enum 定義: 名前 → 定義
    generic_enums: HashMap<String, EnumDef>,
    /// ジェネリック Atom 定義: 名前 → 定義
    generic_atoms: HashMap<String, Atom>,
    /// 使用されている具体的な型インスタンス（例: "Stack<i64>"）
    instances: HashSet<String>,
}

impl Monomorphizer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Phase 1: Items からジェネリック定義を登録し、使用箇所を収集する
    pub fn collect(&mut self, items: &[Item]) {
        // ジェネリック定義を登録
        for item in items {
            match item {
                Item::StructDef(sdef) if !sdef.type_params.is_empty() => {
                    self.generic_structs.insert(sdef.name.clone(), sdef.clone());
                }
                Item::EnumDef(edef) if !edef.type_params.is_empty() => {
                    self.generic_enums.insert(edef.name.clone(), edef.clone());
                }
                Item::Atom(atom) if !atom.type_params.is_empty() => {
                    self.generic_atoms.insert(atom.name.clone(), atom.clone());
                }
                _ => {}
            }
        }

        // 使用箇所を収集
        for item in items {
            match item {
                Item::Atom(atom) => {
                    // パラメータの型から収集
                    for param in &atom.params {
                        if let Some(type_ref) = &param.type_ref {
                            self.collect_from_type_ref(type_ref);
                        }
                    }
                    // body 内の文から収集
                    let body_stmt = parse_body_expr(&atom.body_expr);
                    self.collect_from_stmt(&body_stmt);
                }
                Item::StructDef(sdef) => {
                    for field in &sdef.fields {
                        self.collect_from_type_ref(&field.type_ref);
                    }
                }
                Item::EnumDef(edef) => {
                    for variant in &edef.variants {
                        for ft in &variant.field_types {
                            self.collect_from_type_ref(ft);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// TypeRef から具体的なジェネリック型インスタンスを収集する
    fn collect_from_type_ref(&mut self, type_ref: &TypeRef) {
        if !type_ref.type_args.is_empty() {
            // 型引数がすべて具体型（型パラメータでない）場合のみインスタンスとして登録
            let all_concrete = type_ref.type_args.iter().all(|a| !a.is_type_param());
            if all_concrete
                && (self.generic_structs.contains_key(&type_ref.name)
                    || self.generic_enums.contains_key(&type_ref.name)
                    || self.generic_atoms.contains_key(&type_ref.name))
            {
                self.instances.insert(type_ref.display_name());
            }
            // 再帰的に型引数も収集
            for arg in &type_ref.type_args {
                self.collect_from_type_ref(arg);
            }
        }
    }

    /// 式から StructInit の type_name を走査してジェネリック使用箇所を収集する
    fn collect_from_expr(&mut self, expr: &Expr) {
        match expr {
            Expr::StructInit { type_name, fields } => {
                let tref = parse_type_ref(type_name);
                self.collect_from_type_ref(&tref);
                for (_, field_expr) in fields {
                    self.collect_from_expr(field_expr);
                }
            }
            Expr::Call(name, args) => {
                let tref = parse_type_ref(name);
                self.collect_from_type_ref(&tref);
                for arg in args {
                    self.collect_from_expr(arg);
                }
            }
            Expr::BinaryOp(l, _, r) => {
                self.collect_from_expr(l);
                self.collect_from_expr(r);
            }
            Expr::IfThenElse {
                cond,
                then_branch,
                else_branch,
            } => {
                self.collect_from_expr(cond);
                self.collect_from_stmt(then_branch);
                self.collect_from_stmt(else_branch);
            }
            Expr::Match { target, arms } => {
                self.collect_from_expr(target);
                for arm in arms {
                    self.collect_from_stmt(&arm.body);
                    if let Some(guard) = &arm.guard {
                        self.collect_from_expr(guard);
                    }
                }
            }
            Expr::FieldAccess(expr, _) => {
                self.collect_from_expr(expr);
            }
            Expr::ArrayAccess(_, idx) => {
                self.collect_from_expr(idx);
            }
            Expr::Async { body } => {
                self.collect_from_stmt(body);
            }
            Expr::Await { expr } => {
                self.collect_from_expr(expr);
            }
            Expr::Number(_) | Expr::Float(_) | Expr::Variable(_) => {}
            Expr::AtomRef { name } => {
                // atom_ref(name) は呼び出し先の atom を参照するため、名前を収集
                let tref = parse_type_ref(name);
                self.collect_from_type_ref(&tref);
            }
            Expr::CallRef { callee, args } => {
                self.collect_from_expr(callee);
                for arg in args {
                    self.collect_from_expr(arg);
                }
            }
            Expr::Perform { args, .. } => {
                for arg in args {
                    self.collect_from_expr(arg);
                }
            }
            Expr::Lambda { params, body, .. } => {
                for param in params {
                    if let Some(ref type_ref) = param.type_ref {
                        self.collect_from_type_ref(type_ref);
                    }
                }
                self.collect_from_stmt(body);
            }
        }
    }

    /// 文から StructInit の type_name を走査してジェネリック使用箇所を収集する
    fn collect_from_stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { value, .. } | Stmt::Assign { value, .. } => {
                self.collect_from_expr(value);
            }
            Stmt::Block(stmts) => {
                for s in stmts {
                    self.collect_from_stmt(s);
                }
            }
            Stmt::While {
                cond,
                invariant,
                decreases,
                body,
            } => {
                self.collect_from_expr(cond);
                self.collect_from_expr(invariant);
                if let Some(dec) = decreases {
                    self.collect_from_expr(dec);
                }
                self.collect_from_stmt(body);
            }
            Stmt::Acquire { body, .. } => {
                self.collect_from_stmt(body);
            }
            Stmt::Task { body, .. } => {
                self.collect_from_stmt(body);
            }
            Stmt::TaskGroup { children, .. } => {
                for child in children {
                    self.collect_from_stmt(child);
                }
            }
            Stmt::Expr(expr) => {
                self.collect_from_expr(expr);
            }
        }
    }

    /// Phase 2: 収集したインスタンスを単相化し、具体的な Item のリストを返す。
    /// ジェネリック定義自体は除外され、具体化された定義のみが返される。
    /// `module_env` が提供された場合、単相化時にトレイト境界を検証する。
    pub fn monomorphize(
        &self,
        items: &[Item],
        module_env: Option<&crate::verification::ModuleEnv>,
    ) -> Vec<Item> {
        let mut result: Vec<Item> = Vec::new();

        // 非ジェネリックな Item はそのまま通す
        for item in items {
            match item {
                Item::StructDef(sdef) if !sdef.type_params.is_empty() => {
                    // ジェネリック定義はスキップ（後で展開する）
                }
                Item::EnumDef(edef) if !edef.type_params.is_empty() => {
                    // ジェネリック定義はスキップ
                }
                Item::Atom(atom) if !atom.type_params.is_empty() => {
                    // ジェネリック定義はスキップ
                }
                _ => {
                    result.push(item.clone());
                }
            }
        }

        // 各インスタンスを展開
        for instance_name in &self.instances {
            let tref = parse_type_ref(instance_name);

            // Struct の単相化
            if let Some(generic_def) = self.generic_structs.get(&tref.name) {
                if let Some(mono_struct) = self.monomorphize_struct(generic_def, &tref) {
                    result.push(Item::StructDef(mono_struct));
                }
            }

            // Enum の単相化
            if let Some(generic_def) = self.generic_enums.get(&tref.name) {
                if let Some(mono_enum) = self.monomorphize_enum(generic_def, &tref) {
                    result.push(Item::EnumDef(mono_enum));
                }
            }

            // Atom の単相化
            if let Some(generic_def) = self.generic_atoms.get(&tref.name) {
                if let Some(mono_atom) = self.monomorphize_atom(generic_def, &tref, module_env) {
                    result.push(Item::Atom(mono_atom));
                }
            }
        }

        result
    }

    /// ジェネリック Struct を具体型で単相化する
    fn monomorphize_struct(&self, generic: &StructDef, instance: &TypeRef) -> Option<StructDef> {
        let type_map = self.build_type_map(&generic.type_params, &instance.type_args)?;
        let mono_name = instance.display_name();

        let fields = generic
            .fields
            .iter()
            .map(|f| {
                let new_type_ref = f.type_ref.substitute(&type_map);
                StructField {
                    name: f.name.clone(),
                    type_name: new_type_ref.display_name(),
                    type_ref: new_type_ref,
                    constraint: f.constraint.clone(),
                }
            })
            .collect();

        Some(StructDef {
            name: mono_name,
            type_params: vec![], // 単相化後は型パラメータなし
            fields,
            method_names: vec![],
            span: generic.span.clone(),
        })
    }

    /// ジェネリック Enum を具体型で単相化する
    fn monomorphize_enum(&self, generic: &EnumDef, instance: &TypeRef) -> Option<EnumDef> {
        let type_map = self.build_type_map(&generic.type_params, &instance.type_args)?;
        let mono_name = instance.display_name();

        let variants = generic
            .variants
            .iter()
            .map(|v| {
                let new_field_types: Vec<TypeRef> = v
                    .field_types
                    .iter()
                    .map(|ft| ft.substitute(&type_map))
                    .collect();
                let new_fields: Vec<String> =
                    new_field_types.iter().map(|ft| ft.display_name()).collect();
                let is_recursive = new_fields.iter().any(|f| f == &mono_name);
                EnumVariant {
                    name: v.name.clone(),
                    fields: new_fields,
                    field_types: new_field_types,
                    is_recursive,
                }
            })
            .collect();

        let any_recursive = generic.variants.iter().any(|v| v.is_recursive);

        Some(EnumDef {
            name: mono_name,
            type_params: vec![],
            variants,
            is_recursive: any_recursive,
            span: generic.span.clone(),
        })
    }

    /// ジェネリック Atom を具体型で単相化する
    /// `module_env` が提供された場合、トレイト境界を検証する。
    fn monomorphize_atom(
        &self,
        generic: &Atom,
        instance: &TypeRef,
        module_env: Option<&crate::verification::ModuleEnv>,
    ) -> Option<Atom> {
        let type_map = self.build_type_map(&generic.type_params, &instance.type_args)?;
        let mono_name = instance.display_name();

        // トレイト境界バリデーション
        if let Some(menv) = module_env {
            for bound in &generic.where_bounds {
                if let Some(concrete_type_ref) = type_map.get(&bound.param) {
                    let concrete_name = concrete_type_ref.display_name();
                    for trait_name in &bound.bounds {
                        if trait_name == "Effect" {
                            // "Effect" 境界は特別扱い: エフェクト定義の存在を確認
                            if !menv.has_effect_def(&concrete_name) {
                                eprintln!(
                                    "  \u{26a0}\u{fe0f}  Trait bound violation: '{}' is not a known effect \
                                     (required by bound '{}: Effect' in atom '{}')",
                                    concrete_name, bound.param, generic.name
                                );
                                return None;
                            }
                        } else {
                            // 通常のトレイト境界: impl が存在するか確認
                            if let Err(e) =
                                menv.check_trait_bounds(&concrete_name, &[trait_name.clone()])
                            {
                                eprintln!(
                                    "  \u{26a0}\u{fe0f}  Trait bound violation in monomorphization of '{}': {}",
                                    mono_name, e
                                );
                                return None;
                            }
                        }
                    }
                }
            }
        }

        let params = generic
            .params
            .iter()
            .map(|p| {
                if let Some(tref) = &p.type_ref {
                    let new_type_ref = tref.substitute(&type_map);
                    Param {
                        name: p.name.clone(),
                        type_name: Some(new_type_ref.display_name()),
                        type_ref: Some(new_type_ref),
                        is_ref: p.is_ref,
                        is_ref_mut: p.is_ref_mut,
                        fn_contract_requires: p.fn_contract_requires.clone(),
                        fn_contract_ensures: p.fn_contract_ensures.clone(),
                    }
                } else {
                    p.clone()
                }
            })
            .collect();

        Some(Atom {
            name: mono_name,
            type_params: vec![],
            where_bounds: vec![], // 単相化後は境界なし
            params,
            requires: generic.requires.clone(),
            forall_constraints: generic.forall_constraints.clone(),
            ensures: generic.ensures.clone(),
            body_expr: generic.body_expr.clone(),
            consumed_params: generic.consumed_params.clone(),
            resources: generic.resources.clone(),
            is_async: generic.is_async,
            trust_level: generic.trust_level.clone(),
            max_unroll: generic.max_unroll,
            invariant: generic.invariant.clone(),
            effects: generic.effects.clone(),
            span: generic.span.clone(),
        })
    }

    /// 型パラメータ名と型引数の対応マップを構築する
    fn build_type_map(
        &self,
        type_params: &[String],
        type_args: &[TypeRef],
    ) -> Option<HashMap<String, TypeRef>> {
        if type_params.len() != type_args.len() {
            return None;
        }
        let map: HashMap<String, TypeRef> = type_params
            .iter()
            .zip(type_args.iter())
            .map(|(param, arg)| (param.clone(), arg.clone()))
            .collect();
        Some(map)
    }

    /// ジェネリック定義が存在するかどうか
    pub fn has_generics(&self) -> bool {
        !self.generic_structs.is_empty()
            || !self.generic_enums.is_empty()
            || !self.generic_atoms.is_empty()
    }

    /// 収集されたインスタンス一覧を返す（デバッグ用）
    #[allow(dead_code)]
    pub fn instances(&self) -> &HashSet<String> {
        &self.instances
    }
}
