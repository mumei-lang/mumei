# Verified Microservice Demo

> A "logic fortress" pattern: payment calculations and RBAC verified at compile time by Z3, exposed to Python/C via FFI.

## Motivation

Traditional microservices rely on runtime validation — input checks, unit tests, and integration tests that can miss edge cases. The **logic fortress** pattern inverts this: critical business logic is written in mumei with formal contracts, verified by Z3 at compile time, and then called from any language via FFI.

This ensures:
- **No overflow**: `calc_subtotal` bounds are proven before compilation
- **No negative totals**: `calc_total` guarantees `result >= 0`
- **Access control at compile time**: RBAC roles are verified by Z3, not runtime checks

## Architecture

```
┌─────────────────────────────────────────────────┐
│  Python / Rust / C Application                  │
│  (calls verified functions via FFI)             │
└──────────────────┬──────────────────────────────┘
                   │ ctypes / extern "C"
                   ▼
┌─────────────────────────────────────────────────┐
│  Compiled mumei binary (.so / .dll)             │
│  (all contracts verified by Z3 at build time)   │
└──────────────────┬──────────────────────────────┘
                   │ mumei build --emit c-header
                   ▼
┌─────────────────────────────────────────────────┐
│  payment.mm / rbac.mm                           │
│  (source with requires/ensures contracts)       │
└─────────────────────────────────────────────────┘
```

## Files

| File | Description |
|------|-------------|
| `payment.mm` | Verified payment atoms: `calc_subtotal`, `calc_tax`, `calc_total` |
| `rbac.mm` | Role-based access control with capability security effects |
| `demo_ffi.py` | Python FFI demo using ctypes |
| `build.sh` | Build script: verify → emit c-header |
| `README.md` | This file |

## Quick Start

### 1. Verify

```bash
mumei verify examples/verified_microservice/payment.mm
mumei verify examples/verified_microservice/rbac.mm
```

### 2. Build

```bash
bash examples/verified_microservice/build.sh
```

### 3. Demo (simulated mode — no compiled binary needed)

```bash
python examples/verified_microservice/demo_ffi.py
```

### 4. Demo (real FFI — requires compiled binary)

```bash
# After building, write a C implementation using the generated katana_*.h headers,
# then compile to a shared library:
gcc -shared -fPIC -o payment.so payment_impl.c
python examples/verified_microservice/demo_ffi.py ./payment.so
```

## Payment Atoms

| Atom | Parameters | Key Contract |
|------|-----------|-------------|
| `calc_subtotal` | `price`, `quantity` | `ensures: result == price * quantity && result >= 0` |
| `calc_tax` | `amount`, `tax_rate_pct` | `ensures: result == amount * tax_rate_pct / 100` |
| `calc_total` | `price`, `quantity`, `tax_rate_pct` | `ensures: result >= 0` |

## RBAC Model

Roles and resource levels are integers:

| Level | Role | Resource |
|-------|------|----------|
| 0 | Guest | Public |
| 1 | User | Internal |
| 2 | Admin | Confidential |
| 3 | SuperAdmin | Restricted |

Access is granted when `user_role >= resource_level`, enforced at compile time via the `SafeDataAccess` effect.

## Related

- [mumei Standard Library: libc.mm](../../std/libc.mm) — Verified C library wrappers (SI-2)
- [Capability Security Demo](../capability_demo.mm) — Effect-based security patterns
- [Cross-Project Roadmap](../../docs/CROSS_PROJECT_ROADMAP.md) — Strategic Initiatives overview
