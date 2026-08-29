// Extern-only fixture for the proof-aware runtime monitor emitter.
// An FFI declaration has no body to verify, so its contracts are assumptions
// the monitor must observe at runtime.
// expected: PASS

extern "C" {
    fn read_channel(channel: i64) -> i64
        requires: channel >= 0;
        ensures: result >= 0;
}
