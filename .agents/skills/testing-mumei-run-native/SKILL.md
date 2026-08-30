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

## Task Group Cancellation Flow

Use this when changes touch `task_group:any`, task cancellation, loop checkpoints, channel wakeups, or runtime task-group state. A nested group fixture is more adversarial than a flat blocked sibling: if cancellation only checks the innermost group, the command times out instead of returning the winning outer child value.

```bash
cd /home/ubuntu/repos/mumei
fixture_dir="/home/ubuntu/mumei-task-group-any-artifacts"
mkdir -p "$fixture_dir"
fixture="$fixture_dir/nested_any_cancel.mm"
cat > "$fixture" <<'MM'
trusted atom main()
requires: true;
ensures: true;
body: {
    task_group:any {
        task { 7 };
        task {
            task_group:any {
                task { recv(0) }
            }
        }
    }
};
MM
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  timeout 5s target/debug/mumei run "$fixture" \
  > "$fixture_dir/nested_any_cancel.out" 2> "$fixture_dir/nested_any_cancel.err"
status=$?
printf 'status=%s\nfixture=%s\n' "$status" "$fixture"
cat "$fixture_dir/nested_any_cancel.out"
cat "$fixture_dir/nested_any_cancel.err"
```

Expected assertions:
- Command status is exactly `7`, proving the first completed outer child wins.
- Command status is not `124`; `124` means GNU `timeout` killed a hung nested cancellation flow.
- stdout includes `Mumei Run: verify → codegen → link → execute`.
- stdout includes `Linking 1 atom(s) to native binary` and `Running`.
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

## Channel Payload Round-Trip Flow (`chan<T>`)

Use this when changes touch `HirExpr::ChanSend`/`ChanRecv`, payload marshalling, or
`chan_payload_type` tracking. The runtime slot is a plain `int64_t`, and channel handles are
plain integers, so a literal handle works: `relay(0, 2.5)` gives a real runtime round trip
(channel `0` is lazily created by `runtime/mumei_runtime.c`). Make `atom main()` return `7`
only when the received value equals the expected one — the process exit code then *is* the
assertion, and a zeroed / bit-reinterpreted payload shows up as a different exit code.

```bash
cat > /tmp/f64run.mm <<'MM'
trusted atom relay(ch: chan<f64>, x: f64) -> f64
requires: true; ensures: true;
body: { send(ch, x); recv(ch) };

trusted atom main()
requires: true; ensures: true;
body: { let got = relay(0, 2.5); if got == 2.5 { 7 } else { 0 } };
MM
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  target/debug/mumei run /tmp/f64run.mm; echo "exit=$?"   # expect 7
```

Adversarial variants worth covering (each as its own atom so one fixture exercises many cases):
- payload type matching the channel (`chan<f64>` + `f64`, `chan<i64>` + `i64`) — the IR must
  contain *no* numeric conversion for the int case and only `bitcast double .. to i64` /
  `bitcast i64 .. to double` for the f64 case;
- payload type mismatching the channel (`send(ch, 3)` / `send(ch, n)` on `chan<f64>`, `send(ch, 2.9)`
  on `chan<i64>`, negative values) — these must convert *numerically* (`sitofp`/`fptosi`), never
  bit-preserve, otherwise `recv` reinterprets integer bits as a double;
- alias handles (`let c2 = ch; send(c2, x); recv(c2)`) — payload typing must survive rebinding;
- `chan<Str>`/pointer payloads — IR must show `ptrtoint` before `__mumei_chan_send` and `inttoptr`
  after `__mumei_chan_recv`, with the atom typed `ptr`.

Inspect the IR with `mumei build fixture.mm --emit llvm-ir`, which writes `katana_<atom>.ll` into
the current directory (keep the fixture outside the repo so these files do not dirty it), then
`grep -cE "bitcast|sitofp|fptosi|ptrtoint|inttoptr"` per atom for an exact-count assertion.

Known pre-existing limitation: a fixture containing a **string literal** may fail to link via
`mumei run` with `relocation R_X86_64_32 against '.rodata.str1.1' can not be used when making a
PIE object`. That is unrelated to channel work; use the manual PIC link below to still prove
runtime behavior, and report it as pre-existing rather than a regression.

## Manual PIC Link Flow (arrays via C driver, PIE workaround)

Use this when a value cannot be constructed in Mumei source (there is no array-literal codegen —
`let a = [3,4]` lowers to `i64 0`, so arrays only arrive as atom parameters), or when
`mumei run` cannot link. Arrays use a fat-pointer ABI, so a C driver can supply real storage and
observe what the compiled atom (including code inside `task { ... }`) reads and writes:

```c
/* driver.c */
#include <stdint.h>
typedef struct { int64_t len; int64_t *data; } mm_arr;  /* fat pointer ABI */
int64_t mix_scalar(mm_arr a, int64_t k);
int main(void) {
    int64_t buf[3] = {11, 31, 5};
    mm_arr a = { 3, buf };
    return mix_scalar(a, 100) == 142 ? 0 : 1;   /* also check buf[] after writes */
}
```

```bash
cd /home/ubuntu/mumei-p25-artifacts/arrdir   # fixtures outside the repo
target/debug/mumei build arrcap.mm --emit llvm-ir
/usr/lib/llvm-17/bin/llvm-link -S -o linked.ll katana_*.ll
/usr/lib/llvm-17/bin/llc -filetype=obj -relocation-model=pic -o linked.o linked.ll
gcc -O0 -o arrtest driver.c linked.o /home/ubuntu/repos/mumei/runtime/mumei_runtime.c -lpthread
./arrtest; echo "exit=$?"
```

Notes and gotchas:
- This box has **no `clang`**; `src/linker.rs` falls back to `cc`/`gcc`, and manual IR linking must
  go through `llc` first. `-relocation-model=pic` is what dodges the PIE relocation failure.
- If linking reports an undefined symbol from the string/std helpers (e.g. `mumei_str_eq`), add a
  tiny C shim in the driver TU rather than pulling in more of the runtime.
- For array capture inside tasks, assert on the IR names as well:
  `task_arg_<name>_len_ptr` / `task_arg_<name>_data_ptr` (stores in the parent),
  `task_capture_<name>_len` / `task_capture_<name>_data` (loads in the pthread wrapper), and a
  `getelementptr i64, ptr %task_capture_<name>_data` for element access.
- Have the fixture *write* an element (`task { arr[0] = 777; arr[1] } `) and check the driver's own
  buffer afterwards — that is the strongest proof the task touched the parent's storage.
- Struct field access inside a task body (`p.x`) may fail with
  `Codegen Error: Field 'a' not found on 'p'`; if so, that scenario is not testable through the CLI
  yet — fall back to a struct-by-value build check and report it as untested.

## Cleanup

Keep generated fixtures and binaries outside the repo, preferably under `/tmp` or `/home/ubuntu/mumei-*-artifacts`. Before finishing, run:

```bash
git -C /home/ubuntu/repos/mumei status --short
```

Expected assertion: repo status is clean except for intentional source changes from the task under test.
