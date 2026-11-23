use std::io::Read;
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const MAX_VERIFY_TIME: Duration = Duration::from_secs(120);

fn run_verify_with_timeout(fixture_name: &str) -> Output {
    let dir = tempfile::tempdir().expect("create verification directory");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(fixture_name);
    let copied = dir.path().join(fixture_name);
    std::fs::copy(source, &copied).expect("copy equality fixture");

    let mut child = Command::new(env!("CARGO_BIN_EXE_mumei"))
        .arg("verify")
        .arg("--json")
        .arg("--disable-spurious-detection")
        .arg(&copied)
        .current_dir(dir.path())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn mumei verify");
    let mut stdout = child.stdout.take().expect("capture verify stdout");
    let mut stderr = child.stderr.take().expect("capture verify stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("read verify stdout");
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("read verify stderr");
        bytes
    });

    let deadline = Instant::now() + MAX_VERIFY_TIME;
    loop {
        if let Some(status) = child.try_wait().expect("poll mumei verify") {
            return Output {
                status,
                stdout: stdout_reader.join().expect("join stdout reader"),
                stderr: stderr_reader.join().expect("join stderr reader"),
            };
        }
        if Instant::now() >= deadline {
            if let Err(kill_error) = child.kill() {
                if child
                    .try_wait()
                    .expect("poll timed-out mumei verify")
                    .is_none()
                {
                    panic!("failed to kill timed-out mumei verify: {kill_error}");
                }
            }
            let status = child.wait().expect("wait for killed mumei verify");
            let _ = stdout_reader
                .join()
                .expect("join stdout reader after timeout");
            let _ = stderr_reader
                .join()
                .expect("join stderr reader after timeout");
            panic!("mumei verify exceeded {MAX_VERIFY_TIME:?} and was killed ({status})");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn array_free_nested_enum_equality_completes_within_deadline() {
    let output = run_verify_with_timeout("test_enum_payload_equality_work.mm");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "array-free nested enum equality must verify:\n{stdout}\n{stderr}"
    );
}
