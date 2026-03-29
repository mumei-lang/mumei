#!/bin/bash
# =============================================================
# Verified Microservice: Build Script
# =============================================================
# Verify and build the payment module with C header generation.
#
# Usage:
#   bash examples/verified_microservice/build.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PAYMENT_MM="$SCRIPT_DIR/payment.mm"

echo "=== Verified Microservice Build ==="
echo

# Step 1: Verify payment.mm
echo "[1/2] Verifying payment.mm ..."
mumei verify "$PAYMENT_MM"
echo "  Verification: OK"
echo

# Step 2: Build with C header emission
echo "[2/2] Building with --emit c-header ..."
mumei build "$PAYMENT_MM" --emit c-header
echo "  Build: OK"
echo

echo "=== Build Complete ==="
echo "Generated files:"
echo "  - katana.h (C header with @pre/@post Doxygen annotations)"
echo
echo "To use from Python:"
echo "  1. Compile to shared library: gcc -shared -fPIC -o payment.so katana.c"
echo "  2. Run: python examples/verified_microservice/demo_ffi.py ./payment.so"
