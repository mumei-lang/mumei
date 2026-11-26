enum A {
    More(value: B),
}

enum B {
    More(value: A),
}

atom unsupported_mutual(value: A) -> A
    requires: true;
    ensures: true;
    body: A::More(B::More(value))
