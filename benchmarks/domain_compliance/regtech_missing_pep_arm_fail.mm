// Counterexample case: the PEP (politically exposed person) category is missing
// from the match, so regulatory coverage is incomplete. The exhaustiveness check
// must reject this atom.
// expected: FAIL

enum CustomerType {
    Individual,
    Corporate,
    Government,
    PEP
}

atom classify_without_pep(customer_type: CustomerType)
    requires: true;
    ensures: result >= 0 && result <= 3;
    body: {
        match customer_type {
            Individual => 0,
            Corporate => 1,
            Government => 0
        }
    }
