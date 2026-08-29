// Client role of the oversized BulkChannel protocol (see `bulk_protocol.mm`).
// expected: PASS

import "bulk_protocol" as protocol;

atom bulk_client_send(item: i64)
    effects: [BulkChannel];
    effect_pre: { BulkChannel: S0 };
    effect_post: { BulkChannel: S1 };
    requires: item > 0;
    ensures: result == item;
    body: {
        perform BulkChannel.step0;
        item
    }
