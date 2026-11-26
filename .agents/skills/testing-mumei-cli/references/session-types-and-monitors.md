# Session types, runtime monitors and capability declarations

Read this when changes touch multi-file `--cross-spec-files` session-type checks, bounded session analysis, `--emit runtime-monitor`, or `type X = capability E(..) where C;` declarations.

### Session-Type Protocol Checks (multi-file, `--cross-spec-files`)

Use this flow when changes touch `mumei-core/src/cross_spec/session_types.rs`,
`CrossSpecVerifier::verify_all()`, or `src/pipeline.rs` (`load_cross_spec_files`,
`annotate_atom_source_file`).

Argument order is non-obvious and easy to get wrong:

```bash
cd /home/ubuntu/repos/mumei
rm -rf /tmp/sess && mkdir -p /tmp/sess
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei verify \
  --report-dir /tmp/sess \
  --cross-spec-files tests/fixtures/session_types/order_server.mm \
  tests/fixtures/session_types/order_client.mm
```

- Extra `.mm` files go **after** `--cross-spec-files`, **comma-separated only**
  (`--cross-spec-files a.mm,b.mm`, or one `--cross-spec-files` per file). Space
  separation does not work: the second path is parsed as a positional argument
  and `mumei verify` fails with `error: unexpected argument`.
- The **primary** file must come **last** as the positional argument.
- `--report-dir` is what makes `cross_spec.json` appear. Without it there is no
  machine-checkable output.
- Results live in `cross_spec.json` under `session_protocol_violations` and
  `summary.session_protocol_violation_count`. Each violation makes
  `mumei verify` exit nonzero; in `mumei build` cross-spec verification it exits 1.

Violation kinds and the conditions that trigger them (useful for writing fixtures):

| kind | trigger |
| --- | --- |
| `duality_mismatch` | a send's post-state has no receiving atom **in a different file** (a continuation in the sender's own file does NOT satisfy duality) |
| `unreachable_receive` | a role's pre-state is not reachable by BFS from the effect's `initial` state (this catches disconnected role "islands" that only feed each other) |
| `deadlock_no_progress` | no reachable state is quiescent |

Gotchas that silently produce "no violations" (verify these before calling a
detector broken, and before calling a clean run a real pass):
- The effect is skipped entirely when it has more than `MAX_PROTOCOL_NODES` (32)
  states, more than `MAX_PROTOCOL_ROLES` (64) roles, fewer than 2 roles, or roles
  spanning fewer than 2 files. A >32-state protocol gets **no** session checking
  at all — expected-by-design, but it means genuine violations in large protocols
  are suppressed. Limit-based skips are visible: check
  `session_analysis_skips[]` / `summary.session_analysis_skipped_count` in
  `cross_spec.json` and the `Warning: session protocol not checked: ...` CLI
  line (role/file-count skips are not reported).
- Roles whose atom has no source-file attribution are skipped.
- Single-file protocols are intentionally never reported here; temporal behavior
  inside one file is the Temporal Effect Verifier's job.
- **Clear caches before every run** (`rm -rf .mumei .mumei_cache .mumei_build_cache`
  beside every source file involved) or you get `skipped (unchanged, cached)` and a
  false pass.

To prove a reachability/duality change actually altered behavior, diff the same
fixtures across two binaries using a git worktree of the base commit:

```bash
git worktree add -f /tmp/base <base-commit>
(cd /tmp/base && LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 \
   LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build)   # ~1 min incremental
# run identical commands with /tmp/base/target/debug/mumei vs ./target/debug/mumei
git worktree remove --force /tmp/base                      # keeps repo clean
```

### Proof-Aware Runtime Monitors (`--emit runtime-monitor`)

Use this flow when changes touch `mumei-emit-monitor/`,
`mumei-core/src/trust_boundary.rs`, or emitter dispatch in `src/codegen.rs`.

```bash
mkdir -p /tmp/mon
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu \
  ./target/debug/mumei build tests/fixtures/runtime_monitor/trusted_boundary.mm \
  --emit runtime-monitor --output /tmp/mon/out
```

- Files are written as `<output-base>_<atom>.monitor.rs`. Monitors are emitted
  **only** for trust-boundary atoms; a proven pure atom must produce **zero**
  files (zero-cost). The `boundary:` tag is a `+`-joined combination, e.g.
  `trusted_atom`, `trusted_atom+extern_ffi`, `effect_pre_override`.
- The three boundary kinds to cover with fixtures: a `trusted atom`, an atom
  backed by an `extern "Rust" { ... }` declaration of the same name, and an atom
  with a non-empty `effect_pre`.
- Strongest selectivity test: put a `trusted atom` and a pure atom in **one**
  file and assert exactly one monitor is produced.

Compiling a generated monitor standalone — note the filename contains dots, so
rustc cannot infer a crate name and you must pass `--crate-name`:

```bash
rustc --edition 2021 --crate-type lib -D warnings \
  --crate-name mon_check -o /tmp/mon.rlib /tmp/mon/out_read_sensor.monitor.rs
```

