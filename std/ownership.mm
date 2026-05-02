// =============================================================
// Mumei Standard Library: Ownership Transfer Protocol
// =============================================================
// 検証済み Ownership 移譲プロトコル: Idle から提案し、
// 承認または取消を通じて所有権移譲の時系列安全性を保証する。
// effect_pre/effect_post により、不正な状態遷移をコンパイル時に検出する。
//
// Usage:
//   import "std/ownership" as ownership;

effect Ownership
    states: [Idle, PendingTransfer, Transferred];
    initial: Idle;
    transition propose: Idle -> PendingTransfer;
    transition accept: PendingTransfer -> Transferred;
    transition cancel: PendingTransfer -> Idle;

// 移譲提案: Idle から PendingTransfer へ遷移する
atom propose_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: PendingTransfer };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Ownership.propose;
        new_owner
    };

// 移譲承認: PendingTransfer から Transferred へ遷移する
atom accept_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: PendingTransfer };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        perform Ownership.accept;
        new_owner
    };

// 移譲取消: PendingTransfer から Idle へ戻す
atom cancel_transfer(current_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: PendingTransfer };
    effect_post: { Ownership: Idle };
    requires: current_owner >= 0;
    ensures: result == current_owner;
    body: {
        perform Ownership.cancel;
        current_owner
    };

// 完全移譲: propose_transfer と accept_transfer の契約を合成する
atom full_transfer(new_owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Transferred };
    requires: new_owner >= 0;
    ensures: result == new_owner;
    body: {
        propose_transfer(new_owner);
        accept_transfer(new_owner);
        new_owner
    };

// 提案後取消: propose_transfer と cancel_transfer の契約を合成する
atom propose_and_cancel(owner: i64)
    effects: [Ownership];
    effect_pre: { Ownership: Idle };
    effect_post: { Ownership: Idle };
    requires: owner >= 0;
    ensures: result == owner;
    body: {
        propose_transfer(owner);
        cancel_transfer(owner);
        owner
    };
