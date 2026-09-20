// Negative: a `Qual::V` arm whose qualifier names a different enum than
// the match target's declared type must fail closed — `match e { Other::Nil }`
// on `e: Mine` is a cross-enum typo, not a match on Mine's `Nil`.
enum Mine { Nil, Yes(i64) }
enum Other { Nil, Yes(i64) }

atom t(e: Mine) -> i64
  requires: true;
  ensures: result >= 0;
  body: {
    match e {
      Other::Yes(v) => v
      Other::Nil => 9
    }
  }
