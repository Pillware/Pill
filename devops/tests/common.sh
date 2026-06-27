#!/usr/bin/env bash
# =============================================================================
# devops/tests/common.sh — shared test infrastructure
# =============================================================================
#
# Sourced by all test scripts.  Provides:
#   - terminal colours & global test counters
#   - PillLauncher binary auto-discovery
#   - temporary workspace creation / cleanup
#   - report_pass / report_fail / report_skip helpers
#   - invoke_launcher, assert_ok, assert_fail
#   - print_summary
#
# Idempotent — safe to source multiple times (e.g. from nested scripts).

# ---- idempotency guard ------------------------------------------------------
if [[ "${COMMON_SH_LOADED:-}" == "1" ]]; then
    return 0
fi
COMMON_SH_LOADED=1

# Fail fast on any unhandled error, unset variable, or pipe failure.
set -euo pipefail

# ---- terminal colors --------------------------------------------------------
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'   # No Color (reset)

# ---- global test counters ---------------------------------------------------
tests_passed=0; tests_failed=0; tests_skipped=0

# ===========================================================================
# Binary discovery
# ===========================================================================
# We try several well-known locations so the script works whether run from
# the repo root or from inside engine/pill_launcher.  CI sets
# PILL_LAUNCHER_BIN explicitly after downloading the build artifact.

find_launcher() {
    local search_paths=(
        "./engine/pill_launcher/target/release/PillLauncher"
        "./target/release/PillLauncher"
        "./engine/pill_launcher/target/debug/PillLauncher"
        "./target/debug/PillLauncher"
    )
    for candidate_path in "${search_paths[@]}"; do
        if [ -x "$candidate_path" ] || [ -f "$candidate_path" ]; then
            echo "$candidate_path"
            return
        fi
    done
    echo ""
}

pill_launcher_bin="${PILL_LAUNCHER_BIN:-$(find_launcher)}"
if [ -z "$pill_launcher_bin" ] || [ ! -f "$pill_launcher_bin" ]; then
    echo -e "${RED}FATAL: PillLauncher binary not found.${NC}"
    echo "Build it first:  cargo build -p pill_launcher --manifest-path engine/Cargo.toml"
    echo "Or set PILL_LAUNCHER_BIN=/path/to/PillLauncher"
    exit 1
fi
chmod +x "$pill_launcher_bin" 2>/dev/null || true

# ===========================================================================
# Temporary test directory
# ===========================================================================

TMPDIR="${TMPDIR:-/tmp}"
test_workspace_root="${TEST_ROOT:-$TMPDIR/pill-ci-tests-$$}"
mkdir -p "$test_workspace_root"

cleanup_workspace() {
    rm -rf "$test_workspace_root"
}
trap cleanup_workspace EXIT

# ===========================================================================
# Test result helpers
# ===========================================================================

report_pass() {
    echo -e "  ${GREEN}PASS${NC} $1"
    tests_passed=$((tests_passed + 1))
}

report_fail() {
    echo -e "  ${RED}FAIL${NC} $1 — $2"
    tests_failed=$((tests_failed + 1))
}

report_skip() {
    echo -e "  ${YELLOW}SKIP${NC} $1 — $2"
    tests_skipped=$((tests_skipped + 1))
}

invoke_launcher() {
    "$pill_launcher_bin" "$@"
}

assert_ok() {
    local test_description="$1"; shift
    if invoke_launcher "$@" > /dev/null 2>&1; then
        report_pass "$test_description"
    else
        report_fail "$test_description" "exit code $?"
    fi
}

assert_fail() {
    local test_description="$1"; local expected_substring="$2"; shift 2
    local launcher_output exit_code
    launcher_output=$(invoke_launcher "$@" 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -ne 0 ] && echo "$launcher_output" | grep -qi "$expected_substring"; then
        report_pass "$test_description"
    else
        report_fail "$test_description" \
            "expected error matching '$expected_substring', got exit $exit_code: ${launcher_output:0:200}"
    fi
}

print_summary() {
    local total_tests=$((tests_passed + tests_failed + tests_skipped))
    echo ""
    echo "========================================"
    echo -e "Results: ${GREEN}$tests_passed passed${NC}, ${RED}$tests_failed failed${NC}, ${YELLOW}$tests_skipped skipped${NC} ($total_tests total)"
    echo "========================================"
    [ "$tests_failed" -eq 0 ] || exit 1
}
