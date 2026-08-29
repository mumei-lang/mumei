// `effect_pre` assumes a caller-provided protocol state, so the monitor
// observes it at runtime through the host's effect-state probe.
// expected: PASS

effect SensorChannel
    states: [Idle, Reading];
    initial: Idle;
    transition begin: Idle -> Reading;

atom sensor_begin(channel: i64) -> i64
    effects: [SensorChannel];
    effect_pre: { SensorChannel: Idle };
    effect_post: { SensorChannel: Reading };
    requires: channel >= 0;
    ensures: result >= 0;
    body: {
        perform SensorChannel.begin;
        channel
    }
