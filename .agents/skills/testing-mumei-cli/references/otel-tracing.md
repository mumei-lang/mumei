# OpenTelemetry / TRACEPARENT tracing checks

Read this when changes touch the `otel` feature, `src/telemetry.rs`, span instrumentation, or TRACEPARENT propagation from mumei-agent.

### OpenTelemetry / TRACEPARENT Distributed Tracing

Use this flow when changes touch `src/telemetry.rs`, `Cargo.toml` `otel` feature, `#[cfg(feature = "otel")]` span instrumentation in `verify.rs` or `executor.rs`, or the Python-side `current_traceparent()` / `_env_with_traceparent()` in `mumei-agent`.

**Zero-cost check** — confirm `opentelemetry` is absent from the default dep tree:
```bash
cargo tree --edges no-dev | grep -i opentelemetry
# Expected: no output (empty)
```

**Both build targets compile**:
```bash
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build
LLVM_SYS_170_PREFIX=/usr/lib/llvm-17 LIBCLANG_PATH=/usr/lib/x86_64-linux-gnu cargo build --features otel
```

**Output equivalence** — clear the verification cache before each run to avoid false diffs from cache hits:
```bash
rm -rf .mumei_cache .mumei_build_cache .mumei examples/import_test/lib/.mumei_cache examples/import_test/lib/.mumei_build_cache examples/import_test/lib/.mumei
./target/debug/mumei verify examples/import_test/lib/math_utils.mm > /tmp/out-default.txt 2>/dev/null
# rebuild with --features otel, clear cache again, run again
diff /tmp/out-default.txt /tmp/out-otel.txt
# Expected: no diff
```

**OTEL_ENABLED=true graceful degradation** (no collector running):
```bash
OTEL_ENABLED=true ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, normal verification output, no crash
```

**Valid TRACEPARENT accepted**:
```bash
OTEL_ENABLED=true TRACEPARENT="00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01" \
  ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, output unchanged vs without TRACEPARENT
```

**Invalid TRACEPARENT ignored**:
```bash
OTEL_ENABLED=true TRACEPARENT="invalid-garbage-value" \
  ./target/debug/mumei verify examples/import_test/lib/math_utils.mm
# Expected: exit 0, no crash, no panic, verification succeeds normally
```

**Python-side no-op** (in mumei-agent repo):
```bash
OTEL_ENABLED=false uv run python3 -c "from agent import telemetry; assert telemetry.current_traceparent() is None"
OTEL_ENABLED=false uv run python3 -c "from agent.mumei_client import MumeiClient; assert MumeiClient._env_with_traceparent() is None"
```
