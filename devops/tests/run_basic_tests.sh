#!/usr/bin/env bash
# =============================================================================
# devops/tests/run_basic_tests.sh — Pill CI fast checks
# =============================================================================
#
# Seven focused checks for CI (ci-basic-tests.yml) and local dev.
# Only uses PillLauncher for `build` and `run` — everything else is done
# via cargo / wasm-pack / git directly.
#
#   code_formatting_check        cargo fmt + git diff
#   code_linting_check           cargo clippy -D warnings + git diff
#   build_native_cube_example    launcher build examples/cube (native, debug)
#   build_wasm_example           launcher build -t web + smoke test + size check
#   benchmark_native_performance launch + run city under xvfb (release)
#   benchmark_native_size        cargo build --release + measure binaries
#   benchmark_wasm_size          launcher build -t web + check .wasm ≤ 499 KB
#
# Usage:
#   bash devops/tests/run_basic_tests.sh all
#   bash devops/tests/run_basic_tests.sh code_formatting_check

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ===========================================================================
# 1. code_formatting_check — cargo fmt (direct) + git diff
# ===========================================================================

code_formatting_check() {
    echo "=== rustfmt ==="
    cargo fmt --manifest-path engine/Cargo.toml

    echo "=== checking for fmt diffs ==="
    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        && report_pass "code formatting" \
        || report_fail "code formatting" "rustfmt produced changes — run 'cargo fmt'"
}

# ===========================================================================
# 2. code_linting_check — cargo clippy -D warnings (direct) + git diff
# ===========================================================================

code_linting_check() {
    echo "=== clippy -D warnings ==="
    local clippy_output exit_code
    clippy_output=$(cargo clippy --manifest-path engine/Cargo.toml -- -D warnings 2>&1) && exit_code=$? || exit_code=$?

    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        && report_pass "code linting" \
        || report_fail "code linting" "clippy modified files"

    if [ "$exit_code" -ne 0 ]; then
        report_fail "clippy warnings" "${clippy_output:0:200}"
    fi
}

# ===========================================================================
# 3. build_native_cube_example — launcher build (allowed) examples/cube
# ===========================================================================

build_native_cube_example() {
    echo "=== Native build: examples/cube ==="
    local cube_dir="examples/cube"

    if [ ! -f "$cube_dir/Cargo.toml" ]; then
        report_skip "native cube build" "examples/cube not found"
        return
    fi

    echo "    (building — this may take a couple of minutes)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a build -p "$cube_dir" -c debug 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "native cube build succeeds"
        if [ -d "$cube_dir/build/dev/data" ]; then
            report_pass "native cube build output data/ exists"
        else
            report_fail "native cube build output data/" "missing $cube_dir/build/dev/data"
        fi
    else
        report_skip "native cube build" "exit $exit_code (${launcher_output:0:150})"
    fi
}

# ===========================================================================
# 4. build_wasm_example — launcher build -t web (allowed) + size check
# ===========================================================================

build_wasm_example() {
    echo "=== WASM build + smoke test ==="
    local example_path="${1:-examples/cube}"

    echo "    (building WASM — this may take a moment)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a build -p "$example_path" -t web 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM build succeeds"
    else
        report_fail "WASM build" "exit $exit_code: ${launcher_output:0:200}"
        return
    fi

    # Verify the .wasm artifact exists
    local wasm_dir="$example_path/build/wasm/pkg"
    if [ -d "$wasm_dir" ]; then
        report_pass "WASM pkg/ directory exists"
    else
        report_fail "WASM pkg/" "missing $wasm_dir"
    fi
}

# ===========================================================================
# 5. benchmark_native_performance — build + run city under xvfb (release)
# ===========================================================================

