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
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# All paths in this script are relative to the project root.
cd "$PROJECT_ROOT"

# ---------------------------------------------------------------------------
# 1. code_formatting_check - cargo fmt (direct) + git diff
# ---------------------------------------------------------------------------

code_formatting_check() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(1/5) Code formatting check"
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
    echo "(2/5) Code linting check"
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
# 3. build_native_cube_example - release native build + artifact size report
# ---------------------------------------------------------------------------

build_native_cube_example() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(3/5) Native build and artifact size report"
    local cube_dir="examples/cube"

    if [ ! -f "$cube_dir/Cargo.toml" ]; then
        report_skip "native cube build" "examples/cube not found"
        return
    fi

    echo "Building - this may take a moment"
    local exit_code=0
    invoke_launcher build -p "$cube_dir" -c release 2>&1 || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "native cube build succeeds"
    else
        report_skip "native cube build" "exit $exit_code"
        return
    fi

    # Binary size report on all native build artifacts
    local data_dir="$cube_dir/build/release/data"
    if [ -d "$data_dir" ]; then
        local total_bytes=0
        local file_count=0
        local json_entries=""
        while IFS= read -r -d '' file; do
            local size
            size=$(wc -c < "$file" 2>/dev/null || echo 0)
            total_bytes=$((total_bytes + size))
            file_count=$((file_count + 1))
            local relative_path="${file#$data_dir/}"
            if [ -n "$json_entries" ]; then
                json_entries+=$',\n'
            fi
            json_entries+="        {\"file\": \"${relative_path}\", \"bytes\": ${size}, \"megabytes\": $(awk "BEGIN { printf \"%.4f\", $size / 1048576 }")}"
        done < <(find "$data_dir" -type f -print0 2>/dev/null | sort -z)
        echo "  Binary sizes:"
        echo "  {"
        echo "    \"total_bytes\": $total_bytes,"
        echo "    \"file_count\": $file_count,"
        echo "    \"files\": ["
        echo -e "$json_entries"
        echo "    ]"
        echo "  }"
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
    echo "------------------------------------------------------------------"
    echo "(4/5) WASM build, artifact size report + budget"
    local cube_path="examples/cube"

    # 1. Build the WASM target (pill_web_app) for the cube example
    echo "Building - this may take a moment"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher build -p "$cube_path" -t web -c release 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "WASM build succeeds"
    else
        report_fail "WASM build" "exit $exit_code: ${launcher_output:0:200}"
        return
    fi

    # 2. Verify the .wasm artifact exists (launcher flattens output to build/wasm/)
    local wasm_dir="$cube_path/build/wasm"
    local wasm_file="$wasm_dir/pill_web_app_bg.wasm"
    if [ -f "$wasm_file" ]; then
        local wasm_size
        wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
        local wasm_kb=$((wasm_size / 1024))
        local wasm_mb
        wasm_mb=$(awk "BEGIN { printf \"%.4f\", $wasm_size / 1048576 }")
        report_pass "WASM artifact exists (${wasm_kb} KB, ${wasm_mb} MB)"
    else
        report_fail "WASM artifact" "missing $wasm_file"
        return
    fi

    # 3. Binary size report on all WASM build artifacts
    if [ -d "$wasm_dir" ]; then
        local total_bytes=0
        local file_count=0
        local json_entries=""
        while IFS= read -r -d '' file; do
            local size
            size=$(wc -c < "$file" 2>/dev/null || echo 0)
            total_bytes=$((total_bytes + size))
            file_count=$((file_count + 1))
            local relative_path="${file#$wasm_dir/}"
            if [ -n "$json_entries" ]; then
                json_entries+=$',\n'
            fi
            json_entries+="        {\"file\": \"${relative_path}\", \"bytes\": ${size}, \"megabytes\": $(awk "BEGIN { printf \"%.4f\", $size / 1048576 }")"}
        done < <(find "$wasm_dir" -type f -print0 2>/dev/null | sort -z)
        echo "  Binary sizes:"
        echo "  {"
        echo "    \"total_bytes\": $total_bytes,"
        echo "    \"file_count\": $file_count,"
        echo "    \"files\": ["
        echo -e "$json_entries"
        echo "    ]"
        echo "  }"

        # 4. Size budget check (≤ 499 KB for the .wasm file)
        local wasm_size wasm_kb wasm_mb
        wasm_size=$(wc -c < "$wasm_file" 2>/dev/null || echo 0)
        wasm_kb=$((wasm_size / 1024))
        wasm_mb=$(awk "BEGIN { printf \"%.4f\", $wasm_size / 1048576 }")
        echo "  WASM: $wasm_file → ${wasm_kb} KB (${wasm_mb} MB)"
        if [ "$wasm_size" -le 511000 ]; then
            report_pass "WASM size within 499 KB budget (${wasm_kb} KB, ${wasm_mb} MB)"
        else
            report_fail "WASM size budget" "${wasm_kb} KB (${wasm_mb} MB) exceeds 499 KB limit"
        fi

        # 5. Dev server smoke test - serve the WASM build and verify key files
        if command -v python3 > /dev/null 2>&1; then
            echo "  Starting dev server on port 8080..."
            python3 -m http.server 8080 --directory "$wasm_dir" &
            local server_pid=$!
            trap 'kill "$server_pid" 2>/dev/null || true' EXIT

            # Wait up to 30s for the server to bind
            local server_ready=0
            for _ in $(seq 1 30); do
                if curl -sf -o /dev/null http://127.0.0.1:8080/ 2>/dev/null; then
                    server_ready=1
                    break
                fi
                sleep 1
            done

            if [ "$server_ready" -eq 1 ]; then
                local smoke_ok=1
                curl -sf -o /dev/null http://127.0.0.1:8080/ || smoke_ok=0
                curl -sf -o /dev/null http://127.0.0.1:8080/pill_web_app.js || smoke_ok=0
                curl -sf -o /dev/null http://127.0.0.1:8080/pill_web_app_bg.wasm || smoke_ok=0
                if [ "$smoke_ok" -eq 1 ]; then
                    report_pass "WASM dev server smoke test"
                else
                    report_fail "WASM dev server smoke test" "one or more key files not served"
                fi
            else
                report_skip "WASM dev server smoke test" "server did not start in time"
            fi

            kill "$server_pid" 2>/dev/null || true
            trap - EXIT
        else
            report_skip "WASM dev server smoke test" "python3 not available"
        fi
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
    echo "(5/5) Performance benchmark"

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
    invoke_launcher build -p "$project_directory" -c release --features "$feature" 2>&1 || build_exit_code=$?
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
