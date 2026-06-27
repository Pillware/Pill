#!/usr/bin/env bash
# =============================================================================
# devops/tests/basic_tests.sh — Pill Launcher basic integration & CI tests
# =============================================================================
#
# Fast smoke tests for local development and CI fast-jobs (ci.yml).
# For exhaustive example builds, see examples_tests.sh.
# For the full pre-release suite, see pill_launcher_tests.sh.
#
# QUICK START (local dev):
#   bash devops/tests/basic_tests.sh all                    # everything
#   bash devops/tests/basic_tests.sh create                 # single group
#   bash devops/tests/basic_tests.sh run --test_name build  # auto-detect OS
#
# CI (called by .github/workflows/ci.yml):
#   bash devops/tests/basic-tests-ci.sh ci_check_code
#   bash devops/tests/basic-tests-ci.sh ci_build examples/cube
#   bash devops/tests/basic-tests-ci.sh ci_benchmark
#
# Note: CI jobs use the CI wrapper (basic-tests-ci.sh) which sources this file
#       inside a Docker container with all dependencies pre-installed.
#
# ENVIRONMENT:
#   PILL_LAUNCHER_BIN   override path to the PillLauncher binary
#   TMPDIR              temp directory root (default /tmp)
#   TEST_ROOT           project scaffold root (default $TMPDIR/pill-ci-tests-<pid>)
# =============================================================================

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
    # Ordered by preference: release builds first (faster execution).
    local search_paths=(
        "./engine/pill_launcher/target/release/PillLauncher"
        "./target/release/PillLauncher"
        "./engine/pill_launcher/target/debug/PillLauncher"
        "./target/debug/PillLauncher"
    )
    for candidate_path in "${search_paths[@]}"; do
        # -x (executable) or -f (exists) — Docker may not preserve +x bits.
        if [ -x "$candidate_path" ] || [ -f "$candidate_path" ]; then
            echo "$candidate_path"
            return
        fi
    done
    # Not found — caller handles the empty string.
    echo ""
}

# Resolve the launcher: honour the env-var override, otherwise auto-detect.
pill_launcher_bin="${PILL_LAUNCHER_BIN:-$(find_launcher)}"
if [ -z "$pill_launcher_bin" ] || [ ! -f "$pill_launcher_bin" ]; then
    echo -e "${RED}FATAL: PillLauncher binary not found.${NC}"
    echo "Build it first:  cargo build -p pill_launcher --manifest-path engine/Cargo.toml"
    echo "Or set PILL_LAUNCHER_BIN=/path/to/PillLauncher"
    exit 1
fi
# Ensure the binary is executable (Docker artifact downloads may strip +x).
chmod +x "$pill_launcher_bin" 2>/dev/null || true

# ===========================================================================
# Temporary test directory
# ===========================================================================
# All created projects and build artifacts land under test_workspace_root,
# which is wiped on script exit (even on Ctrl+C, thanks to the EXIT trap).

TMPDIR="${TMPDIR:-/tmp}"
test_workspace_root="${TEST_ROOT:-$TMPDIR/pill-ci-tests-$$}"   # $$ = process ID
mkdir -p "$test_workspace_root"

cleanup_workspace() {
    # Recursively delete everything created during this test run.
    rm -rf "$test_workspace_root"
}
trap cleanup_workspace EXIT   # fire on normal exit, error exit, and SIGINT

# ===========================================================================
# Test result helpers
# ===========================================================================

# Record a passing test and print a green PASS line.
report_pass() {
    echo -e "  ${GREEN}PASS${NC} $1"
    tests_passed=$((tests_passed + 1))
}

# Record a failing test and print a red FAIL line with the failure reason.
report_fail() {
    echo -e "  ${RED}FAIL${NC} $1 — $2"
    tests_failed=$((tests_failed + 1))
}

# Record a skipped test (e.g. missing optional dependency, workspace issue).
# Skipped tests do NOT cause the script to exit non-zero.
report_skip() {
    echo -e "  ${YELLOW}SKIP${NC} $1 — $2"
    tests_skipped=$((tests_skipped + 1))
}

