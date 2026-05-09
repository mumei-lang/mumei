// Negative: forged std modules reject invalid preconditions.
import "std/container/stack" as stack;
import "std/math/safe_div" as sdiv;

atom test_stack_push_over_capacity()
    requires: true;
    ensures: result >= 0;
    body: {
        stack::stack_push(5, 5)
    };

atom test_safe_div_zero_divisor()
    requires: true;
    ensures: result >= 0;
    body: {
        sdiv::safe_div(10, 0)
    };
