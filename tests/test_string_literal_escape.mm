// Regression: string literals must round-trip through the atom-body
// re-lex. The lexer stores decoded content (`\n` → real newline,
// `\"` → real quote); before the fix, collect_brace_body / token Display /
// token_to_source re-serialized the raw content, so `"a\"b"` closed the
// literal early (unresolved identifiers) and `"a\nb"` (backslash-n)
// silently became `"a<newline>b"`.
atom str_escape_quote() -> Str
  requires: true;
  ensures: result == "a\"b";
  body: {
    let s = "a\"b"
    s
  };

// Backslash-n (2 chars) and a real newline escape (\n, 1 char) are
// different strings — the round-trip must keep them distinct.
atom str_backslash_n_distinct() -> i64
  requires: true;
  ensures: result == 1;
  body: {
    let a = "x\\ny"
    let b = "x\ny"
    if a != b { 1 } else { 0 }
  };

atom str_tab_round_trip() -> Str
  requires: true;
  ensures: result == "c\td";
  body: {
    "c\td"
  };
