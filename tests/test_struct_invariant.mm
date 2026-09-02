// =============================================================
// Cross-field struct invariants (positive fixture)
// =============================================================
// `invariant: <expr>` relates several fields of a struct. It is
//   * assumed for every struct-typed parameter,
//   * checked at every struct literal, and
//   * imposed on `result` of every atom returning the struct
//     (an implicit `ensures`), even when the body returns a parameter
//     or a call result without building a new literal.
// Everything stays in QF_LIA: no quantifiers, linear i64 arithmetic only.

struct Scheduler {
    active_tasks: i64 where v >= 0,
    max_tasks: i64 where v > 0,
    invariant: self.active_tasks <= self.max_tasks
}

atom scheduler_new(max_tasks: i64) -> Scheduler
requires: max_tasks > 0;
ensures: result.active_tasks == 0 && result.max_tasks == max_tasks;
body: Scheduler { active_tasks: 0, max_tasks: max_tasks };

// Valid transition: the invariant is what lets the guard `< max_tasks`
// prove the new `active_tasks` is still within bounds.
atom scheduler_spawn(s: Scheduler) -> Scheduler
requires: s.active_tasks < s.max_tasks;
ensures: result.active_tasks == s.active_tasks + 1 && result.max_tasks == s.max_tasks;
body: Scheduler { active_tasks: s.active_tasks + 1, max_tasks: s.max_tasks };

atom scheduler_finish(s: Scheduler) -> Scheduler
requires: s.active_tasks > 0;
ensures: result.active_tasks == s.active_tasks - 1;
body: Scheduler { active_tasks: s.active_tasks - 1, max_tasks: s.max_tasks };

// Direct return of the parameter: no literal is built, yet the implicit
// postcondition on `result` is discharged from the assumed invariant on `s`.
atom scheduler_identity(s: Scheduler) -> Scheduler
requires: true;
ensures: result.active_tasks == s.active_tasks;
body: s;

// Alias through `let` and a conditional over two struct values.
atom scheduler_try_spawn(s: Scheduler) -> Scheduler
requires: true;
ensures: result.active_tasks <= result.max_tasks;
body: {
    let next = if s.active_tasks < s.max_tasks {
        Scheduler { active_tasks: s.active_tasks + 1, max_tasks: s.max_tasks }
    } else {
        Scheduler { active_tasks: s.active_tasks, max_tasks: s.max_tasks }
    };
    next
};

// Call result: the callee's invariant is assumed at the call site.
atom scheduler_fresh(max_tasks: i64) -> Scheduler
requires: max_tasks > 0;
ensures: result.active_tasks <= result.max_tasks;
body: scheduler_new(max_tasks);

// The invariant is available on a parameter even in a scalar-returning atom.
atom scheduler_free_slots(s: Scheduler) -> i64
requires: true;
ensures: result >= 0;
body: s.max_tasks - s.active_tasks;

// Higher-order forwarding: `call(atom_ref(...))` of a struct-returning atom
// yields a struct value whose invariant is assumed like an ordinary call.
atom scheduler_fresh_ref(max_tasks: i64) -> Scheduler
requires: max_tasks > 0;
ensures: result.active_tasks <= result.max_tasks;
body: call(atom_ref(scheduler_new), max_tasks);

// Rebinding an alias replaces its field projections; the stale ones from
// the first struct must not leak into the result.
atom scheduler_rebind(s: Scheduler) -> Scheduler
requires: s.active_tasks > 0;
ensures: result.active_tasks == s.active_tasks - 1;
body: {
    let cur = Scheduler { active_tasks: s.active_tasks, max_tasks: s.max_tasks };
    let cur = Scheduler { active_tasks: s.active_tasks - 1, max_tasks: s.max_tasks };
    cur
};
