trusted atom clamp_dose(requested: i64, max_dose: i64) -> i64
    requires: max_dose >= 0 && requested >= 0;
    ensures: result >= 0 && result <= max_dose && result <= requested;
    body: {
        0
    };
