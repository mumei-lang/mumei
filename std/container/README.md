# std/container

Verified container primitives for the Mumei standard library.

## SortedMap

`sorted_map.mm` adds a bounded sorted key-value map model backed by parallel
`keys` and `values` arrays. Its contracts mirror `bounded_array.mm` length and
capacity bookkeeping, then add a sorted-key invariant:

```mumei
forall(i, 0, n - 1, keys[i] <= keys[i + 1])
```

The insertion atom appends a key that is at least every existing key. Its body is
**fully Z3-verified** (no `trusted`): the append-at-end `forall + store`
postcondition is discharged by lowering the bounded sortedness quantifier into an
explicit index range (`0..map_len`); on `unknown` the atom escalates to
mumei-lean rather than being trusted. `sorted_map_insert_position` and
`sorted_map_get` expose binary-search result witnesses with verified bounds for
caller-side integration.
The module also exposes length and remaining-capacity helpers for composing
sorted-map updates with other bounded containers.
