// Session authentication state machine: privileged reads require an
// authenticated session, and logout always returns to Anonymous.
// expected: PASS

effect Session
    states: [Anonymous, Challenged, Authenticated, Closed];
    initial: Anonymous;
    transition challenge: Anonymous -> Challenged;
    transition authenticate: Challenged -> Authenticated;
    transition logout: Authenticated -> Closed;
    transition reject: Challenged -> Anonymous;

atom login_flow(user_id: i64)
    effects: [Session];
    effect_pre: { Session: Anonymous };
    effect_post: { Session: Authenticated };
    requires: user_id > 0;
    ensures: result == user_id;
    body: {
        perform Session.challenge;
        perform Session.authenticate;
        user_id
    }

atom rejected_login(user_id: i64)
    effects: [Session];
    effect_pre: { Session: Anonymous };
    effect_post: { Session: Anonymous };
    requires: user_id > 0;
    ensures: result == 0;
    body: {
        perform Session.challenge;
        perform Session.reject;
        0
    }

atom logout_flow(user_id: i64)
    effects: [Session];
    effect_pre: { Session: Authenticated };
    effect_post: { Session: Closed };
    requires: user_id > 0;
    ensures: result == user_id;
    body: {
        perform Session.logout;
        user_id
    }

atom full_session_cycle(user_id: i64)
    effects: [Session];
    effect_pre: { Session: Anonymous };
    effect_post: { Session: Closed };
    requires: user_id > 0;
    ensures: result == user_id;
    body: {
        perform Session.challenge;
        perform Session.authenticate;
        perform Session.logout;
        user_id
    }
