// =============================================================
// PII Data Pipeline: Compile-Time Anonymization Enforcement
// =============================================================
// Demonstrates how Mumei's Temporal Effect Verification prevents
// logging raw (un-anonymized) personal data at compile time.

// Define a DataPipeline stateful effect with Raw/Anonymized states
effect DataPipeline
    states: [Raw, Anonymized];
    initial: Raw;
    transition load: Raw -> Raw;
    transition anonymize: Raw -> Anonymized;
    transition log: Anonymized -> Anonymized;

// ✅ Valid: load → anonymize → log (correct pipeline)
atom safe_pipeline(user_id: i64)
    effects: [DataPipeline];
    requires: user_id >= 0;
    ensures: result >= 0;
    body: {
        perform DataPipeline.load(user_id);
        perform DataPipeline.anonymize(user_id);
        perform DataPipeline.log(user_id);
        user_id
    };

// ❌ Invalid: load → log (skips anonymize)
// Uncomment to see compile-time error:
//   "Temporal effect violation: 'DataPipeline' operation 'log'
//    requires state 'Anonymized' but current state is 'Raw'"
//
// atom unsafe_pipeline(user_id: i64)
//     effects: [DataPipeline];
//     requires: user_id >= 0;
//     ensures: result >= 0;
//     body: {
//         perform DataPipeline.load(user_id);
//         perform DataPipeline.log(user_id);
//         user_id
//     };
