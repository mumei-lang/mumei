// Shared protocol declaration for the split-file session-type benchmarks.
// The client role lives in `order_client.mm`, the server role in
// `order_server.mm`; both import this effect so the protocol graph is shared.
// expected: PASS

effect OrderChannel
    states: [Idle, RequestSent, ResponseSent, Settled];
    initial: Idle;
    transition send_request: Idle -> RequestSent;
    transition send_response: RequestSent -> ResponseSent;
    transition settle: ResponseSent -> Settled;
