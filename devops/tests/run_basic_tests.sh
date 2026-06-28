#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo, rustfmt, clippy), wasm-pack, git,
#               a compiled PillLauncher binary (auto-discovered or set via
#               PILL_LAUNCHER_BIN).

# DESCRIPTION: Pill  CI fast checks - Tests that validate
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
#   build_native_cube_example             build examples/cube (native, release)
#   build_wasm_cube_example               build WASM (cube) + verify artifact
#   benchmark_native_performance          build + run city (release, 3 runs)
#   benchmark_native_size                 release build + binary size JSON
#   benchmark_wasm_size [path]            WASM binary size budget (≤ 499 KB)

# EXAMPLE USAGE:
#   bash devops/tests/run_basic_tests.sh all
#   bash devops/tests/run_basic_tests.sh benchmark_native_performance
#   bash devops/tests/run_basic_tests.sh build_wasm_cube_example

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ---------------------------------------------------------------------------
# 1. code_formatting_check - cargo fmt (direct) + git diff
# ---------------------------------------------------------------------------

code_formatting_check() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(1/7) Code formatting check"
    echo "Running cargo fmt"
    cargo fmt --all --manifest-path engine/Cargo.toml

    # Exclude Cargo.toml files - the launcher rewrites workspace paths
    # (NO_PATH → absolute), which are not formatting issues.
    if git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        ':(exclude)examples/cube/Cargo.toml'; then
        report_pass "code formatting"
    else
        report_fail "code formatting" "rustfmt produced changes - run 'cargo fmt'"
    fi
}

# ---------------------------------------------------------------------------
# 2. code_linting_check - cargo clippy -D warnings (direct)
# ---------------------------------------------------------------------------

code_linting_check() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(2/7) Code linting check"
    echo "Running clippy"
    local clippy_output exit_code
    clippy_output=$(cargo clippy --all --manifest-path engine/Cargo.toml -- -D warnings 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "code linting"
    else
        report_fail "clippy warnings" "${clippy_output:0:300}"
    fi
}

# ---------------------------------------------------------------------------
# 3. build_native_cube_example - launcher build examples/cube
# ---------------------------------------------------------------------------

build_native_cube_example() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(3/7) Native build check"
    local cube_dir="examples/cube"

    if [ ! -f "$cube_dir/Cargo.toml" ]; then
        report_skip "native cube build" "examples/cube not found"
        return
    fi

    echo "Building - this may take a moment"
    local exit_code=0
    invoke_launcher -a build -p "$cube_dir" -c release 2>&1 || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "native cube build succeeds"
        if [ -d "$cube_dir/build/release/data" ]; then
            report_pass "native cube build output data/ exists"
        else
            report_fail "native cube build output data/" "missing $cube_dir/build/release/data"
        fi
    else
        report_skip "native cube build" "exit $exit_code"
    fi
}

# ---------------------------------------------------------------------------
# 4. build_wasm_cube_example - launcher build -t web + artifact & size check
# ---------------------------------------------------------------------------

build_wasm_cube_example() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(4/7) WASM build check"
    local cube_path="examples/cube"

    echo "Building - this may take a moment"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a build -p "$cube_path" -t web 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM build succeeds"
    else
        report_fail "WASM build" "exit $exit_code: ${launcher_output:0:200}"
        return
    fi

    # Verify the .wasm artifact exists (launcher flattens output to build/wasm/)
    local wasm_file="$cube_path/build/wasm/pill_web_app_bg.wasm"
    if [ -f "$wasm_file" ]; then
        local wasm_size
        wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
        local wasm_kb=$((wasm_size / 1024))
        report_pass "WASM artifact exists (${wasm_kb} KB)"
    else
        report_fail "WASM artifact" "missing $wasm_file"
    fi
}

