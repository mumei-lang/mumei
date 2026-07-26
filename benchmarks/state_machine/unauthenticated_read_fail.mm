// Counterexample case: the atom declares it ends in `Authenticated` but only
// performs the challenge step, so the declared post-state is unreachable.
// expected: FAIL

effect Session
    states: [Anonymous, Challenged, Authenticated, Closed];
    initial: Anonymous;
    transition challenge: Anonymous -> Challenged;
    transition authenticate: Challenged -> Authenticated;
    transition logout: Authenticated -> Closed;

atom read_before_authentication(user_id: i64)
    effects: [Session];
    effect_pre: { Session: Anonymous };
    effect_post: { Session: Authenticated };
    requires: user_id > 0;
    ensures: result == user_id;
    body: {
        perform Session.challenge;
        user_id
    }
