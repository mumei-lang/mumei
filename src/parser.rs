use crate::ast::TypeRef;
use regex::Regex;

// --- 0. ソース位置情報 (Span) ---

/// ソースコード内の位置情報。全 AST ノードに付与して診断メッセージの精度を向上させる。
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    /// ソースファイル名（空文字列は不明を表す）
    pub file: String,
    /// 行番号（1-indexed、0 は不明）
    pub line: usize,
    /// 列番号（1-indexed、0 は不明）
    pub col: usize,
    /// トークン長（0 は不明）
    pub len: usize,
}

impl Default for Span {
    fn default() -> Self {
        Span {
            file: String::new(),
            line: 0,
            col: 0,
            len: 0,
        }
    }
}

impl Span {
    /// 既知の位置情報を持つ Span を生成する
    pub fn new(file: impl Into<String>, line: usize, col: usize, len: usize) -> Self {
        Span {
            file: file.into(),
            line,
            col,
            len,
        }
    }
}

/// ソース文字列内のバイトオフセットから (1-indexed line, 1-indexed col) を計算する。
fn offset_to_line_col(source: &str, offset: usize) -> (usize, usize) {
    let mut line = 1;
    let mut col = 1;
    for (i, c) in source.char_indices() {
        if i >= offset {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// ソース文字列内の regex マッチからSpanを生成するヘルパー。
fn span_from_offset(source: &str, offset: usize, len: usize) -> Span {
    let (line, col) = offset_to_line_col(source, offset);
    Span::new("", line, col, len)
}

impl std::fmt::Display for Span {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.is_empty() {
            write!(f, "<unknown>:{}:{}", self.line, self.col)
        } else {
            write!(f, "{}:{}:{}", self.file, self.line, self.col)
        }
    }
}

// --- 1. 数式の構造定義 (AST: Abstract Syntax Tree) ---

#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Neq,
    Gt,
    Lt,
    Ge,
    Le,
    And,
    Or,
    Implies,
}

// =============================================================================
// 非同期処理 + リソース管理 (Async/Await + Resource Hierarchy)
// =============================================================================

/// リソースの優先度（Priority）定義。
/// デッドロック防止のため、リソース取得順序を静的に制約する。
/// 不変条件: スレッド T がリソース L1 を保持したまま L2 を要求する場合、
///           Priority(L2) > Priority(L1) でなければならない。
#[derive(Debug, Clone)]
pub struct ResourceDef {
    /// リソース名（例: "mutex_a", "db_conn"）
    pub name: String,
    /// 優先度（数値が大きいほど後に取得すべき）
    pub priority: i64,
    /// アクセスモード: exclusive（書き込み）または shared（読み取り）
    pub mode: ResourceMode,
    /// ソース位置情報
    pub span: Span,
}

/// リソースのアクセスモード
#[derive(Debug, Clone, PartialEq)]
pub enum ResourceMode {
    /// 排他的アクセス（書き込み可能、他者はアクセス不可）
    Exclusive,
    /// 共有アクセス（読み取り専用、他者も読み取り可能）
    Shared,
}

#[derive(Debug, Clone)]
pub enum Expr {
    Number(i64),
    Float(f64),
    Variable(String),
    ArrayAccess(String, Box<Expr>),
    BinaryOp(Box<Expr>, Op, Box<Expr>),
    IfThenElse {
        cond: Box<Expr>,
        then_branch: Box<Expr>,
        else_branch: Box<Expr>,
    },
    Let {
        var: String,
        value: Box<Expr>,
    },
    Assign {
        var: String,
        value: Box<Expr>,
    },
    Block(Vec<Expr>),
    While {
        cond: Box<Expr>,
        invariant: Box<Expr>,
        /// 停止性証明用の減少式（Ranking Function）。None なら停止性チェックをスキップ
        decreases: Option<Box<Expr>>,
        body: Box<Expr>,
    },
    Call(String, Vec<Expr>),
    /// 構造体インスタンス生成: TypeName { field1: expr1, field2: expr2 }
    StructInit {
        type_name: String,
        fields: Vec<(String, Expr)>,
    },
    /// フィールドアクセス: expr.field_name
    FieldAccess(Box<Expr>, String),
    /// Match 式: match expr { Pattern => expr, ... }
    Match {
        target: Box<Expr>,
        arms: Vec<MatchArm>,
    },
    /// リソース取得: acquire resource_name { body }
    /// body 実行中はリソースを保持し、ブロック終了時に自動解放する。
    /// Z3 検証時にリソース階層制約をチェックする。
    Acquire {
        resource: String,
        body: Box<Expr>,
    },
    /// 非同期式: async { body }
    /// body を非同期コンテキストで実行する。暗黙的に Control エフェクトを持つ。
    Async {
        body: Box<Expr>,
    },
    /// 待機式: await expr
    /// 非同期式の結果を待機する。await ポイントで所有権の検証が行われる。
    Await {
        expr: Box<Expr>,
    },
    /// タスク式: task { body }
    /// 構造化並行性のための子タスクを生成する。
    /// 親タスクが子タスクより先に終了しないことを Z3 で保証する。
    Task {
        body: Box<Expr>,
        /// タスクグループ名（省略時は暗黙のデフォルトグループ）
        group: Option<String>,
    },
    /// タスクグループ式: task_group { task { ... }, task { ... } }
    /// 複数の子タスクをグループ化し、全タスクの完了を待機する。
    TaskGroup {
        children: Vec<Expr>,
        /// Join セマンティクス: all（全タスク完了待ち）または any（最初の完了で終了）
        join_semantics: JoinSemantics,
    },
}

/// Match 式のアーム（パターン → 式）
#[derive(Debug, Clone)]
pub struct MatchArm {
    pub pattern: Pattern,
    /// オプションのガード条件: match x { Pattern if cond => ... }
    pub guard: Option<Box<Expr>>,
    pub body: Box<Expr>,
}

/// タスクグループの Join セマンティクス
#[derive(Debug, Clone, PartialEq)]
pub enum JoinSemantics {
    /// 全タスクの完了を待つ（デフォルト）
    All,
    /// 最初に完了したタスクの結果を返す（残りはキャンセル）
    Any,
}

/// パターン
#[derive(Debug, Clone)]
pub enum Pattern {
    /// ワイルドカード: _
    Wildcard,
    /// リテラル整数: 42
    Literal(i64),
    /// 変数バインド: x（小文字始まり）
    Variable(String),
    /// Enum Variant パターン: Circle(r) or None
    Variant {
        variant_name: String,
        fields: Vec<Pattern>,
    },
}

/// Enum Variant 定義
#[derive(Debug, Clone)]
pub struct EnumVariant {
    pub name: String,
    /// Variant が保持するフィールドの型名リスト（Unit variant なら空）
    /// 再帰的 ADT: フィールド型に自身の Enum 名（例: "List"）を含めることで
    /// `Cons(i64, List)` のような再帰的データ構造を定義可能。
    /// パーサーは "Self" を Enum 自身の名前に自動展開する。
    pub fields: Vec<String>,
    /// Generics: フィールドの型参照（TypeRef 版）。
    /// fields (String) との後方互換性のため両方保持する。
    pub field_types: Vec<TypeRef>,
    /// このバリアントが再帰的か（フィールドに自身の Enum 名を含むか）
    #[allow(dead_code)]
    pub is_recursive: bool,
}

/// Enum 定義
#[derive(Debug, Clone)]
pub struct EnumDef {
    pub name: String,
    /// Generics: 型パラメータリスト（例: ["T", "U"]）。非ジェネリックなら空。
    pub type_params: Vec<String>,
    pub variants: Vec<EnumVariant>,
    /// この Enum が再帰的データ型か（いずれかの Variant が自身を参照するか）
    #[allow(dead_code)]
    pub is_recursive: bool,
    /// ソース位置情報
    pub span: Span,
}

// --- 2. 量子化子、精緻型、および Item の定義 ---

#[derive(Debug, Clone, PartialEq)]
pub enum QuantifierType {
    ForAll,
    Exists,
}

#[derive(Debug, Clone)]
pub struct Quantifier {
    pub q_type: QuantifierType,
    pub var: String,
    pub start: String,
    pub end: String,
    pub condition: String,
}

#[derive(Debug, Clone)]
pub struct RefinedType {
    pub name: String,
    pub _base_type: String, // i64, u64, f64 を保持
    pub operand: String,
    pub predicate_raw: String,
    /// ソース位置情報
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub type_name: Option<String>,
    /// Generics: 型参照（TypeRef 版）。type_name との後方互換性のため両方保持。
    pub type_ref: Option<TypeRef>,
    /// 参照渡し修飾子（Borrowing）: `ref v: Vector<T>` の場合 true。
    /// ref パラメータは読み取り専用で貸し出され、所有権は移動しない。
    /// 借用中は所有者が free/consume できないことを Z3 で保証する。
    pub is_ref: bool,
    /// 排他的可変参照修飾子: `ref mut v: Vector<T>` の場合 true。
    /// ref mut パラメータは書き込み可能な排他的参照。
    /// Z3 で以下の排他性制約を検証する:
    /// - ref mut が存在する場合、同じ変数への他の ref/ref mut は存在できない
    /// - ref mut パラメータへの書き込みは所有者に反映される
    /// - ref mut は同時に1つのみ存在可能（エイリアシング防止）
    pub is_ref_mut: bool,
}

#[derive(Debug, Clone)]
pub struct Atom {
    pub name: String,
    /// Generics: 型パラメータリスト（例: ["T", "U"]）。非ジェネリックなら空。
    pub type_params: Vec<String>,
    /// トレイト境界: 型パラメータに課す制約（例: [TypeParamBound { param: "T", bounds: ["Comparable"] }]）
    /// 単相化時のトレイト境界バリデーションで使用（将来の拡張）
    #[allow(dead_code)]
    pub where_bounds: Vec<TypeParamBound>,
    pub params: Vec<Param>,
    pub requires: String,
    pub forall_constraints: Vec<Quantifier>,
    pub ensures: String,
    pub body_expr: String,
    /// 所有権の消費対象パラメータ名リスト（Linear Types）
    /// `atom take(x: T) consume x;` の場合: consumed_params = ["x"]
    /// consume されたパラメータは body 内で使用後、再利用不可となる。
    /// LinearityCtx が Z3 と連携して二重使用・Use-After-Free を検出する。
    pub consumed_params: Vec<String>,
    /// この atom が使用するリソース名リスト（非同期安全性検証用）
    /// `atom transfer() resources: [db, cache];` の場合: resources = ["db", "cache"]
    /// Z3 がリソース階層制約を検証し、デッドロックの可能性を検出する。
    pub resources: Vec<String>,
    /// この atom が非同期（async）かどうか
    /// `async atom fetch(url: Str)` の場合: is_async = true
    pub is_async: bool,
    /// 信頼レベル（外部ライブラリとの境界）
    /// - Verified: 完全に検証される（デフォルト）
    /// - Trusted: requires/ensures の契約のみ信頼し、body は検証しない
    /// - Unverified: 未検証コード。呼び出し時に警告を出す
    pub trust_level: TrustLevel,
    /// BMC のループ展開回数上限（atom 単位のオーバーライド）
    /// `max_unroll: 5;` で指定。None の場合はグローバルデフォルト（3）を使用。
    pub max_unroll: Option<usize>,
    /// atom レベルの状態不変量（Invariant）。
    /// 再帰的 async atom や状態を持つ atom に対して、
    /// 呼び出し前後で維持されるべき論理的性質を記述する。
    ///
    /// ```mumei
    /// async atom process(state: i64)
    /// invariant: state >= 0;
    /// requires: state >= 0;
    /// ensures: result >= 0;
    /// body: { ... };
    /// ```
    ///
    /// Z3 検証:
    /// 1. 導入 (Induction Base): requires が成立するとき invariant が成立することを証明
    /// 2. 維持 (Preservation): invariant が成立する状態で body を実行した後も invariant が維持されることを証明
    /// 3. 再帰呼び出し時: 呼び出し先の invariant を仮定として使用（帰納法の仮定）
    pub invariant: Option<String>,
    /// ソース位置情報
    pub span: Span,
}

// =============================================================================
// 信頼境界 (Trust Boundary)
// =============================================================================

/// 外部ライブラリとの信頼レベル。
/// mumei で検証された安全な世界と、未検証の外部コードの境界を定義する。
#[derive(Debug, Clone, PartialEq)]
pub enum TrustLevel {
    /// 完全に検証される（デフォルト）。body, requires, ensures すべてを Z3 で検証。
    Verified,
    /// 信頼済み外部コード。requires/ensures の契約のみ信頼し、body は検証しない。
    /// `trusted atom ffi_read(fd: i64) ...` で宣言。
    /// 外部 C/Rust ライブラリの FFI ラッパーに使用する。
    Trusted,
    /// 未検証コード。呼び出し時に「検証スキップ」の警告を出す。
    /// `unverified atom legacy_code(x: i64) ...` で宣言。
    /// レガシーコードの段階的な移行に使用する。
    Unverified,
}

/// 構造体フィールド定義（オプションで精緻型制約を保持）
#[derive(Debug, Clone)]
pub struct StructField {
    pub name: String,
    pub type_name: String,
    /// Generics: フィールドの型参照（TypeRef 版）
    pub type_ref: TypeRef,
    /// フィールドの精緻型制約（例: "v >= 0"）。None なら制約なし
    pub constraint: Option<String>,
}

/// 構造体定義
#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    /// Generics: 型パラメータリスト（例: ["T"]）。非ジェネリックなら空。
    pub type_params: Vec<String>,
    pub fields: Vec<StructField>,
    /// 構造体に紐付けられた Atom（メソッド）の名前リスト。
    /// `impl Stack { atom push(...) ... }` で定義されたメソッドを追跡する。
    /// 実際の Atom 定義は ModuleEnv.atoms に "Stack::push" のような FQN で登録される。
    #[allow(dead_code)]
    pub method_names: Vec<String>,
    /// ソース位置情報
    pub span: Span,
}

