//! Bounded regex → Z3 RegLan compiler (P10-B).
//!
//! Compiles a decidable fragment of Rust-regex syntax into `z3::ast::Regexp`
//! (RegLan / `str.in_re`) so `matches(param, "...")` effect constraints and
//! `matches` / `match_regex` / `re_match` contract builtins are checked
//! natively by Z3 instead of being approximated or dropped.
//!
//! Supported fragment (matches Rust `regex::Regex::is_match` semantics —
//! substring search: unanchored ends are wrapped in `Σ*`):
//!   literal chars, escaped metachars (`\.` `\*` `\\` …), `.` (any char
//!   except `\n`, matching Rust's default), character classes `[a-zA-Z_]`
//!   including ranges and `[^…]` negation (exact via `re.complement`
//!   intersected with the single-char domain), `*` `+` `?` `{n}` `{n,}`
//!   `{n,m}` repetition, `|` alternation, `()` grouping, and `^` / `$`
//!   anchors at the outermost edges only.
//!
//! Unsupported (returns `None`, callers fall back / escalate): `\b` and other
//! zero-width assertions, backreferences, lookarounds, interior `^`/`$`,
//! `{n,m}` with `m` above `MAX_LOOP_BOUND` (bounds the RegLan blowup).

use z3::ast::Regexp;
use z3::Context;

/// Upper bound on `{n}` / `{n,m}` expansion — keeps the emitted RegLan small.
const MAX_LOOP_BOUND: u32 = 64;

/// Highest character `re.range` can address. Z3's range bounds are limited
/// to the 7-bit ASCII plane — empirically a `hi` above `'\u{7f}'` makes the
/// whole range empty, so larger bounds are rejected as unsupported rather
/// than silently compiled to an empty language.
const MAX_RANGE_CHAR: char = '\u{7f}';

/// Single-character domain Σ (one ASCII scalar value, `\u{1}`–`\u{7f}`).
/// The lower bound is `\u{1}` — Z3 string literals are C strings and
/// cannot embed NUL, so `\0` is unrepresentable anyway.
fn sigma<'ctx>(ctx: &'ctx Context) -> Regexp<'ctx> {
    Regexp::range(ctx, &'\u{1}', &MAX_RANGE_CHAR)
}

/// Σ \ {c} — every single character except `c` (e.g. `.` excludes `\n`).
fn sigma_except<'ctx>(ctx: &'ctx Context, c: char) -> Regexp<'ctx> {
    let all = sigma(ctx);
    let bad = Regexp::literal(ctx, &c.to_string()).complement();
    Regexp::intersect(ctx, &[&all, &bad])
}

/// Compile `pattern` into a RegLan expression with *full-match* semantics.
/// Returns `None` when the pattern uses constructs outside the fragment.
fn compile_full<'ctx>(ctx: &'ctx Context, pattern: &str) -> Option<Regexp<'ctx>> {
    let mut p = Parser::new(ctx, pattern);
    let re = p.alternation()?;
    if !p.at_end() {
        return None;
    }
    Some(re)
}

/// Compile `pattern` into `regexp` with Rust `is_match` substring semantics:
/// `^` at the start removes the leading `Σ*`, `$` at the end removes the
/// trailing `Σ*`. Returns `None` for unsupported constructs (including
/// anchors that are not at the outermost edges).
pub(crate) fn compile_search<'ctx>(ctx: &'ctx Context, pattern: &str) -> Option<Regexp<'ctx>> {
    // Z3 string literals are C strings — NUL cannot be embedded.
    if pattern.contains('\0') {
        return None;
    }
    let chars: Vec<char> = pattern.chars().collect();
    let (mut lo, mut hi) = (0usize, chars.len());
    let anchored_start = chars.first() == Some(&'^');
    // A trailing `$` is an anchor only when it is not escaped — count the
    // backslashes immediately before it (`a\$` = literal "a$").
    let trailing_backslashes = chars[..hi]
        .iter()
        .rev()
        .skip(1)
        .take_while(|c| **c == '\\')
        .count();
    let anchored_end = hi > lo && chars[hi - 1] == '$' && trailing_backslashes % 2 == 0;
    if anchored_start {
        lo = 1;
    }
    if anchored_end {
        hi -= 1;
    }
    let inner: String = chars[lo..hi].iter().collect();
    let body = compile_full(ctx, &inner)?;
    let mut seq: Vec<Regexp> = Vec::with_capacity(3);
    if !anchored_start {
        seq.push(Regexp::full(ctx));
    }
    seq.push(body);
    if !anchored_end {
        seq.push(Regexp::full(ctx));
    }
    if seq.len() == 1 {
        return seq.into_iter().next();
    }
    let refs: Vec<&Regexp> = seq.iter().collect();
    Some(Regexp::concat(ctx, &refs))
}

