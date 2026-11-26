struct Point {
    x: i64,
}

enum GroupA {
    Stop,
    Next(value: GroupB),
}

enum GroupB {
    Stop,
    Next(value: GroupC),
}

enum GroupC {
    Stop,
    Next(value: GroupD),
}

enum GroupD {
    Stop,
    Next(value: GroupE),
}

enum GroupE {
    Stop,
    Next(value: GroupA),
}

atom group_larger_than_four_is_closed() -> GroupA
    requires: true;
    ensures: true;
    body: GroupA::Next(GroupB::Stop)

enum UninhabitedA {
    Only(value: UninhabitedB),
}

enum UninhabitedB {
    Only(value: UninhabitedA),
}

atom uninhabited_group_is_closed(value: UninhabitedA) -> UninhabitedA
    requires: true;
    ensures: true;
    body: UninhabitedA::Only(UninhabitedB::Only(value))

struct StructCyclePayload {
    value: StructCycle,
}

enum StructCycle {
    Empty,
    More(value: StructCyclePayload),
}

atom recursion_through_struct_is_closed() -> StructCycle
    requires: true;
    ensures: true;
    body: StructCycle::More(StructCyclePayload { value: StructCycle::Empty })

enum ArrayCycle {
    Empty,
    More(values: [ArrayCycle]),
}

atom recursion_through_array_is_closed(values: [ArrayCycle]) -> ArrayCycle
    requires: true;
    ensures: true;
    body: ArrayCycle::More(values)

enum GenericA<T> {
    Stop,
    Next(value: GenericB<T>),
}

enum GenericB<T> {
    Stop,
    Next(value: GenericA<T>),
}

atom generic_recursive_group_is_closed() -> GenericA<i64>
    requires: true;
    ensures: true;
    body: GenericA::Next(GenericB::Stop)

enum StructArrayA {
    Stop,
    Next(value: StructArrayB),
    Points(values: [Point]),
}

enum StructArrayB {
    Stop,
    Next(value: StructArrayA),
}

atom struct_array_payload_group_is_closed(values: [Point]) -> StructArrayA
    requires: true;
    ensures: true;
    body: StructArrayA::Points(values)

enum RecursiveList {
    Nil,
    Cons(value: i64, rest: RecursiveList),
}

enum NonGroupWrapper {
    Wrap(value: RecursiveList),
}

atom non_group_recursive_enum_payload_stays_closed() -> NonGroupWrapper
    requires: true;
    ensures: true;
    body: NonGroupWrapper::Wrap(RecursiveList::Nil)
