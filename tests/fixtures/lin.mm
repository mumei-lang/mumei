struct Token { id: i64 }

atom use_twice(t: Token) -> i64
    body: {
        let a = t;
        let b = t;
        a.id + b.id
    };
