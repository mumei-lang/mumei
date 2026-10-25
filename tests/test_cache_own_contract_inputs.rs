use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
    sync::atomic::{AtomicUsize, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

static NEXT_TEMP_DIR: AtomicUsize = AtomicUsize::new(0);

struct TempModule {
    dir: PathBuf,
    file: PathBuf,
}

impl TempModule {
    fn new(tag: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mumei_cache_own_contract_inputs_{}_{}_{}",
            std::process::id(),
            nonce,
            NEXT_TEMP_DIR.fetch_add(1, Ordering::SeqCst)
        ));
        fs::create_dir_all(dir.join("reports")).expect("create temporary directory");
        let file = dir.join(format!("{tag}.mm"));
        Self { dir, file }
    }

    fn write(&self, source: &str) {
        fs::write(&self.file, source).expect("write verification module");
    }

    fn verify(&self) -> Output {
        Command::new(env!("CARGO_BIN_EXE_mumei"))
            .arg("verify")
            .arg("--report-dir")
            .arg(self.dir.join("reports"))
            .arg(&self.file)
            .current_dir(env!("CARGO_MANIFEST_DIR"))
            .output()
            .expect("run mumei verify")
    }

    fn clear_cache(&self) {
        let cache_dir = self.dir.join(".mumei/cache");
        if cache_dir.exists() {
            fs::remove_dir_all(cache_dir).expect("remove verification cache");
        }
    }
}

