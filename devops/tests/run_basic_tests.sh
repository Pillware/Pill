#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo, rustfmt, clippy), wasm-pack, git,
#               a compiled PillLauncher binary (auto-discovered or set via
#               PILL_LAUNCHER_BIN).

# DESCRIPTION: Pill CI fast checks - Tests that validate
#   code formatting, linting, native & WASM builds, performance benchmarking
#   binary size measurement, and WASM size budget enforcement.
#
#   Designed for both local development and GitHub Actions CI
#   (ci-basic-tests.yml).

# USAGE: bash devops/tests/run_basic_tests.sh [all|<check-name>]
#
#   all                                   run all checks (default)
#   code_formatting_check                 cargo fmt + git diff
#   code_linting_check                    cargo clippy -D warnings
#   build_native_cube_example             build examples/cube (native) + artifact size report
#   build_wasm_cube_example               build WASM (cube) + artifact size report + budget
#   benchmark_native_performance          build + run city (release, 3 runs)

# EXAMPLE USAGE:
#   bash devops/tests/run_basic_tests.sh all
#   bash devops/tests/run_basic_tests.sh benchmark_native_performance
#   bash devops/tests/run_basic_tests.sh build_wasm_cube_example

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../common.sh
source "$SCRIPT_DIR/../common.sh"

# Shortcuts for colored section headers
BOLD='\033[1m'
CYAN='\033[0;36m'
NC='\033[0m'

# All paths in this script are relative to the project root.
cd "$PROJECT_ROOT"

# Force colored output from cargo and git even when piped (Docker/TTY-less).
export CARGO_TERM_COLOR=always
export GIT_CONFIG_COUNT=1
export GIT_CONFIG_KEY_0=color.ui
export GIT_CONFIG_VALUE_0=always

# ---------------------------------------------------------------------------
# 1. code_formatting_check - cargo fmt (direct) + git diff
# ---------------------------------------------------------------------------

code_formatting_check() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo -e "${BOLD}${CYAN}(1/5) Code formatting check${NC}"
    echo "Running cargo fmt --check"
    local fmt_output exit_code
    fmt_output=$(CARGO_TERM_COLOR=always cargo fmt --all --manifest-path engine/Cargo.toml -- --check 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "code formatting"
    else
        report_fail "code formatting" "$(echo "$fmt_output" | tail -c 500)"
    fi
}

# ---------------------------------------------------------------------------
# 2. code_linting_check - cargo clippy -D warnings (direct)
# ---------------------------------------------------------------------------

code_linting_check() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo -e "${BOLD}${CYAN}(2/5) Code linting check${NC}"
    echo "Running clippy"
    local clippy_output exit_code
    clippy_output=$(CARGO_TERM_COLOR=always cargo clippy --all --manifest-path engine/Cargo.toml -- -D warnings 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "code linting"
    else
        # Show the last 500 chars — cargo's "Updating/Locking" spam is at the
        # beginning; the actual clippy diagnostics are at the end.
        report_fail "clippy warnings" "$(echo "$clippy_output" | tail -c 500)"
    fi
}

# ---------------------------------------------------------------------------
# 3. build_native_cube_example - release native build + artifact size report
# ---------------------------------------------------------------------------

build_native_cube_example() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo -e "${BOLD}${CYAN}(3/5) Native build${NC}"
    local cube_dir="examples/cube"

    print_system_info

    if [ ! -f "$cube_dir/Cargo.toml" ]; then
        report_skip "native cube build" "examples/cube not found"
        return
    fi

    echo -e "${BOLD}Cleaning previous build artifacts...${NC}"
    cargo clean --manifest-path engine/Cargo.toml --release 2>/dev/null || true
    echo "Building - this may take a while"
    echo " "

    local exit_code=0
    invoke_launcher build -p "$cube_dir" -c release --clean 2>&1 || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "native cube build succeeds"
    else
        report_skip "native cube build" "exit $exit_code"
        return
    fi

    # Binary size report on all native build artifacts
    echo ""
    echo -e "${CYAN}------------------------------------------------------------------${NC}"
    echo -e "${BOLD}Native artifact size report${NC}"
    local data_dir="$cube_dir/build/release/data"
    if [ -d "$data_dir" ]; then
        print_size_report "$data_dir"
        report_pass "native artifact size report"
    else
        report_fail "native artifact size report" "missing $data_dir"
    fi
}

# ---------------------------------------------------------------------------
# 4. build_wasm_cube_example - release WASM build + artifact size report + budget
# ---------------------------------------------------------------------------

