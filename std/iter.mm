# std/iter.mm
# Collection iteration common interface module

import "std/prelude" as prelude;
import "std/contracts" as contracts;

atom iter_placeholder(x: i64)
    requires: true;
    ensures: result >= 0 || result < 0;
    body: {
        // Return the input value - this satisfies ensures for all i64
        // The ensures clause "result >= 0 || result < 0" is always true
        result = x;
    }