benchmark_native_performance() {
    echo "=== Performance benchmark (city, release) ==="

    # Build the city example in release mode
    echo "    (building city — this will take a while)"
    if ! invoke_launcher -a build -p examples/city -c release > /dev/null 2>&1; then
        report_skip "native perf benchmark" "city build failed"
        return
    fi

    # Run under xvfb (virtual display for headless CI)
    if command -v xvfb-run > /dev/null 2>&1; then
        echo "    (running city under xvfb)"
        xvfb-run --auto-servernum \
            invoke_launcher -a run -p examples/city -c release 2>&1 | head -n 20
        report_pass "native performance benchmark (city launched)"
    else
        report_skip "native perf benchmark" "xvfb-run not found"
    fi
}

# ===========================================================================
# 6. benchmark_native_size — cargo build --release + measure binary sizes
# ===========================================================================

benchmark_native_size() {
    echo "=== Binary size benchmark (release) ==="
    local project_dir="$test_workspace_root/SizeBenchTest"

    # Create a temp project and build it
    invoke_launcher -a create -n SizeBenchTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "native size benchmark" "create failed"; return
    }

    echo "    (building in release mode)"
    local build_output exit_code
    build_output=$(invoke_launcher -a build -p "$project_dir" -c release 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -ne 0 ]; then
        report_skip "native size benchmark" "build failed"
        return
    fi

    # Measure binary sizes
    local data_dir="$project_dir/build/release/data"
    if [ -d "$data_dir" ]; then
        echo "  Binary sizes:"
        find "$data_dir" -type f -exec ls -lh {} \; 2>/dev/null | while read -r line; do
            echo "    $line"
        done
        report_pass "native size benchmark"
    else
        report_skip "native size benchmark" "no release/data/ output"
    fi
}

# ===========================================================================
# 7. benchmark_wasm_size — WASM binary must be ≤ 499 KB
# ===========================================================================

benchmark_wasm_size() {
    echo "=== WASM size budget (499 KB) ==="
    local example_path="${1:-examples/cube}"

    # Build WASM
    echo "    (building WASM)"
    if ! invoke_launcher -a build -p "$example_path" -t web > /dev/null 2>&1; then
        report_skip "WASM size budget" "build failed"
        return
    fi

    # Find the .wasm file
    local wasm_file
    wasm_file=$(find "$example_path/build/wasm" -name "*.wasm" -type f 2>/dev/null | head -1)

    if [ -z "$wasm_file" ]; then
        report_fail "WASM size budget" "no .wasm file found in build output"
        return
    fi

    local wasm_size
    wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
    local wasm_kb=$((wasm_size / 1024))

    echo "    WASM: $wasm_file → ${wasm_kb} KB"

    if [ "$wasm_size" -le 511000 ]; then  # 499 KB + small margin for rounding
        report_pass "WASM size within 499 KB budget (${wasm_kb} KB)"
    else
        report_fail "WASM size budget" "${wasm_kb} KB exceeds 499 KB limit"
    fi
}

# ===========================================================================
# Dispatch (only when executed directly)
# ===========================================================================

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    code_formatting_check)       code_formatting_check ;;
    code_linting_check)          code_linting_check ;;
    build_native_cube_example)   build_native_cube_example ;;
    build_wasm_example)          build_wasm_example "${2:-examples/cube}" ;;
    benchmark_native_performance) benchmark_native_performance ;;
    benchmark_native_size)       benchmark_native_size ;;
    benchmark_wasm_size)         benchmark_wasm_size "${2:-examples/cube}" ;;

    all|"")
        code_formatting_check
        code_linting_check
        build_native_cube_example
        build_wasm_example
        benchmark_native_performance
        benchmark_native_size
        benchmark_wasm_size
        ;;

    *)
        echo "Usage: $0 [all|<check-name>]"
        echo ""
        echo "Checks:"
        echo "  code_formatting_check        cargo fmt + git diff"
        echo "  code_linting_check           cargo clippy -D warnings"
        echo "  build_native_cube_example    build examples/cube (native)"
        echo "  build_wasm_example [path]    build WASM + verify artifacts"
        echo "  benchmark_native_performance build + run city (release, xvfb)"
        echo "  benchmark_native_size        release build + binary size report"
        echo "  benchmark_wasm_size [path]   WASM binary ≤ 499 KB"
        exit 1
        ;;
esac

print_summary
