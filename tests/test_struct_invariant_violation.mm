// =============================================================
// Cross-field struct invariants (negative fixture)
// =============================================================
// `scheduler_spawn_unchecked` has no guard, so `active_tasks + 1` may exceed
// `max_tasks`: the implicit postcondition `Invariant(result)` is refuted and
// verification must fail even though the explicit `ensures` holds.

struct Scheduler {
    active_tasks: i64 where v >= 0,
    max_tasks: i64 where v > 0,
    invariant: self.active_tasks <= self.max_tasks
}

atom scheduler_spawn_unchecked(s: Scheduler) -> Scheduler
requires: true;
ensures: result.active_tasks == s.active_tasks + 1;
body: Scheduler { active_tasks: s.active_tasks + 1, max_tasks: s.max_tasks };
