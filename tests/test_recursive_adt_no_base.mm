enum NoBase {
    Again(next: Self),
}

atom int_tag_path(value: NoBase) -> i64
    requires: true;
    ensures: result == 0;
    body: {
        match value {
            NoBase::Again(_) => 0
        }
    }
