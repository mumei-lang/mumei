# Toolchain installer (`mumei setup`) end-to-end testing

Read this when changes touch `src/setup.rs` (Z3/LLVM pins, archive suffixes, glibc detection, env-script generation, installation checks) or the `$z3Version` pin in `.github/workflows/release.yml`.

## Toolchain installer (`mumei setup`) end-to-end testing

Use this flow when changes touch `src/setup.rs` (Z3/LLVM version pins, `Z3Build`
archive suffixes, `select_z3_build`, `detect_host_glibc`/`parse_glibc_version`,
`generate_env_script`, `verify_installation`) or the `$z3Version` pin in
`.github/workflows/release.yml`.

`mumei setup` writes the **real** `~/.mumei`, which also holds toolchains other
workflows use. Check whether it exists first and restore it afterwards (on a
fresh box it is usually absent — then just `rm -rf ~/.mumei` at the end).

### Sandbox runs with `$HOME` instead of touching the real toolchain

`mumei setup` ignores `MUMEI_HOME` (only `scripts/install.sh` honours it):
`manifest::mumei_home()` is `dirs::home_dir()/.mumei`, and `dirs::home_dir()`
honours `$HOME`, so each adversarial case can get its own throwaway home:

```bash
mkdir -p /home/$USER/mumei-home-case/.mumei/toolchains
# hardlink-copy the already-downloaded LLVM so the ~700MB download is skipped
cp -al ~/.mumei/toolchains/llvm-18.1.8 /home/$USER/mumei-home-case/.mumei/toolchains/
HOME=/home/$USER/mumei-home-case ./target/debug/mumei setup
```
Do one real-`$HOME` run first so a real LLVM dir exists to copy; otherwise
every sandbox case re-downloads hundreds of MB. Use `cp -al`, never a symlinked
directory — the installer may `remove_dir_all` the tree, and see the hardlink
rationale below.

### Forcing any libc verdict by shadowing `ldd` on PATH

`detect_host_glibc()` shells out to `Cmd::new("ldd").arg("--version")`, resolved
through PATH, so a stub script is the cheapest way to reach branches this host
can't reach naturally:

```bash
mkdir -p /tmp/fakebin && printf '#!/bin/sh\necho "ldd (GNU libc) 2.17"\n' > /tmp/fakebin/ldd
chmod +x /tmp/fakebin/ldd
HOME=/tmp/mumei-home-old PATH=/tmp/fakebin:$PATH ./target/debug/mumei setup
```
Cases worth covering: musl-style output (`musl libc (x86_64)`) → undetectable
libc branch; a version below every archive floor (2.17) → host rejected, and
assert **no** `z3-*` dir is created and the generated env exports no
`Z3_SYS_Z3_HEADER`/`Z3_SYS_Z3_LIB_DIR`/`CPATH`/`LD_LIBRARY_PATH`; and a *lying*
high version (2.40 on an older host) → forces the newest archive, which then
fails to exec.

### Assert env-script paths exist, don't just eyeball them

Upstream Z3 ZIPs ship `libz3.{so,dylib,a}` in **`bin/`**, not `lib/` — true for
4.13.4, 4.14.1 and 5.1.0 alike — so any `<z3>/lib` in the generated env is a
dead path. Source the script and stat everything:

```bash
bash -c '. ~/.mumei/env
  for p in "$Z3_SYS_Z3_HEADER" "$Z3_SYS_Z3_LIB_DIR" "$Z3_SYS_Z3_LIB_DIR/libz3.so"; do
    [ -e "$p" ] && echo "EXISTS  $p" || echo "MISSING $p"; done'
```
The script appends to `$CPATH`/`$LDFLAGS` etc. On branches containing #524 every
such export uses `${VAR:-}`, so sourcing under `set -u` must succeed; on builds
without #524 (which is merged into `develop`, so only older tags/branches)
sourcing dies with `CPATH: parameter not set`, which is expected there.

### Prove the loader actually picks the toolchain libz3

Distro Z3 and toolchain Z3 differ in version, which makes a decisive probe:

```bash
cat > /tmp/probe_z3.py <<'PY'
import ctypes
lib = ctypes.CDLL("libz3.so")
lib.Z3_get_full_version.restype = ctypes.c_char_p
print("version:", lib.Z3_get_full_version().decode())
print("resolved:", [l.split()[-1] for l in open("/proc/self/maps") if "libz3" in l][0])
PY
env -u LD_LIBRARY_PATH python3 /tmp/probe_z3.py     # expect the system libz3
bash -c '. ~/.mumei/env; python3 /tmp/probe_z3.py'  # expect the toolchain libz3
```
Running only the second half proves nothing — without the unset-baseline you
cannot tell a working loader export from a coincidence.

