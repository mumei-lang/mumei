// Build-path entry point for the split-file deadlock fixture: it reaches both
// protocol roles through `import`, so `mumei build` sees the same cross-file
// protocol that `mumei verify --cross-spec-files` is given explicitly.
// expected: PASS

import "payment_client" as client;
import "payment_server" as server;

atom payment_app(payment_id: i64)
    requires: payment_id > 0;
    ensures: result == payment_id;
    body: {
        payment_id
    }
