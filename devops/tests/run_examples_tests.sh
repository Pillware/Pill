#!/usr/bin/env bash
# =============================================================================
# devops/tests/run_examples_tests.sh — Build every Pill example project
# =============================================================================
#
# Usage:
#   bash devops/tests/run_examples_tests.sh all               # build everything
#   bash devops/tests/run_examples_tests.sh examples/cube      # build one example
#   bash devops/tests/run_examples_tests.sh native             # native examples only
#   bash devops/tests/run_examples_tests.sh wasm               # WASM examples only
#
# Prerequisites:
#   - PillLauncher binary must be built (auto-detected, or set PILL_LAUNCHER_BIN)
#   - cargo, wasm-pack (for WASM targets)

set -euo pipefail

# ---------------------------------------------------------------------------
# Source shared helpers (binary discovery, report_*, etc.)
# ---------------------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ---------------------------------------------------------------------------
# Example lists
# ---------------------------------------------------------------------------

# Native examples built via `PillLauncher -a build`
NATIVE_EXAMPLES=(
    "examples/cube"
    "examples/floating_pills"
    "examples/italian_brainrot"
    "examples/city"
)

# Standalone Cargo projects (no PillLauncher needed)
STANDALONE_CRATES=(
    "examples/net_minimal/client"
    "examples/net_minimal/server"
)

# WASM examples built via `PillLauncher -a check-wasm`
WASM_EXAMPLES=(
    "examples/cube"
    "examples/pill_tunel"
    "examples/pbr_helmet"
    "examples/pbr_balls"
)

# ---------------------------------------------------------------------------
# Build functions
# ---------------------------------------------------------------------------

# Build a single native Pill example.
build_native_example() {
    local example_path="$1"
    echo "--- Building native: $example_path ---"
    invoke_launcher -a build -p "$example_path" -c debug
}

# Build a standalone Cargo crate (not a Pill project).
build_standalone_crate() {
    local crate_path="$1"
    echo "--- Building standalone: $crate_path ---"
    cargo build --manifest-path "$crate_path/Cargo.toml"
}

# Build and smoke-test a WASM example.
build_wasm_example() {
    local example_path="$1"
    echo "--- Building WASM: $example_path ---"
    invoke_launcher -a check-wasm -p "$example_path"
}

# ---------------------------------------------------------------------------
# Batch runners
# ---------------------------------------------------------------------------

build_all_native() {
    echo "=== Building all native examples ==="
    local failed=0
    for example in "${NATIVE_EXAMPLES[@]}"; do
        if build_native_example "$example"; then
            report_pass "native: $example"
        else
            report_fail "native: $example" "build failed"
            failed=$((failed + 1))
        fi
    done

    for crate in "${STANDALONE_CRATES[@]}"; do
        if build_standalone_crate "$crate"; then
            report_pass "standalone: $crate"
        else
            report_fail "standalone: $crate" "build failed"
            failed=$((failed + 1))
        fi
    done

    return $failed
}

build_all_wasm() {
    echo "=== Building all WASM examples ==="
    local failed=0
    for example in "${WASM_EXAMPLES[@]}"; do
        if build_wasm_example "$example"; then
            report_pass "wasm: $example"
        else
            report_fail "wasm: $example" "build failed"
            failed=$((failed + 1))
        fi
    done
    return $failed
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

case "${1:-all}" in
    all)
        build_all_native
        build_all_wasm
        ;;
    native)
        build_all_native
        ;;
    wasm)
        build_all_wasm
        ;;
    *)
        # Assume it's a path to a single example
        example_path="$1"
        if [ -f "$example_path/Cargo.toml" ]; then
            # Determine type by checking for a res/ directory (Pill project marker)
            if [ -d "$example_path/res" ]; then
                build_native_example "$example_path"
            else
                build_standalone_crate "$example_path"
            fi
        else
            echo "Usage: $0 [all|native|wasm|<example-path>]"
            echo ""
            echo "Native examples:"
            for e in "${NATIVE_EXAMPLES[@]}"; do echo "  $e"; done
            echo ""
            echo "Standalone crates:"
            for c in "${STANDALONE_CRATES[@]}"; do echo "  $c"; done
            echo ""
            echo "WASM examples:"
            for w in "${WASM_EXAMPLES[@]}"; do echo "  $w"; done
            exit 1
        fi
        ;;
esac

print_summary
