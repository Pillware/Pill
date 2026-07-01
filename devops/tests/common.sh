#!/usr/bin/env bash
# ---------------------------------------------------------------------------==
# devops/tests/common.sh - shared test infrastructure
# ---------------------------------------------------------------------------==
#
# Sourced by all test scripts.  Provides:
#   - terminal colours & global test counters
#   - PillLauncher binary auto-discovery
#   - temporary workspace creation / cleanup
#   - report_pass / report_fail / report_skip helpers
#   - invoke_launcher, assert_ok, assert_fail
#   - print_summary
#
# Idempotent - safe to source multiple times (e.g. from nested scripts).

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

# ---- global test counters & per-result log ---------------------------------
tests_passed=0; tests_failed=0; tests_skipped=0
test_results=()  # stores "PASS|<description>", "FAIL|<description> - reason", etc.

# ---------------------------------------------------------------------------
# Binary discovery
# ---------------------------------------------------------------------------
# We try several well-known locations so the script works whether run from
# the repo root or from inside engine/pill_launcher.  CI sets
# PILL_LAUNCHER_BIN explicitly after downloading the build artifact.

find_launcher() {
    local search_paths=(
        "./engine/pill_launcher/target/release/PillLauncher"
        "./engine/pill_launcher/target/release/PillLauncher.exe"
        "./target/release/PillLauncher"
        "./target/release/PillLauncher.exe"
        "./engine/pill_launcher/target/debug/PillLauncher"
        "./engine/pill_launcher/target/debug/PillLauncher.exe"
        "./target/debug/PillLauncher"
        "./target/debug/PillLauncher.exe"
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

# Clear stale cargo package-cache lock (can block parallel builds).
rm -f "${HOME}/.cargo/.package-cache" 2>/dev/null || true

# Restore engine/Cargo.toml to its committed state - previous test runs may
# have left stale workspace-member entries if they crashed mid-build.
if [ -f "engine/Cargo.toml" ] && command -v git > /dev/null 2>&1; then
    git checkout -- engine/Cargo.toml 2>/dev/null || true
fi

# ---------------------------------------------------------------------------
# Temporary test directory
# ---------------------------------------------------------------------------

# Use Windows TEMP if available (Git Bash), fall back to /tmp
if [ -d "$TEMP" ]; then
    TMPDIR="${TMPDIR:-$TEMP}"
elif [ -d "$TMP" ]; then
    TMPDIR="${TMPDIR:-$TMP}"
else
    TMPDIR="${TMPDIR:-/tmp}"
fi
test_workspace_root="${TEST_ROOT:-$TMPDIR/pill-ci-tests-$$}"
mkdir -p "$test_workspace_root"

# cleanup_workspace() {
#     rm -rf "$test_workspace_root"
# }
# trap cleanup_workspace EXIT

# ---------------------------------------------------------------------------
# Utility helpers
# ---------------------------------------------------------------------------

# Format a byte count as human-readable (e.g. "1.2 MB", "337 KB", "359 B").
# Uses numfmt if available (GNU coreutils), otherwise falls back to a simple
# integer division that matches ls -lh style (no decimal places).
format_size() {
    local bytes=$1
    if command -v numfmt > /dev/null 2>&1; then
        numfmt --to=iec --suffix=B "$bytes" 2>/dev/null || echo "${bytes} B"
    elif [ "$bytes" -ge 1048576 ]; then
        echo "$((bytes / 1048576)) MB"
    elif [ "$bytes" -ge 1024 ]; then
        echo "$((bytes / 1024)) KB"
    else
        echo "${bytes} B"
    fi
}

# ---------------------------------------------------------------------------
# Test result helpers
# ---------------------------------------------------------------------------

report_pass() {
    echo -e "  ${GREEN}PASS${NC} $1"
    tests_passed=$((tests_passed + 1))
    test_results+=("PASS|$1")
}

report_fail() {
    echo -e "  ${RED}FAIL${NC} $1 - $2"
    tests_failed=$((tests_failed + 1))
    test_results+=("FAIL|$1 - $2")
}

report_skip() {
    echo -e "  ${YELLOW}SKIP${NC} $1 - $2"
    tests_skipped=$((tests_skipped + 1))
    test_results+=("SKIP|$1 - $2")
}

# Export binary path so timeout (separate process) can use it
export pill_launcher_bin

invoke_launcher() {
    "$pill_launcher_bin" "$@"
}

# Also export the function for timeout/background processes
export -f invoke_launcher

# ---------------------------------------------------------------------------
# assert_ok — Run a launcher command and expect exit 0
# Usage: assert_ok "test description" <launcher args...>
#   Example: assert_ok "create project" create -n MyGame -p /tmp
# ---------------------------------------------------------------------------
assert_ok() {
    local test_description="$1"; shift
    if invoke_launcher "$@" > /dev/null 2>&1; then
        report_pass "$test_description"
    else
        report_fail "$test_description" "exit code $?"
    fi
}

# ---------------------------------------------------------------------------
# assert_fail — Run a launcher command, expect non-zero exit +
#   stderr containing a case-insensitive substring
# Usage: assert_fail "test description" "expected error substring" <launcher args...>
#   Example: assert_fail "duplicate create" "already exists" create -n Foo -p /tmp
# ---------------------------------------------------------------------------
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
    local index=0
    for entry in "${test_results[@]}"; do
        index=$((index + 1))
        local status="${entry%%|*}"
        local description="${entry#*|}"
        case "$status" in
            PASS) echo -e "($index/$total_tests) ${GREEN}PASS${NC} - $description" ;;
            FAIL) echo -e "($index/$total_tests) ${RED}FAIL${NC} - $description" ;;
            SKIP) echo -e "($index/$total_tests) ${YELLOW}SKIP${NC} - $description" ;;
        esac
    done
    echo "========================================"
    echo -e "Results: ${GREEN}$tests_passed passed${NC}, ${RED}$tests_failed failed${NC}, ${YELLOW}$tests_skipped skipped${NC} ($total_tests total)"
    echo "========================================"
    [ "$tests_failed" -eq 0 ] || exit 1
}
