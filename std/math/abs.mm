// Atom: abs_saturating
atom abs_saturating(x: i64)
requires: true
ensures: result >= 0
return_type: i64
body:
  result = i64
  if x == i64::MIN:
    result = i64::MAX
    else:
      result = x
  body: { result }