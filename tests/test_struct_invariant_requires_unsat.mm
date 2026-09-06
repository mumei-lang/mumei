// A `requires` that contradicts the struct invariant of a parameter must be
// reported as an unsatisfiable precondition, not proven vacuously.
struct Scheduler {
    active_tasks: i64 where v >= 0,
    max_tasks: i64 where v > 0,
    invariant: self.active_tasks <= self.max_tasks
}

atom scheduler_impossible(s: Scheduler) -> i64
requires: s.active_tasks > s.max_tasks;
ensures: result == 0 && result == 1;
body: 0;