# ---------------------------------------------------------------------------
# 5. benchmark_native_performance - build + run city (release)
# ---------------------------------------------------------------------------
#
# Strategy (no xvfb needed - the engine has a built-in headless mode):
#   Windows    → build once, then run the compiled exe directly 3 times.
#   Linux/macOS → if no $DISPLAY / $WAYLAND_DISPLAY, skip to headless;
#                 otherwise try windowed first; fall back to
#                 `--features benchmark_headless` on GPU failure.
#   The benchmark spawns 10 000 citizens, runs 5 000 frames (1 000 warmup),
#   prints per-frame stats as JSON, then auto-exits.

benchmark_native_performance() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(5/7) Performance benchmark"

    local operating_system
    operating_system=$(uname -s 2>/dev/null || echo "Windows")

    # -- Windows: benchmark_window renders + auto-exits after 5 000 frames ---
    if [[ "$operating_system" == *"MINGW"* ]] || [[ "$operating_system" == *"MSYS"* ]] || [[ "$operating_system" == "Windows" ]]; then
        echo "Building + running benchmark (windowed, 3 runs)"
        _run_benchmark_loop "benchmark_window" "windowed"
        return
    fi

    # -- Linux / macOS: try windowed benchmark, fall back to headless --------
    # Quick check: if no display is available, skip straight to headless.
    if [ -z "${DISPLAY:-}" ] && [ -z "${WAYLAND_DISPLAY:-}" ]; then
        echo "No display detected - using headless benchmark"
        _run_benchmark_loop "benchmark_headless" "headless"
        return
    fi
    echo "Building + running benchmark (windowed, 3 runs)"
    _run_benchmark_loop "benchmark_window" "windowed"
    local windowed_ok=$?
    if [ "$windowed_ok" -eq 0 ]; then
        return
    fi

    # GPU unavailable - rebuild with benchmark_headless
    echo "GPU unavailable - switching to headless benchmark"
    echo "Building + running headless benchmark (10 000 citizens, 5 000 frames, 3 runs)"
    _run_benchmark_loop "benchmark_headless" "headless"
}