/// インポート宣言
#[derive(Debug, Clone)]
pub struct ImportDecl {
    /// ソース位置情報
    pub span: Span,
    /// インポート対象のファイルパス（例: "./lib/math.mm")
    pub path: String,
    /// エイリアス（例: as math → Some("math")）
    pub alias: Option<String>,
}

/// トレイト境界: 型パラメータに課す制約（例: "T: Comparable"）
#[derive(Debug, Clone, PartialEq)]
pub struct TypeParamBound {
    /// 型パラメータ名（例: "T"）
    pub param: String,
    /// 制約トレイト名のリスト（例: ["Comparable", "Numeric"]）
    pub bounds: Vec<String>,
}

/// トレイトのメソッドシグネチャ
#[derive(Debug, Clone)]
pub struct TraitMethod {
    /// メソッド名（例: "leq"）
    pub name: String,
    /// パラメータの型名リスト（Self は暗黙）
    pub param_types: Vec<String>,
    /// 戻り値型名（例: "bool", "i64"）
    pub return_type: String,
    /// パラメータごとの精緻型制約（例: "v != 0"）。制約がないパラメータは None。
    /// `fn div(a: Self, b: Self where v != 0) -> Self;` の場合:
    /// param_constraints = [None, Some("v != 0")]
    #[allow(dead_code)]
    pub param_constraints: Vec<Option<String>>,
}

/// トレイト定義
/// ```mumei
/// trait Comparable {
///     fn leq(a: Self, b: Self) -> bool;
///     law reflexive: leq(x, x) == true;
///     law transitive: leq(a, b) && leq(b, c) => leq(a, c);
/// }
/// ```
#[derive(Debug, Clone)]
pub struct TraitDef {
    /// トレイト名（例: "Comparable"）
    pub name: String,
    /// メソッドシグネチャ
    pub methods: Vec<TraitMethod>,
    /// 法則（Laws）: トレイトが満たすべき論理的性質。
    /// 各要素は (法則名, 論理式の文字列) のペア。
    pub laws: Vec<(String, String)>,
    /// ソース位置情報
    pub span: Span,
}

/// トレイト実装定義
/// ```mumei
/// impl Comparable for i64 {
///     fn leq(a: i64, b: i64) -> bool { a <= b }
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ImplDef {
    /// 実装対象のトレイト名（例: "Comparable"）
    pub trait_name: String,
    /// 実装する型名（例: "i64"）
    pub target_type: String,
    /// メソッド実装: (メソッド名, body 式の文字列)
    pub method_bodies: Vec<(String, String)>,
    /// ソース位置情報
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Item {
    Atom(Atom),
    TypeDef(RefinedType),
    StructDef(StructDef),
    EnumDef(EnumDef),
    Import(ImportDecl),
    TraitDef(TraitDef),
    ImplDef(ImplDef),
    /// リソース定義: resource name priority mode;
    ResourceDef(ResourceDef),
    /// extern ブロック: extern "Lang" { fn ...; }
    ExternBlock(ExternBlock),
}

// =============================================================================
// FFI Bridge (extern ブロック)
// =============================================================================

/// extern ブロック内の関数シグネチャ
/// ```mumei
/// extern "Rust" {
///     fn sqrt(x: f64) -> f64;
///     fn abs(x: i64) -> i64;
/// }
/// ```
#[derive(Debug, Clone)]
pub struct ExternFn {
    /// 関数名（外部シンボル名）
    pub name: String,
    /// パラメータの型名リスト
    pub param_types: Vec<String>,
    /// 戻り値型名
    pub return_type: String,
    /// ソース位置情報
    pub span: Span,
}

/// extern ブロック定義
#[derive(Debug, Clone)]
pub struct ExternBlock {
    /// 外部言語名（例: "Rust", "C"）
    pub language: String,
    /// 関数シグネチャリスト
    pub functions: Vec<ExternFn>,
    /// ソース位置情報
    pub span: Span,
}

// --- 3. Generics パースヘルパー ---

/// 型パラメータリスト `<T, U>` をパースする。
/// input は `<` で始まる文字列を想定。成功時は (パラメータリスト, 消費バイト数) を返す。
fn parse_type_params_from_str(input: &str) -> (Vec<String>, usize) {
    if !input.starts_with('<') {
        return (vec![], 0);
    }
    let mut depth = 0;
    let mut end = 0;
    for (i, c) in input.char_indices() {
        match c {
            '<' => depth += 1,
            '>' => {
                depth -= 1;
                if depth == 0 {
                    end = i;
                    break;
                }
            }
            _ => {}
        }
    }
    if end == 0 {
        return (vec![], 0);
    }
    let inner = &input[1..end];
    let params: Vec<String> = inner
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (params, end + 1)
}

/// 型参照文字列（例: "Stack<i64>", "i64", "Map<String, List<i64>>"）を TypeRef にパースする。
pub fn parse_type_ref(input: &str) -> TypeRef {
    let input = input.trim();
    if let Some(angle_pos) = input.find('<') {
        // ジェネリック型: "Stack<i64>" → name="Stack", type_args=[TypeRef("i64")]
        let name = input[..angle_pos].trim().to_string();
        // 最後の '>' を見つける
        let inner = if input.ends_with('>') {
            &input[angle_pos + 1..input.len() - 1]
        } else {
            &input[angle_pos + 1..]
        };
        // カンマで分割（ネストした <> を考慮）
        let args = split_type_args(inner);
        let type_args: Vec<TypeRef> = args.iter().map(|a| parse_type_ref(a)).collect();
        TypeRef::generic(&name, type_args)
    } else {
        TypeRef::simple(input)
    }
}

/// ネストした `<>` を考慮してカンマで型引数を分割する
fn split_type_args(input: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in input.chars() {
        match c {
            '<' => {
                depth += 1;
                current.push(c);
            }
            '>' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                let trimmed = current.trim().to_string();
                if !trimmed.is_empty() {
                    result.push(trimmed);
                }
                current.clear();
            }
            _ => {
                current.push(c);
            }
        }
    }
    let trimmed = current.trim().to_string();
    if !trimmed.is_empty() {
        result.push(trimmed);
    }
    result
}

