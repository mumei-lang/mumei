// Intentionally invalid: demonstrates compile-time rejection
effect DataPipeline
    states: [Raw, Anonymized];
    initial: Raw;
    transition load: Raw -> Raw;
    transition anonymize: Raw -> Anonymized;
    transition log: Anonymized -> Anonymized;

atom unsafe_log_raw_data(user_id: i64)
    effects: [DataPipeline];
    requires: user_id >= 0;
    ensures: result >= 0;
    body: {
        perform DataPipeline.load(user_id);
        // ERROR: log requires Anonymized, but state is Raw
        perform DataPipeline.log(user_id);
        user_id
    };
