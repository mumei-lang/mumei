// `"a\nb"` (backslash + n) must NOT verify as equal to `"a<newline>b"` —
// the pre-fix re-serialization collapsed both to the same decoded value.
atom str_escape_collapse() -> Str
  requires: true;
  ensures: result == "a\nb";
  body: {
    "a\\nb"
  };