### Validating archive-name pins without installing anything

Every `(version, arch, suffix)` combination the code can emit should be
HEAD-checked, **plus a deliberately wrong suffix as a negative control** (GitHub
returns 404 for missing assets, so a control proves the 200s mean something):

```bash
curl -sIL -o /dev/null -w '%{http_code}\n' \
  https://github.com/Z3Prover/z3/releases/download/z3-5.1.0/z3-5.1.0-x64-glibc-2.39.zip
```
Archive **names** are build-image names and are not the real floor: verify the
actual requirement with `objdump -T <libz3.so> | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1`
(5.1.0's x64 archive is named `glibc-2.39` but only imports up to `GLIBC_2.38`).
Check `GLIBCXX_`/`CXXABI_` too when a host fails to load a lib that glibc alone
says should work.

### Host prerequisites that make a real end-to-end `setup` run possible

Real downloads are fast (Z3 ~50 MB, LLVM ~1 GB, seconds each), so prefer a real
run over stubs — but three host details decide whether it can succeed:

- Put sandbox homes on a **disk-backed** path (e.g. `/home/<user>/...`), not
  `/tmp`: `/tmp` is often a ~4 GB tmpfs and the extracted LLVM tree overflows
  it, producing `tar: ... Cannot write: No space left on device`.
- A minimal `PATH` harness must include **`xz`** (and `gzip`), otherwise
  `extract_tar_xz` dies with `tar (child): xz: Cannot exec`.
- Upstream `clang+llvm-18.1.8-x86_64-linux-gnu-ubuntu-18.04` `bin/llc` needs
  `libtinfo.so.5`, which Ubuntu 22.04+ does not ship (`libtinfo.so.6` is not
  ABI-compatible — symlinking it fails with
  `NCURSES_TINFO_5.0.19991023 not found`). Install it with
  `sudo apt-get install -y libtinfo5` before expecting a healthy LLVM verdict.
  Without it a *correct* installer legitimately reports LLVM unusable.

### PATH harnesses to force network / system-z3 branches deterministically

Build two directories of symlinks and use `env -i HOME=... PATH=...` so the run
is hermetic:

```bash
mkdir -p /tmp/mfb-net /tmp/mfb-nonet
for t in curl unzip tar xz gzip ldd; do
  ln -sf $(command -v $t) /tmp/mfb-net/$t; ln -sf $(command -v $t) /tmp/mfb-nonet/$t
done
rm -f /tmp/mfb-nonet/curl
printf '#!/bin/sh\necho "curl: (6) Could not resolve host" >&2\nexit 6\n' > /tmp/mfb-nonet/curl
chmod +x /tmp/mfb-nonet/curl
```

- Neither dir contains `z3`, so `report_version("Z3 (system)", "z3")` fails →
  the "no bundled and no system Z3" branch. Append `:/usr/bin:/bin` to get a
  working system `z3` instead (that is how you reach "no bundle + system z3 →
  exit 0").
- The failing-`curl` dir is the cheap way to make an install attempt fail
  without waiting on the network, i.e. to force `z3_dir = None`.
- Reuse an already-downloaded `llvm-18.1.8` across sandbox homes with a
  **hardlink copy** (`cp -al <src>/llvm-18.1.8 <home>/.mumei/toolchains/`):
  costs ~no disk, and if the installer decides to `remove_dir_all` the tree
  (it now does when `bin/llc` is unusable) only the links die, never the
  pristine source. Do **not** use a symlinked directory anymore — since
  `llvm_install_is_usable`/force can remove the dir, a symlink both hides the
  real repair behavior and risks deleting the shared master copy.
- A fabricated `llvm-18.1.8/bin/llc` shell stub (`exit 1` vs. `echo` a version)
  is the way to test the LLVM-unusable branch; an `llvm-18.1.8/bin/` with any
  other file but no `llc` covers the "partial install" branch.

### Exit-code and env-file contract (only on branches containing #524)

**Scope:** #524 is merged into `develop`, so this applies to `develop` and
anything branched from it after the merge — but not to older tags/branches
(e.g. 0.6.16), where setup exits 0 even when Z3/LLVM are unusable, partial trees
are accepted as `already installed`, and the env file is not `set -u` safe.
Check which side the build under test is on by grepping `src/setup.rs` for
`z3_install_is_usable`.

On branches containing #524 (`z3_install_is_usable`, `InstallationStatus`) the
exit code is meaningful, so assert on it as well as on the lines:

- Usable bundled Z3 + runnable `llc` → exit 0, `🎉 Setup complete!`.
- No bundled Z3 but working system `z3` on PATH → still exit 0, plus
  `✅ Z3 (system): ...` and `ℹ️  Using system z3 because no bundled toolchain is
  present`; the env file carries
  `# Z3: no bundled toolchain on this host — using the system install` and no
  `Z3_SYS_Z3_*`.
- `llc --version` not runnable, or neither bundled nor system Z3 → exit **1**
  with `❌ Setup incomplete:` and one `   - ...` line per problem.
- A pre-existing partial (`bin/z3` only) or broken (`bin/z3` exits 1) tree must
  never print `already installed`; it is deleted and reinstalled, and if the
  reinstall fails no Z3 paths are exported.
- Every append-style export uses `${VAR:-}`, so
  `env -u CPATH -u LIBRARY_PATH -u LD_LIBRARY_PATH -u LDFLAGS -u CPPFLAGS \
   sh -euc '. <home>/.mumei/env && echo OK'` must print `OK`. Always pair this
  with the control `sh -euc 'echo "$CPATH"'` (must fail `parameter not set`),
  otherwise the `OK` proves nothing.
- `install_llvm` validates a pre-existing dir with `llvm_install_is_usable`
  (`bin/llc` is a file **and** `llc --version` exits 0 with a non-empty first
  line). So a dir missing `bin/llc`, or whose `llc` exits 1, must **not** print
  `✅ LLVM 18.1.8: already installed`; it prints `📦 Downloading LLVM ...` and
  the dir is removed. With the offline-curl harness the run must end in
  `❌ LLVM install failed`, `❌ Setup incomplete:` and exit 1 — never a silent
  success. Pair each failure case with an **online** repair run to prove the
  reinstall really produces a runnable `llc` (exit 0, `installed to ...`).
  A healthy real tree must still fast-path (`already installed`, no
  `Downloading LLVM` line, exit 0).
  Control: the commit before this behavior prints `already installed` on both
  broken fixtures and leaves the broken dir in place, while still exiting 1 —
  so assert on the *stdout lines and the dir state*, not just the exit code.
- Staging is per-process (`z3-{pid}.zip`, `.staging-{pid}-z3`), and a lost
  rename race is tolerated when the destination validates. Two parallel runs
  against one `$HOME` must both exit 0, never print `Failed to move`, and leave
  no `.staging-*` / `*.zip` behind. Sample `ls -a toolchains/` every 0.1-0.2 s
  in a background loop to actually capture the two distinct pid-named artifacts.
  Run the same test with `--force` on a home that already has both toolchains:
  that is the case where the two processes race to `remove_dir_all` the same
  destination, so assert neither prints `Failed to remove` / `Failed to move`
  (`ErrorKind::NotFound` is tolerated) and both exit 0. Budget ~16 GB of free
  disk: two concurrent forced LLVM installs extract two ~7 GB trees plus two
  ~1 GB archives.

A `git worktree` build of the base branch is the strongest control here: before
#524 all of the broken fixtures above exit 0 with `🎉 Setup complete!` and the
generated env fails under `set -u`.

Known rough edge to watch (not a regression): the env file is written *before*
verification, so a run that fails to install LLVM still exits 1 but leaves
`LLVM_SYS_170_PREFIX`/`PATH`/`LDFLAGS`/`CPPFLAGS` pointing at the
now-removed `llvm-18.1.8` dir. Check whether that is intended before reporting
it as a bug.

PATH harness dirs under `/tmp` may vanish between sessions (tmpfs / reboot);
keep them next to the sandbox homes on disk (e.g. `/home/<user>/mst/mfb-net`)
and re-create them at the start of every run.

Since #524 the `cargo-test` pre-commit hook is scoped to
`cargo test --workspace --lib --bins`, which excludes `tests/` integration tests
and completes locally. Older branches run plain `cargo test`, which includes
`tests/test_verify_json_diagnostics.rs` and can hang on
`escalation_candidate_diagnostic_carries_reason` (nonlinear goal vs. Z3's soft
`timeout`; #527 bounds it, but only on Z3 >= 4.14 — a distro Z3 4.8.12 ignores
the interrupt). Read `.pre-commit-config.yaml` on the branch under test rather
than assuming either scope. `cargo test --bin mumei setup::` remains the focused
installer-unit-test command.

### Regression: `verify` must be unaffected by installer changes

`setup.rs` is not on the verification path, but prove it rather than assume:
run all `std/*.mm` + `examples/*.mm` through both the branch binary and a
`git worktree` build of the base, clearing `.mumei`/`.mumei_cache`/
`.mumei_build_cache` (repo root **and** `std/`, `examples/`) before every single
file, then diff the two `<file> exit=<code>` tables. On 0.6.16 twelve files fail
identically on both sides — including `examples/libc_demo.mm` with exit **101**
(a panic, not a verification failure) — so "some files fail" is expected; only a
*difference* between the tables is a regression.
