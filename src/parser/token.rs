// =============================================================================
// Token definitions for the mumei lexer
// =============================================================================

/// All token types in the mumei language.
#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    // --- Keywords ---
    Atom,
    Let,
    If,
    Else,
    While,
    Match,
    Fn,
    Struct,
    Enum,
    Trait,
    Impl,
    Import,
    Type,
    Where,
    Requires,
    Ensures,
    Body,
    True,
    False,
    Trusted,
    Unverified,
    Async,
    Await,
    Task,
    TaskGroup,
    Acquire,
    Resource,
    Effect,
    Extern,
    Consume,
    Invariant,
    Decreases,
    MaxUnroll,
    Effects,
    Resources,
    For,
    Forall,
    Exists,
    Ref,
    Mut,
    AtomRef,
    Call,
    Perform,
    Law,
    Priority,
    Mode,
    Exclusive,
    Shared,
    Includes,
    Parent,
    As,
    Contract,

    // --- Literals ---
    IntLit(i64),
    FloatLit(f64),
    StringLit(String),

    // --- Identifiers ---
    Ident(String),

    // --- Operators ---
    Plus,      // +
    Minus,     // -
    Star,      // *
    Slash,     // /
    Eq,        // ==
    Neq,       // !=
    Gt,        // >
    Lt,        // <
    Ge,        // >=
    Le,        // <=
    And,       // &&
    Or,        // ||
    Arrow,     // ->
    FatArrow,  // =>
    Pipe,      // |>
    Assign,    // =
    Dot,       // .
    Comma,     // ,
    Colon,     // :
    Semicolon, // ;
    Bang,      // !
    At,        // @

    // --- Delimiters ---
    LParen,   // (
    RParen,   // )
    LBrace,   // {
    RBrace,   // }
    LBracket, // [
    RBracket, // ]

    // --- Special ---
    Eof,
}

impl Token {
    /// Check if two tokens match by variant (ignoring inner values for literals/idents).
    // NOTE: same_kind is infrastructure for future token comparison in error recovery and diagnostics
    #[allow(dead_code)]
    pub fn same_kind(&self, other: &Token) -> bool {
        std::mem::discriminant(self) == std::mem::discriminant(other)
    }
}

impl std::fmt::Display for Token {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Token::Atom => write!(f, "atom"),
            Token::Let => write!(f, "let"),
            Token::If => write!(f, "if"),
            Token::Else => write!(f, "else"),
            Token::While => write!(f, "while"),
            Token::Match => write!(f, "match"),
            Token::Fn => write!(f, "fn"),
            Token::Struct => write!(f, "struct"),
            Token::Enum => write!(f, "enum"),
            Token::Trait => write!(f, "trait"),
            Token::Impl => write!(f, "impl"),
            Token::Import => write!(f, "import"),
            Token::Type => write!(f, "type"),
            Token::Where => write!(f, "where"),
            Token::Requires => write!(f, "requires"),
            Token::Ensures => write!(f, "ensures"),
            Token::Body => write!(f, "body"),
            Token::True => write!(f, "true"),
            Token::False => write!(f, "false"),
            Token::Trusted => write!(f, "trusted"),
            Token::Unverified => write!(f, "unverified"),
            Token::Async => write!(f, "async"),
            Token::Await => write!(f, "await"),
            Token::Task => write!(f, "task"),
            Token::TaskGroup => write!(f, "task_group"),
            Token::Acquire => write!(f, "acquire"),
            Token::Resource => write!(f, "resource"),
            Token::Effect => write!(f, "effect"),
            Token::Extern => write!(f, "extern"),
            Token::Consume => write!(f, "consume"),
            Token::Invariant => write!(f, "invariant"),
            Token::Decreases => write!(f, "decreases"),
            Token::MaxUnroll => write!(f, "max_unroll"),
            Token::Effects => write!(f, "effects"),
            Token::Resources => write!(f, "resources"),
            Token::For => write!(f, "for"),
            Token::Forall => write!(f, "forall"),
            Token::Exists => write!(f, "exists"),
            Token::Ref => write!(f, "ref"),
            Token::Mut => write!(f, "mut"),
            Token::AtomRef => write!(f, "atom_ref"),
            Token::Call => write!(f, "call"),
            Token::Perform => write!(f, "perform"),
            Token::Law => write!(f, "law"),
            Token::Priority => write!(f, "priority"),
            Token::Mode => write!(f, "mode"),
            Token::Exclusive => write!(f, "exclusive"),
            Token::Shared => write!(f, "shared"),
            Token::Includes => write!(f, "includes"),
            Token::Parent => write!(f, "parent"),
            Token::As => write!(f, "as"),
            Token::Contract => write!(f, "contract"),
            Token::IntLit(n) => write!(f, "{}", n),
            Token::FloatLit(n) => write!(f, "{}", n),
            Token::StringLit(s) => write!(f, "\"{}\"", s),
            Token::Ident(s) => write!(f, "{}", s),
            Token::Plus => write!(f, "+"),
            Token::Minus => write!(f, "-"),
            Token::Star => write!(f, "*"),
            Token::Slash => write!(f, "/"),
            Token::Eq => write!(f, "=="),
            Token::Neq => write!(f, "!="),
            Token::Gt => write!(f, ">"),
            Token::Lt => write!(f, "<"),
            Token::Ge => write!(f, ">="),
            Token::Le => write!(f, "<="),
            Token::And => write!(f, "&&"),
            Token::Or => write!(f, "||"),
            Token::Arrow => write!(f, "->"),
            Token::FatArrow => write!(f, "=>"),
            Token::Pipe => write!(f, "|>"),
            Token::Assign => write!(f, "="),
            Token::Dot => write!(f, "."),
            Token::Comma => write!(f, ","),
            Token::Colon => write!(f, ":"),
            Token::Semicolon => write!(f, ";"),
            Token::Bang => write!(f, "!"),
            Token::At => write!(f, "@"),
            Token::LParen => write!(f, "("),
            Token::RParen => write!(f, ")"),
            Token::LBrace => write!(f, "{{"),
            Token::RBrace => write!(f, "}}"),
            Token::LBracket => write!(f, "["),
            Token::RBracket => write!(f, "]"),
            Token::Eof => write!(f, "<EOF>"),
        }
    }
}

/// A token with its source span information.
#[derive(Debug, Clone)]
// NOTE: SpannedToken fields (line, col, len) are read during span construction for diagnostics and LSP positioning
#[allow(dead_code)]
pub struct SpannedToken {
    pub token: Token,
    pub line: usize,
    pub col: usize,
    pub len: usize,
}
