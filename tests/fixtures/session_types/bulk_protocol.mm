// Oversized protocol fixture: 33 states exceed MAX_PROTOCOL_NODES (32), so the
// Session Types analysis skips this effect and reports the skip instead.
// expected: PASS

effect BulkChannel
    states: [S0, S1, S2, S3, S4, S5, S6, S7, S8, S9, S10, S11, S12, S13, S14, S15, S16, S17, S18, S19, S20, S21, S22, S23, S24, S25, S26, S27, S28, S29, S30, S31, S32];
    initial: S0;
    transition step0: S0 -> S1;
    transition step1: S1 -> S2;
    transition step2: S2 -> S3;
    transition step3: S3 -> S4;
    transition step4: S4 -> S5;
    transition step5: S5 -> S6;
    transition step6: S6 -> S7;
    transition step7: S7 -> S8;
    transition step8: S8 -> S9;
    transition step9: S9 -> S10;
    transition step10: S10 -> S11;
    transition step11: S11 -> S12;
    transition step12: S12 -> S13;
    transition step13: S13 -> S14;
    transition step14: S14 -> S15;
    transition step15: S15 -> S16;
    transition step16: S16 -> S17;
    transition step17: S17 -> S18;
    transition step18: S18 -> S19;
    transition step19: S19 -> S20;
    transition step20: S20 -> S21;
    transition step21: S21 -> S22;
    transition step22: S22 -> S23;
    transition step23: S23 -> S24;
    transition step24: S24 -> S25;
    transition step25: S25 -> S26;
    transition step26: S26 -> S27;
    transition step27: S27 -> S28;
    transition step28: S28 -> S29;
    transition step29: S29 -> S30;
    transition step30: S30 -> S31;
    transition step31: S31 -> S32;