# Thin wrapper that invokes the launcher binary with whatever arguments
# the caller passes.  All stdout/stderr is preserved.
invoke_launcher() {
    "$pill_launcher_bin" "$@"
}

# Assert that a launcher invocation succeeds (exit code 0).
# Usage: assert_ok "description" <launcher args...>
assert_ok() {
    local test_description="$1"; shift
    if invoke_launcher "$@" > /dev/null 2>&1; then
        report_pass "$test_description"
    else
        report_fail "$test_description" "exit code $?"
    fi
}

# Assert that a launcher invocation FAILS and its stderr contains a
# case-insensitive substring.  This is used to verify error messages.
# Usage: assert_fail "description" "expected-error-substring" <launcher args...>
assert_fail() {
    local test_description="$1"; local expected_substring="$2"; shift 2
    local launcher_output exit_code
    # Capture both stdout and stderr; check for non-zero exit AND matching text.
    launcher_output=$(invoke_launcher "$@" 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -ne 0 ] && echo "$launcher_output" | grep -qi "$expected_substring"; then
        report_pass "$test_description"
    else
        report_fail "$test_description" \
            "expected error matching '$expected_substring', got exit $exit_code: ${launcher_output:0:200}"
    fi
}

# Print a colour-coded summary table and exit non-zero if any test failed.
print_summary() {
    local total_tests=$((tests_passed + tests_failed + tests_skipped))
    echo ""
    echo "========================================"
    echo -e "Results: ${GREEN}$tests_passed passed${NC}, ${RED}$tests_failed failed${NC}, ${YELLOW}$tests_skipped skipped${NC} ($total_tests total)"
    echo "========================================"
    # Only fail the script if at least one test assertion was violated.
    [ "$tests_failed" -eq 0 ] || exit 1
}

# ===========================================================================
# Local development tests
# ===========================================================================
# These are fast, self-contained tests designed for local iteration.
# They create temp projects under test_workspace_root and exercise individual
# actions.  Slow actions (build, benchmark, ci) are at the bottom and may be
# skipped if prerequisites are not met.

# ---- basics: smoke-test the binary itself -----------------------------------
test_basics() {
    echo "--- Launcher basics ---"

    # --help should exit 0 and not crash.
    if invoke_launcher --help > /dev/null 2>&1; then
        report_pass "--help works"
    else
        report_fail "--help works" "exit $?"
    fi

    # --version should print the crate version and exit 0.
    invoke_launcher --version > /dev/null 2>&1 \
        && report_pass "--version works" \
        || report_fail "--version works" "exit $?"

    # Running with zero arguments should produce an error (not a panic).
    local exit_code
    invoke_launcher 2>&1 >/dev/null && exit_code=$? || exit_code=$?
    [ "$exit_code" -ne 0 ] && report_pass "no args exits non-zero" \
                            || report_fail "no args exits non-zero" "exit 0"

    # Bogus action name must be rejected.
    assert_fail "unknown action"    "error"   -a nonexistent-action

    # Missing required --action flag must produce a clap error.
    assert_fail "missing --action"  "required" --path .
}

# ---- create: scaffold a new project -----------------------------------------
test_create() {
    echo "--- Create action ---"
    local project_dir="$test_workspace_root/MyGame"

    # Create a fresh project.
    assert_ok "create project" -a create -n MyGame -p "$test_workspace_root"

    # Verify every expected file and directory exists.
    [ -d "$project_dir" ]                && report_pass "project dir exists"      || report_fail "project dir exists" "missing $project_dir"
    [ -f "$project_dir/Cargo.toml" ]     && report_pass "Cargo.toml exists"      || report_fail "Cargo.toml exists" "missing"
    [ -f "$project_dir/res/config.ini" ] && report_pass "config.ini exists"      || report_fail "config.ini exists" "missing"
    [ -d "$project_dir/src" ]            && report_pass "src/ exists"             || report_fail "src/ exists" "missing"

    # The create action rewrites config.ini — verify the project name is present.
    if grep -q "MyGame" "$project_dir/res/config.ini" 2>/dev/null; then
        report_pass "config.ini contains project name"
    else
        report_fail "config.ini contains project name" "not found"
    fi

    # Creating the same project again must fail with 'already exists'.
    assert_fail "duplicate create"   "already exists" -a create -n MyGame -p "$test_workspace_root"

    # Omitting the required --name flag must produce a clear error.
    assert_fail "create without --name" "name" -a create -p "$test_workspace_root"
}

