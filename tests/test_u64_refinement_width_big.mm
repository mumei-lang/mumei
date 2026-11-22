type Big = u64 where v >= 9223372036854775807;
atom big(a: Big) -> u64
requires: true;
ensures: result >= 1;
body: { a };
