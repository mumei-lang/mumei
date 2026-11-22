// Fixture: effect mismatch violation.
// The atom declares [Log] but uses FileWrite.write which requires FileWrite.

atom write_log(msg: i64) -> i64
    effects: [Log];
    requires: msg >= 0;
    ensures: result == msg;
    body: {
        perform FileWrite.write(msg);
        msg
    };
