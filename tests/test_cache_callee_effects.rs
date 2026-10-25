use std::{
    fs,
    path::PathBuf,
    process::{Command, Output},
};

struct TempModule {
    dir: PathBuf,
    file: PathBuf,
}

impl TempModule {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "mumei_cache_callee_effects_{}_{}",
            std::process::id(),
            tag
        ));
        if dir.exists() {
            fs::remove_dir_all(&dir).expect("remove stale temporary directory");
        }
        fs::create_dir_all(dir.join("reports")).expect("create temporary directory");
        let file = dir.join("repro.mm");
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

fn assert_not_cached(text: &str, atom: &str) {
    assert!(
        !text.contains(&format!("'{atom}': skipped (unchanged, cached)")),
        "{atom} should have been re-verified\n{text}"
    );
}

const EFFECT_PRE_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom f(x: i64) -> i64
    effect_pre: { File: Closed };
    requires: true;
    ensures: result == x;
    body: x;

atom g(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        f(x);
        perform File.open(x);
        x
    };
"#;

#[test]
fn effect_pre_change_reverifies_and_fails_the_caller() {
    let module = TempModule::new("effect_pre");
    module.write(EFFECT_PRE_SOURCE);

    let initial = module.verify();
    let initial_text = output_text(&initial);
    assert!(initial.status.success(), "{initial_text}");

    let unchanged = module.verify();
    let unchanged_text = output_text(&unchanged);
    assert!(unchanged.status.success(), "{unchanged_text}");
    assert!(
        unchanged_text.contains("'g': skipped (unchanged, cached)"),
        "{unchanged_text}"
    );

    let changed_source = EFFECT_PRE_SOURCE.replacen(
        "atom f(x: i64) -> i64\n    effect_pre: { File: Closed };",
        "atom f(x: i64) -> i64\n    effect_pre: { File: Open };",
        1,
    );
    assert_ne!(changed_source, EFFECT_PRE_SOURCE);
    module.write(&changed_source);

    let changed = module.verify();
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert_not_cached(&changed_text, "g");
    assert!(
        changed_text.contains(
            "'File' operation 'call f' requires state 'Open' but current state is 'Closed'"
        ),
        "{changed_text}"
    );

    module.clear_cache();
    let fresh = module.verify();
    let fresh_text = output_text(&fresh);
    assert!(!fresh.status.success(), "{fresh_text}");
    assert!(fresh_text.contains("'f': verified"), "{fresh_text}");
    assert!(
        fresh_text.contains(
            "'File' operation 'call f' requires state 'Open' but current state is 'Closed'"
        ),
        "{fresh_text}"
    );
}

const EFFECT_POST_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom f(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        perform File.open(x);
        x
    };

atom g(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Closed };
    requires: true;
    ensures: result == x;
    body: {
        f(x);
        perform File.close(x);
        x
    };
"#;

#[test]
fn effect_post_change_reverifies_and_fails_the_caller() {
    let module = TempModule::new("effect_post");
    module.write(EFFECT_POST_SOURCE);

    let initial = module.verify();
    let initial_text = output_text(&initial);
    assert!(initial.status.success(), "{initial_text}");

    let changed_source = EFFECT_POST_SOURCE.replacen(
        "effect_post: { File: Open };",
        "effect_post: { File: Closed };",
        1,
    );
    assert_ne!(changed_source, EFFECT_POST_SOURCE);
    module.write(&changed_source);

    let changed = module.verify();
    let changed_text = output_text(&changed);
    assert!(!changed.status.success(), "{changed_text}");
    assert_not_cached(&changed_text, "g");
    assert!(
        changed_text.contains(
            "'File' operation 'close' requires state 'Open' but current state is 'Closed'"
        ),
        "{changed_text}"
    );
}

const TRANSITIVE_SOURCE: &str = r#"
effect File
    states: [Closed, Open];
    initial: Closed;
    transition open: Closed -> Open;
    transition close: Open -> Closed;

atom f(x: i64) -> i64
    effect_pre: { File: Closed };
    requires: true;
    ensures: result == x;
    body: x;

atom h(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Open };
    requires: true;
    ensures: result == x;
    body: {
        f(x);
        perform File.open(x);
        x
    };

atom g(x: i64) -> i64
    effects: [File];
    effect_pre: { File: Closed };
    effect_post: { File: Closed };
    requires: true;
    ensures: result == x;
    body: {
        h(x);
        perform File.close(x);
        x
    };
"#;

#[test]
fn transitive_callee_effect_change_reverifies_the_root() {
    let module = TempModule::new("transitive");
    module.write(TRANSITIVE_SOURCE);

    let initial = module.verify();
    let initial_text = output_text(&initial);
    assert!(initial.status.success(), "{initial_text}");

    let changed_source = TRANSITIVE_SOURCE.replacen(
        "atom f(x: i64) -> i64\n    effect_pre: { File: Closed };",
        "atom f(x: i64) -> i64\n    effect_pre: { File: Open };",
        1,
    );
    assert_ne!(changed_source, TRANSITIVE_SOURCE);
    module.write(&changed_source);

    let changed = module.verify();
    let changed_text = output_text(&changed);
    assert_not_cached(&changed_text, "g");
    assert!(changed_text.contains("'g': verified"), "{changed_text}");
}