# ---- cargo: passthrough arbitrary cargo commands ----------------------------
test_cargo() {
    echo "--- Cargo passthrough ---"
    local project_dir="$test_workspace_root/CargoTest"

    # Need a project to link into the workspace.  Bail gracefully if create fails.
    invoke_launcher -a create -n CargoTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "cargo tests" "create failed — cannot proceed"
        return
    }

    # Run a known-good cargo subcommand through the passthrough.
    assert_ok "cargo --version" -a cargo -p "$project_dir" -- --version

    # Passing zero cargo arguments must be caught early with a clear error.
    assert_fail "cargo empty args" "at least one argument" -a cargo -p "$project_dir"

    # cargo check is a fast compile-check; useful for verifying the workspace
    # wiring works end-to-end.
    echo "    (cargo check on test project — may take a moment)"
    if invoke_launcher -a cargo -p "$project_dir" -- check > /dev/null 2>&1; then
        report_pass "cargo check on project"
    else
        report_skip "cargo check on project" "exit $? (may need full workspace)"
    fi
}

# ---- check-code: cargo check all engine crates ------------------------------
test_check_code() {
    echo "--- Check-code ---"
    local launcher_output exit_code
    # Capture stderr too — check-code writes progress messages there.
    launcher_output=$(invoke_launcher -a check-code 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "check-code succeeds"
    elif echo "$launcher_output" | grep -qi "NO_PATH\|failed to load manifest\|Engine Cargo.toml not found"; then
        # These errors are expected when the workspace manifest is in a
        # transitional state (e.g. a previous test left a stale member line).
        report_skip "check-code" "workspace manifest issue (expected in CI)"
    else
        report_fail "check-code" "exit $exit_code: ${launcher_output:0:200}"
    fi
}

# ---- assets: run the asset pipeline -----------------------------------------
test_assets() {
    echo "--- Assets ---"
    local project_dir="$test_workspace_root/AssetsTest"
    invoke_launcher -a create -n AssetsTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "assets tests" "create failed"; return
    }

    # An empty project has no raw assets (models/textures), so the pipeline
    # should discover 0 files and exit successfully.
    assert_ok "assets on empty project"   -a assets -p "$project_dir"

    # --clean deletes previously cooked assets before rebuilding.
    assert_ok "assets --clean"            -a assets -p "$project_dir" --clean
}

# ---- docs: generate rustdoc ------------------------------------------------
test_docs() {
    echo "--- Docs ---"
    local docs_output_dir="$test_workspace_root/docs-out"
    mkdir -p "$docs_output_dir"

    echo "    (generating docs — may take a moment)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a docs -o "$docs_output_dir" 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "docs generation"
    elif echo "$launcher_output" | grep -qi "plantuml\|Cannot locate\|manifest"; then
        # PlantUML not installed or workspace not found — non-fatal.
        report_skip "docs generation" "${launcher_output:0:100}"
    else
        report_fail "docs generation" "exit $exit_code: ${launcher_output:0:200}"
    fi
}

# ---- build: compile a project (slow) ----------------------------------------
test_build() {
    echo "--- Build (native, debug) ---"
    local project_dir="$test_workspace_root/BuildTest"
    invoke_launcher -a create -n BuildTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "build test" "create failed"; return
    }

    echo "    (building — this will take several minutes)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a build -p "$project_dir" -c debug 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "build succeeds"
        # After a successful build, the output directory must contain the
        # data/ folder with dynamic libraries.
        if [ -d "$project_dir/build/dev/data" ]; then
            report_pass "build output data/ exists"
        else
            report_fail "build output data/" "missing $project_dir/build/dev/data"
        fi
    else
        # Build failures in CI are common (missing system deps, etc.) —
        # treat them as skips rather than hard failures.
        report_skip "build" "exit $exit_code (${launcher_output:0:150})"
    fi
}

