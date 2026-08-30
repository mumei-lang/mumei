// Server role of the OrderChannel protocol (see `order_protocol.mm`).
// The server answers a request that the client has already sent.
// expected: PASS

import "order_protocol" as protocol;

atom server_send_response(order_id: i64)
    effects: [OrderChannel];
    effect_pre: { OrderChannel: RequestSent };
    effect_post: { OrderChannel: ResponseSent };
    requires: order_id > 0;
    ensures: result == order_id;
    body: {
        perform OrderChannel.send_response;
        order_id
    }
