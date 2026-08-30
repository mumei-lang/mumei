// Client role of the OrderChannel protocol (see `order_protocol.mm`).
// The client sends the request and settles once the server has responded.
// expected: PASS

import "order_protocol" as protocol;

atom client_send_request(order_id: i64)
    effects: [OrderChannel];
    effect_pre: { OrderChannel: Idle };
    effect_post: { OrderChannel: RequestSent };
    requires: order_id > 0;
    ensures: result == order_id;
    body: {
        perform OrderChannel.send_request;
        order_id
    }

atom client_settle(order_id: i64)
    effects: [OrderChannel];
    effect_pre: { OrderChannel: ResponseSent };
    effect_post: { OrderChannel: Settled };
    requires: order_id > 0;
    ensures: result == order_id;
    body: {
        perform OrderChannel.settle;
        order_id
    }
