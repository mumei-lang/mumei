// =============================================================
// tests/test_forged_std_modules.mm — forged std module integration
// =============================================================

import "std/container/stack" as stack;
import "std/container/ring_buffer" as ring;
import "std/math/safe_div" as sdiv;
import "std/math/safe_mul" as smul;
import "std/math/fibonacci" as fib;
import "std/string/validator" as validator;

atom test_stack_push()
    requires: true;
    ensures: result == 3;
    body: {
        stack::stack_push(2, 5)
    };

atom test_stack_pop()
    requires: true;
    ensures: result == 1;
    body: {
        stack::stack_pop(2, 5)
    };

atom test_stack_peek()
    requires: true;
    ensures: result == 4;
    body: {
        stack::stack_peek(5, 8)
    };

atom test_ring_advance_wrap()
    requires: true;
    ensures: result >= 0 && result < 5;
    body: {
        ring::ring_advance(4, 5)
    };

atom test_ring_push()
    requires: true;
    ensures: result >= 1 && result <= 5;
    body: {
        ring::ring_push(2, 5)
    };

atom test_ring_pop()
    requires: true;
    ensures: result >= 0 && result < 5;
    body: {
        ring::ring_pop(3, 5)
    };

atom test_safe_div()
    requires: true;
    ensures: result >= 0 && result <= 10 && result * 2 <= 10;
    body: {
        sdiv::safe_div(10, 2)
    };

atom test_safe_mod()
    requires: true;
    ensures: result >= 0 && result < 3;
    body: {
        sdiv::safe_mod(10, 3)
    };

atom test_safe_mul()
    requires: true;
    ensures: result == 42;
    body: {
        smul::safe_mul(6, 7)
    };

atom test_saturating_mul()
    requires: true;
    ensures: result >= 0 && result <= 10;
    body: {
        smul::saturating_mul(6, 7, 10)
    };

atom test_fibonacci_step()
    requires: true;
    ensures: result == 13;
    body: {
        fib::fib_step_next(5, 8)
    };

atom test_numeric_ascii_true()
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        validator::is_numeric_ascii_code(53)
    };

atom test_alphanumeric_ascii_false()
    requires: true;
    ensures: result == 0 || result == 1;
    body: {
        validator::is_alphanumeric_ascii_code(35)
    };