/// Parse-only check: `true` when the pattern is inside the supported
/// fragment. Used by the decidable-fragment detector so uncompilable
/// patterns keep the `regex_semantics` tag (Lean delegation).
pub(crate) fn supported(pattern: &str) -> bool {
    // Compile against a scratch context; a pattern is supported iff the
    // compiler accepts it. `Config::new`/`Context::new` is cheap enough for
    // the contract-size patterns used here.
    let cfg = z3::Config::new();
    let ctx = Context::new(&cfg);
    let ok = compile_search(&ctx, pattern).is_some();
    ok
}

struct Parser<'a, 'ctx> {
    ctx: &'ctx Context,
    chars: Vec<char>,
    pos: usize,
    _pattern: &'a str,
}

impl<'a, 'ctx> Parser<'a, 'ctx> {
    fn new(ctx: &'ctx Context, pattern: &'a str) -> Self {
        Self {
            ctx,
            chars: pattern.chars().collect(),
            pos: 0,
            _pattern: pattern,
        }
    }

    fn at_end(&self) -> bool {
        self.pos >= self.chars.len()
    }

    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn bump(&mut self) -> Option<char> {
        let c = self.peek()?;
        self.pos += 1;
        Some(c)
    }

    fn literal(&self, text: &str) -> Regexp<'ctx> {
        Regexp::literal(self.ctx, text)
    }

    /// alternation := concat ('|' concat)*
    fn alternation(&mut self) -> Option<Regexp<'ctx>> {
        let mut branches = vec![self.concat()?];
        while self.peek() == Some('|') {
            self.bump();
            branches.push(self.concat()?);
        }
        if branches.len() == 1 {
            return branches.into_iter().next();
        }
        let refs: Vec<&Regexp> = branches.iter().collect();
        Some(Regexp::union(self.ctx, &refs))
    }

    /// concat := postfix*
    fn concat(&mut self) -> Option<Regexp<'ctx>> {
        let mut seq: Vec<Regexp> = Vec::new();
        while let Some(c) = self.peek() {
            if c == '|' || c == ')' {
                break;
            }
            seq.push(self.postfix()?);
        }
        if seq.is_empty() {
            return Some(self.literal(""));
        }
        if seq.len() == 1 {
            return seq.into_iter().next();
        }
        let refs: Vec<&Regexp> = seq.iter().collect();
        Some(Regexp::concat(self.ctx, &refs))
    }

    /// postfix := atom (('*'|'+'|'?') '?'? | '{' repet '}')*
    fn postfix(&mut self) -> Option<Regexp<'ctx>> {
        let mut re = self.atom()?;
        loop {
            match self.peek() {
                Some('*') => {
                    self.bump();
                    if self.peek() == Some('?') {
                        self.bump(); // lazy flag: same language
                    }
                    re = re.star();
                }
                Some('+') => {
                    self.bump();
                    if self.peek() == Some('?') {
                        self.bump();
                    }
                    re = re.plus();
                }
                Some('?') => {
                    self.bump();
                    if self.peek() == Some('?') {
                        self.bump();
                    }
                    re = re.r#loop(0, 1);
                }
                Some('{') => {
                    self.bump();
                    let (lo, hi) = match self.repeat_bounds() {
                        // Rust regex rejects malformed `{…}` (bare `{`,
                        // whitespace bounds, reversed ranges) — reject rather
                        // than reinterpret as a literal (fail-closed).
                        RepeatBounds::Invalid => return None,
                        RepeatBounds::Bounds(lo, hi) => (lo, hi),
                    };
                    // `{n,m}?` is the lazy counted form — same language as
                    // `{n,m}` under `is_match`, so consume (don't apply) `?`.
                    if self.peek() == Some('?') {
                        self.bump();
                    }
                    if hi != u32::MAX && hi > MAX_LOOP_BOUND {
                        return None;
                    }
                    if lo > MAX_LOOP_BOUND {
                        return None;
                    }
                    // re{n,} desugars to re{n} re* (Z3 re.loop is bounded).
                    let mut parts: Vec<Regexp> = Vec::new();
                    if lo > 0 {
                        parts.push(re.r#loop(lo, lo));
                    }
                    if hi > lo {
                        parts.push(if hi == u32::MAX {
                            re.star()
                        } else {
                            re.r#loop(0, hi - lo)
                        });
                    }
                    re = match parts.len() {
                        0 => self.literal(""),
                        1 => parts.into_iter().next().unwrap(),
                        _ => {
                            let refs: Vec<&Regexp> = parts.iter().collect();
                            Regexp::concat(self.ctx, &refs)
                        }
                    };
                }
                _ => break,
            }
        }
        Some(re)
    }

    /// Parses `n` / `n,` / `n,m` inside `{…}` after the `{` was consumed.
    /// `u32::MAX` encodes an unbounded upper count.
    fn repeat_bounds(&mut self) -> RepeatBounds {
        let mut lo_str = String::new();
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() {
                lo_str.push(c);
                self.bump();
            } else {
                break;
            }
        }
        let Ok(lo) = lo_str.parse::<u32>() else {
            return RepeatBounds::Invalid;
        };
        match self.peek() {
            Some('}') => {
                self.bump();
                RepeatBounds::Bounds(lo, lo)
            }
            Some(',') => {
                self.bump();
                let mut hi_str = String::new();
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() {
                        hi_str.push(c);
                        self.bump();
                    } else {
                        break;
                    }
                }
                if self.bump() != Some('}') {
                    return RepeatBounds::Invalid;
                }
                if hi_str.is_empty() {
                    RepeatBounds::Bounds(lo, u32::MAX)
                } else {
                    let Ok(hi) = hi_str.parse::<u32>() else {
                        return RepeatBounds::Invalid;
                    };
                    // `a{3,2}` is an error in Rust regex, not a literal.
                    if hi < lo {
                        return RepeatBounds::Invalid;
                    }
                    RepeatBounds::Bounds(lo, hi)
                }
            }
            _ => RepeatBounds::Invalid,
        }
    }

    /// atom := '(' alternation ')' | '[' class ']' | '.' | escape | literal
    fn atom(&mut self) -> Option<Regexp<'ctx>> {
        match self.bump()? {
            '(' => {
                let inner = self.alternation()?;
                if self.bump() != Some(')') {
                    return None;
                }
                Some(inner)
            }
            '[' => self.char_class(),
            '.' => Some(sigma_except(self.ctx, '\n')),
            '\\' => self.escape(),
            '^' | '$' => None, // anchors inside the body are unsupported
            // Bare quantifiers without an operand are a parse error in Rust
            // regex ("repetition operator missing expression") — reject
            // rather than silently treating them as literal characters. `{`
            // is also a bare-quantifier error at atom position (only `}` is a
            // legal literal outside a class).
            '*' | '+' | '?' | '{' => None,
            c => Some(self.literal(&c.to_string())),
        }
    }

    /// Escape sequences outside a class: `\d` `\w` `\s` (+ uppercase
    /// complements) and escaped literal metacharacters.
    fn escape(&mut self) -> Option<Regexp<'ctx>> {
        let c = self.bump()?;
        match c {
            'd' | 'w' | 's' => Some(class_char_union(self.ctx, c)),
            'D' | 'W' | 'S' => {
                let all = sigma(self.ctx);
                let cls = class_char_union(self.ctx, c.to_ascii_lowercase());
                Some(Regexp::intersect(self.ctx, &[&all, &cls.complement()]))
            }
            'n' => Some(self.literal("\n")),
            'r' => Some(self.literal("\r")),
            't' => Some(self.literal("\t")),
            'f' => Some(self.literal("\u{c}")),
            'v' => Some(self.literal("\u{b}")),
            'a' => Some(self.literal("\u{7}")),
            'x' => {
                let hi = self.bump()?;
                let lo = self.bump()?;
                let code = u32::from_str_radix(&format!("{hi}{lo}"), 16).ok()?;
                let ch = char::from_u32(code)?;
                if ch == '\0' {
                    return None; // NUL cannot be embedded in a Z3 literal
                }
                Some(self.literal(&ch.to_string()))
            }
            // Escaped metacharacters become literals. `-` is escaped for
            // symmetry with the in-class case (`a\-b` is legal in Rust).
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'
            | '/' | '-' => Some(self.literal(&c.to_string())),
            _ => None,
        }
    }

    /// `'['` already consumed: parse items until `]`, honoring a leading `^`.
    fn char_class(&mut self) -> Option<Regexp<'ctx>> {
        let negated = self.peek() == Some('^');
        if negated {
            self.bump();
        }
        // Single-character members as explicit chars or range pairs.
        let mut singles: Vec<char> = Vec::new();
        let mut ranges: Vec<(char, char)> = Vec::new();
        let mut first = true;
        loop {
            let c = self.peek()?;
            if c == ']' && !first {
                self.bump();
                break;
            }
            first = false;
            self.bump();
            let lo = if c == '\\' {
                match self.escape_class_char()? {
                    ClassChar::Single(ch) => ch,
                    // A multi-char class escape (\d etc.) contributes its
                    // member ranges rather than a single item.
                    ClassChar::Ranges(rs) => {
                        ranges.extend(rs);
                        continue;
                    }
                }
            } else {
                c
            };
            // range: lo '-' hi, where '-' is only a separator when followed
            // by a non-']' char.
            if self.peek() == Some('-')
                && self
                    .chars
                    .get(self.pos + 1)
                    .copied()
                    .is_some_and(|n| n != ']')
            {
                self.bump(); // '-'
                let hc = self.bump()?;
                let hi = if hc == '\\' {
                    match self.escape_class_char()? {
                        ClassChar::Single(ch) => ch,
                        ClassChar::Ranges(_) => return None,
                    }
                } else {
                    hc
                };
                if hi < lo {
                    return None;
                }
                if lo > MAX_RANGE_CHAR || hi > MAX_RANGE_CHAR {
                    // re.range bounds above the ASCII plane collapse the
                    // range to an empty language — reject as unsupported.
                    return None;
                }
                ranges.push((lo, hi));
            } else {
                singles.push(lo);
            }
        }
        let mut members: Vec<Regexp> = Vec::new();
        for (lo, hi) in ranges {
            members.push(Regexp::range(self.ctx, &lo, &hi));
        }
        // Each member must stay a *single-char* language — a multi-char
        // literal like {"ab"} is a two-character sequence, not the class
        // {a,b}.
        for ch in singles {
            members.push(Regexp::literal(self.ctx, &ch.to_string()));
        }
        if members.is_empty() {
            return None; // `[]` is not a valid class
        }
        let union = if members.len() == 1 {
            members.into_iter().next().unwrap()
        } else {
            let refs: Vec<&Regexp> = members.iter().collect();
            Regexp::union(self.ctx, &refs)
        };
        if negated {
            let all = sigma(self.ctx);
            return Some(Regexp::intersect(self.ctx, &[&all, &union.complement()]));
        }
        Some(union)
    }

    /// Inside a class, `\d` etc. expand to ranges; escaped chars are singles.
    fn escape_class_char(&mut self) -> Option<ClassChar> {
        let c = self.bump()?;
        match c {
            'd' => Some(ClassChar::Ranges(vec![('0', '9')])),
            'w' => Some(ClassChar::Ranges(vec![
                ('0', '9'),
                ('A', 'Z'),
                ('_', '_'),
                ('a', 'z'),
            ])),
            's' => Some(ClassChar::Ranges(
                [' ', '\t', '\n', '\r', '\u{b}', '\u{c}']
                    .iter()
                    .map(|&ch| (ch, ch))
                    .collect(),
            )),
            'n' => Some(ClassChar::Single('\n')),
            'r' => Some(ClassChar::Single('\r')),
            't' => Some(ClassChar::Single('\t')),
            'f' => Some(ClassChar::Single('\u{c}')),
            'v' => Some(ClassChar::Single('\u{b}')),
            'a' => Some(ClassChar::Single('\u{7}')),
            'x' => {
                let hi = self.bump()?;
                let lo = self.bump()?;
                let code = u32::from_str_radix(&format!("{hi}{lo}"), 16).ok()?;
                let ch = char::from_u32(code)?;
                if ch == '\0' {
                    return None; // NUL cannot be embedded in a Z3 literal
                }
                Some(ClassChar::Single(ch))
            }
            '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '^' | '$' | '\\'
            | '/' | '-' => Some(ClassChar::Single(c)),
            _ => None,
        }
    }
}