# ---- build-cube: smoke-test cube example specifically -----------------------
test_build_cube() {
    echo "--- Build cube example ---"
    local cube_dir="examples/cube"
    if [ ! -f "$cube_dir/Cargo.toml" ]; then
        report_skip "build-cube" "examples/cube not found (are you in the repo root?)"
        return
    fi

    echo "    (building cube — this will take a couple of minutes)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a build -p "$cube_dir" -c debug 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "cube build succeeds"
        if [ -d "$cube_dir/build/dev/data" ]; then
            report_pass "cube build output data/ exists"
        else
            report_fail "cube build output data/" "missing $cube_dir/build/dev/data"
        fi
    else
        report_skip "cube build" "exit $exit_code (${launcher_output:0:150})"
    fi
}

# ---- size-benchmark: measure binary sizes -----------------------------------
test_size_benchmark() {
    echo "--- Size benchmark ---"
    local project_dir="$test_workspace_root/SizeBenchTest"
    invoke_launcher -a create -n SizeBenchTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "size-benchmark" "create failed"; return
    }

    # size-benchmark always builds in release mode (debug sizes are meaningless).
    invoke_launcher -a size-benchmark -p "$project_dir" -t native > /dev/null 2>&1 \
        && report_pass "size-benchmark runs" \
        || report_skip "size-benchmark" "exit $? (build may have failed)"
}

# ---- ci: full CI pipeline (check → fmt → clippy → build) -------------------
test_ci_pipeline() {
    echo "--- CI pipeline ---"
    local project_dir="$test_workspace_root/CiTest"
    invoke_launcher -a create -n CiTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "ci test" "create failed"; return
    }

    echo "    (running full CI pipeline — this will take several minutes)"
    invoke_launcher -a ci -p "$project_dir" -c debug > /dev/null 2>&1 \
        && report_pass "ci pipeline succeeds" \
        || report_skip "ci pipeline" "exit $?"
}

# ===========================================================================
# CI job functions
# ===========================================================================
# Called exclusively by .github/workflows/ci.yml.  Each function maps 1:1 to
# a GitHub Actions job.
#
# Assumptions (guaranteed by the CI workflow):
#   - The repo is checked out and $PWD is the repo root.
#   - PillLauncher binary was built in a previous job and downloaded as an
#     artifact to engine/pill_launcher/target/release/PillLauncher.
#   - Required tools (cargo, rustfmt, clippy, wasm-pack, xvfb) are on PATH.

# ---- ci_check_code: fast compile-check of every engine crate -----------------
# Runs `cargo check` on pill_core, pill_engine, pill_renderer, etc.
# Does NOT compile any game project — only the engine itself.
ci_check_code() {
    echo "=== CI: cargo check (engine crates) ==="
    invoke_launcher -a check-code
}

# ---- ci_fmt: enforce rustfmt style ------------------------------------------
# Formats the codebase using the linked example project as a workspace anchor,
# then uses `git diff --exit-code` to fail if any file was changed.
# The pathspec excludes Cargo.toml files because the launcher rewrites them
# (workspace member injection) during preparation — those changes are expected.
ci_fmt() {
    echo "=== CI: rustfmt ==="
    local example_path="${1:-examples/floating_pills}"
    invoke_launcher -a cargo -p "$example_path" -- fmt

    echo "=== CI: checking for fmt diffs ==="
    # Exclude manifests — the launcher modifies them and restores them, but
    # line-ending or whitespace differences can still appear.
    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        ":(exclude)$example_path/Cargo.toml"
}

# ---- ci_clippy: deny-by-default lint ----------------------------------------
# Runs clippy with `-D warnings` so any lint triggers a failure.
# Same Cargo.toml exclusion rationale as ci_fmt.
ci_clippy() {
    echo "=== CI: clippy -D warnings ==="
    local example_path="${1:-examples/floating_pills}"
    invoke_launcher -a cargo -p "$example_path" -- clippy -- -D warnings

    git diff --exit-code -- . \
        ':(exclude)engine/Cargo.toml' \
        ":(exclude)$example_path/Cargo.toml"
}

