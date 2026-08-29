//! Golden tests for the proof-aware runtime monitor emitter.
//!
//! Two properties are pinned: trust boundaries get a monitor, and fully proven
//! pure atoms stay zero-cost (no artifact, no runtime dependency).

use std::path::PathBuf;
use std::process::Command;

fn build_monitor(tag: &str, source: &str) -> (PathBuf, String) {
    let bin = env!("CARGO_BIN_EXE_mumei");
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let out_dir = std::env::temp_dir().join(format!(
        "mumei_runtime_monitor_{tag}_{}",
        std::process::id()
    ));
    if out_dir.exists() {
        std::fs::remove_dir_all(&out_dir).expect("clean stale output dir");
    }
    std::fs::create_dir_all(&out_dir).expect("create output dir");

    let output = Command::new(bin)
        .arg("build")
        .arg(source)
        .arg("--emit")
        .arg("runtime-monitor")
        .arg("--output")
        .arg(out_dir.join("out"))
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|err| panic!("failed to run mumei build: {err}"));

    let log = format!(
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "mumei build failed\n{log}");
    (out_dir, log)
}

fn monitor_files(dir: &PathBuf) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .expect("read output dir")
        .filter_map(|entry| {
            let path = entry.expect("dir entry").path();
            path.to_string_lossy()
                .ends_with(".monitor.rs")
                .then_some(path)
        })
        .collect()
}

#[test]
fn trust_boundary_atom_gets_a_monitor() {
    let (dir, log) = build_monitor(
        "trusted",
        "tests/fixtures/runtime_monitor/trusted_boundary.mm",
    );
    let files = monitor_files(&dir);
    assert_eq!(files.len(), 1, "expected one monitor artifact\n{log}");

    let source = std::fs::read_to_string(&files[0]).expect("read monitor");
    assert!(source.contains("pub fn read_sensor_monitored("));
    assert!(source.contains("boundary: \"trusted_atom\""));
    // Violations are observed, not enforced: no panics in generated code.
    assert!(!source.contains("panic!"));
    assert!(!source.contains("assert!"));
    // NoOp fallback plus OTLP endpoint wiring.
    assert!(source.contains("OTEL_ENABLED"));
    assert!(source.contains("OTEL_EXPORTER_OTLP_ENDPOINT"));
    assert!(source.contains("if mumei_monitor::enabled() && !(channel >= 0)"));
    assert!(source.contains("if mumei_monitor::enabled() && !(result >= 0)"));
}

/// A generated monitor observes contract violations: a host hook that panics
/// must be contained by the runtime instead of unwinding through the monitored
/// call.
#[test]
fn a_panicking_host_hook_does_not_escape_the_monitor() {
    let (dir, log) = build_monitor(
        "hook_panic",
        "tests/fixtures/runtime_monitor/trusted_boundary.mm",
    );
    let files = monitor_files(&dir);
    assert_eq!(files.len(), 1, "expected one monitor artifact\n{log}");
    let monitor = std::fs::read_to_string(&files[0]).expect("read monitor");

    let host = format!(
        r#"{monitor}

mod host_impl {{
    #[no_mangle]
    pub extern "C" fn read_sensor(channel: i64) -> i64 {{
        channel
    }}
}}

fn panicking_hook(_violation: &mumei_monitor::Violation) {{
    panic!("hook is broken");
}}

fn main() {{
    mumei_monitor::set_violation_hook(panicking_hook).expect("install hook");
    let result = read_sensor_monitored(-1);
    println!("survived={{result}}");
}}
"#
    );
    let host_path = dir.join("hook_panic_host.rs");
    std::fs::write(&host_path, host).expect("write host program");

    let bin_path = dir.join("hook_panic_host");
    let compile = Command::new("rustc")
        .arg("--edition")
        .arg("2021")
        .arg("--crate-name")
        .arg("hook_panic_host")
        .arg(&host_path)
        .arg("-o")
        .arg(&bin_path)
        .output()
        .unwrap_or_else(|err| panic!("failed to run rustc: {err}"));
    assert!(
        compile.status.success(),
        "generated monitor did not compile\n{}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let run = Command::new(&bin_path)
        .env("OTEL_ENABLED", "1")
        .output()
        .unwrap_or_else(|err| panic!("failed to run host program: {err}"));
    let stdout = String::from_utf8_lossy(&run.stdout);
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(
        run.status.success(),
        "hook panic escaped the monitor\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(stdout.contains("survived=-1"), "stdout:\n{stdout}");
    assert!(
        stderr.contains("mumei.monitor.hook_panicked"),
        "stderr:\n{stderr}"
    );
}

#[test]
fn proven_pure_atom_is_zero_cost() {
    let (dir, log) = build_monitor("pure", "tests/fixtures/runtime_monitor/pure_proven.mm");
    assert!(
        monitor_files(&dir).is_empty(),
        "proven pure atoms must not be instrumented\n{log}"
    );
}
