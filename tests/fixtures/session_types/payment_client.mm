// Client role of the PaymentChannel protocol (see `payment_protocol.mm`).
// The client retries instead of settling, so the protocol can only cycle
// between `ServerWait` and `ClientWait`: no role ever performs `finish`.
// expected: PASS

import "payment_protocol" as protocol;

atom payment_client_request(payment_id: i64)
    effects: [PaymentChannel];
    effect_pre: { PaymentChannel: Idle };
    effect_post: { PaymentChannel: ServerWait };
    requires: payment_id > 0;
    ensures: result == payment_id;
    body: {
        perform PaymentChannel.request;
        payment_id
    }

atom payment_client_retry(payment_id: i64)
    effects: [PaymentChannel];
    effect_pre: { PaymentChannel: ClientWait };
    effect_post: { PaymentChannel: ServerWait };
    requires: payment_id > 0;
    ensures: result == payment_id;
    body: {
        perform PaymentChannel.retry;
        payment_id
    }