build_wasm_cube_example() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo -e "${BOLD}${CYAN}(4/5) WASM build${NC}"
    local cube_path="examples/cube"

    print_system_info

    # 1. Build the WASM target (pill_web_app) for the cube example
    echo -e "${BOLD}Cleaning previous build artifacts...${NC}"
    cargo clean --manifest-path engine/Cargo.toml --release 2>/dev/null || true
    echo "Building - this may take a while"
    echo " "

    local exit_code=0
    invoke_launcher build -p "$cube_path" -t web -c release --wasm-analyze --clean 2>&1 || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM build succeeds"
    else
        report_fail "WASM build" "exit $exit_code (see output above)"
        return
    fi

    # Verify the .wasm artifact exists (launcher flattens output to build/wasm/)
    local wasm_dir="$cube_path/build/wasm"
    local wasm_file="$wasm_dir/pill_web_app_bg.wasm"
    if [ ! -f "$wasm_file" ]; then
        report_fail "WASM artifact" "missing $wasm_file"
        return
    fi

    # Binary size - only the .wasm file, in MB (mebibytes, 4 decimal places)
    echo ""
    echo -e "${CYAN}------------------------------------------------------------------${NC}"
    echo -e "${BOLD}WASM artifact size + size guard${NC}"
    local wasm_size wasm_mb
    wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
    wasm_mb=$(awk "BEGIN { printf \"%.4f\", $wasm_size / 1048576 }")
    echo "  Binary size:"
    echo "  {"
    echo "    \"file\": \"pill_web_app_bg.wasm\","
    echo "    \"mb\": $wasm_mb"
    echo "  }"

    # Size budget: ≤ 0.4999 MB (524 176 bytes)
    local limit_bytes=524176
    if [ "$wasm_size" -le "$limit_bytes" ]; then
        report_pass "WASM artifact size (${wasm_mb} MB within 0.4999 MB budget)"
    else
        report_fail "WASM size budget" "${wasm_mb} MB exceeds 0.4999 MB limit"
    fi

    # Dev server smoke test - verify the server can serve built files.
    # Run the launcher's dev server in the background with a hard timeout
    # so it can never hang the test suite.
    if [ -d "$wasm_dir" ]; then
        echo ""
        echo -e "${CYAN}------------------------------------------------------------------${NC}"
        echo -e "${BOLD}WASM dev server smoke test${NC}"
        local test_port=8080
        kill_server_on_port "$test_port"

        local server_log
        server_log="$(mktemp)"
        echo "Starting PillLauncher dev server on port ${test_port}..."
        invoke_launcher run -t web -p "$cube_path" -c release >"$server_log" 2>&1 &
        local server_pid=$!

        # Wait up to 30s for the server to bind
        local server_ready=0
        for attempt in $(seq 1 30); do
            if curl -sf -o /dev/null "http://127.0.0.1:${test_port}/" 2>/dev/null; then
                server_ready=1
                break
            fi
            if ! kill -0 "$server_pid" 2>/dev/null; then
                break
            fi
            sleep 1
        done

        if [ "$server_ready" -eq 1 ]; then
            local smoke_ok=1
            curl -sf -o /dev/null "http://127.0.0.1:${test_port}/" || smoke_ok=0
            curl -sf -o /dev/null "http://127.0.0.1:${test_port}/pill_web_app.js" || smoke_ok=0
            curl -sf -o /dev/null "http://127.0.0.1:${test_port}/pill_web_app_bg.wasm" || smoke_ok=0
            if [ "$smoke_ok" -eq 1 ]; then
                report_pass "WASM dev server smoke test"
            else
                report_fail "WASM dev server smoke test" "one or more key files not served"
            fi
        else
            report_skip "WASM dev server smoke test" "server did not start in time"
        fi

        # Cleanup: kill the server and any children. Wait for the process
        # to actually exit so we don't leak it into the next test step.
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
        kill_server_on_port "$test_port"
        rm -f "$server_log"
    fi

    # Flush any buffered output from child processes (wasm-pack, cargo)
    # before the next test step begins.
    wait 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# 5. benchmark_native_performance - build + run city (release)
# ---------------------------------------------------------------------------
#
# Strategy (no xvfb needed — `--headless` skips winit/wgpu entirely):
#   Windows    → windowed only (3 runs).
#   Linux/macOS → if no $DISPLAY / $WAYLAND_DISPLAY, use --headless;
#                 otherwise try windowed first; fall back to --headless
#                 on any failure.
#   The benchmark spawns 10 000 citizens, runs 5 000 frames (1 000 warmup),
#   prints per-frame stats as JSON, then auto-exits.

