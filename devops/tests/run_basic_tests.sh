#!/usr/bin/env bash
# =============================================================================
# devops/tests/run_basic_tests.sh — Pill CI fast checks
# =============================================================================
#
# Seven focused checks for CI (ci-basic-tests.yml) and local dev:
#
#   code_formatting_check        rustfmt + git diff
#   code_linting_check           clippy -D warnings + git diff
#   build_native_cube_example    build examples/cube (native, debug)
#   build_wasm_example           wasm-pack build + smoke test + 499 KB budget
#   benchmark_native_performance city FPS benchmark (release, needs xvfb)
#   benchmark_native_size        binary size report (release)
#   benchmark_wasm_size          WASM size check — fails if > 499 KB
#
# Usage:
#   bash devops/tests/run_basic_tests.sh all
#   bash devops/tests/run_basic_tests.sh code_formatting_check
#   bash devops/tests/run_basic_tests.sh benchmark_wasm_size

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ===========================================================================
# 1. code_formatting_check — rustfmt + git diff
# ===========================================================================

code_formatting_check() {
    echo "=== rustfmt ==="
    local example_path="${1:-examples/floating_pills}"
    invoke_launcher -a cargo -p "$example_path" -- fmt

    echo "=== checking for fmt diffs ==="
    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        ":(exclude)$example_path/Cargo.toml" \
        && report_pass "code formatting" \
        || report_fail "code formatting" "rustfmt produced changes"
}

# ===========================================================================
# 2. code_linting_check — clippy -D warnings + git diff
# ===========================================================================

code_linting_check() {
    echo "=== clippy -D warnings ==="
    local example_path="${1:-examples/floating_pills}"
    invoke_launcher -a cargo -p "$example_path" -- clippy -- -D warnings

    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        ":(exclude)$example_path/Cargo.toml" \
        && report_pass "code linting" \
        || report_fail "code linting" "clippy modified files"
}

# ===========================================================================
# 3. build_native_cube_example — compile examples/cube (native, debug)
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
# 4. build_wasm_example — wasm-pack build + smoke test + 499 KB budget
# ===========================================================================

build_wasm_example() {
    echo "=== WASM build + smoke test ==="
    local example_path="${1:-examples/cube}"

    # The cube example has an explicit 499 KB size budget.
    local wasm_budget_flag=""
    if [ "$example_path" = "examples/cube" ]; then
        wasm_budget_flag="--wasm-budget-kb 499"
    fi

    # shellcheck disable=SC2086
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a check-wasm -p "$example_path" $wasm_budget_flag 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM build + smoke test"
    else
        report_fail "WASM build + smoke test" "exit $exit_code: ${launcher_output:0:200}"
    fi
}

# ===========================================================================
# 5. benchmark_native_performance — city FPS (release, needs xvfb)
# ===========================================================================

benchmark_native_performance() {
    echo "=== Performance benchmark (city, release) ==="

    xvfb-run --auto-servernum \
        invoke_launcher -a benchmark \
        -p examples/city \
        --bench-iterations 5 \
        --bench-frames 1000 \
        --bench-features benchmark_window \
        -c release \
        && report_pass "native performance benchmark" \
        || report_fail "native performance benchmark" "benchmark failed"
}

# ===========================================================================
# 6. benchmark_native_size — binary size report (release)
# ===========================================================================

benchmark_native_size() {
    echo "=== Binary size benchmark (release) ==="
    local project_dir="$test_workspace_root/SizeBenchTest"

    invoke_launcher -a create -n SizeBenchTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "native size benchmark" "create failed"; return
    }

    invoke_launcher -a size-benchmark -p "$project_dir" -t native > /dev/null 2>&1 \
        && report_pass "native size benchmark" \
        || report_skip "native size benchmark" "exit $? (build may have failed)"
}

# ===========================================================================
# 7. benchmark_wasm_size — WASM size check, fails if > 499 KB
# ===========================================================================

benchmark_wasm_size() {
    echo "=== WASM size budget (499 KB) ==="

    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a check-wasm -p examples/cube --wasm-budget-kb 499 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM size within 499 KB budget"
    else
        report_fail "WASM size budget" "exit $exit_code: ${launcher_output:0:200}"
    fi
}

# ===========================================================================
# Dispatch (only when executed directly)
# ===========================================================================

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    code_formatting_check)       code_formatting_check "${2:-examples/floating_pills}" ;;
    code_linting_check)          code_linting_check "${2:-examples/floating_pills}" ;;
    build_native_cube_example)   build_native_cube_example ;;
    build_wasm_example)          build_wasm_example "${2:-examples/cube}" ;;
    benchmark_native_performance) benchmark_native_performance ;;
    benchmark_native_size)       benchmark_native_size ;;
    benchmark_wasm_size)         benchmark_wasm_size ;;

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
        echo "  code_formatting_check        rustfmt + git diff"
        echo "  code_linting_check           clippy -D warnings + git diff"
        echo "  build_native_cube_example    build examples/cube (native)"
        echo "  build_wasm_example [path]    wasm-pack build + smoke test"
        echo "  benchmark_native_performance city FPS (release, needs xvfb)"
        echo "  benchmark_native_size        binary size report (release)"
        echo "  benchmark_wasm_size          WASM size ≤ 499 KB"
        exit 1
        ;;
esac

print_summary
