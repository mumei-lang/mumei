#!/usr/bin/env python3
"""Verified Microservice FFI Demo.

Demonstrates calling verified mumei payment functions from Python via ctypes.

Prerequisites:
    1. Verify the payment module:
       mumei verify examples/verified_microservice/payment.mm

    2. Build the shared library with C header:
       mumei build examples/verified_microservice/payment.mm --emit c-header

    3. Compile the generated C to a shared library (example with gcc):
       gcc -shared -fPIC -o payment.so katana.c  # actual compilation may vary

Usage:
    python examples/verified_microservice/demo_ffi.py
"""
from __future__ import annotations

import ctypes
import os
import sys


def load_payment_library(lib_path: str = "./payment.so") -> ctypes.CDLL:
    """Load the compiled mumei payment shared library.

    Args:
        lib_path: Path to the compiled shared library (.so/.dll).

    Returns:
        ctypes.CDLL handle to the library.

    Raises:
        FileNotFoundError: If the library file does not exist.
    """
    if not os.path.exists(lib_path):
        print(f"Library not found: {lib_path}")
        print()
        print("To build the payment library:")
        print("  1. mumei verify examples/verified_microservice/payment.mm")
        print("  2. mumei build examples/verified_microservice/payment.mm --emit c-header")
        print("  3. Compile the generated C/header files into a .so")
        print()
        print("Running in demo mode with simulated values...")
        return None
    return ctypes.CDLL(lib_path)


def demo_with_library(lib: ctypes.CDLL) -> None:
    """Run the FFI demo using the real compiled library."""
    # Set up function signatures
    lib.calc_subtotal.argtypes = [ctypes.c_int64, ctypes.c_int64]
    lib.calc_subtotal.restype = ctypes.c_int64

    lib.calc_tax.argtypes = [ctypes.c_int64, ctypes.c_int64]
    lib.calc_tax.restype = ctypes.c_int64

    lib.calc_total.argtypes = [ctypes.c_int64, ctypes.c_int64, ctypes.c_int64]
    lib.calc_total.restype = ctypes.c_int64

    # Test cases
    print("=== Verified Payment FFI Demo ===")
    print()

    price, quantity, tax_rate = 1500, 3, 10

    subtotal = lib.calc_subtotal(price, quantity)
    assert subtotal == price * quantity, f"Contract violation: subtotal={subtotal}"
    print(f"calc_subtotal({price}, {quantity}) = {subtotal}")

    tax = lib.calc_tax(subtotal, tax_rate)
    assert tax >= 0, f"Contract violation: tax={tax}"
    print(f"calc_tax({subtotal}, {tax_rate}) = {tax}")

    total = lib.calc_total(price, quantity, tax_rate)
    assert total >= 0, f"Contract violation: total={total}"
    print(f"calc_total({price}, {quantity}, {tax_rate}) = {total}")

    print()
    print("All runtime assertions passed — contracts hold at runtime.")


def demo_simulated() -> None:
    """Run the demo with simulated values (no compiled library needed)."""
    print("=== Verified Payment FFI Demo (Simulated) ===")
    print()

    price, quantity, tax_rate = 1500, 3, 10

    # Simulate the verified functions
    subtotal = price * quantity
    assert subtotal == price * quantity and subtotal >= 0
    print(f"calc_subtotal({price}, {quantity}) = {subtotal}")

    tax = subtotal * tax_rate // 100
    assert tax >= 0
    print(f"calc_tax({subtotal}, {tax_rate}) = {tax}")

    total = subtotal + subtotal * tax_rate // 100
    assert total >= 0
    print(f"calc_total({price}, {quantity}, {tax_rate}) = {total}")

    print()
    print("All simulated assertions passed.")
    print()
    print("Note: In production, these functions are called via ctypes FFI")
    print("from the compiled mumei binary, with contracts verified by Z3.")


def main() -> None:
    """Entry point."""
    lib_path = sys.argv[1] if len(sys.argv) > 1 else "./payment.so"
    lib = load_payment_library(lib_path)

    if lib is not None:
        demo_with_library(lib)
    else:
        demo_simulated()


if __name__ == "__main__":
    main()
