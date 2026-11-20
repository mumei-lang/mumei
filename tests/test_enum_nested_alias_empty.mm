// A payload field whose type is an alias to an empty struct: the alias
// flattens to zero leaves, so rebuilding the bound handle must resolve the
// alias before deciding between an inner handle and a scalar slice —
// indexing an empty slice panics.
struct Empty {
}
type AliasE = Empty;
struct Wrap {
    e: AliasE,
    x: i64
}
enum DE {
    W(w: Wrap),
    N
}
atom probe(e: DE) -> i64
    requires: true;
    ensures: result == 1;
    body: {
        match e {
            DE::W(m) => {
                let z = m.e;
                1
            },
            _ => 1
        }
    }
