// =============================================================
// Test: Concurrency Runtime — task / task_group / chan codegen
// =============================================================
// Plan 21 — verifies that the LLVM IR emitted for `task`,
// `task_group(all)`, `send`, and `recv` actually goes through the
// pthread + runtime helpers (rather than being inlined into the
// caller as the pre-Plan-21 stub did).
//
// The verifier still treats these atoms as ordinary functions, so
// only the *result* of joining the spawned tasks contributes to
// `ensures`. The IR-level concurrency is exercised indirectly by
// `mumei build tests/test_concurrency_runtime.mm` — the resulting
// `.ll` should contain `pthread_create`, `pthread_join`,
// `__mumei_chan_send`, and `__mumei_chan_recv` calls.
//
// See `runtime/mumei_runtime.c` for the channel-side runtime.

// --- Single task: result of `task { … }` is the body's value ---
atom spawn_single_task(n: i64)
requires: n >= 0;
ensures: result == n;
body: {
    task { n }
}

// --- task_group:all — both children join, last result is returned ---
atom spawn_task_group_all(a: i64, b: i64)
requires: a >= 0 && b >= 0;
ensures: result == b;
body: {
    task_group:all {
        task { a };
        task { b }
    }
}

// --- Channel send / recv (codegen smoke test) ---
//
// `ch` is a channel handle (i64). The runtime's
// `__mumei_chan_send` blocks if a value is already pending, and
// `__mumei_chan_recv` blocks until one arrives. The verifier does
// NOT yet model the channel's transfer semantics — so the
// postcondition only asserts that `recv` returns *some* i64. The
// real check this atom exercises is on the codegen side: the
// emitted `.ll` must contain `__mumei_chan_send` and
// `__mumei_chan_recv` external calls (see
// `mumei-emit-llvm/src/codegen.rs`).
atom chan_send_recv(ch: i64)
requires: ch >= 0;
ensures: true;
body: {
    send(ch, 42);
    recv(ch)
}

// --- Concurrent rendezvous inside `task_group:all` ---
//
// This atom is the runtime correctness test for `task_group:all`:
// child A blocks in `recv(ch)` until child B calls `send(ch, …)`.
// Under a sequential `spawn-join-spawn-join` lowering this would
// deadlock — the parent would join A before B is ever spawned —
// so reaching the join+return is itself the proof that the IR
// pipeline emits all `pthread_create`s before any `pthread_join`.
//
// The verifier only sees the post-join scalar result (`a`), so the
// `ensures` asserts the value of the receiving task's body. Real
// ordering is exercised by linking the emitted `.ll` against
// `runtime/mumei_runtime.c` and observing a normal exit.
atom chan_rendezvous_in_group(ch: i64)
requires: ch >= 0;
ensures: result == 42;
body: {
    task_group:all {
        task { recv(ch) };
        task { send(ch, 42); 42 }
    }
}

// --- P25: polymorphic `chan<T>` payload marshalling ---
//
// The runtime channel slot stays `int64_t`, so a non-i64 payload is
// bit-preserved into it by codegen: `chan<f64>` emits a `bitcast`
// pair around the send/recv calls, and `chan<Str>` a
// `ptrtoint` / `inttoptr` pair. Before P25 the send arm collapsed
// every non-int value to `i64 0` and `recv` always yielded a raw
// i64, so both payloads were lost.
trusted atom chan_f64_round_trip(ch: chan<f64>, x: f64) -> f64
requires: true;
ensures: true;
body: {
    send(ch, x);
    recv(ch)
}

trusted atom chan_str_round_trip(ch: chan<Str>, s: Str) -> Str
requires: true;
ensures: true;
body: {
    send(ch, s);
    recv(ch)
}

// --- P25: array element storage captured by a task body ---
//
// `arr` is a fat pointer `(len, data)`. Both halves are stored into
// the pthread args struct and reloaded inside the wrapper, so the
// task body indexes the *parent's* element storage (bounds check
// included). Before P25 the task body was compiled with an empty
// array map, so `arr[i]` inside a task had no backing storage.
trusted atom task_sums_captured_array(arr: [i64]) -> i64
requires: true;
ensures: true;
body: {
    task { arr[0] + arr[1] }
}
