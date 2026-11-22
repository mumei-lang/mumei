type Small = u64 where v <= 10;
struct Box { value: Small }
atom read_value(b: Box) -> u64
requires: true;
ensures: result <= 10;
body: { b.value };
