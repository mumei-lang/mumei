//! Emitter Plugin Architecture Phase 3: `mumei add --emitter <name>` install path.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn plugin_filename(name: &str) -> String {
    let (prefix, ext) = if cfg!(target_os = "windows") {
        ("", ".dll")
    } else if cfg!(target_os = "macos") {
        ("lib", ".dylib")
    } else {
        ("lib", ".so")
    };
    format!("{prefix}mumei_emit_{name}{ext}")
}

fn fixture_home(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("mumei_add_emitter_{}_{}", name, std::process::id()));
    if dir.exists() {
        std::fs::remove_dir_all(&dir).expect("clean stale emitter fixture home");
    }
    std::fs::create_dir_all(&dir).expect("create emitter fixture home");
    dir
}

fn add_emitter(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("add")
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .current_dir(home)
        .output()
        .expect("run mumei add --emitter")
}

fn installed_path(home: &Path, name: &str) -> PathBuf {
    home.join(".mumei")
        .join("emitters")
        .join(name)
        .join(plugin_filename(name))
}

#[test]
fn rejects_invalid_emitter_name() {
    let home = fixture_home("invalid_name");
    let output = add_emitter(&home, &["--emitter", "../evil"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("invalid emitter name"), "stderr: {stderr}");
}

#[test]
fn reports_expected_library_filename_when_missing() {
    let home = fixture_home("missing_lib");
    let output = add_emitter(
        &home,
        &["--emitter", "wasm", "--path", &home.to_string_lossy()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(
        stderr.contains(&plugin_filename("wasm")),
        "stderr: {stderr}"
    );
    assert!(!installed_path(&home, "wasm").exists());
}

/// A file that is not a loadable library must not be left behind: install
/// validation goes through the same `load_external_emitter` path the build
/// uses, so a failure rolls the destination back.
#[test]
fn rejects_and_rolls_back_a_non_library_file() {
    let home = fixture_home("bogus_lib");
    let source = home.join(plugin_filename("bogus"));
    std::fs::write(&source, b"not a shared object").expect("write bogus plugin");

    let output = add_emitter(
        &home,
        &["--emitter", "bogus", "--path", &source.to_string_lossy()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("failed validation"), "stderr: {stderr}");
    assert!(
        !installed_path(&home, "bogus").exists(),
        "a plugin that fails validation must not stay installed"
    );
}

/// The install reuses `EMITTER_ABI_VERSION` checking rather than trusting the
/// dropped-in library, so a plugin reporting another ABI version is refused.
#[test]
fn rejects_abi_version_mismatch() {
    if cfg!(target_os = "windows") {
        return;
    }
    let home = fixture_home("abi_mismatch");
    let src = home.join("plugin.rs");
    std::fs::write(
        &src,
        "#[no_mangle]\npub extern \"C\" fn mumei_emitter_abi_version() -> u32 { 9999 }\n",
    )
    .expect("write plugin source");

    let lib = home.join(plugin_filename("stale"));
    let rustc = Command::new("rustc")
        .args(["--crate-type", "cdylib", "-o"])
        .arg(&lib)
        .arg(&src)
        .output();
    match rustc {
        Ok(out) if out.status.success() => {}
        _ => return, // no usable rustc in this environment
    }

    let output = add_emitter(
        &home,
        &["--emitter", "stale", "--path", &lib.to_string_lossy()],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("ABI version mismatch"), "stderr: {stderr}");
    assert!(!installed_path(&home, "stale").exists());
}

/// `--force` stages the candidate next to the installed library and only
/// renames it into place after validation, so a rejected reinstall leaves the
/// previous bytes byte-for-byte intact and drops the staged file.
#[test]
fn failed_force_reinstall_keeps_the_previous_install() {
    let home = fixture_home("force_rollback");
    let installed = installed_path(&home, "keep");
    std::fs::create_dir_all(installed.parent().expect("emitter dir")).expect("create emitter dir");
    std::fs::write(&installed, b"PREVIOUS-PLUGIN").expect("seed previous install");

    let source = home.join(plugin_filename("keep"));
    std::fs::write(&source, b"not a shared object").expect("write bogus plugin");

    let output = add_emitter(
        &home,
        &[
            "--emitter",
            "keep",
            "--path",
            &source.to_string_lossy(),
            "--force",
        ],
    );
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert_eq!(
        std::fs::read(&installed).expect("previous install still readable"),
        b"PREVIOUS-PLUGIN",
        "a rejected --force reinstall must not touch the previous install"
    );
    let staged = installed
        .parent()
        .expect("emitter dir")
        .join(format!(".{}.incoming", plugin_filename("keep")));
    assert!(!staged.exists(), "staged candidate must be cleaned up");
}

/// `--path` / `--force` are emitter-only, and clap's `requires` does not fire
/// once the positional dependency is present, so the dispatcher rejects them
/// rather than dropping them silently.
#[test]
fn dependency_mode_rejects_emitter_only_flags() {
    let home = fixture_home("dep_flags");
    let output = add_emitter(&home, &["some_dep", "--path", "/tmp", "--force"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("--path and --force"), "stderr: {stderr}");
}

#[test]
fn emitter_flag_conflicts_with_dependency_argument() {
    let home = fixture_home("conflict");
    let output = add_emitter(&home, &["some_dep", "--emitter", "wasm"]);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "stderr: {stderr}");
    assert!(stderr.contains("cannot be used with"), "stderr: {stderr}");
}
