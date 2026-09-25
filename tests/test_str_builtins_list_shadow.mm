import "std/list" as list;

atom unqualified_list_is_empty() -> i64
    requires: true;
    ensures: result == 1;
    body: {
        let checked = is_empty(0);
        if checked >= 0 { 1 } else { 1 }
    }

atom qualified_list_is_empty() -> i64
    requires: true;
    ensures: result == 1;
    body: {
        let checked = list.is_empty(0);
        if checked >= 0 { 1 } else { 1 }
    }
