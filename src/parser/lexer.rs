// =============================================================================
// Lexer: converts source string into Vec<SpannedToken>
// =============================================================================

use super::token::{SpannedToken, Token};

/// Lexer that converts a source string into a sequence of tokens.
pub struct Lexer<'a> {
    // NOTE: source is retained for future span-to-source-text resolution and error reporting
    #[allow(dead_code)]
    source: &'a str,
    chars: Vec<char>,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> Lexer<'a> {
    pub fn new(source: &'a str) -> Self {
        Lexer {
            source,
            chars: source.chars().collect(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn tokenize(&mut self) -> Vec<SpannedToken> {
        let mut tokens = Vec::new();
        loop {
            self.skip_whitespace_and_comments();
            if self.pos >= self.chars.len() {
                tokens.push(SpannedToken {
                    token: Token::Eof,
                    line: self.line,
                    col: self.col,
                    len: 0,
                });
                break;
            }
            let tok = self.next_token();
            tokens.push(tok);
        }
        tokens
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_next(&self) -> Option<char> {
        self.chars.get(self.pos + 1).copied()
    }

    fn advance(&mut self) -> char {
        let c = self.chars[self.pos];
        self.pos += 1;
        if c == '\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        c
    }

    fn skip_whitespace_and_comments(&mut self) {
        loop {
            // Skip whitespace
            while self.pos < self.chars.len() && self.chars[self.pos].is_whitespace() {
                self.advance();
            }
            // Skip line comments: // ...
            if self.pos + 1 < self.chars.len()
                && self.chars[self.pos] == '/'
                && self.chars[self.pos + 1] == '/'
            {
                while self.pos < self.chars.len() && self.chars[self.pos] != '\n' {
                    self.advance();
                }
                continue;
            }
            break;
        }
    }

    fn next_token(&mut self) -> SpannedToken {
        let start_line = self.line;
        let start_col = self.col;
        let c = self.advance();

        let token = match c {
            '+' => Token::Plus,
            '*' => Token::Star,
            ',' => Token::Comma,
            ':' => Token::Colon,
            ';' => Token::Semicolon,
            '(' => Token::LParen,
            ')' => Token::RParen,
            '{' => Token::LBrace,
            '}' => Token::RBrace,
            '[' => Token::LBracket,
            ']' => Token::RBracket,
            '@' => Token::At,
            '.' => Token::Dot,

            '-' => {
                if self.peek() == Some('>') {
                    self.advance();
                    Token::Arrow
                } else {
                    Token::Minus
                }
            }

            '=' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::Eq
                } else if self.peek() == Some('>') {
                    self.advance();
                    Token::FatArrow
                } else {
                    Token::Assign
                }
            }

            '!' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::Neq
                } else {
                    Token::Bang
                }
            }

            '>' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::Ge
                } else {
                    Token::Gt
                }
            }

            '<' => {
                if self.peek() == Some('=') {
                    self.advance();
                    Token::Le
                } else {
                    Token::Lt
                }
            }

            '&' => {
                if self.peek() == Some('&') {
                    self.advance();
                    Token::And
                } else {
                    // Single & not used in mumei, treat as unknown ident
                    Token::Ident("&".to_string())
                }
            }

            '|' => {
                if self.peek() == Some('|') {
                    self.advance();
                    Token::Or
                } else if self.peek() == Some('>') {
                    self.advance();
                    Token::Pipe
                } else {
                    Token::Bar
                }
            }

            '/' => Token::Slash,

            '"' => self.read_string_literal(),

            _ if c.is_ascii_digit() => self.read_number(c, start_line, start_col),

            _ if c.is_alphabetic() || c == '_' => self.read_identifier(c),

            _ => {
                // Unknown character, skip
                Token::Ident(c.to_string())
            }
        };

        let len = match &token {
            Token::Ident(s) => s.len(),
            Token::StringLit(s) => s.len() + 2,
            Token::IntLit(n) => format!("{}", n).len(),
            Token::FloatLit(n) => format!("{}", n).len(),
            _ => format!("{}", token).len(),
        };

        SpannedToken {
            token,
            line: start_line,
            col: start_col,
            len,
        }
    }

    fn read_string_literal(&mut self) -> Token {
        let mut s = String::new();
        while self.pos < self.chars.len() && self.chars[self.pos] != '"' {
            let c = self.advance();
            if c == '\\' && self.pos < self.chars.len() {
                let escaped = self.advance();
                match escaped {
                    'n' => s.push('\n'),
                    't' => s.push('\t'),
                    '\\' => s.push('\\'),
                    '"' => s.push('"'),
                    _ => {
                        s.push('\\');
                        s.push(escaped);
                    }
                }
            } else {
                s.push(c);
            }
        }
        // consume closing quote
        if self.pos < self.chars.len() {
            self.advance();
        }
        Token::StringLit(s)
    }

    fn read_number(&mut self, first: char, _start_line: usize, _start_col: usize) -> Token {
        let mut num_str = String::new();
        num_str.push(first);
        let mut is_float = false;

        while self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            if c.is_ascii_digit() {
                num_str.push(c);
                self.advance();
            } else if c == '.' && self.peek_next().is_some_and(|n| n.is_ascii_digit()) {
                // Only treat as float if digit follows the dot
                is_float = true;
                num_str.push(c);
                self.advance();
            } else {
                break;
            }
        }

        if is_float {
            Token::FloatLit(num_str.parse::<f64>().unwrap_or(0.0))
        } else {
            Token::IntLit(num_str.parse::<i64>().unwrap_or(0))
        }
    }

    fn read_identifier(&mut self, first: char) -> Token {
        let mut ident = String::new();
        ident.push(first);
        while self.pos < self.chars.len() {
            let c = self.chars[self.pos];
            if c.is_alphanumeric() || c == '_' {
                ident.push(c);
                self.advance();
            } else {
                break;
            }
        }

        // Match keywords (identifier reader already consumed underscores,
        // so compound keywords like atom_ref come in as full strings)
        match ident.as_str() {
            "atom_ref" => Token::AtomRef,
            "atom" => Token::Atom,
            "task_group" => Token::TaskGroup,
            "task" => Token::Task,
            "max_unroll" => Token::MaxUnroll,
            "let" => Token::Let,
            "if" => Token::If,
            "else" => Token::Else,
            "while" => Token::While,
            "match" => Token::Match,
            "fn" => Token::Fn,
            "struct" => Token::Struct,
            "enum" => Token::Enum,
            "trait" => Token::Trait,
            "impl" => Token::Impl,
            "import" => Token::Import,
            "type" => Token::Type,
            "where" => Token::Where,
            "requires" => Token::Requires,
            "ensures" => Token::Ensures,
            "body" => Token::Body,
            "true" => Token::True,
            "false" => Token::False,
            "trusted" => Token::Trusted,
            "unverified" => Token::Unverified,
            "async" => Token::Async,
            "await" => Token::Await,
            "acquire" => Token::Acquire,
            "resource" => Token::Resource,
            "effect" => Token::Effect,
            "extern" => Token::Extern,
            "consume" => Token::Consume,
            "invariant" => Token::Invariant,
            "decreases" => Token::Decreases,
            "effects" => Token::Effects,
            "resources" => Token::Resources,
            "for" => Token::For,
            "forall" => Token::Forall,
            "exists" => Token::Exists,
            "ref" => Token::Ref,
            "mut" => Token::Mut,
            "call" => Token::Call,
            "perform" => Token::Perform,
            "law" => Token::Law,
            "priority" => Token::Priority,
            "mode" => Token::Mode,
            "exclusive" => Token::Exclusive,
            "shared" => Token::Shared,
            "includes" => Token::Includes,
            "parent" => Token::Parent,
            "as" => Token::As,
            "contract" => Token::Contract,
            _ => Token::Ident(ident),
        }
    }
}

/// Legacy tokenize function for backward compatibility.
/// Converts source string to Vec<String> tokens.
pub fn legacy_tokenize(input: &str) -> Vec<String> {
    let mut lexer = Lexer::new(input);
    let tokens = lexer.tokenize();
    tokens
        .into_iter()
        .filter(|t| !matches!(t.token, Token::Eof))
        .map(|t| match &t.token {
            Token::IntLit(n) => format!("{}", n),
            Token::FloatLit(n) => {
                let s = format!("{}", n);
                // Preserve the float format with decimal point
                if s.contains('.') {
                    s
                } else {
                    format!("{}.0", s)
                }
            }
            Token::StringLit(s) => format!("\"{}\"", s),
            Token::Ident(s) => s.clone(),
            // Keywords and operators use their Display impl
            other => format!("{}", other),
        })
        .collect()
}
