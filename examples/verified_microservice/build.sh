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
RBAC_MM="$SCRIPT_DIR/rbac.mm"

echo "=== Verified Microservice Build ==="
echo

# Step 1: Verify payment.mm
echo "[1/3] Verifying payment.mm ..."
mumei verify "$PAYMENT_MM"
echo "  Verification: OK"
echo

# Step 2: Verify rbac.mm
echo "[2/3] Verifying rbac.mm ..."
mumei verify "$RBAC_MM"
echo "  Verification: OK"
echo

# Step 3: Build with C header emission
echo "[3/3] Building payment.mm with --emit c-header ..."
mumei build "$PAYMENT_MM" --emit c-header
echo "  Build: OK"
echo

echo "=== Build Complete ==="
echo "Generated files:"
echo "  - katana_calc_subtotal.h, katana_calc_tax.h, katana_calc_total.h"
echo "    (C headers with @pre/@post Doxygen annotations)"
echo
echo "To use from Python:"
echo "  1. Compile your C implementation against the generated headers into a .so"
echo "  2. Run: python examples/verified_microservice/demo_ffi.py ./payment.so"
