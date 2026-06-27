#!/usr/bin/env bash
# =============================================================================
# devops/tests/pill_launcher_tests.sh — Comprehensive Pill Launcher test suite
# =============================================================================
#
# Runs ALL tests: basic smoke tests, every example build (native + WASM),
# CI checks (fmt, clippy), and the city benchmark.
#
# This is the script you want for a full pre-release validation.
# For day-to-day development, use `basic_tests.sh` instead.
#
# Usage:
#   bash devops/tests/pill_launcher_tests.sh all       # everything
#   bash devops/tests/pill_launcher_tests.sh quick     # fast checks only
#   bash devops/tests/pill_launcher_tests.sh full      # everything incl. benchmarks

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"

# ---------------------------------------------------------------------------
# Source the component test scripts
# ---------------------------------------------------------------------------
# shellcheck source=./basic_tests.sh
source "$SCRIPT_DIR/basic_tests.sh"
# shellcheck source=./examples_tests.sh
source "$SCRIPT_DIR/examples_tests.sh"

# ---------------------------------------------------------------------------
# Full suite runners
# ---------------------------------------------------------------------------

run_quick_suite() {
    echo "=== Pill Launcher — Quick Suite ==="
    test_basics
    test_create
    test_cargo
    test_check_code
    test_assets
    test_docs
    # Build just the cube example (fastest)
    build_native_example "examples/cube"
}

run_full_suite() {
    echo "=== Pill Launcher — Full Suite ==="
    run_quick_suite
    echo ""
    # All native + standalone examples
    build_all_native
    echo ""
    # All WASM examples
    build_all_wasm
    echo ""
    # CI checks
    ci_fmt
    ci_clippy
    echo ""
    # Performance benchmark
    echo "=== Benchmark ==="
    ci_benchmark
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

case "${1:-all}" in
    all|full)
        run_full_suite
        ;;
    quick)
        run_quick_suite
        ;;
    *)
        echo "Usage: $0 [all|full|quick]"
        echo ""
        echo "  all / full — everything: basic tests, all examples, CI checks, benchmark"
        echo "  quick      — fast subset: basic tests + cube build"
        exit 1
        ;;
esac

print_summary
