enum ArrayPayload { Empty, Values(values: [u32]) }

atom enum_array_payload_domain_is_checked(n: i64) -> i64
    requires: true;
    ensures: result >= 0;
    body: {
        let value = ArrayPayload::Values([n]);
        match value {
            ArrayPayload::Empty => 0,
            ArrayPayload::Values(values) => values[0]
        }
    }