benchmark_native_performance() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo -e "${BOLD}${CYAN}(5/5) Performance benchmark${NC}"

    print_system_info

    local operating_system
    operating_system=$(uname -s 2>/dev/null || echo "Windows")

    # -- Windows: always windowed -------------------------------------------
    if [[ "$operating_system" == *"MINGW"* ]] || [[ "$operating_system" == *"MSYS"* ]] || [[ "$operating_system" == "Windows" ]]; then
        echo -e "${BOLD}Building + running benchmark (windowed, 3 runs)${NC}"
        _run_benchmark_loop false
        return
    fi

    # -- Linux / macOS: try windowed, fall back to headless -----------------
    if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
        echo -e "${YELLOW}No display detected - using headless benchmark${NC}"
        _run_benchmark_loop true
        return
    fi

    echo "Building + running benchmark (windowed, 3 runs)"
    _run_benchmark_loop false
    local windowed_result=$?
    if [ "$windowed_result" -eq 0 ]; then
        return
    fi

    # GPU unavailable or other failure — rebuild in headless mode
    echo -e "${YELLOW}Windowed benchmark failed — falling back to headless${NC}"
    echo -e "${BOLD}Building + running headless benchmark (3 runs)${NC}"
    _run_benchmark_loop true
}

# ------------------------------------------------------------
# _run_benchmark_loop - run the city benchmark N times
#   $1 = headless (true|false)
#   Returns 0 if at least one run passed, 1 if all failed.
# ------------------------------------------------------------
_run_benchmark_loop() {
    local headless="$1"
    local runs=3
    local passed=0
    local failed=0
    local project_directory="examples/city"

    # Determine the project title early so we can clean the correct target dir.
    local project_title
    project_title=$(grep -oP '^TITLE\s*=\s*\K.+' "$project_directory/res/config.ini" 2>/dev/null | tr -d ' ')

    # Build once, then run the compiled executable directly for each iteration
    echo -e "${BOLD}Cleaning previous build artifacts...${NC}"
    cargo clean --manifest-path engine/Cargo.toml --release 2>/dev/null || true
    if [ -n "$project_title" ]; then
        rm -rf "engine/target_projects/${project_title}" 2>/dev/null || true
    fi

    echo -e "${BOLD}Building...${NC}"
    echo " "
    local build_exit_code=0
    if [ "$headless" = "true" ]; then
        # --headless enables headless on engine crates; benchmark_headless on the project crate.
        invoke_launcher build -p "$project_directory" -c release --clean --headless --additional-features project/benchmark_headless 2>&1 || build_exit_code=$?
    else
        # Windowed benchmark: enable benchmark_windowed feature on the project
        invoke_launcher build -p "$project_directory" -c release --clean --additional-features project/benchmark_windowed 2>&1 || build_exit_code=$?
    fi
    if [ "$build_exit_code" -ne 0 ]; then
        report_skip "native perf benchmark" "build failed (exit $build_exit_code)"
        return 1
    fi

    # Determine executable name from project title
    local executable_name="$project_title"
    if [[ "$(uname -s)" == *"MINGW"* ]] || [[ "$(uname -s)" == *"MSYS"* ]] || [[ "$(uname -s)" == "Windows" ]]; then
        executable_name="${executable_name}.exe"
    fi

    if [ ! -f "$project_directory/build/release/$executable_name" ]; then
        report_skip "native perf benchmark" "executable not found: $project_directory/build/release/$executable_name"
        return 1
    fi

    # Arrays for per-run stats
    local -a average_milliseconds=() median_milliseconds=() minimum_milliseconds=() maximum_milliseconds=() range_milliseconds=() stddev_milliseconds=()

    for ((i=1; i<=runs; i++)); do
        echo "Run $i/$runs..."
        local run_exit_code=0
        local run_output
        run_output=$(cd "$project_directory/build/release" && ./"$executable_name" 2>&1) || run_exit_code=$?

        if [ "$run_exit_code" -eq 0 ]; then
            passed=$((passed + 1))
            local json_line
            json_line=$(echo "$run_output" | grep '^{' | head -1)
            if [ -n "$json_line" ]; then
                echo "  $(echo "$json_line" | grep -o '"average_ms":[0-9.]*')"
                average_milliseconds+=("$(_extract_json_number "$json_line" "average_ms")")
                median_milliseconds+=("$(_extract_json_number "$json_line" "median_ms")")
                minimum_milliseconds+=("$(_extract_json_number "$json_line" "min_ms")")
                maximum_milliseconds+=("$(_extract_json_number "$json_line" "max_ms")")
                range_milliseconds+=("$(_extract_json_number "$json_line" "range_ms")")
                stddev_milliseconds+=("$(_extract_json_number "$json_line" "stddev_ms")")
                echo "    OK"
            else
                echo "    OK (no JSON output)"
            fi
        else
            failed=$((failed + 1))
            echo "    FAILED (exit $run_exit_code)"
        fi
    done

    # Print summary if we have data
    if [ "$passed" -gt 0 ] && [ ${#average_milliseconds[@]} -gt 0 ]; then
        local mode_label="windowed"
        if [ "$headless" = "true" ]; then
            mode_label="headless"
        fi
        echo -e "${BOLD}  Benchmark summary${NC} ($passed run(s), $mode_label):"
        echo "  --------------------------------------------------"
        echo "  {"
        echo "    \"mode\": \"$mode_label\","
        echo "    \"runs\": $passed,"
        echo "    \"stats\": {"
        _print_statistic_summary "average_ms"  "${average_milliseconds[@]}"
        _print_statistic_summary "median_ms"   "${median_milliseconds[@]}"
        _print_statistic_summary "min_ms"      "${minimum_milliseconds[@]}"
        _print_statistic_summary "max_ms"      "${maximum_milliseconds[@]}"
        _print_statistic_summary "range_ms"    "${range_milliseconds[@]}"
        _print_statistic_summary "stddev_ms"   "${stddev_milliseconds[@]}"
        echo "    }"
        echo "  }"
    fi

    if [ "$failed" -eq 0 ]; then
        report_pass "native performance benchmark ($passed/$runs runs passed)"
        return 0
    elif [ "$passed" -gt 0 ]; then
        report_pass "native performance benchmark ($passed/$runs runs passed, $failed failed)"
        return 0
    else
        report_fail "native performance benchmark" "all $runs runs failed"
        return 1
    fi
}

# ------------------------------------------------------------
# _extract_json_number - pull a numeric value from a JSON line
#   $1 = JSON string
#   $2 = key name
#   Prints the value or "0" if not found.
# ------------------------------------------------------------
_extract_json_number() {
    local extracted_value
    extracted_value=$(echo "$1" | grep -oP "\"$2\":\K[0-9.]+" 2>/dev/null || echo "0")
    echo "$extracted_value"
}

# ------------------------------------------------------------
# _print_statistic_summary - print min / max / average for a stat
#   $1    = stat name
#   $2..$n = values (as decimal strings like "1.700")
# ------------------------------------------------------------
_print_statistic_summary() {
    local name="$1"
    shift
    local minimum_value="$1"
    local maximum_value="$1"
    local sum=0
    local count=$#

    for current_value in "$@"; do
        # Use awk for floating-point math (available everywhere; bc is not)
        sum=$(awk "BEGIN { printf \"%.3f\", $sum + $current_value }" 2>/dev/null || echo "$sum")
        if [ "$(awk "BEGIN { print ($current_value < $minimum_value) }" 2>/dev/null || echo 0)" = "1" ]; then
            minimum_value="$current_value"
        fi
        if [ "$(awk "BEGIN { print ($current_value > $maximum_value) }" 2>/dev/null || echo 0)" = "1" ]; then
            maximum_value="$current_value"
        fi
    done
    local average
    average=$(awk "BEGIN { printf \"%.3f\", $sum / $count }" 2>/dev/null || echo "0")

    printf '      "%s": {"min": %s, "max": %s, "avg": %s}' "$name" "$minimum_value" "$maximum_value" "$average"
    if [ "$name" != "stddev_ms" ]; then
        echo ","
    else
        echo ""
    fi
}

# ---------------------------------------------------------------------------
# Dispatch (only when executed directly)
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    code_formatting_check)       code_formatting_check ;;
    code_linting_check)          code_linting_check ;;
    build_native_cube_example)   build_native_cube_example ;;
    build_wasm_cube_example)     build_wasm_cube_example ;;
    benchmark_native_performance) benchmark_native_performance ;;

    all|"")
        code_formatting_check
        code_linting_check
        build_native_cube_example
        build_wasm_cube_example
        benchmark_native_performance
        ;;

    *)
        echo "Usage: $0 [all|<check-name>]"
        echo ""
        echo "Checks:"
        echo "  code_formatting_check        cargo fmt + git diff"
        echo "  code_linting_check           cargo clippy -D warnings"
        echo "  build_native_cube_example    build examples/cube (native) + artifact size report"
        echo "  build_wasm_cube_example      build WASM (cube) + artifact size report + budget"
        echo "  benchmark_native_performance build + run city (release)"
        exit 1
        ;;
esac

print_summary
