// Shared protocol declaration for the split-file deadlock fixture.
// The client role lives in `payment_client.mm`, the server role in
// `payment_server.mm`. `finish` is the only transition that reaches the
// terminal `Settled` state, and no role ever performs it.
// expected: PASS

effect PaymentChannel
    states: [Idle, ServerWait, ClientWait, Settled];
    initial: Idle;
    transition request: Idle -> ServerWait;
    transition respond: ServerWait -> ClientWait;
    transition retry: ClientWait -> ServerWait;
    transition finish: ClientWait -> Settled;