impl Drop for TempModule {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn output_text(output: &Output) -> String {
    format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn assert_initial_verification(module: &TempModule, atom: &str) {
    let initial = module.verify();
    let text = output_text(&initial);
    assert!(initial.status.success(), "{text}");
    assert!(text.contains(&format!("'{atom}': verified")), "{text}");
}

fn assert_cached_and_fresh_failure(module: &TempModule, atom: &str, failure: &str) {
    let changed = module.verify();
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert!(
        !changed_text.contains(&format!("'{atom}': skipped (unchanged, cached)")),
        "{atom} should have been re-verified\n{changed_text}"
    );
    assert!(changed_text.contains(failure), "{changed_text}");

    module.clear_cache();
    let fresh = module.verify();
    let fresh_text = output_text(&fresh);
    assert!(!fresh.status.success(), "{fresh_text}");
    assert!(fresh_text.contains(failure), "{fresh_text}");
}

const OWN_EFFECT_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom g(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        perform File.open(x);
        x
    };
"#;

#[test]
fn own_effect_pre_change_reverifies_and_fails() {
    let module = TempModule::new("own_effect_pre");
    module.write(OWN_EFFECT_SOURCE);
    assert_initial_verification(&module, "g");

    let changed =
        OWN_EFFECT_SOURCE.replace("effect_pre: { File: Closed }", "effect_pre: { File: Open }");
    assert_ne!(changed, OWN_EFFECT_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(
        &module,
        "g",
        "operation 'open' requires state 'Closed' but current state is 'Open'",
    );
}

#[test]
fn own_effect_post_change_reverifies_and_fails() {
    let module = TempModule::new("own_effect_post");
    module.write(OWN_EFFECT_SOURCE);
    assert_initial_verification(&module, "g");

    let changed = OWN_EFFECT_SOURCE.replace(
        "effect_post: { File: Open }",
        "effect_post: { File: Closed }",
    );
    assert_ne!(changed, OWN_EFFECT_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(
        &module,
        "g",
        "final state 'Open' but effect_post declares 'Closed'",
    );
}

const QUANTIFIED_SOURCE: &str = r#"
atom qg(arr: [i64], n: i64) -> i64
    requires: n >= 1 && len(arr) >= n && forall(i, 0, n, arr[i] > 0);
    ensures: result > 0;
    body: arr[0];
"#;

#[test]
fn own_forall_constraint_change_reverifies_and_fails() {
    let module = TempModule::new("own_forall");
    module.write(QUANTIFIED_SOURCE);
    assert_initial_verification(&module, "qg");

    let unchanged = module.verify();
    let unchanged_text = output_text(&unchanged);
    assert!(unchanged.status.success(), "{unchanged_text}");
    assert!(
        unchanged_text.contains("'qg': skipped (unchanged, cached)"),
        "{unchanged_text}"
    );

    let changed = QUANTIFIED_SOURCE.replace("arr[i] > 0", "arr[i] >= 0");
    assert_ne!(changed, QUANTIFIED_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(&module, "qg", "Postcondition (ensures) is not satisfied");
}

const EFFECT_PARENT_SOURCE: &str = r#"
effect ParentA;
effect ParentB;
effect Child parent: ParentA;

atom cf(x: i64) -> i64
    effects: [Child];
    requires: true;
    ensures: result == x;
    body: x;

atom gp(x: i64) -> i64
    effects: [ParentA];
    requires: true;
    ensures: result == x;
    body: cf(x);
"#;

#[test]
fn callee_effect_parent_change_reverifies_and_fails() {
    let module = TempModule::new("effect_parent");
    module.write(EFFECT_PARENT_SOURCE);
    assert_initial_verification(&module, "gp");

    let changed = EFFECT_PARENT_SOURCE.replace(
        "effect Child parent: ParentA;",
        "effect Child parent: ParentB;",
    );
    assert_ne!(changed, EFFECT_PARENT_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(&module, "gp", "Effect propagation violation");
}

const EFFECT_INCLUDES_SOURCE: &str = r#"
effect FileRead;
effect FileWrite;
effect IO includes: [FileRead];

atom fi(x: i64) -> i64
    effects: [IO];
    requires: true;
    ensures: result == x;
    body: x;

atom hi(x: i64) -> i64
    effects: [IO];
    requires: true;
    ensures: result == x;
    body: fi(x);

atom gi(x: i64) -> i64
    effects: [FileRead];
    requires: true;
    ensures: result == x;
    body: hi(x);
"#;

#[test]
fn transitive_callee_effect_includes_change_reverifies_and_fails() {
    let module = TempModule::new("effect_includes");
    module.write(EFFECT_INCLUDES_SOURCE);
    assert_initial_verification(&module, "gi");

    let changed = EFFECT_INCLUDES_SOURCE.replace(
        "effect IO includes: [FileRead];",
        "effect IO includes: [FileWrite];",
    );
    assert_ne!(changed, EFFECT_INCLUDES_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(&module, "gi", "Effect propagation violation");
}

const EFFECT_TRANSITION_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom ft(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        perform File.open(x);
        x
    };

atom gt(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Closed };
    requires: true;
    ensures: result == x;
    body: {
        ft(x);
        perform File.close(x);
        x
    };
"#;

#[test]
fn callee_effect_transition_change_reverifies_and_fails() {
    let module = TempModule::new("effect_transition");
    module.write(EFFECT_TRANSITION_SOURCE);
    assert_initial_verification(&module, "gt");

    let changed = EFFECT_TRANSITION_SOURCE.replace(
        "transition close: Open -> Closed;",
        "transition close: Open -> Open;",
    );
    assert_ne!(changed, EFFECT_TRANSITION_SOURCE);
    module.write(&changed);
    assert_cached_and_fresh_failure(
        &module,
        "gt",
        "final state 'Open' but effect_post declares 'Closed'",
    );
}

const UNRELATED_EFFECT_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;

effect Other
    states: [Before, After];
    initial: Before;
    transition advance: Before -> After;

atom gu(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        perform File.open(x);
        x
    };
"#;

#[test]
fn unrelated_effect_definition_change_keeps_the_root_cached() {
    let module = TempModule::new("unrelated_effect");
    module.write(UNRELATED_EFFECT_SOURCE);
    assert_initial_verification(&module, "gu");

    let changed = UNRELATED_EFFECT_SOURCE.replace(
        "transition advance: Before -> After;",
        "transition advance: Before -> Before;",
    );
    assert_ne!(changed, UNRELATED_EFFECT_SOURCE);
    module.write(&changed);

    let cached = module.verify();
    let cached_text = output_text(&cached);
    assert!(cached.status.success(), "{cached_text}");
    assert!(
        cached_text.contains("'gu': skipped (unchanged, cached)"),
        "{cached_text}"
    );

    module.clear_cache();
    let fresh = module.verify();
    let fresh_text = output_text(&fresh);
    assert!(fresh.status.success(), "{fresh_text}");
    assert!(fresh_text.contains("'gu': verified"), "{fresh_text}");
}
