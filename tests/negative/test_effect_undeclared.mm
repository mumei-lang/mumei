// =============================================================
// Negative Test: Undeclared effect usage — should FAIL
// =============================================================
// Atom declares [Log] but uses Network.send which is not declared.

effect Log;
effect Network;

atom log_only(msg: i64)
effects: [Log];
requires: msg >= 0;
ensures: result >= 0;
body: {
    perform Log.info(msg);
    perform Network.send(msg);
    msg
};