Generated code must contain no `panic!`/`assert!` and must read `OTEL_ENABLED`
(NoOp when unset/false) and `OTEL_EXPORTER_OTLP_ENDPOINT`
(default `http://localhost:4318`).

To exercise a monitor at **runtime** (much stronger than grepping the source),
`include!` the unmodified generated file inside its own module in a small driver
and link a stub for the wrapped atom. The module wrapper is required: the
generated `extern "C" { fn <atom>(..) }` declaration otherwise collides with your
stub definition at name resolution instead of resolving at link time.

```rust
mod monitor { include!("/tmp/mon/out_mon_send.monitor.rs"); }
use monitor::{mon_send_monitored, mumei_monitor};
#[no_mangle] pub extern "C" fn mon_send(x: i64) -> i64 { x }
fn main() {
    mumei_monitor::set_effect_state_probe(|e| (e == "MonChannel").then(|| "Sent".to_string())).ok();
    mon_send_monitored(-5); // violations go to the hook, or stderr by default
}
```

Then assert: with `OTEL_ENABLED` unset stderr is empty; with `OTEL_ENABLED=true`
violations print as `mumei.monitor.contract_violation atom=... contract=... observed=...`;
`effect_pre` violations are only reported when an effect-state probe is installed
(without a probe the state is unobservable and nothing is reported).

Zero-cost dependency gate — must print nothing:
```bash
cargo tree --edges no-dev | grep -i opentelemetry
```

## Session-type protocol checks (`--cross-spec-files`)

