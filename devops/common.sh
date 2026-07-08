#!/usr/bin/env bash

# REQUIREMENTS: cargo, git, bash 4+, awk, curl, sed
#
# DESCRIPTION: Shared test infrastructure for Pill CI Pipeline and local
#   development.  Provides binary auto-discovery, project root resolution,
#   stale workspace-member cleanup, temporary workspace management, binary
#   size reporting, dev-server smoke testing, and coloured pass/fail/skip
#   result helpers.  Sourced (not executed) by all test scripts.
#   Idempotent - safe to source multiple times.

# --- SCRIPT ---

# ---- Idempotency guard ------------------------------------------------------
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

# ---- Global test counters & per-result log ---------------------------------
tests_passed=0; tests_failed=0; tests_skipped=0
test_results=()  # stores "PASS|<description>", "FAIL|<description> - reason", etc.

# ---------------------------------------------------------------------------
# Binary discovery
# ---------------------------------------------------------------------------
# We try several well-known locations so the script works whether run from
# the repo root or from inside engine/pill_launcher. CI sets
# PILL_LAUNCHER_BIN explicitly after downloading the build artifact.

# Walk up from the directory containing this script until we find a known
# project-root marker, then return that absolute path.
find_project_root() {
    local script_dir
    script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    local current_dir="$script_dir"
    while [ "$current_dir" != "/" ]; do
        if [ -f "$current_dir/engine/Cargo.toml" ] || [ -f "$current_dir/engine/pill_launcher/Cargo.toml" ]; then
            echo "$current_dir"
            return 0
        fi
        current_dir="$(dirname "$current_dir")"
    done
    # Fallback: use git root if available
    if command -v git > /dev/null 2>&1; then
        git rev-parse --show-toplevel 2>/dev/null && return 0 || true
    fi
    return 1
}

find_launcher() {
    local project_root
    project_root="$(find_project_root)" || project_root="."

    # On Linux/macOS, skip .exe files — they are Windows binaries and won't run.
    local windows_host=false
    case "$(uname -s)" in
        MINGW*|MSYS*|CYGWIN*) windows_host=true ;;
        *)                      windows_host=false ;;
    esac

    local search_paths=(
        "$project_root/engine/pill_launcher/target/release/PillLauncher"
        "$project_root/target/release/PillLauncher"
        "$project_root/engine/pill_launcher/target/debug/PillLauncher"
        "$project_root/target/debug/PillLauncher"
    )
    if [ "$windows_host" = true ]; then
        search_paths=(
            "$project_root/engine/pill_launcher/target/release/PillLauncher.exe"
            "$project_root/target/release/PillLauncher.exe"
            "$project_root/engine/pill_launcher/target/debug/PillLauncher.exe"
            "$project_root/target/debug/PillLauncher.exe"
        )
    fi

    for candidate_path in "${search_paths[@]}"; do
        if [ -x "$candidate_path" ] || [ -f "$candidate_path" ]; then
            echo "$candidate_path"
            return
        fi
    done

    # Check system PATH as a last resort
    if command -v PillLauncher > /dev/null 2>&1; then
        command -v PillLauncher
        return
    fi

    echo ""
}

# Determine the project root once and export it for all sourcing scripts.
PROJECT_ROOT="$(find_project_root)"
export PROJECT_ROOT

pill_launcher_bin="${PILL_LAUNCHER_BIN:-$(find_launcher)}"
if [ -z "$pill_launcher_bin" ] || [ ! -f "$pill_launcher_bin" ]; then
    echo -e "${RED}FATAL: PillLauncher binary not found.${NC}"
    echo "Build it first:  cargo build --release --manifest-path engine/pill_launcher/Cargo.toml"
    echo "Or set PILL_LAUNCHER_BIN=/path/to/PillLauncher"
    exit 1
fi
chmod +x "$pill_launcher_bin" 2>/dev/null || true

# Clear stale cargo package-cache lock (can block parallel builds).
rm -f "${HOME}/.cargo/.package-cache" 2>/dev/null || true

