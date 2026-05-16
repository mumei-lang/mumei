# Module: iterable
# Common interface for Vector/List/BoundedArray. Connects Sequential trait with iterator trait.
import "std/prelude" as prelude;
import "std/alloc" as alloc;

atom iterable_placeholder(x: i64)
    effects: []
    requires: true;
    ensures: result >= 0 || result < 0;
    body: {
        return 0;
    }

// Note: This is a trait placeholder atom that:
// - Accepts any i64 input (no side effects from input)
// - Returns any i64 value (the postcondition allows all i64)
// - In a real implementation, this would typically call the iterator protocol methods
// - Here we use return 0 as a placeholder to satisfy the contract
