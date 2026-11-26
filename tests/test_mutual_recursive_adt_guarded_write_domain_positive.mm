enum SelfList {
    Nil,
    Cons(value: u32, tail: SelfList),
}

atom guarded_self_recursive_write(l: SelfList, value: i64) -> bool
    requires: value == 9;
    ensures: result == true;
    body: match l {
        SelfList::Nil => true,
        SelfList::Cons(_, _) => {
            l.value = value;
            true
        }
    }

enum Tree {
    Leaf(value: u32),
    Node(forest: Forest),
}

enum Forest {
    Nil,
    Cons(tree: Tree, rest: Forest, value: u32),
}

atom guarded_mutual_recursive_write(forest: Forest, value: i64) -> bool
    requires: value == 9;
    ensures: result == true;
    body: match forest {
        Forest::Nil => true,
        Forest::Cons(_, _, _) => {
            forest.value = value;
            true
        }
    }
