enum A {
    Stop,
    More(value: B),
}

enum B {
    Stop,
    More(value: A),
}

atom unsupported_mutual() -> A
    requires: true;
    ensures: true;
    body: A::More(B::More(A::Stop))
