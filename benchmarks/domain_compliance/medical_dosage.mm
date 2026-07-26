// Medical device dosage control: cumulative dose must never exceed the daily
// maximum, and every administered dose stays inside the therapeutic window.
// expected: PASS

type DoseMicrograms = i64 where v >= 0 && v <= 4000;

atom dose_within_therapeutic_window(dose: i64)
    requires: dose >= 100 && dose <= 1000;
    ensures: result == dose && result >= 100 && result <= 1000;
    body: dose;

atom clamp_to_daily_maximum(administered: i64, requested: i64)
    requires: administered >= 0 && administered <= 4000 && requested >= 0 && requested <= 1000;
    ensures: result >= 0 && result <= 4000 && result >= administered;
    body: { if administered + requested > 4000 { 4000 } else { administered + requested } };

atom remaining_daily_allowance(administered: i64)
    requires: administered >= 0 && administered <= 4000;
    ensures: result >= 0 && result <= 4000 && result == 4000 - administered;
    body: 4000 - administered;

atom infusion_rate_is_bounded(volume_ml: i64, minutes: i64)
    requires: volume_ml >= 0 && volume_ml <= 1000 && minutes >= 1 && minutes <= 1440;
    ensures: result >= 0 && result <= 1000;
    body: volume_ml / minutes;
