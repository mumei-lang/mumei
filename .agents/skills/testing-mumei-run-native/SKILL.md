---
name: testing-mumei-run-native
description: Test Mumei native `run` and `--emit binary` CLI flows end-to-end. Use when changes touch cmd_run, binary LLVM codegen, linker inputs, runtime stubs, or Rust FFI native linking.
---
# Testing Mumei Native Run

## Devin Secrets Needed

None for local CLI native run testing.

## Prerequisites

Build the CLI from the repo root before testing:

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
```

The binary is at `target/debug/mumei`. LLVM 17, libclang, `z3`, and `libz3` must be available because native run verifies with Z3, emits LLVM, compiles to an object, and links a native executable.

No browser recording is useful for this flow; collect command stdout/stderr/status as text evidence.

## Native `run` Exit-Code Flow

Use this when validating that `mumei run` verifies, lowers, links, executes, and propagates `atom main()` as the process exit code.

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei run examples/run_demo.mm > /tmp/mumei-run-demo.out 2> /tmp/mumei-run-demo.err
status=$?
printf 'status=%s\n' "$status"
cat /tmp/mumei-run-demo.out
cat /tmp/mumei-run-demo.err
```

Expected assertions:
- Command status is exactly `15` for `examples/run_demo.mm`.
- stdout includes `Mumei Run: verify → codegen → link → execute`.
- stdout includes `Linking` and `Running`.
- stderr does not include `Codegen failed`, `Linking failed`, or `undefined reference`.

## Persistent Binary Flow

Use this when validating `--emit binary -o` behavior. `mumei run --emit binary -o <path>` writes the requested binary, then executes it and exits with the child binary's status. Do not expect status `0` unless the Mumei `main` returns `0`.

```bash
cd /home/ubuntu/repos/mumei
out_dir="$(mktemp -d /tmp/mumei-bin.XXXXXX)"
bin="$out_dir/run_demo_app"
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei run examples/run_demo.mm --emit binary -o "$bin" \
  > /tmp/mumei-emit-binary.out 2> /tmp/mumei-emit-binary.err
emit_status=$?
test -x "$bin"; bin_exists=$?
"$bin"; bin_status=$?
printf 'emit_status=%s\nbin_exists_status=%s\nbin_status=%s\nbin=%s\n' \
  "$emit_status" "$bin_exists" "$bin_status" "$bin"
cat /tmp/mumei-emit-binary.out
cat /tmp/mumei-emit-binary.err
```

Expected assertions:
- `emit_status` is exactly `15` for `examples/run_demo.mm`.
- stdout includes `Running $bin` and `Binary written to: $bin`.
- `bin_exists_status` is `0`, proving the requested output exists and is executable.
- `bin_status` is exactly `15`, proving the persisted binary independently executes with the same `atom main()` result.

## Rust FFI Runtime Link Flow

Use this when native run/linking changes touch `extern "Rust"`, runtime staticlib generation, linker arguments, or FFI backends.

```bash
cd /home/ubuntu/repos/mumei
fixture_dir="$(mktemp -d /tmp/mumei-rust-ffi.XXXXXX)"
fixture="$fixture_dir/rust_ffi.mm"
cat > "$fixture" <<'MM'
extern "Rust" {
    fn json_from_bool(value: i64) -> i64
        requires: value >= 0 && value <= 1;
        ensures: result >= 0;
}

atom main()
requires: true;
ensures: result >= 0;
body: { json_from_bool(0) };
MM
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei run "$fixture" > /tmp/mumei-rust-ffi.out 2> /tmp/mumei-rust-ffi.err
status=$?
printf 'status=%s\nfixture=%s\n' "$status" "$fixture"
cat /tmp/mumei-rust-ffi.out
cat /tmp/mumei-rust-ffi.err
```

Expected assertions:
- Command status is exactly `1`, the runtime handle returned by `json_from_bool(0)`.
- stdout includes `Linking` and `Running`.
- stderr includes `FFI Bridge: registered 1 extern function(s) from "Rust" block`.
- stderr does not include `undefined reference`, `Rust FFI runtime build failed`, or `Linking failed`.

## Cleanup

Keep generated fixtures and binaries outside the repo, preferably under `/tmp` or `/home/ubuntu/mumei-*-artifacts`. Before finishing, run:

```bash
git -C /home/ubuntu/repos/mumei status --short
```

Expected assertion: repo status is clean except for intentional source changes from the task under test.
