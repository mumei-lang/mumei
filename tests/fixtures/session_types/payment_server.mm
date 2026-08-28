// Server role of the PaymentChannel protocol (see `payment_protocol.mm`).
// The server always answers with `respond`, handing control back to a client
// that only retries — the two roles wait on each other forever.
// expected: PASS

import "payment_protocol" as protocol;

atom payment_server_respond(payment_id: i64)
    effects: [PaymentChannel];
    effect_pre: { PaymentChannel: ServerWait };
    effect_post: { PaymentChannel: ClientWait };
    requires: payment_id > 0;
    ensures: result == payment_id;
    body: {
        perform PaymentChannel.respond;
        payment_id
    }
