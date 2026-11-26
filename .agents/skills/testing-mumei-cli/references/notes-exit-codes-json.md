# General notes and the exit-code/JSON contract

Read this when asserting `verify` exit codes or `--json` output, testing benchmark verdict classification, or comparing two binaries' output.

## Notes

### Exit-code/JSON contract testing

- `verify` exits 0 verified, 1 rejected, 2 usage, 3 inconclusive/unverifiable,
  4 input error, 5 internal error (`VerifyOutcome` in `src/commands/verify.rs`,
  table in `docs/CLI.md`; `benchmarks/run_benchmarks.py::classify_exit_code`
  derives verdicts from the exit code alone). Do not assume every nonzero code
  means rejection. Older examples in this skill that expect 1 for
  `unverifiable` predate this contract.
- Parse the **entire stdout** with `json.loads`, and compare its `exit_code`
  against the actual process return code. Do not extract the first JSON-looking
  substring: combinations such as `--json --emit loss-vector` or
  `--json --emit structured-feedback` may produce multiple documents. Test
  usage errors separately since clap can exit before JSON handling.
- Check status vocabulary on both fresh and warm caches: per-atom reports may
  use `success` while aggregate summaries use `passed`. Avoid treating them as
  interchangeable when the task explicitly tests the documented schema.
- Malformed source is not necessarily a reliable input-error fixture: the
  parser can recover from incomplete syntax. Assert malformed cases directly,
  and use an unresolved import or unreadable file as an independent exit-4
  control. For artifact-write failure, chmod a destination directory to 0555
  under a non-root user, or use a regular file as the destination's parent.
- The flag for peer files is `--cross-spec-files` (plural). Verify any desired
  singular alias through `--help` rather than assuming it exists.
- Benchmark CLI may have no subset option. Import
  `benchmarks/run_benchmarks.py` and call `run_category_benchmarks` with a small
  fixture directory and the absolute freshly-built binary path. Set the
  module's `REPO_ROOT` to a sandbox working directory to isolate emitted
  artifacts; pass `lean_bridge=None` to avoid unrelated Lean work. This still
  invokes real CLI subprocesses. Repeat with a wrapper that discards stdout
  and stderr but returns the real CLI exit status to prove verdict
  classification does not depend on human-readable summary text.

- Prefer using absolute `LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu` env vars for all cargo/CLI commands in this repo.
- No browser recording is useful for shell-only CLI flows; collect command output and generated JSON instead.
- Mumei verification commands may emit `cross_spec.json` in the current working directory; delete temporary copies before final `git status`.
- When comparing verify output between two binaries, always clear `.mumei_cache` / `.mumei_build_cache` / `.mumei` directories before each run — the verification cache causes "skipped (unchanged, cached)" lines that create false diffs.