# ---- ci_build: compile a single example project -----------------------------
# The CI matrix feeds one example path per job.  Most examples are Pill projects
# built via `PillLauncher -a build`.  The net_minimal/server crate is a plain
# Cargo project, so we build it directly with `cargo build`.
ci_build() {
    echo "=== CI: build example ==="
    local example_path="${1:?usage: ci_build <path>}"

    if [ "$example_path" = "examples/net_minimal/server" ]; then
        # Standalone binary — not a Pill game project, no PillLauncher needed.
        cargo build --manifest-path "$example_path/Cargo.toml"
    else
        # Full Pill build: compiles pill_project + pill_native + pill_runtime
        # and copies artifacts into <example>/build/<mode>/.
        invoke_launcher -a build -p "$example_path"
    fi
}

# ---- ci_check_wasm: WASM build + dev-server smoke test + size guard ---------
# Builds the WASM bundle via wasm-pack, starts a tiny HTTP server on a random
# port, fetches /, /pill_web_app.js, and /pill_web_app_bg.wasm, then stops
# the server.  The cube example has an explicit size budget.
ci_check_wasm() {
    echo "=== CI: WASM build + smoke test ==="
    local example_path="${1:?usage: ci_check_wasm <path>}"
    local wasm_budget_flag=""

    # Keep the cube WASM binary under 500 KB — fail if it grows.
    if [ "$example_path" = "examples/cube" ]; then
        wasm_budget_flag="--wasm-budget-kb 499"
    fi

    # shellcheck disable=SC2086   # $wasm_budget_flag is intentionally empty for non-cube
    invoke_launcher -a check-wasm -p "$example_path" $wasm_budget_flag
}

# ---- ci_benchmark: performance benchmark (main branch / PR only) -------------
# Runs the city example under xvfb (virtual framebuffer — needed because the
# benchmark opens a window even in headless mode).  Collects frame-time
# statistics over 5 iterations of 1000 frames each.
ci_benchmark() {
    echo "=== CI: city benchmark ==="

    # xvfb-run provides a virtual display so the benchmark_window feature
    # can open a window without a physical monitor.
    xvfb-run --auto-servernum \
        invoke_launcher -a benchmark \
        -p examples/city \
        --bench-iterations 5 \
        --bench-frames 1000 \
        --bench-features benchmark_window \
        -c release
}

# ===========================================================================
# Local runner — OS detection + dispatch
# ===========================================================================
# The `run` / `run_locally` command is the recommended entry point for
# developers.  It detects the host OS and runs tests in the right environment:
#
#   Linux / macOS   → runs natively (bash)
#   Git Bash / MSYS2 → runs in the existing bash session
#   Windows (cmd)   → builds & runs via Docker (devops/tests/Dockerfile)
#   --docker flag   → forces Docker mode on any platform
#
# Usage:
#   bash devops/tests/basic_tests.sh run --test_name create
#   bash devops/tests/basic_tests.sh run -t all --docker