# ------------------------------------------------------------
# _run_benchmark_loop - run the city benchmark N times
#   $1 = cargo feature  (benchmark_window | benchmark_headless)
#   $2 = label          (windowed | headless)
#   Returns 0 if at least one run passed, 1 if all failed.
# ------------------------------------------------------------
_run_benchmark_loop() {
    local feature="$1"
    local label="$2"
    local runs=3
    local passed=0
    local failed=0
    local project_directory="examples/city"

    # Build once, then run the compiled executable directly for each iteration
    echo "  Building..."
    local build_exit_code=0
    invoke_launcher -a build -p "$project_directory" -c release --features "$feature" 2>&1 || build_exit_code=$?
    if [ "$build_exit_code" -ne 0 ]; then
        report_skip "native perf benchmark" "build failed (exit $build_exit_code)"
        return 1
    fi

    # Determine executable name from project config.ini TITLE
    local project_title executable_name
    project_title=$(grep -oP '^TITLE\s*=\s*\K.+' "$project_directory/res/config.ini" 2>/dev/null | tr -d ' ')
    executable_name="$project_title"
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
        echo "  Run $i/$runs..."
        local run_exit_code=0
        local run_output
        run_output=$(cd "$project_directory/build/release" && ./"$executable_name" 2>&1) || run_exit_code=$?

        if [ "$run_exit_code" -eq 0 ]; then
            passed=$((passed + 1))
            local json_line
            json_line=$(echo "$run_output" | grep '^{' | head -1)
            if [ -n "$json_line" ]; then
                echo "    $(echo "$json_line" | grep -o '"average_ms":[0-9.]*')"
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
        echo ""
        echo "  Benchmark summary ($passed run(s), $label):"
        echo "  --------------------------------------------------"
        echo "  {"
        echo "    \"mode\": \"$label\","
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
        report_pass "native performance benchmark ($passed/$runs $label runs passed)"
        return 0
    elif [ "$passed" -gt 0 ]; then
        report_pass "native performance benchmark ($passed/$runs $label runs passed, $failed failed)"
        return 0
    else
        report_fail "native performance benchmark" "all $runs $label runs failed"
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
# 6. benchmark_native_size - temp project release build + binary size JSON
# ---------------------------------------------------------------------------

benchmark_native_size() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(6/7) Binary size benchmark"
    local project_directory="$test_workspace_root/SizeBenchTest"

    # Create a temp project and build it
    invoke_launcher -a create -n SizeBenchTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "native size benchmark" "create failed"; return
    }

    echo "Building in release mode"
    local build_output exit_code
    build_output=$(invoke_launcher -a build -p "$project_directory" -c release 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -ne 0 ]; then
        report_skip "native size benchmark" "build failed: ${build_output:0:200}"
        return
    fi

    # Measure binary sizes
    local data_directory="$project_directory/build/release/data"
    if [ -d "$data_directory" ]; then
        local total_bytes=0
        local file_count=0
        local json_entries=""
        while IFS= read -r -d '' file; do
            local size
            size=$(wc -c < "$file" 2>/dev/null || echo 0)
            total_bytes=$((total_bytes + size))
            file_count=$((file_count + 1))
            local relative_path="${file#$data_directory/}"
            if [ -n "$json_entries" ]; then
                json_entries+=$',\n'
            fi
            json_entries+="        {\"file\": \"${relative_path}\", \"bytes\": ${size}, \"megabytes\": $(awk "BEGIN { printf \"%.2f\", $size / 1048576 }")}"
        done < <(find "$data_directory" -type f -print0 2>/dev/null | sort -z)
        echo "  Binary sizes:"
        echo "  {"
        echo "    \"total_bytes\": $total_bytes,"
        echo "    \"file_count\": $file_count,"
        echo "    \"files\": ["
        echo -e "$json_entries"
        echo "    ]"
        echo "  }"
        report_pass "native size benchmark"
    else
        report_skip "native size benchmark" "no release/data/ output"
    fi
}

# ---------------------------------------------------------------------------
# 7. benchmark_wasm_size - WASM binary must be ≤ 499 KB
# ---------------------------------------------------------------------------

benchmark_wasm_size() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(7/7) WASM size budget"
    local example_path="${1:-examples/cube}"

    # Build WASM in release mode (debug sizes are meaningless)
    echo "Building WASM release - this may take a moment"
    if ! invoke_launcher -a build -p "$example_path" -t web -c release > /dev/null 2>&1; then
        report_skip "WASM size budget" "build failed"
        return
    fi

    # Find the .wasm file (launcher flattens to build/wasm/)
    local wasm_file="$example_path/build/wasm/pill_web_app_bg.wasm"

    if [ -z "$wasm_file" ]; then
        report_fail "WASM size budget" "no .wasm file found in build output"
        return
    fi

    local wasm_size
    wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
    local wasm_kb=$((wasm_size / 1024))

    echo "  WASM: $wasm_file → ${wasm_kb} KB"

    if [ "$wasm_size" -le 511000 ]; then  # 499 KB + small margin for rounding
        report_pass "WASM size within 499 KB budget (${wasm_kb} KB)"
    else
        report_fail "WASM size budget" "${wasm_kb} KB exceeds 499 KB limit"
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
    benchmark_native_size)       benchmark_native_size ;;
    benchmark_wasm_size)         benchmark_wasm_size "${2:-examples/cube}" ;;

    all|"")
        code_formatting_check
        code_linting_check
        build_native_cube_example
        build_wasm_cube_example
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
        echo "  build_wasm_cube_example      build WASM (cube) + verify artifacts"
        echo "  benchmark_native_performance build + run city (release)"
        echo "  benchmark_native_size        release build + binary size report"
        echo "  benchmark_wasm_size [path]   WASM binary ≤ 499 KB"
        exit 1
        ;;
esac

print_summary
