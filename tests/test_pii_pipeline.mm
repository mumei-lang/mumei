// =============================================================
// tests/test_pii_pipeline.mm — PII Pipeline E2E Test
// =============================================================
// Integration test for PII Data Pipeline example.
// Usage: mumei check tests/test_pii_pipeline.mm

effect DataPipeline
    states: [Raw, Anonymized];
    initial: Raw;
    transition load: Raw -> Raw;
    transition anonymize: Raw -> Anonymized;
    transition log: Anonymized -> Anonymized;

// Test 1: Basic valid pipeline
atom test_basic_pipeline(user_id: i64)
    effects: [DataPipeline];
    requires: user_id >= 0;
    ensures: result >= 0;
    body: {
        perform DataPipeline.load(user_id);
        perform DataPipeline.anonymize(user_id);
        perform DataPipeline.log(user_id);
        user_id
    };

// Test 2: Multiple loads before anonymize
atom test_multiple_loads(user_id: i64)
    effects: [DataPipeline];
    requires: user_id >= 0;
    ensures: result >= 0;
    body: {
        perform DataPipeline.load(user_id);
        perform DataPipeline.load(user_id);
        perform DataPipeline.anonymize(user_id);
        perform DataPipeline.log(user_id);
        user_id
    };

// Test 3: Multiple logs after anonymize
atom test_multiple_logs(user_id: i64)
    effects: [DataPipeline];
    requires: user_id >= 0;
    ensures: result >= 0;
    body: {
        perform DataPipeline.load(user_id);
        perform DataPipeline.anonymize(user_id);
        perform DataPipeline.log(user_id);
        perform DataPipeline.log(user_id);
        user_id
    };
