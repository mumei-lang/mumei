struct L25 {
    value: i64
}

struct L24 { next: L25 }
struct L23 { next: L24 }
struct L22 { next: L23 }
struct L21 { next: L22 }
struct L20 { next: L21 }
struct L19 { next: L20 }
struct L18 { next: L19 }
struct L17 { next: L18 }
struct L16 { next: L17 }
struct L15 { next: L16 }
struct L14 { next: L15 }
struct L13 { next: L14 }
struct L12 { next: L13 }
struct L11 { next: L12 }
struct L10 { next: L11 }
struct L09 { next: L10 }
struct L08 { next: L09 }
struct L07 { next: L08 }
struct L06 { next: L07 }
struct L05 { next: L06 }
struct L04 { next: L05 }
struct L03 { next: L04 }
struct L02 { next: L03 }
struct L01 { next: L02 }
struct L00 { next: L01 }

enum Chain {
    Wrap(root: L00),
    Stop
}

atom deep_match_bound_field_chain(e: Chain) -> i64
requires: match e {
    Chain::Wrap(_) => true,
    _ => false
};
ensures: result == result;
body: match e {
    Chain::Wrap(root) => root.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.next.value,
    _ => 0
};
