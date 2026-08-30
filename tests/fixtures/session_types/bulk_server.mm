// Server role of the oversized BulkChannel protocol (see `bulk_protocol.mm`).
// expected: PASS

import "bulk_protocol" as protocol;

atom bulk_server_recv(item: i64)
    effects: [BulkChannel];
    effect_pre: { BulkChannel: S1 };
    effect_post: { BulkChannel: S2 };
    requires: item > 0;
    ensures: result == item;
    body: {
        perform BulkChannel.step1;
        item
    }