enum RepeatBounds {
    /// `{n}` / `{n,m}` / `{n,}` parsed cleanly; `u32::MAX` = unbounded.
    Bounds(u32, u32),
    /// Not a valid `{…}` spec — Rust regex rejects bare/malformed braces
    /// (`a{`, `a{1,` — an unclosed `{` is a parse error, not a literal).
    Invalid,
}

enum ClassChar {
    Single(char),
    Ranges(Vec<(char, char)>),
}

/// Union Regexp for the `\d` / `\w` / `\s` shorthand classes.
fn class_char_union<'ctx>(ctx: &'ctx Context, which: char) -> Regexp<'ctx> {
    let ranges: &[(char, char)] = match which {
        'd' => &[('0', '9')],
        'w' => &[('0', '9'), ('A', 'Z'), ('_', '_'), ('a', 'z')],
        's' => &[
            (' ', ' '),
            ('\t', '\t'),
            ('\n', '\n'),
            ('\r', '\r'),
            ('\u{b}', '\u{b}'),
            ('\u{c}', '\u{c}'),
        ],
        _ => &[],
    };
    let members: Vec<Regexp> = ranges
        .iter()
        .map(|(lo, hi)| Regexp::range(ctx, lo, hi))
        .collect();
    if members.len() == 1 {
        return members.into_iter().next().unwrap();
    }
    let refs: Vec<&Regexp> = members.iter().collect();
    Regexp::union(ctx, &refs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use z3::ast::String as Z3String;
    use z3::{Config, SatResult, Solver};

    /// Whether Z3 decides `input ∈ compile_search(pattern)` — mirrors Rust
    /// `regex::Regex::is_match` substring semantics.
    fn z3_match(pattern: &str, input: &str) -> Option<bool> {
        let cfg = Config::new();
        let ctx = Context::new(&cfg);
        let re = compile_search(&ctx, pattern)?;
        let s = Z3String::from_str(&ctx, input).unwrap();
        let solver = Solver::new(&ctx);
        solver.assert(&s.regex_matches(&re));
        Some(matches!(solver.check(), SatResult::Sat))
    }

    fn rust_match(pattern: &str, input: &str) -> bool {
        regex::Regex::new(pattern).unwrap().is_match(input)
    }

    #[test]
    fn parity_corpus_matches_rust_regex() {
        let cases: &[(&str, &str)] = &[
            // effects.mm production pattern
            ("^/tmp/[a-z]+/.*", "/tmp/abc/x"),
            ("^/tmp/[a-z]+/.*", "/tmp/ABC/x"),
            ("^/tmp/[a-z]+/.*", "/var/x"),
            ("^/tmp/[a-z]+/.*", "/tmp/a/"),
            // substring semantics (unanchored)
            ("abc", "xxabcxx"),
            ("abc", "ab"),
            ("^abc", "abcx"),
            ("^abc", "xabc"),
            ("abc$", "xabc"),
            ("abc$", "abcx"),
            // repetition
            ("^a+b$", "aab"),
            ("^a+b$", "aa"),
            ("^a{2,3}$", "aa"),
            ("^a{2,3}$", "a"),
            ("^a{2,3}$", "aaaa"),
            ("a{2,}", "xaaax"),
            ("colou?r", "color"),
            ("colou?r", "colour"),
            ("colou?r", "colouur"),
            // alternation / groups
            ("foo|bar", "xfoo"),
            ("foo|bar", "ybar"),
            ("foo|bar", "baz"),
            ("(ab)*c", "c"),
            ("(ab)*c", "abababc"),
            ("(ab)*c", "ab"),
            ("(a|b)+c", "ababc"),
            ("(a|b)+c", "xyz"),
            // escapes and classes
            ("^\\d{3}$", "123"),
            ("^\\d{3}$", "12"),
            ("^\\d{3}$", "12a"),
            ("\\w+", "abc_"),
            ("\\w+", "!!!"),
            ("^[^0-9]+$", "abc"),
            ("[^0-9]+", "a1"),
            ("[^0-9]+", "123"),
            ("\\x41+", "AAA"),
            ("\\x41+", "BBB"),
            // '.' excludes '\n' — the critical parity case
            ("f.o", "fzo"),
            ("f.o", "f\no"),
            // anchors
            ("^$", ""),
            ("^$", "a"),
            ("", "anything"),
            ("^a+$", "aaa"),
            // lazy counted repetition: `a{2,3}?` must NOT become optional
            ("a{2,3}?", "aa"),
            ("a{2,3}?", "b"),
            ("a{2,3}?", "xax"),
            // escaped trailing `$` is a literal, not an anchor
            ("a\\$", "a$"),
            ("a\\$", "a"),
            ("a\\$", "xa$y"),
            // `a\\$` = literal "a\" at end (escaped backslash + real anchor)
            ("a\\\\$", "a\\"),
            ("a\\\\$", "a\\x"),
            // stacked quantifiers compose like Rust's
            ("a**", ""),
            ("a**", "xyz"),
            // control-char escapes and in-class escapes
            ("\\f", "\u{c}"),
            ("\\v", "\u{b}"),
            ("\\a", "\u{7}"),
            ("a\\fb", "a\u{c}b"),
            ("[\\x41]", "A"),
            ("[\\x41]", "B"),
            ("[\\-]", "-"),
            ("[\\]]", "]"),
            ("\\.", "."),
            ("\\.", "x"),
        ];
        for (pattern, input) in cases {
            assert_eq!(
                z3_match(pattern, input),
                Some(rust_match(pattern, input)),
                "parity mismatch: pattern {pattern:?} vs input {input:?}"
            );
        }
    }

    #[test]
    fn unsupported_constructs_return_none() {
        for pattern in [
            "\\bword",
            "a(?=b)",
            "a(?!b)",
            "(x)\\1",
            "a^b",        // interior anchor
            "a$b",        // interior anchor
            "a{200,300}", // above MAX_LOOP_BOUND
            "a{3,2}",     // inverted bounds
            "a{",         // bare `{` is a parse error in Rust regex
            "a{1,",       // unclosed count — ditto
            "a{ 2}",      // whitespace bounds (Rust accepts; keep fail-closed)
            "(",          // unbalanced
            "[",          // unbalanced class
            "a\\Q",       // unknown escape
        ] {
            assert!(!supported(pattern), "expected unsupported: {pattern:?}");
        }
    }

    #[test]
    fn supported_matches_compile_search() {
        for pattern in [
            "^/tmp/[a-z]+/.*",
            "\\w+\\s\\d+",
            "^(a|b){1,2}c$",
            "[\\d_]x?",
        ] {
            assert!(supported(pattern), "expected supported: {pattern:?}");
        }
    }
}