The flows in this section and the two that follow need the P22/P23 Session
Types and Proof-Aware Observability work (mumei-lang/mumei#499); on a branch
without it there is no `session_protocol_violations`, no
`session_analysis_skips`, and no `runtime-monitor` emit target.

Cross-file session protocol violations (`duality_mismatch`,
`unreachable_receive`, `deadlock_no_progress`) are produced by `mumei verify`
when the peer files are passed explicitly. `verify` always writes
`cross_spec.json` — into the current working directory by default, or into
`--report-dir` when given, which is what keeps runs from clobbering each other:

```bash
rm -rf .mumei .mumei_cache .mumei_build_cache   # warm caches print "skipped (unchanged, cached)"
mumei verify --report-dir /tmp/rd \
  --cross-spec-files protocol.mm,server.mm client.mm
```

`--cross-spec-files` accepts **comma-separated** paths or repeated flags only.
Space-separated extra paths become positionals and fail with
`error: unexpected argument`. The primary file goes last.

Each violation counts as a failure, so `verify` exits nonzero. Three conditions
silently yield "no violations" and are easy to mistake for a pass: states > 32,
fewer than 2 roles, or fewer than 2 distinct source files. Single-file protocols
are intentionally not reported (that is the Temporal Effect Verifier's job), and
so is a protocol whose peer roles are different atoms in the same file — that is
what a verified library owning both ends looks like (`std/ownership.mm`).

## Bounded session analysis: skips are reported, not silent

When an effect exceeds a bound the analysis skips it and says so, rather than
failing open silently. `cross_spec.json` gains `session_analysis_skips[]`
(`effect`, `reason`, `state_count`, `role_count`, `limit`, `message`) plus
`summary.session_analysis_skipped_count`, and the CLI prints
`Warning: session protocol not checked: ...` once per skipped effect.

- `state_limit_exceeded` — `states > MAX_PROTOCOL_NODES` (32); `role_count` is
  `null` because roles are never collected.
- `role_limit_exceeded` — `roles > MAX_PROTOCOL_ROLES` (64); `role_count` is
  populated. To build one, keep states small (e.g. 3) and generate >64 atoms
  declaring `effect_pre`/`effect_post` for the same effect across two files.

Skips are warnings, never failures: exit stays 0. Genuine violations remain hard
errors. Skips are deduplicated by **unqualified** effect name, so an effect
visible both as `Channel` and `protocol::Channel` warns once — but two
differently-named effects each warn, so don't assume "one warning" in general.
Two files that both declare `effect Channel` also collapse to one entry in the
module environment (`effect_defs` is keyed by name), upstream of this dedup.

## Session violations through `mumei build`

`mumei build` has **no** `--report-dir`, `--cross-spec-files`, or
`--cross-spec-verify` flags — cross-spec verification is manifest-driven
(`ProofConfig::cross_spec_verify`, default on) and it sees peer files only
through `import`. `cross_spec.json` lands in the resolved output directory (the
parent of `--output`), falling back to the working directory, so a build run
with `--output /tmp/out/out` leaves it at `/tmp/out/cross_spec.json` — don't
search the repo root for it.

Role collection drops any atom without `source_file` attribution
(`session_types.rs::collect_roles`). `pipeline.rs::annotate_source_file` covers
only the primary input, so imported atoms rely on
`resolver/imports.rs` attributing them to the resolved import path; a fixture
whose entry file merely `import`s both roles is therefore the way to exercise
build-path enforcement, and it must exit 1 with
`❌ Session protocol violation (...)` on stderr. If a build-path run comes back
"clean", cross-check the same files through `verify --cross-spec-files` before
believing it — a silently unattributed role looks exactly like a pass.

## Proof-aware runtime monitors (`--emit runtime-monitor`)

```bash
mumei build fixture.mm --emit runtime-monitor --output /tmp/out/out
```

Monitors are emitted only for trust-boundary atoms (`trusted` atoms, extern/FFI
atoms, atoms with `effect_pre`) as `<base>_<atom>.monitor.rs`; proven pure atoms
must produce no file at all. Note that "not every atom emits a monitor" is a bad
assertion for files like `std/ownership.mm` where every atom carries
`effect_pre` — assert the exact expected set instead.

To compile a generated monitor standalone you must pass `--crate-name`, because
the `<base>_<atom>.monitor.rs` filename contains dots and rustc cannot infer a
crate name from it:

```bash
rustc --edition 2021 --crate-type lib -D warnings \
  --crate-name mon_probe -o /tmp/mon.rlib /tmp/out/out_read_sensor.monitor.rs
```

To exercise a monitor at runtime, `include!` it inside a `mod` wrapper and
provide the extern symbol it wraps, otherwise the stub collides with the
generated function. Behaviour depends on env vars: with `OTEL_ENABLED` unset the
monitor is a true no-op (zero bytes on stderr); with `OTEL_ENABLED=true`
violations are reported via hook/stderr and it never panics.
`OTEL_EXPORTER_OTLP_ENDPOINT` defaults to `http://localhost:4318`. `effect_pre`
violations are only observable after installing
`mumei_monitor::set_effect_state_probe(...)`; without a probe the state is
unobservable and nothing is reported.

## Capability declarations (`type X = capability E(..) where C;`)

Use this flow when changes touch `parse_capability_def` / `resolve_capability_params`
in `mumei-core/src/parser/item.rs`, `TypeRef::carries_effects()` in `ast.rs`, the
capability-constraint branch of `verification/translator/expr.rs`, or the
`Item::CapabilityDef` arms in `src/pipeline.rs` / `commands/check.rs` /
`commands/build.rs`.

Fixtures: `tests/test_capability_stage1.mm` (3 atoms, all must verify) and
`tests/test_capability_stage1_missing_effect.mm` (must exit 1 with
`Effect polymorphism violation: ... accepts capability parameter 'cap' with effect
[SafeFileRead]`, `report.json` `failure_type == "effect_not_allowed"`).

- `mumei check` / `mumei build` print `🔑 Capability: '<Name>' (effect: <Effect>)`.
  That line is the cheapest proof the declaration parsed as a capability rather
  than a refined type.
- BEFORE/AFTER discriminator: on a build without the feature the same file fails
  with `failed to lower refinement type 'FileCap': Unknown function: <Effect>`
  (the `type X = ...` is parsed as a refinement), so a base-binary run via
  `git worktree add -f /home/ubuntu/mumei-base origin/develop` is a strong check.
- To prove the capability `where` constraint is really threaded into Z3, declare
  the underlying effect **without** a constraint and put the constraint only on
  the capability:
  `effect LooseFileRead(path: Str);` +
  `type StrictCap = capability LooseFileRead(path: Str) where starts_with(path, "/tmp/");`
  A body with `let path = "/etc/passwd"; perform cap.read(path);` must fail with
  `Verification Error: Contradiction found.`, while `"/tmp/ok.log"` verifies.
  Without the constraint plumbing both cases verify identically.
- Known limitation (pre-existing for plain effect constraints too, reproducible on
  `origin/develop` with a constrained `effect`): a fully symbolic, unconstrained
  argument (`perform cap.read(user_path)` where `user_path: Str` has no `requires`)
  **verifies** — constant/derived paths are checked, free symbolic ones are not.
  Do not report this as a capability regression; compare against the equivalent
  `perform <Effect>.op(sym)` form first.
- String-literal safety of the `perform cap.op` textual rewrite is best proven
  through emitted IR, not stdout: build with `--emit llvm-ir` and grep the `.ll`
  for the literal, e.g. `grep -o 'c"[^"]*"' out_<atom>.ll` must still contain
  `c"perform cap.read(/tmp/x)\00"` (never `perform <Effect>.read`).
- Two capability types over the **same** effect with different constraints are
  conjoined, so such an atom fails with `Contradiction found` even when only one
  capability is performed. This is a documented Stage 1 over-restriction.
- `capability` is contextual: only a keyword directly after `type X =`. Regression
  fixture worth keeping: an atom with params literally named `capability` and
  `grant`, plus `type capability_alias = i64 where v >= 0;` — must verify.
- `mumei build --output <dir>/<base>` fails with
  `Codegen Error: No such file or directory` when `<dir>` does not exist **and**
  verification was served from cache (`Verification: Skipped (unchanged, cached)`).
  Pre-existing on `origin/develop`; `mkdir -p` the output dir before building.