# Fix engine/Cargo.toml workspace members - the launcher injects absolute-path
# workspace members during builds and may leave them stale if interrupted.
# This sed removes any line that has the pill-launcher-managed marker.
fix_stale_workspace_members() {
    local cargo_toml="${PROJECT_ROOT:-.}/engine/Cargo.toml"
    if [ -f "$cargo_toml" ]; then
        sed -i '/pill-launcher-managed-workspace-member/d' "$cargo_toml" 2>/dev/null || true
    fi
}
fix_stale_workspace_members

# ---------------------------------------------------------------------------
# Temporary test directory
# ---------------------------------------------------------------------------

# Use Windows TEMP if available (Git Bash), fall back to /tmp
if [ -d "${TEMP:-}" ]; then
    TMPDIR="${TMPDIR:-$TEMP}"
elif [ -d "${TMP:-}" ]; then
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

# Kill any process listening on the given port.  Works on Windows (netstat +
# taskkill) and Unix (fuser / lsof + kill).  Also kills any lingering
# PillLauncher processes to release locked executables.
kill_server_on_port() {
    local port="$1"
    # Windows: netstat + taskkill
    local stale_pid
    stale_pid=$(netstat -ano 2>/dev/null | grep ":${port} " | grep LISTENING | awk '{print $NF}' | head -1 || true)
    if [ -n "$stale_pid" ]; then
        taskkill //PID "$stale_pid" //F //T > /dev/null 2>&1 || true
    fi
    # Also kill any remaining PillLauncher processes (may hold exe lock)
    taskkill //F //IM PillLauncher.exe > /dev/null 2>&1 || true
    # Unix fallback
    if command -v fuser > /dev/null 2>&1; then
        fuser -k "${port}/tcp" > /dev/null 2>&1 || true
    fi
    sleep 1
}

# Print a JSON binary-size report for all files under a directory.
# Outputs nothing if the directory does not exist.  Sizes in MB (x.xxxx).
print_size_report() {
    local directory="$1"
    if [ ! -d "$directory" ]; then
        return
    fi
    local total_mb=0
    local file_count=0
    local json_entries=""
    while IFS= read -r -d '' file; do
        local size
        size=$(wc -c < "$file" 2>/dev/null || echo 0)
        local mb
        mb=$(awk "BEGIN { printf \"%.4f\", $size / 1048576 }")
        total_mb=$(awk "BEGIN { printf \"%.4f\", $total_mb + $mb }")
        file_count=$((file_count + 1))
        local relative_path="${file#$directory/}"
        if [ -n "$json_entries" ]; then
            json_entries+=$',\n'
        fi
        json_entries+="        {\"file\": \"${relative_path}\", \"mb\": ${mb}}"
    done < <(find "$directory" -type f -print0 2>/dev/null | sort -z)
    echo "  Binary sizes:"
    echo "  {"
    echo "    \"total_mb\": $total_mb,"
    echo "    \"file_count\": $file_count,"
    echo "    \"files\": ["
    echo -e "$json_entries"
    echo "    ]"
    echo "  }"
}

# ---------------------------------------------------------------------------
# System info — machine specs for benchmark context
# ---------------------------------------------------------------------------