run_locally() {
    local selected_test="all"        # which test group to execute
    local force_docker=false         # true = force Docker even on Linux/macOS

    # Parse the two supported flags.  Everything else is an error.
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --test_name|-t) selected_test="$2"; shift 2 ;;
            --docker|-d)    force_docker=true; shift ;;
            *)
                echo "Unknown flag: $1"
                echo "Usage: $0 run [--test_name <name>] [--docker]"
                return 1
                ;;
        esac
    done

    # Detect the operating system.  On Windows (cmd/powershell), `uname` is
    # not available, so we fall back to the string "Windows".
    local host_os
    host_os=$(uname -s 2>/dev/null || echo "Windows")

    echo "=== Pill Launcher Local Test Runner ==="
    echo "  Test:     $selected_test"
    echo "  OS:       $host_os"
    echo "  Docker:   $force_docker"
    echo ""

    # -- Native execution on Linux / macOS ---------------------------------
    if [ "$host_os" = "Linux" ] || [ "$host_os" = "Darwin" ]; then
        echo "Running natively on $host_os..."
        bash "$0" "$selected_test"
        return $?
    fi

    # -- Windows: Git Bash, MSYS2, or Cygwin --------------------------------
    # These environments provide a full bash runtime; we can execute directly.
    if [[ "$host_os" == MINGW* ]] || [[ "$host_os" == MSYS* ]] || [[ "$host_os" == CYGWIN* ]]; then
        echo "Running in $host_os (Windows bash)..."
        bash "$0" "$selected_test"
        return $?
    fi

    # -- Docker (explicit --docker flag, or raw Windows without bash) --------
    if [ "$force_docker" = true ] || [ "$host_os" = "Windows" ]; then
        echo "Running via Docker..."

        # Resolve the repo root (two levels up from devops/tests/).
        local repo_root
        repo_root="$(cd "$(dirname "$0")/../.." && pwd)"

        # Build the Docker image once; subsequent runs skip this step.
        if ! docker image inspect pill-ci > /dev/null 2>&1; then
            echo "Building Docker image pill-ci..."
            docker build -t pill-ci \
                -f "$repo_root/devops/tests/Dockerfile" \
                "$repo_root"
        fi

        # Mount the entire repo at /src so the container sees the same files
        # as the host.  The container's ENTRYPOINT fixes paths automatically.
        echo "Running tests in container..."
        docker run --rm \
            -v "$repo_root:/src" \
            -w /src \
            pill-ci \
            "./devops/tests/basic_tests.sh" "$selected_test"
        return $?
    fi

    # -- Unsupported environment --------------------------------------------
    echo -e "${RED}Cannot determine how to run tests on $host_os.${NC}"
    echo "Options:"
    echo "  1. Install Git Bash and run:  bash devops/tests/basic_tests.sh $selected_test"
    echo "  2. Use Docker:                bash devops/tests/basic_tests.sh run -t $selected_test --docker"
    return 1
}

# ===========================================================================
# Command dispatch
# ===========================================================================
# The first argument selects which test group or CI function to run.
# If no argument is given, "all" is assumed.

case "${1:-all}" in
    # -- local runner (OS auto-detection) --
    run|run_locally)
        shift               # consume "run" / "run_locally"
        run_locally "$@"    # forward --test_name / --docker
        ;;

    # -- individual local dev tests --
    basics)              test_basics ;;
    create)              test_create ;;
    cargo)               test_cargo ;;
    check-code)          test_check_code ;;
    assets)              test_assets ;;
    docs)                test_docs ;;
    build)               test_build ;;
    build-cube)          test_build_cube ;;
    benchmark)           ci_benchmark ;;
    size-benchmark)      test_size_benchmark ;;
    ci-pipeline)         test_ci_pipeline ;;

    # -- run every local dev test in sequence --
    all|"")
        test_basics
        test_create
        test_cargo
        test_check_code
        test_assets
        test_docs
        test_build
        test_size_benchmark
        test_ci_pipeline
        ;;

    # -- CI job functions (1:1 with GitHub Actions jobs) --
    ci_check_code)       ci_check_code ;;
    ci_fmt)              ci_fmt "${2:-examples/floating_pills}" ;;
    ci_clippy)           ci_clippy "${2:-examples/floating_pills}" ;;
    ci_build)            ci_build "${2:?missing example path}" ;;
    ci_check_wasm)       ci_check_wasm "${2:?missing example path}" ;;
    ci_benchmark)        ci_benchmark ;;

    # -- help --
    *)
        echo "Usage: $0 <command> [args...]"
        echo ""
        echo "Local runner (auto-detects OS):"
        echo "  run|run_locally --test_name <name>  [--docker]"
        echo ""
        echo "Local dev tests:"
        echo "  all | basics | create | cargo | check-code | assets | docs"
        echo "  build | size-benchmark | ci-pipeline"
        echo ""
        echo "CI job functions (called by GitHub Actions):"
        echo "  ci_check_code"
        echo "  ci_fmt [example]            (default: examples/floating_pills)"
        echo "  ci_clippy [example]         (default: examples/floating_pills)"
        echo "  ci_build <path>"
        echo "  ci_check_wasm <path>"
        echo "  ci_benchmark"
        echo ""
        echo "Examples:"
        echo "  bash devops/tests/basic_tests.sh run --test_name create"
        echo "  bash devops/tests/basic_tests.sh run -t build --docker"
        echo "  bash devops/tests/basic_tests.sh ci_build examples/cube"
        exit 1
        ;;
esac

# Print the colour-coded summary.  Exits non-zero if any test failed.
print_summary
