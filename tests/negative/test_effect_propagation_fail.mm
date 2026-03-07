// =============================================================
// Negative Test: Effect propagation failure — should FAIL
// =============================================================
// Atom A (effects: [Log]) calls Atom B (effects: [Log, Network]).
// A's effects do not include Network, so propagation fails.

effect Log;
effect Network;

atom network_logger(msg: i64)
effects: [Log, Network];
requires: msg >= 0;
ensures: result >= 0;
body: {
    perform Log.info(msg);
    perform Network.send(msg);
    msg
};

atom log_only_caller(x: i64)
effects: [Log];
requires: x >= 0;
ensures: result >= 0;
body: {
    network_logger(x)
};