/// 型パラメータリストから境界付きパラメータをパースする。
/// 例: "<T: Comparable, U>" → type_params=["T","U"], bounds=[{param:"T", bounds:["Comparable"]}]
fn parse_type_params_with_bounds(input: &str) -> (Vec<String>, Vec<TypeParamBound>) {
    let (raw_params, _) = parse_type_params_from_str(input);
    let mut type_params = Vec::new();
    let mut bounds = Vec::new();

    for raw in &raw_params {
        if let Some((param, bound_str)) = raw.split_once(':') {
            let param = param.trim().to_string();
            let param_bounds: Vec<String> = bound_str
                .split('+')
                .map(|b| b.trim().to_string())
                .filter(|b| !b.is_empty())
                .collect();
            bounds.push(TypeParamBound {
                param: param.clone(),
                bounds: param_bounds,
            });
            type_params.push(param);
        } else {
            type_params.push(raw.trim().to_string());
        }
    }
    (type_params, bounds)
}

// --- 4. メインパーサーロジック ---

pub fn parse_module(source: &str) -> Vec<Item> {
    let mut items = Vec::new();

    // コメント除去: // から行末までを削除（文字列リテラル内は考慮しない簡易実装）
    let comment_re = Regex::new(r"//[^\n]*").unwrap();
    let source = comment_re.replace_all(source, "").to_string();
    let source = source.as_str();

    // import 定義: import "path" as alias; または import "path";
    let import_re = Regex::new(r#"(?m)^import\s+"([^"]+)"(?:\s+as\s+(\w+))?\s*;"#).unwrap();
    // type 定義: i64 | u64 | f64 を許容するように変更
    let type_re = Regex::new(r"(?m)^type\s+(\w+)\s*=\s*(\w+)\s+where\s+([^;]+);").unwrap();
    let atom_re = Regex::new(r"atom\s+\w+").unwrap();
    // struct 定義: struct Name { field: Type, ... } または struct Name<T> { field: T, ... }
    let struct_re = Regex::new(r"(?m)^struct\s+(\w+)\s*(<[^>]*>)?\s*\{([^}]*)\}").unwrap();

    // import 宣言のパース
    for cap in import_re.captures_iter(source) {
        let path = cap[1].to_string();
        let alias = cap.get(2).map(|m| m.as_str().to_string());
        let m = cap.get(0).unwrap();
        items.push(Item::Import(ImportDecl {
            span: span_from_offset(source, m.start(), m.end() - m.start()),
            path,
            alias,
        }));
    }

    for cap in type_re.captures_iter(source) {
        let full_predicate = cap[3].trim().to_string();
        let tokens = tokenize(&full_predicate);
        let operand = tokens.first().cloned().unwrap_or_else(|| "v".to_string());
        let m = cap.get(0).unwrap();
        items.push(Item::TypeDef(RefinedType {
            name: cap[1].to_string(),
            _base_type: cap[2].to_string(),
            operand,
            predicate_raw: full_predicate,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    for cap in struct_re.captures_iter(source) {
        let name = cap[1].to_string();
        // Generics: 型パラメータ <T, U> のパース
        let type_params = cap
            .get(2)
            .map(|m| {
                let (params, _) = parse_type_params_from_str(m.as_str());
                params
            })
            .unwrap_or_default();
        let fields_raw = &cap[3];
        let fields: Vec<StructField> = fields_raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                // "x: f64 where v >= 0.0" → name="x", type="f64", constraint=Some("v >= 0.0")
                let (field_part, constraint) = if let Some(idx) = s.find("where") {
                    (s[..idx].trim(), Some(s[idx + 5..].trim().to_string()))
                } else {
                    (s.trim(), None)
                };
                let parts: Vec<&str> = field_part.splitn(2, ':').collect();
                let type_name_str = parts
                    .get(1)
                    .map(|t| t.trim().to_string())
                    .unwrap_or_else(|| "i64".to_string());
                let type_ref = parse_type_ref(&type_name_str);
                StructField {
                    name: parts[0].trim().to_string(),
                    type_name: type_name_str,
                    type_ref,
                    constraint,
                }
            })
            .collect();
        let m = cap.get(0).unwrap();
        items.push(Item::StructDef(StructDef {
            name,
            type_params,
            fields,
            method_names: vec![],
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // enum 定義: enum Name { ... } または enum Name<T> { ... }
    // 再帰的 ADT: フィールド型に "Self" または Enum 自身の名前を記述可能
    let enum_re = Regex::new(r"(?m)^enum\s+(\w+)\s*(<[^>]*>)?\s*\{([^}]*)\}").unwrap();
    for cap in enum_re.captures_iter(source) {
        let name = cap[1].to_string();
        // Generics: 型パラメータ <T, U> のパース
        let type_params = cap
            .get(2)
            .map(|m| {
                let (params, _) = parse_type_params_from_str(m.as_str());
                params
            })
            .unwrap_or_default();
        let variants_raw = &cap[3];
        let mut any_recursive = false;
        let variants: Vec<EnumVariant> = variants_raw
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|s| {
                // "Circle(f64)" or "None" or "Cons(i64, Self)" or "Cons(i64, List)"
                if let Some(paren_start) = s.find('(') {
                    let variant_name = s[..paren_start].trim().to_string();
                    let fields_str = &s[paren_start + 1..s.rfind(')').unwrap_or(s.len())];
                    let fields: Vec<String> = fields_str
                        .split(',')
                        .map(|f| {
                            let f = f.trim().to_string();
                            // "Self" を Enum 自身の名前に展開
                            if f == "Self" {
                                name.clone()
                            } else {
                                f
                            }
                        })
                        .filter(|f| !f.is_empty())
                        .collect();
                    // TypeRef 版のフィールド型も生成
                    let field_types: Vec<TypeRef> =
                        fields.iter().map(|f| parse_type_ref(f)).collect();
                    // 再帰判定: フィールドに自身の Enum 名を含むか
                    let is_recursive = fields.iter().any(|f| f == &name);
                    if is_recursive {
                        any_recursive = true;
                    }
                    EnumVariant {
                        name: variant_name,
                        fields,
                        field_types,
                        is_recursive,
                    }
                } else {
                    EnumVariant {
                        name: s.to_string(),
                        fields: vec![],
                        field_types: vec![],
                        is_recursive: false,
                    }
                }
            })
            .collect();
        let m = cap.get(0).unwrap();
        items.push(Item::EnumDef(EnumDef {
            name,
            type_params,
            variants,
            is_recursive: any_recursive,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // trait 定義: trait Name { fn method(a: Type) -> Type; law name: expr; }
    let trait_re = Regex::new(r"(?m)^trait\s+(\w+)\s*\{([^}]*)\}").unwrap();
    for cap in trait_re.captures_iter(source) {
        let name = cap[1].to_string();
        let body = &cap[2];
        let mut methods = Vec::new();
        let mut laws = Vec::new();

        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            if line.starts_with("fn ") {
                // fn leq(a: Self, b: Self) -> bool;
                // fn div(a: Self, b: Self where v != 0) -> Self;
                let fn_re = Regex::new(r"fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)").unwrap();
                if let Some(fcap) = fn_re.captures(line) {
                    let method_name = fcap[1].to_string();
                    let params_str = &fcap[2];
                    let return_type = fcap[3].to_string();
                    let mut param_types: Vec<String> = Vec::new();
                    let mut param_constraints: Vec<Option<String>> = Vec::new();
                    for p in params_str.split(',') {
                        let p = p.trim();
                        if p.is_empty() {
                            continue;
                        }
                        // "b: Self where v != 0" → type="Self", constraint=Some("v != 0")
                        if let Some((before_where, constraint)) = p.split_once("where") {
                            let type_str = if let Some((_, t)) = before_where.split_once(':') {
                                t.trim().to_string()
                            } else {
                                before_where.trim().to_string()
                            };
                            param_types.push(type_str);
                            param_constraints.push(Some(constraint.trim().to_string()));
                        } else if let Some((_, t)) = p.split_once(':') {
                            param_types.push(t.trim().to_string());
                            param_constraints.push(None);
                        } else {
                            param_types.push(p.to_string());
                            param_constraints.push(None);
                        }
                    }
                    methods.push(TraitMethod {
                        name: method_name,
                        param_types,
                        return_type,
                        param_constraints,
                    });
                }
            } else if line.starts_with("law ") {
                // law reflexive: leq(x, x) == true;
                let law_re = Regex::new(r"law\s+(\w+)\s*:\s*([^;]+)").unwrap();
                if let Some(lcap) = law_re.captures(line) {
                    let law_name = lcap[1].to_string();
                    let law_expr = lcap[2].trim().to_string();
                    laws.push((law_name, law_expr));
                }
            }
        }
        let m = cap.get(0).unwrap();
        items.push(Item::TraitDef(TraitDef {
            name,
            methods,
            laws,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // impl 定義: impl TraitName for TypeName { fn method(params) -> Type { body } }
    // ネストした {} を正しく処理するためにカスタムパーサーを使用
    let impl_header_re = Regex::new(r"(?m)^impl\s+(\w+)\s+for\s+(\w+)\s*\{").unwrap();
    for cap in impl_header_re.captures_iter(source) {
        let trait_name = cap[1].to_string();
        let target_type = cap[2].to_string();
        // impl ブロックの開始位置から、ネストした {} を考慮して終了位置を探す
        let block_start = cap.get(0).unwrap().end(); // '{' の直後
        let mut depth = 1;
        let mut block_end = block_start;
        for (i, c) in source[block_start..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        block_end = block_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let body = &source[block_start..block_end];
        let mut method_bodies = Vec::new();

        // fn method(params) -> Type { body } をパース（ネスト対応）
        let fn_header_re = Regex::new(r"fn\s+(\w+)\s*\([^)]*\)\s*->\s*\w+\s*\{").unwrap();
        for fcap in fn_header_re.captures_iter(body) {
            let method_name = fcap[1].to_string();
            let fn_body_start = fcap.get(0).unwrap().end();
            let mut fn_depth = 1;
            let mut fn_body_end = fn_body_start;
            for (i, c) in body[fn_body_start..].char_indices() {
                match c {
                    '{' => fn_depth += 1,
                    '}' => {
                        fn_depth -= 1;
                        if fn_depth == 0 {
                            fn_body_end = fn_body_start + i;
                            break;
                        }
                    }
                    _ => {}
                }
            }
            let method_body = body[fn_body_start..fn_body_end].trim().to_string();
            method_bodies.push((method_name, method_body));
        }
        let m = cap.get(0).unwrap();
        items.push(Item::ImplDef(ImplDef {
            trait_name,
            target_type,
            method_bodies,
            span: span_from_offset(source, m.start(), block_end + 1 - m.start()),
        }));
    }

    // resource 定義: resource name priority:<N> mode:exclusive|shared;
    let resource_re =
        Regex::new(r"(?m)^resource\s+(\w+)\s+priority:\s*(-?\d+)\s+mode:\s*(exclusive|shared)\s*;")
            .unwrap();
    for cap in resource_re.captures_iter(source) {
        let name = cap[1].to_string();
        let priority = cap[2].parse::<i64>().unwrap_or(0);
        let mode = match &cap[3] {
            "exclusive" => ResourceMode::Exclusive,
            _ => ResourceMode::Shared,
        };
        let m = cap.get(0).unwrap();
        items.push(Item::ResourceDef(ResourceDef {
            name,
            priority,
            mode,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // extern ブロック: extern "Rust" { fn name(params) -> RetType; ... }
    let extern_re = Regex::new(r#"(?m)^extern\s+"(\w+)"\s*\{([^}]*)\}"#).unwrap();
    for cap in extern_re.captures_iter(source) {
        let language = cap[1].to_string();
        let body = &cap[2];
        let body_offset = cap.get(2).unwrap().start();
        let mut functions = Vec::new();
        let fn_re = Regex::new(r"fn\s+(\w+)\s*\(([^)]*)\)\s*->\s*(\w+)").unwrap();
        for fcap in fn_re.captures_iter(body) {
            let name = fcap[1].to_string();
            let params_str = &fcap[2];
            let param_types: Vec<String> = params_str
                .split(',')
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .map(|p| {
                    if let Some((_, t)) = p.split_once(':') {
                        t.trim().to_string()
                    } else {
                        p.to_string()
                    }
                })
                .collect();
            let return_type = fcap[3].to_string();
            let fm = fcap.get(0).unwrap();
            functions.push(ExternFn {
                name,
                param_types,
                return_type,
                span: span_from_offset(source, body_offset + fm.start(), fm.end() - fm.start()),
            });
        }
        let m = cap.get(0).unwrap();
        items.push(Item::ExternBlock(ExternBlock {
            language,
            functions,
            span: span_from_offset(source, m.start(), m.end() - m.start()),
        }));
    }

    // 修飾子付き atom のパース: "async atom", "trusted atom", "unverified atom",
    // "async trusted atom" 等の組み合わせを先に検出
    let modified_atom_re = Regex::new(r"(?:(?:async|trusted|unverified)\s+)+atom\s+\w+").unwrap();
    let modified_atom_indices: Vec<_> = modified_atom_re.find_iter(source).collect();
    let mut modified_atom_starts: std::collections::HashSet<usize> =
        std::collections::HashSet::new();
    for mat in &modified_atom_indices {
        let start = mat.start();
        modified_atom_starts.insert(start);
        let atom_source = &source[start..];
        // 修飾子を解析
        let mut is_async = false;
        let mut trust_level = TrustLevel::Verified;
        let mut remaining = atom_source;
        loop {
            remaining = remaining.trim_start();
            if remaining.starts_with("async")
                && remaining[5..].starts_with(|c: char| c.is_whitespace())
            {
                is_async = true;
                remaining = &remaining[5..];
            } else if remaining.starts_with("trusted")
                && remaining[7..].starts_with(|c: char| c.is_whitespace())
            {
                trust_level = TrustLevel::Trusted;
                remaining = &remaining[7..];
            } else if remaining.starts_with("unverified")
                && remaining[10..].starts_with(|c: char| c.is_whitespace())
            {
                trust_level = TrustLevel::Unverified;
                remaining = &remaining[10..];
            } else {
                break;
            }
        }
        // "atom" から始まる部分を切り出して parse_atom に渡す
        let atom_start_in_remaining = remaining.find("atom").unwrap_or(0);
        let atom_text = &remaining[atom_start_in_remaining..];
        // 次の atom の開始位置を探す
        let next_atom_pos = atom_re
            .find(atom_text.get(5..).unwrap_or(""))
            .map(|m| m.start() + 5)
            .unwrap_or(atom_text.len());
        let atom_slice = &atom_text[..next_atom_pos];
        let mut atom = parse_atom(atom_slice);
        atom.is_async = is_async;
        atom.trust_level = trust_level;
        items.push(Item::Atom(atom));
    }

    let atom_indices: Vec<_> = atom_re.find_iter(source).map(|m| m.start()).collect();
    for i in 0..atom_indices.len() {
        let start = atom_indices[i];
        // 修飾子付き atom の一部として既にパース済みならスキップ
        let skip = modified_atom_starts.iter().any(|&ms| {
            start > ms && start < ms + 30 // 修飾子 + "atom" の最大長以内
        });
        if skip {
            continue;
        }
        // 直前に修飾子キーワードがある場合もスキップ
        let prefix = &source[start.saturating_sub(12)..start];
        if prefix.contains("async") || prefix.contains("trusted") || prefix.contains("unverified") {
            continue;
        }
        let end = if i + 1 < atom_indices.len() {
            atom_indices[i + 1]
        } else {
            source.len()
        };
        let atom_source = &source[start..end];
        items.push(Item::Atom(parse_atom(atom_source)));
    }

    items
}

pub fn parse_atom(source: &str) -> Atom {
    // Generics 対応: atom name<T, U>(params) の形式もパース
    let name_re = Regex::new(r"atom\s+(\w+)\s*(<[^>]*>)?\s*\(([^)]*)\)").unwrap();
    let req_re = Regex::new(r"requires:\s*([^;]+);").unwrap();
    let ens_re = Regex::new(r"ensures:\s*([^;]+);").unwrap();

    let forall_re =
        Regex::new(r"forall\(\s*(\w+)\s*,\s*([^,]+)\s*,\s*([^,]+)\s*,\s*([^)]+)\)").unwrap();
    let exists_re =
        Regex::new(r"exists\(\s*(\w+)\s*,\s*([^,]+)\s*,\s*([^,]+)\s*,\s*([^)]+)\)").unwrap();

    let name_caps = name_re.captures(source).expect("Failed to parse atom name");
    let name = name_caps[1].to_string();
    // Generics: 型パラメータ <T: Trait, U> のパース（トレイト境界対応）
    let (type_params, where_bounds) = name_caps
        .get(2)
        .map(|m| parse_type_params_with_bounds(m.as_str()))
        .unwrap_or_default();
    let params: Vec<Param> = name_caps[3]
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| {
            // ref mut / ref 修飾子の検出:
            // "ref mut v: Vector<T>" → is_ref=false, is_ref_mut=true
            // "ref v: Vector<T>" → is_ref=true, is_ref_mut=false
            let (is_ref, is_ref_mut, s_stripped) = if s.starts_with("ref mut ") {
                (false, true, s[8..].trim())
            } else if s.starts_with("ref ") {
                (true, false, s[4..].trim())
            } else {
                (false, false, s)
            };
            if let Some((param_name, type_name)) = s_stripped.split_once(':') {
                let type_name_str = type_name.trim().to_string();
                let type_ref = parse_type_ref(&type_name_str);
                Param {
                    name: param_name.trim().to_string(),
                    type_name: Some(type_name_str),
                    type_ref: Some(type_ref),
                    is_ref,
                    is_ref_mut,
                }
            } else {
                Param {
                    name: s_stripped.to_string(),
                    type_name: None,
                    type_ref: None,
                    is_ref,
                    is_ref_mut,
                }
            }
        })
        .collect();

    let requires_raw = req_re
        .captures(source)
        .map_or("true".to_string(), |c| c[1].trim().to_string());
    let ensures = ens_re
        .captures(source)
        .map_or("true".to_string(), |c| c[1].trim().to_string());

    let body_marker = "body:";
    let body_start_pos =
        source.find(body_marker).expect("Failed to find body:") + body_marker.len();
    let body_snippet = source[body_start_pos..].trim();

    let mut body_raw = String::new();
    if body_snippet.starts_with('{') {
        let mut brace_count = 0;
        for c in body_snippet.chars() {
            body_raw.push(c);
            if c == '{' {
                brace_count += 1;
            } else if c == '}' {
                brace_count -= 1;
                if brace_count == 0 {
                    break;
                }
            }
        }
    } else {
        body_raw = body_snippet.split(';').next().unwrap_or("").to_string();
    }

    let mut forall_constraints = Vec::new();
    for cap in forall_re.captures_iter(&requires_raw) {
        forall_constraints.push(Quantifier {
            q_type: QuantifierType::ForAll,
            var: cap[1].to_string(),
            start: cap[2].trim().to_string(),
            end: cap[3].trim().to_string(),
            condition: cap[4].trim().to_string(),
        });
    }
    for cap in exists_re.captures_iter(&requires_raw) {
        forall_constraints.push(Quantifier {
            q_type: QuantifierType::Exists,
            var: cap[1].to_string(),
            start: cap[2].trim().to_string(),
            end: cap[3].trim().to_string(),
            condition: cap[4].trim().to_string(),
        });
    }

    // consume 句のパース: "consume x, y;" または "consume x;"
    // body: の前に出現する consume 宣言を検出
    let consume_re = Regex::new(r"consume\s+([^;]+);").unwrap();
    let consumed_params: Vec<String> = consume_re
        .captures_iter(source)
        .flat_map(|cap| {
            cap[1]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();

    // resources 句のパース: "resources: [db, cache];" または "resources: db, cache;"
    let resources_re = Regex::new(r"resources:\s*\[?([^\];]+)\]?\s*;").unwrap();
    let resources: Vec<String> = resources_re
        .captures_iter(source)
        .flat_map(|cap| {
            cap[1]
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        })
        .collect();

    // max_unroll 句のパース: "max_unroll: 5;" — BMC 展開回数のオーバーライド
    let max_unroll_re = Regex::new(r"max_unroll:\s*(\d+)\s*;").unwrap();
    let max_unroll = max_unroll_re
        .captures(source)
        .and_then(|cap| cap[1].parse::<usize>().ok());

    // invariant 句のパース: "invariant: <expr>;"
    // atom レベルの状態不変量。再帰呼び出しの帰納的検証に使用。
    let invariant_re = Regex::new(r"(?m)^invariant:\s*([^;]+);").unwrap();
    let invariant = invariant_re
        .captures(source)
        .map(|cap| cap[1].trim().to_string());

    let atom_match = name_caps.get(0).unwrap();
    Atom {
        name,
        type_params,
        where_bounds,
        params,
        requires: forall_re
            .replace_all(&exists_re.replace_all(&requires_raw, "true"), "true")
            .to_string(),
        forall_constraints,
        ensures,
        body_expr: body_raw,
        consumed_params,
        resources,
        is_async: false,
        trust_level: TrustLevel::Verified,
        max_unroll,
        invariant,
        span: span_from_offset(source, atom_match.start(), source.len()),
    }
}

pub fn tokenize(input: &str) -> Vec<String> {
    // 小数点(.)を含む数値リテラルを先にマッチし、残りの `.` はフィールドアクセス演算子として扱う
    let re =
        Regex::new(r"(\d+\.\d+|\d+|[a-zA-Z_]\w*|==|!=|>=|<=|=>|&&|\|\||[+\-*/><()\[\]{};=,:.])")
            .unwrap();
    re.find_iter(input)
        .map(|m| m.as_str().to_string())
        .collect()
}

pub fn parse_expression(input: &str) -> Expr {
    let tokens = tokenize(input);
    let mut pos = 0;
    parse_block_or_expr(&tokens, &mut pos)
}

fn parse_block_or_expr(tokens: &[String], pos: &mut usize) -> Expr {
    if *pos < tokens.len() && tokens[*pos] == "{" {
        *pos += 1;
        let mut stmts = Vec::new();
        while *pos < tokens.len() && tokens[*pos] != "}" {
            stmts.push(parse_statement(tokens, pos));
            if *pos < tokens.len() && tokens[*pos] == ";" {
                *pos += 1;
            }
        }
        if *pos < tokens.len() && tokens[*pos] == "}" {
            *pos += 1;
        }
        Expr::Block(stmts)
    } else {
        parse_implies(tokens, pos)
    }
}

/// match アームの body 専用パーサー。
/// `{...}` ブロックの場合は通常通りパース。
/// それ以外の場合は `parse_logical_or` を使い、`=>` を含意演算子として消費しない。
/// これにより `0 => match x { 0 => 1, _ => 2 }, 1 => ...` のネストが正しく動作する。
fn parse_match_arm_body(tokens: &[String], pos: &mut usize) -> Expr {
    if *pos < tokens.len() && tokens[*pos] == "{" {
        // ブロック式: 通常通りパース（内部の `=>` は match パーサーが処理する）
        parse_block_or_expr(tokens, pos)
    } else if *pos < tokens.len() && tokens[*pos] == "match" {
        // ネストした match 式: match パーサーに委譲（parse_primary 経由）
        parse_implies(tokens, pos)
    } else if *pos < tokens.len() && tokens[*pos] == "if" {
        // if-then-else 式
        parse_implies(tokens, pos)
    } else {
        // それ以外: `=>` を消費しないレベルでパース
        parse_logical_or(tokens, pos)
    }
}

fn parse_statement(tokens: &[String], pos: &mut usize) -> Expr {
    if *pos < tokens.len() && tokens[*pos] == "let" {
        *pos += 1;
        let var = tokens[*pos].clone();
        *pos += 1;
        if *pos < tokens.len() && tokens[*pos] == "=" {
            *pos += 1;
        }
        let value = parse_implies(tokens, pos);
        Expr::Let {
            var,
            value: Box::new(value),
        }
    } else if *pos + 1 < tokens.len()
        && tokens[*pos]
            .chars()
            .next()
            .map_or(false, |c| c.is_alphabetic() || c == '_')
        && tokens[*pos + 1] == "="
    {
        let var = tokens[*pos].clone();
        *pos += 1;
        *pos += 1;
        let value = parse_implies(tokens, pos);
        Expr::Assign {
            var,
            value: Box::new(value),
        }
    } else {
        parse_implies(tokens, pos)
    }
}

fn parse_implies(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_logical_or(tokens, pos);
    while *pos < tokens.len() && tokens[*pos] == "=>" {
        *pos += 1;
        let right = parse_logical_or(tokens, pos);
        node = Expr::BinaryOp(Box::new(node), Op::Implies, Box::new(right));
    }
    node
}

fn parse_logical_or(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_logical_and(tokens, pos);
    while *pos < tokens.len() && tokens[*pos] == "||" {
        *pos += 1;
        let right = parse_logical_and(tokens, pos);
        node = Expr::BinaryOp(Box::new(node), Op::Or, Box::new(right));
    }
    node
}

fn parse_logical_and(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_comparison(tokens, pos);
    while *pos < tokens.len() && tokens[*pos] == "&&" {
        *pos += 1;
        let right = parse_comparison(tokens, pos);
        node = Expr::BinaryOp(Box::new(node), Op::And, Box::new(right));
    }
    node
}

fn parse_comparison(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_add_sub(tokens, pos);
    if *pos < tokens.len() {
        let op = match tokens[*pos].as_str() {
            ">" => Some(Op::Gt),
            "<" => Some(Op::Lt),
            "==" => Some(Op::Eq),
            "!=" => Some(Op::Neq),
            ">=" => Some(Op::Ge),
            "<=" => Some(Op::Le),
            _ => None,
        };
        if let Some(operator) = op {
            *pos += 1;
            let right = parse_add_sub(tokens, pos);
            node = Expr::BinaryOp(Box::new(node), operator, Box::new(right));
        }
    }
    node
}

fn parse_add_sub(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_mul_div(tokens, pos);
    while *pos < tokens.len() && (tokens[*pos] == "+" || tokens[*pos] == "-") {
        let op = if tokens[*pos] == "+" {
            Op::Add
        } else {
            Op::Sub
        };
        *pos += 1;
        let right = parse_mul_div(tokens, pos);
        node = Expr::BinaryOp(Box::new(node), op, Box::new(right));
    }
    node
}

fn parse_mul_div(tokens: &[String], pos: &mut usize) -> Expr {
    let mut node = parse_primary(tokens, pos);
    while *pos < tokens.len() && (tokens[*pos] == "*" || tokens[*pos] == "/") {
        let op = if tokens[*pos] == "*" {
            Op::Mul
        } else {
            Op::Div
        };
        *pos += 1;
        let right = parse_primary(tokens, pos);
        node = Expr::BinaryOp(Box::new(node), op, Box::new(right));
    }
    node
}

fn parse_primary(tokens: &[String], pos: &mut usize) -> Expr {
    if *pos >= tokens.len() {
        return Expr::Number(0);
    }
    let token = &tokens[*pos];

    // acquire 式: acquire resource_name { body }
    if token == "acquire" {
        *pos += 1;
        let resource = if *pos < tokens.len() {
            let r = tokens[*pos].clone();
            *pos += 1;
            r
        } else {
            "unknown".to_string()
        };
        let body = parse_block_or_expr(tokens, pos);
        return Expr::Acquire {
            resource,
            body: Box::new(body),
        };
    }

    // async 式: async { body }
    if token == "async" {
        *pos += 1;
        let body = parse_block_or_expr(tokens, pos);
        return Expr::Async {
            body: Box::new(body),
        };
    }

    // await 式: await expr
    if token == "await" {
        *pos += 1;
        let expr = parse_primary(tokens, pos);
        return Expr::Await {
            expr: Box::new(expr),
        };
    }

    // task 式: task { body } または task group_name { body }
    if token == "task" {
        *pos += 1;
        let group = if *pos < tokens.len() && tokens[*pos] != "{" {
            let g = Some(tokens[*pos].clone());
            *pos += 1;
            g
        } else {
            None
        };
        let body = parse_block_or_expr(tokens, pos);
        return Expr::Task {
            body: Box::new(body),
            group,
        };
    }

    // task_group 式: task_group { task { ... }; task { ... } }
    // task_group:any { task { ... }; task { ... } }
    if token == "task_group" {
        *pos += 1;
        let join_semantics = if *pos < tokens.len() && tokens[*pos] == ":" {
            *pos += 1; // skip ":"
            if *pos < tokens.len() && tokens[*pos] == "any" {
                *pos += 1;
                JoinSemantics::Any
            } else if *pos < tokens.len() && tokens[*pos] == "all" {
                *pos += 1;
                JoinSemantics::All
            } else {
                let unknown = if *pos < tokens.len() {
                    tokens[*pos].clone()
                } else {
                    "<EOF>".to_string()
                };
                panic!(
                    "Unknown task_group join semantics '{}'. Expected 'all' or 'any'.",
                    unknown
                );
            }
        } else {
            JoinSemantics::All
        };
        // { task { ... }; task { ... } } をパース
        let body = parse_block_or_expr(tokens, pos);
        let children = if let Expr::Block(stmts) = body {
            stmts
        } else {
            vec![body]
        };
        return Expr::TaskGroup {
            children,
            join_semantics,
        };
    }

    // while, if 処理 (既存通り)
    if token == "while" {
        *pos += 1;
        let cond = parse_implies(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == "invariant" {
            *pos += 1;
            // `invariant:` の `:` をスキップ（tokenizer が `:` を独立トークンとして分離するため）
            if *pos < tokens.len() && tokens[*pos] == ":" {
                *pos += 1;
            }
            let inv = parse_implies(tokens, pos);
            // オプション: decreases 句（停止性証明用の減少式）
            let decreases = if *pos < tokens.len() && tokens[*pos] == "decreases" {
                *pos += 1;
                // `decreases:` の `:` もスキップ
                if *pos < tokens.len() && tokens[*pos] == ":" {
                    *pos += 1;
                }
                Some(Box::new(parse_implies(tokens, pos)))
            } else {
                None
            };
            let body = parse_block_or_expr(tokens, pos);
            return Expr::While {
                cond: Box::new(cond),
                invariant: Box::new(inv),
                decreases,
                body: Box::new(body),
            };
        }
        panic!("Mumei loops require an 'invariant'.");
    }

    if token == "if" {
        *pos += 1;
        let cond = parse_implies(tokens, pos);
        let then_branch = parse_block_or_expr(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == "else" {
            *pos += 1;
            let else_branch = parse_block_or_expr(tokens, pos);
            return Expr::IfThenElse {
                cond: Box::new(cond),
                then_branch: Box::new(then_branch),
                else_branch: Box::new(else_branch),
            };
        }
        panic!("Mumei requires an 'else' branch.");
    }

    // match 式: match expr { Pattern => expr, ... }
    if token == "match" {
        *pos += 1;
        let target = parse_implies(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == "{" {
            *pos += 1; // skip {
        }
        let mut arms = Vec::new();
        while *pos < tokens.len() && tokens[*pos] != "}" {
            let pattern = parse_pattern(tokens, pos);
            // オプション: ガード条件 "if cond"
            // parse_logical_or を使い、`=>` を含意演算子として消費しない。
            // これにより `Pattern if cond => body` の `=>` がアーム区切りとして正しく処理される。
            let guard = if *pos < tokens.len() && tokens[*pos] == "if" {
                *pos += 1;
                Some(Box::new(parse_logical_or(tokens, pos)))
            } else {
                None
            };
            // "=>" をスキップ
            if *pos < tokens.len() && tokens[*pos] == "=" {
                *pos += 1;
                if *pos < tokens.len() && tokens[*pos] == ">" {
                    *pos += 1;
                }
            } else if *pos < tokens.len() && tokens[*pos] == "=>" {
                *pos += 1;
            }
            // アーム body のパース:
            // `=>` を含意演算子として消費しないよう parse_match_arm_body を使用。
            // これにより `0 => match x { ... }, 1 => ...` のネストが正しく解析される。
            let body = parse_match_arm_body(tokens, pos);
            arms.push(MatchArm {
                pattern,
                guard,
                body: Box::new(body),
            });
            // アーム間の "," をスキップ
            if *pos < tokens.len() && tokens[*pos] == "," {
                *pos += 1;
            }
        }
        if *pos < tokens.len() && tokens[*pos] == "}" {
            *pos += 1;
        }
        return Expr::Match {
            target: Box::new(target),
            arms,
        };
    }

    *pos += 1;
    let mut node = if token == "(" {
        let node = parse_implies(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == ")" {
            *pos += 1;
        }
        node
    } else if let Ok(n) = token.parse::<i64>() {
        Expr::Number(n)
    } else if let Ok(f) = token.parse::<f64>() {
        if token.contains('.') {
            Expr::Float(f)
        } else {
            Expr::Number(token.parse().unwrap())
        }
    } else if *pos < tokens.len() && tokens[*pos] == "{" {
        // 構造体初期化: TypeName { field: expr, ... }
        // 大文字始まりの識別子の後に { が来たら構造体と判定
        if token.chars().next().map_or(false, |c| c.is_uppercase()) {
            *pos += 1; // skip {
            let mut fields = Vec::new();
            while *pos < tokens.len() && tokens[*pos] != "}" {
                let field_name = tokens[*pos].clone();
                *pos += 1;
                if *pos < tokens.len() && tokens[*pos] == ":" {
                    *pos += 1;
                }
                let value = parse_implies(tokens, pos);
                fields.push((field_name, value));
                if *pos < tokens.len() && tokens[*pos] == "," {
                    *pos += 1;
                }
            }
            if *pos < tokens.len() && tokens[*pos] == "}" {
                *pos += 1;
            }
            Expr::StructInit {
                type_name: token.clone(),
                fields,
            }
        } else {
            Expr::Variable(token.clone())
        }
    } else if *pos < tokens.len() && tokens[*pos] == "(" {
        // 関数呼び出し: name(args)
        *pos += 1; // (
        let mut args = Vec::new();
        while *pos < tokens.len() && tokens[*pos] != ")" {
            args.push(parse_implies(tokens, pos));
            if *pos < tokens.len() && tokens[*pos] == "," {
                *pos += 1;
            }
        }
        if *pos < tokens.len() && tokens[*pos] == ")" {
            *pos += 1;
        }
        Expr::Call(token.clone(), args)
    } else if *pos < tokens.len() && tokens[*pos] == "[" {
        // 配列アクセス
        *pos += 1; // [
        let index = parse_implies(tokens, pos);
        if *pos < tokens.len() && tokens[*pos] == "]" {
            *pos += 1;
        }
        Expr::ArrayAccess(token.clone(), Box::new(index))
    } else {
        Expr::Variable(token.clone())
    };

    // フィールドアクセスチェーン: expr.field1.field2 ...
    while *pos < tokens.len() && tokens[*pos] == "." {
        *pos += 1; // skip .
        if *pos < tokens.len() {
            let field = tokens[*pos].clone();
            *pos += 1;
            node = Expr::FieldAccess(Box::new(node), field);
        }
    }
    node
}

/// パターンをパースする
/// - "_" → Wildcard
/// - 数値リテラル → Literal
/// - 大文字始まり識別子 + "(" ... ")" → Variant パターン
/// - 大文字始まり識別子（括弧なし） → Unit Variant パターン
/// - 小文字始まり識別子 → 変数バインド
fn parse_pattern(tokens: &[String], pos: &mut usize) -> Pattern {
    if *pos >= tokens.len() {
        return Pattern::Wildcard;
    }

    let token = &tokens[*pos];

    if token == "_" {
        *pos += 1;
        return Pattern::Wildcard;
    }

    // 負の数値リテラル: "-" + 数字
    if token == "-" && *pos + 1 < tokens.len() {
        if let Ok(n) = tokens[*pos + 1].parse::<i64>() {
            *pos += 2;
            return Pattern::Literal(-n);
        }
    }

    // 数値リテラル
    if let Ok(n) = token.parse::<i64>() {
        *pos += 1;
        return Pattern::Literal(n);
    }

    // 識別子
    if token
        .chars()
        .next()
        .map_or(false, |c| c.is_alphabetic() || c == '_')
    {
        let name = token.clone();
        *pos += 1;

        // 大文字始まり → Variant パターン
        if name.chars().next().map_or(false, |c| c.is_uppercase()) {
            if *pos < tokens.len() && tokens[*pos] == "(" {
                *pos += 1; // skip (
                let mut fields = Vec::new();
                while *pos < tokens.len() && tokens[*pos] != ")" {
                    fields.push(parse_pattern(tokens, pos));
                    if *pos < tokens.len() && tokens[*pos] == "," {
                        *pos += 1;
                    }
                }
                if *pos < tokens.len() && tokens[*pos] == ")" {
                    *pos += 1;
                }
                return Pattern::Variant {
                    variant_name: name,
                    fields,
                };
            }
            // Unit variant（括弧なし）
            return Pattern::Variant {
                variant_name: name,
                fields: vec![],
            };
        }

        // 小文字始まり → 変数バインド
        return Pattern::Variable(name);
    }

    *pos += 1;
    Pattern::Wildcard
}

// =============================================================================
// Generics テスト
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ast::TypeRef;

    #[test]
    fn test_parse_type_ref_simple() {
        let tr = parse_type_ref("i64");
        assert_eq!(tr.name, "i64");
        assert!(tr.type_args.is_empty());
    }

    #[test]
    fn test_parse_type_ref_generic() {
        let tr = parse_type_ref("Stack<i64>");
        assert_eq!(tr.name, "Stack");
        assert_eq!(tr.type_args.len(), 1);
        assert_eq!(tr.type_args[0].name, "i64");
    }

    #[test]
    fn test_parse_type_ref_nested() {
        let tr = parse_type_ref("Map<String, List<i64>>");
        assert_eq!(tr.name, "Map");
        assert_eq!(tr.type_args.len(), 2);
        assert_eq!(tr.type_args[0].name, "String");
        assert_eq!(tr.type_args[1].name, "List");
        assert_eq!(tr.type_args[1].type_args[0].name, "i64");
    }

    #[test]
    fn test_parse_type_ref_display() {
        let tr = parse_type_ref("Stack<i64>");
        assert_eq!(tr.display_name(), "Stack<i64>");

        let tr2 = parse_type_ref("Map<String, List<i64>>");
        assert_eq!(tr2.display_name(), "Map<String, List<i64>>");
    }

    #[test]
    fn test_type_ref_substitute() {
        use std::collections::HashMap;
        let mut map = HashMap::new();
        map.insert("T".to_string(), TypeRef::simple("i64"));

        let tr = TypeRef::simple("T");
        let result = tr.substitute(&map);
        assert_eq!(result.name, "i64");

        let tr2 = TypeRef::generic("Stack", vec![TypeRef::simple("T")]);
        let result2 = tr2.substitute(&map);
        assert_eq!(result2.display_name(), "Stack<i64>");
    }

    #[test]
    fn test_parse_generic_struct() {
        let source = r#"
struct Pair<T, U> {
    first: T,
    second: U
}
"#;
        let items = parse_module(source);
        let struct_items: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::StructDef(s) = i {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(struct_items.len(), 1);
        let s = &struct_items[0];
        assert_eq!(s.name, "Pair");
        assert_eq!(s.type_params, vec!["T", "U"]);
        assert_eq!(s.fields.len(), 2);
        assert_eq!(s.fields[0].name, "first");
        assert_eq!(s.fields[0].type_ref.name, "T");
        assert_eq!(s.fields[1].name, "second");
        assert_eq!(s.fields[1].type_ref.name, "U");
    }

    #[test]
    fn test_parse_generic_enum() {
        let source = r#"
enum Option<T> {
    Some(T),
    None
}
"#;
        let items = parse_module(source);
        let enum_items: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EnumDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(enum_items.len(), 1);
        let e = &enum_items[0];
        assert_eq!(e.name, "Option");
        assert_eq!(e.type_params, vec!["T"]);
        assert_eq!(e.variants.len(), 2);
        assert_eq!(e.variants[0].name, "Some");
        assert_eq!(e.variants[0].field_types[0].name, "T");
        assert_eq!(e.variants[1].name, "None");
        assert!(e.variants[1].fields.is_empty());
    }

    #[test]
    fn test_parse_generic_atom() {
        let source = r#"
atom identity<T>(x: T)
requires: true;
ensures: true;
body: x;
"#;
        let items = parse_module(source);
        let atom_items: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atom_items.len(), 1);
        let a = &atom_items[0];
        assert_eq!(a.name, "identity");
        assert_eq!(a.type_params, vec!["T"]);
        assert_eq!(a.params.len(), 1);
        assert_eq!(a.params[0].name, "x");
        assert_eq!(a.params[0].type_ref.as_ref().unwrap().name, "T");
    }

    #[test]
    fn test_parse_trait_def() {
        let source = r#"
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
    law reflexive: leq(x, x) == true;
    law transitive: leq(a, b) && leq(b, c) => leq(a, c);
}
"#;
        let items = parse_module(source);
        let traits: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::TraitDef(t) = i {
                    Some(t)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(traits.len(), 1);
        let t = &traits[0];
        assert_eq!(t.name, "Comparable");
        assert_eq!(t.methods.len(), 1);
        assert_eq!(t.methods[0].name, "leq");
        assert_eq!(t.methods[0].param_types, vec!["Self", "Self"]);
        assert_eq!(t.methods[0].return_type, "bool");
        assert_eq!(t.laws.len(), 2);
        assert_eq!(t.laws[0].0, "reflexive");
        assert_eq!(t.laws[1].0, "transitive");
    }

    #[test]
    fn test_parse_impl_def() {
        let source = r#"
impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool { a <= b }
}
"#;
        let items = parse_module(source);
        let impls: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ImplDef(im) = i {
                    Some(im)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(impls.len(), 1);
        assert_eq!(impls[0].trait_name, "Comparable");
        assert_eq!(impls[0].target_type, "i64");
        assert_eq!(impls[0].method_bodies.len(), 1);
        assert_eq!(impls[0].method_bodies[0].0, "leq");
        assert_eq!(impls[0].method_bodies[0].1, "a <= b");
    }

    #[test]
    fn test_parse_atom_with_trait_bounds() {
        let source = r#"
atom min<T: Comparable>(a: T, b: T)
requires: true;
ensures: true;
body: a;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        let a = &atoms[0];
        assert_eq!(a.name, "min");
        assert_eq!(a.type_params, vec!["T"]);
        assert_eq!(a.where_bounds.len(), 1);
        assert_eq!(a.where_bounds[0].param, "T");
        assert_eq!(a.where_bounds[0].bounds, vec!["Comparable"]);
    }

    #[test]
    fn test_parse_atom_with_multiple_bounds() {
        let source = r#"
atom sorted_min<T: Comparable + Numeric>(a: T, b: T)
requires: true;
ensures: true;
body: a;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        let a = &atoms[0];
        assert_eq!(a.type_params, vec!["T"]);
        assert_eq!(a.where_bounds.len(), 1);
        assert_eq!(a.where_bounds[0].bounds, vec!["Comparable", "Numeric"]);
    }

    #[test]
    fn test_parse_non_generic_backward_compat() {
        // 非ジェネリック定義が引き続き正しくパースされることを確認
        let source = r#"
struct Point {
    x: f64,
    y: f64
}

enum Color {
    Red,
    Green,
    Blue
}

atom add(a: i64, b: i64)
requires: true;
ensures: true;
body: a + b;
"#;
        let items = parse_module(source);

        let structs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::StructDef(s) = i {
                    Some(s)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(structs.len(), 1);
        assert_eq!(structs[0].name, "Point");
        assert!(structs[0].type_params.is_empty());

        let enums: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::EnumDef(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Color");
        assert!(enums[0].type_params.is_empty());

        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();
        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "add");
        assert!(atoms[0].type_params.is_empty());
    }

    // =========================================================================
    // 非同期処理 + リソース管理のテスト
    // =========================================================================

    #[test]
    fn test_parse_resource_def() {
        let source = r#"
resource db_conn priority: 1 mode: exclusive;
resource cache priority: 2 mode: shared;
"#;
        let items = parse_module(source);
        let resources: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ResourceDef(r) = i {
                    Some(r)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(resources.len(), 2);
        assert_eq!(resources[0].name, "db_conn");
        assert_eq!(resources[0].priority, 1);
        assert_eq!(resources[0].mode, ResourceMode::Exclusive);
        assert_eq!(resources[1].name, "cache");
        assert_eq!(resources[1].priority, 2);
        assert_eq!(resources[1].mode, ResourceMode::Shared);
    }

    #[test]
    fn test_parse_atom_with_resources() {
        let source = r#"
atom transfer(amount: i64)
resources: [db, cache];
requires: amount >= 0;
ensures: true;
body: amount;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        let a = &atoms[0];
        assert_eq!(a.name, "transfer");
        assert_eq!(a.resources, vec!["db", "cache"]);
        assert!(!a.is_async);
    }

    #[test]
    fn test_parse_acquire_expression() {
        let expr = parse_expression("acquire mutex_a { x + 1 }");
        match expr {
            Expr::Acquire { resource, body } => {
                assert_eq!(resource, "mutex_a");
                // body should be a Block containing x + 1
                match *body {
                    Expr::Block(_) => {} // OK
                    _ => panic!("Expected Block in acquire body"),
                }
            }
            _ => panic!("Expected Acquire expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_async_expression() {
        let expr = parse_expression("async { x + 1 }");
        match expr {
            Expr::Async { body } => {
                match *body {
                    Expr::Block(_) => {} // OK
                    _ => panic!("Expected Block in async body"),
                }
            }
            _ => panic!("Expected Async expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_trusted_atom() {
        let source = r#"
trusted atom ffi_read(fd: i64)
requires: fd >= 0;
ensures: result >= 0;
body: fd;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "ffi_read");
        assert_eq!(atoms[0].trust_level, TrustLevel::Trusted);
        assert!(!atoms[0].is_async);
    }

    #[test]
    fn test_parse_unverified_atom() {
        let source = r#"
unverified atom legacy_code(x: i64)
requires: true;
ensures: true;
body: x;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "legacy_code");
        assert_eq!(atoms[0].trust_level, TrustLevel::Unverified);
    }

    #[test]
    fn test_parse_async_trusted_atom() {
        let source = r#"
async trusted atom fetch_external(url: i64)
requires: url >= 0;
ensures: result >= 0;
body: url;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].name, "fetch_external");
        assert!(atoms[0].is_async);
        assert_eq!(atoms[0].trust_level, TrustLevel::Trusted);
    }

    #[test]
    fn test_parse_max_unroll() {
        let source = r#"
atom loop_with_acquire(n: i64)
max_unroll: 5;
requires: n >= 0;
ensures: true;
body: n;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        assert_eq!(atoms[0].max_unroll, Some(5));
    }

    #[test]
    fn test_parse_atom_invariant() {
        let source = r#"
async atom process(state: i64)
invariant: state >= 0;
requires: state >= 0;
ensures: result >= 0;
body: state + 1;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        let a = &atoms[0];
        assert_eq!(a.name, "process");
        assert!(a.is_async);
        assert_eq!(a.invariant, Some("state >= 0".to_string()));
    }

    #[test]
    fn test_parse_ref_mut_param() {
        let source = r#"
atom modify(ref mut v: i64, ref r: i64)
requires: v >= 0;
ensures: result >= 0;
body: v + r;
"#;
        let items = parse_module(source);
        let atoms: Vec<_> = items
            .iter()
            .filter_map(|i| if let Item::Atom(a) = i { Some(a) } else { None })
            .collect();

        assert_eq!(atoms.len(), 1);
        let a = &atoms[0];
        assert_eq!(a.params.len(), 2);
        // ref mut v
        assert_eq!(a.params[0].name, "v");
        assert!(!a.params[0].is_ref);
        assert!(a.params[0].is_ref_mut);
        // ref r
        assert_eq!(a.params[1].name, "r");
        assert!(a.params[1].is_ref);
        assert!(!a.params[1].is_ref_mut);
    }

    #[test]
    fn test_parse_await_expression() {
        let expr = parse_expression("await x");
        match expr {
            Expr::Await { expr } => match *expr {
                Expr::Variable(ref name) => assert_eq!(name, "x"),
                _ => panic!("Expected Variable in await expr"),
            },
            _ => panic!("Expected Await expression, got {:?}", expr),
        }
    }

    // =========================================================================
    // task / task_group パーステスト
    // =========================================================================

    #[test]
    fn test_parse_task_expression() {
        let expr = parse_expression("task { x + 1 }");
        match expr {
            Expr::Task { body, group } => {
                assert!(group.is_none());
                match *body {
                    Expr::Block(_) => {} // OK
                    _ => panic!("Expected Block in task body"),
                }
            }
            _ => panic!("Expected Task expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_task_with_group_name() {
        let expr = parse_expression("task workers { x + 1 }");
        match expr {
            Expr::Task { body, group } => {
                assert_eq!(group, Some("workers".to_string()));
                match *body {
                    Expr::Block(_) => {} // OK
                    _ => panic!("Expected Block in task body"),
                }
            }
            _ => panic!("Expected Task expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_task_group_default_semantics() {
        let expr = parse_expression("task_group { task { x }; task { y } }");
        match expr {
            Expr::TaskGroup {
                children,
                join_semantics,
            } => {
                assert_eq!(join_semantics, JoinSemantics::All);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected TaskGroup expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_task_group_any_semantics() {
        let expr = parse_expression("task_group:any { task { x }; task { y } }");
        match expr {
            Expr::TaskGroup {
                children,
                join_semantics,
            } => {
                assert_eq!(join_semantics, JoinSemantics::Any);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected TaskGroup expression, got {:?}", expr),
        }
    }

    #[test]
    fn test_parse_task_group_all_semantics() {
        let expr = parse_expression("task_group:all { task { x }; task { y } }");
        match expr {
            Expr::TaskGroup {
                children,
                join_semantics,
            } => {
                assert_eq!(join_semantics, JoinSemantics::All);
                assert_eq!(children.len(), 2);
            }
            _ => panic!("Expected TaskGroup expression, got {:?}", expr),
        }
    }

    #[test]
    #[should_panic(expected = "Unknown task_group join semantics")]
    fn test_parse_task_group_unknown_semantics_panics() {
        parse_expression("task_group:bogus { task { x } }");
    }

    // =========================================================================
    // extern ブロック パーステスト
    // =========================================================================

    #[test]
    fn test_parse_extern_block() {
        let source = r#"
extern "Rust" {
    fn sqrt(x: f64) -> f64;
    fn abs(x: i64) -> i64;
}
"#;
        let items = parse_module(source);
        let externs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ExternBlock(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(externs.len(), 1);
        let eb = &externs[0];
        assert_eq!(eb.language, "Rust");
        assert_eq!(eb.functions.len(), 2);
        assert_eq!(eb.functions[0].name, "sqrt");
        assert_eq!(eb.functions[0].param_types, vec!["f64"]);
        assert_eq!(eb.functions[0].return_type, "f64");
        assert_eq!(eb.functions[1].name, "abs");
        assert_eq!(eb.functions[1].param_types, vec!["i64"]);
        assert_eq!(eb.functions[1].return_type, "i64");
        // Span should be populated
        assert!(eb.span.line > 0);
    }

    #[test]
    fn test_parse_extern_block_c() {
        let source = r#"
extern "C" {
    fn printf(fmt: i64) -> i64;
}
"#;
        let items = parse_module(source);
        let externs: Vec<_> = items
            .iter()
            .filter_map(|i| {
                if let Item::ExternBlock(e) = i {
                    Some(e)
                } else {
                    None
                }
            })
            .collect();

        assert_eq!(externs.len(), 1);
        assert_eq!(externs[0].language, "C");
        assert_eq!(externs[0].functions.len(), 1);
        assert_eq!(externs[0].functions[0].name, "printf");
    }
}
