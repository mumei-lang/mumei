effect Network;

// `T: Comparable` on `apply` requires an impl for the concrete type arg —
// `apply<i64, Network>` only monomorphizes when `i64: Comparable` holds.
trait Comparable {
    fn leq(a: Self, b: Self) -> bool;
}

impl Comparable for i64 {
    fn leq(a: i64, b: i64) -> bool {
        a <= b
    }
}

atom apply<T: Comparable, E: Effect>(
    x: i64,
    f: atom_ref(i64) -> i64 with E
)
    effects: [E];
    requires: x >= 0;
    ensures: true;
    body: call(f, x);

atom net_fn(x: i64)
    effects: [Network];
    requires: x >= 0;
    ensures: true;
    body: { perform Network.get(x); x };

atom main()
    effects: [Network];
    requires: true;
    ensures: true;
    body: apply<i64, Network>(42, atom_ref(net_fn));
