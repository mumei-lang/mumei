enum Recursive {
    R,
    Link(next: Recursive)
}

enum ArrayPayload {
    R,
    Values(values: [i64])
}

struct RecursiveField {
    value: Recursive
}

struct ArrayPayloadField {
    value: ArrayPayload
}

struct NestedArrayField {
    values: [[i64]]
}

struct PayloadStruct {
    value: i64
}

enum StructArrayPayload {
    R,
    Values(values: [PayloadStruct])
}

enum Generic<T> {
    Some(value: T),
    None
}

struct GenericEnumField {
    value: Generic
}

struct StructArrayPayloadField {
    value: StructArrayPayload
}

atom recursive_enum_leaf(values: [RecursiveField]) -> bool
requires: len(values) > 0;
ensures: true;
body: true;

atom array_payload_enum_leaf(values: [ArrayPayloadField]) -> bool
requires: len(values) > 0;
ensures: true;
body: true;

atom nested_array_leaf(values: [NestedArrayField]) -> bool
requires: len(values) > 0;
ensures: true;
body: true;

atom struct_array_enum_payload_leaf(values: [StructArrayPayloadField]) -> bool
requires: len(values) > 0;
ensures: true;
body: true;

atom generic_enum_leaf(values: [GenericEnumField]) -> bool
requires: len(values) > 0;
ensures: true;
body: true;
