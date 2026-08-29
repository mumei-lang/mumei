// Trust boundary fixture for the proof-aware runtime monitor emitter.
// `trusted` means the contract is assumed rather than proven, so the monitor
// must observe it at runtime.
// expected: PASS

trusted atom read_sensor(channel: i64) -> i64 {
    requires: channel >= 0;
    ensures: result >= 0;
    body: {
        channel
    }
}