print_system_info() {
    echo ""
    echo "---------- System Information ----------"

    # --- OS & kernel ---
    echo "  OS:      $(uname -s 2>/dev/null || echo 'Windows') $(uname -r 2>/dev/null || echo '')"
    if command -v sw_vers >/dev/null 2>&1; then
        echo "  macOS:   $(sw_vers -productName 2>/dev/null) $(sw_vers -productVersion 2>/dev/null)"
    elif [ -f /etc/os-release ]; then
        echo "  Distro:  $(grep ^PRETTY_NAME= /etc/os-release 2>/dev/null | cut -d= -f2 | tr -d '"')"
    fi

    # --- CPU ---
    local cpu_model="unknown"
    local cpu_cores="unknown"
    if [ -f /proc/cpuinfo ]; then
        cpu_model=$(grep -m1 'model name' /proc/cpuinfo 2>/dev/null | cut -d: -f2 | xargs || echo "unknown")
        cpu_cores=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || echo "unknown")
    elif command -v sysctl >/dev/null 2>&1; then
        cpu_model=$(sysctl -n machdep.cpu.brand_string 2>/dev/null || echo "unknown")
        cpu_cores=$(sysctl -n hw.ncpu 2>/dev/null || echo "unknown")
    elif command -v wmic >/dev/null 2>&1; then
        cpu_model=$(wmic cpu get name 2>/dev/null | tail -n +2 | head -1 | xargs || echo "unknown")
        cpu_cores=$(wmic cpu get NumberOfCores 2>/dev/null | tail -n +2 | head -1 | xargs || echo "unknown")
    fi
    echo "  CPU:     ${cpu_model} (${cpu_cores} cores)"

    # --- RAM ---
    local ram_total="unknown"
    local ram_available="unknown"
    if [ -f /proc/meminfo ]; then
        ram_total=$(awk '/^MemTotal:/ {printf "%.1f GB", $2/1048576}' /proc/meminfo 2>/dev/null || echo "unknown")
        ram_available=$(awk '/^MemAvailable:/ {printf "%.1f GB", $2/1048576}' /proc/meminfo 2>/dev/null || echo "unknown")
    elif command -v sysctl >/dev/null 2>&1; then
        local mem_bytes
        mem_bytes=$(sysctl -n hw.memsize 2>/dev/null || echo 0)
        ram_total=$(awk "BEGIN {printf \"%.1f GB\", $mem_bytes/1073741824}" 2>/dev/null || echo "unknown")
        ram_available="N/A (macOS)"
    elif command -v wmic >/dev/null 2>&1; then
        local mem_kb
        mem_kb=$(wmic OS get TotalVisibleMemorySize 2>/dev/null | tail -n +2 | head -1 | xargs || echo 0)
        ram_total=$(awk "BEGIN {printf \"%.1f GB\", $mem_kb/1048576}" 2>/dev/null || echo "unknown")
        ram_available="N/A (Windows)"
    fi
    echo "  RAM:     ${ram_total} total, ${ram_available} available"

    # --- Docker / cgroup limits (if running in container) ---
    if [ -f /proc/1/cgroup ] && grep -q 'docker\|container' /proc/1/cgroup 2>/dev/null; then
        echo "  Environment: Docker container"
        if [ -f /sys/fs/cgroup/cpu.max ]; then
            local cpu_quota cpu_period
            read -r cpu_quota cpu_period < /sys/fs/cgroup/cpu.max 2>/dev/null || true
            if [ -n "$cpu_quota" ] && [ "$cpu_quota" != "max" ] && [ -n "$cpu_period" ]; then
                echo "  CPU limit: $(awk "BEGIN {printf \"%.2f\", $cpu_quota/$cpu_period}" 2>/dev/null) cores"
            else
                echo "  CPU limit: unrestricted"
            fi
        fi
        if [ -f /sys/fs/cgroup/memory.max ]; then
            local mem_limit
            mem_limit=$(cat /sys/fs/cgroup/memory.max 2>/dev/null || echo "unknown")
            if [ "$mem_limit" != "max" ]; then
                echo "  RAM limit: $(awk "BEGIN {printf \"%.1f GB\", $mem_limit/1073741824}" 2>/dev/null) "
            else
                echo "  RAM limit: unrestricted"
            fi
        fi
    fi

    # --- Rust toolchain ---
    if command -v rustc >/dev/null 2>&1; then
        echo "  rustc:   $(rustc --version 2>/dev/null || echo 'unknown')"
    fi
    if command -v cargo >/dev/null 2>&1; then
        echo "  cargo:   $(cargo --version 2>/dev/null || echo 'unknown')"
    fi

    echo "----------------------------------------"
    echo ""
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
# assert_ok - Run a launcher command and expect exit 0
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
# assert_fail - Run a launcher command, expect non-zero exit +
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
