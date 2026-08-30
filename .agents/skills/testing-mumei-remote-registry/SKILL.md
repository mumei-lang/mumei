---
name: testing-mumei-remote-registry
description: Test the opt-in remote package registry end-to-end with the real CLI — building real package + proof-certificate fixtures with `mumei publish`, serving them over loopback HTTP, and exercising transport policy, redirects, path sanitization, size caps, staging/rollback and the no-registry regression path. Use when changes touch `mumei-core/src/registry/remote.rs`, `[registry]` config in `manifest.rs`, `MUMEI_REGISTRY_URL`, remote fallback in `resolver/dependencies.rs`, or `mumei add` remote resolution.
---

# Testing the Mumei remote registry (P24)

Build first (see `testing-mumei-cli` for the full toolchain notes):

```bash
cd /home/ubuntu/repos/mumei
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
M=/home/ubuntu/repos/mumei/target/debug/mumei
```

**Always use an isolated `HOME`** for every case (`HOME=/tmp/p24/homeX $M ...`): the cache lives in
`$HOME/.mumei/packages/<name>/<version>/` and the registry index in `$HOME/.mumei/registry.json`
(`manifest::mumei_home()` = `dirs::home_dir()/.mumei`). A fresh `HOME` per case is the only reliable
way to force a re-fetch; alternatively delete `registry.json` to force re-resolution while keeping
the cached files (useful for stale-cache tests).

## Building real fixtures (do not hand-write certificates)

```bash
cd <pkg-project> && HOME=/tmp/p24/pubhome $M publish   # -> pubhome/.mumei/packages/<n>/<v>/
```

Then lay out the served tree and copy the *generated* certificate:

```
srv/packages/<name>/index.json                        {"latest":"0.1.0","versions":{"0.1.0":{"files":[...],"cert_hash":"<sha256>"}}}
srv/packages/<name>/<version>/<each file in files[]>
srv/packages/<name>/<version>/.proof-cert.json        cp of publish's proof_certificate.json
```

`cert_hash` must be `sha256sum` of the served `.proof-cert.json` **bytes**.

Gotchas:
- A copied certificate keeps the original `package_name`/`package_version`. Reusing `mathpkg`'s cert
  for a fixture named `otherpkg` fails the identity check, so cert-related fixtures silently prove
  nothing. Either publish a real package per fixture name, or expect the identity error.
- Re-serializing the cert JSON (e.g. via `json.dump`) changes its hash — that is exactly how to
  build a hash-mismatch fixture.
- Serve on **loopback only** (`127.0.0.1`/`localhost`): non-loopback plaintext registries are
  rejected unless `MUMEI_REGISTRY_ALLOW_PLAINTEXT=1`.

`cd srv && python3 -m http.server 8123 &` is enough for the static cases; its request log doubles as
evidence of whether the CLI touched the network at all (diff line counts before/after a run).

## A custom server unlocks the hard cases

A small `BaseHTTPRequestHandler` on another port covers what `http.server` cannot:
- **Redirects**: `/rl/<rest>` → 302 `http://127.0.0.1:8123/<rest>` (loopback, must still work, including
  multi-hop chains) and `/rx/<rest>` → 302 `http://192.0.2.10:8123/<rest>` (non-loopback plaintext,
  must be blocked). `192.0.2.0/24` (TEST-NET-1) needs no DNS/`/etc/hosts` and the transport check is
  hostname-based, so the request is never made. A blocked redirect surfaces as
  `GET <url> returned 302 Found`, i.e. a fetch error rather than a silent downgrade.
- **Unknown-length bodies**: reply with `Transfer-Encoding: chunked` and >8 MiB of data to exercise the
  read cap (`... is more than 8388608 bytes`), separate from the `Content-Length` cap
  (`... is <n> bytes (limit 8388608)`), which a static file >8 MiB already covers.

## Case checklist that has caught real bugs

- Adversarial `index.json` `files[]`: `../../evil.mm`, and entries containing `?`, `#`, `%`, `:`, `@`
  → rejected with `rejected file '<x>' (...)`; assert no file lands outside
  `$HOME/.mumei/packages/<name>/`. A **space** is *not* rejected (URL-encoded to `%20` and fetched
  successfully) — if that ever needs to change, this is the test to update.
- Malformed index (`<html>not json</html>`) → `cannot parse <url>: expected value at line 1 column 1`.
- Dead port (`http://127.0.0.1:9`) fails in milliseconds; unroutable host + `[registry] timeout_ms = 1500`
  must return in ~1.5 s — time it with `date +%s%3N` to prove `timeout_ms` is honoured.
- Failure mid-fetch (index lists a file that 404s) with an already-cached version: the published
  cache dir must be byte-identical afterwards and no `.staging-*` / `*.replaced-*` dirs may remain.
- Strict vs non-strict: `--strict-imports` turns missing cert / hash mismatch / missing
  `package_name` into hard exit-1 errors and leaves **no** version dir; non-strict caches the package
  but deletes the unverifiable cert and records `verified: false` with no `cert_path`/`cert_hash`.
- No-registry regression: path deps, git deps (`git = "<local repo path>"` clones offline — no network
  needed) and local published name deps must all resolve, with zero lines added to the fixture
  server logs and zero `remote registry` output.

## Reading exit codes correctly

`mumei add` prints many lines; piping to `head` gives the CLI a `SIGPIPE`/broken-pipe **panic and exit
101**, which looks like a product failure but is not. Capture full output (or `grep`) when the exit
code matters.

Also note: `mumei add <name>` exits **0** even when remote resolution fails (bad URL, malformed index,
rejected file) — it warns and writes `<name> = "*"` into `mumei.toml`. Assert on stderr/warning text
and on cache contents, not on the exit code, for `add` failure cases.

## Devin Secrets Needed

None — everything runs locally on loopback.
