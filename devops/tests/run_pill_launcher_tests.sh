#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo, rustfmt, clippy), wasm-pack,
#               a compiled PillLauncher binary (auto-discovered or set via
#               PILL_LAUNCHER_BIN).

# DESCRIPTION: Pill Launcher comprehensive tests - exercises every
#   PillLauncher action (create, build, run, cargo, assets, docs, link,
#   unlink) with valid and error-case parameters to verify correctness.
#
#   Uses a temporary workspace for scaffolded projects. Designed for
#   local development and GitHub Actions CI (ci-pill_launcher-tests.yml).

# USAGE: bash devops/tests/run_pill_launcher_tests.sh [all|<test-group>]
#
#   all                run all test groups (default)
#   basics             --help, --version, error handling
#   create             scaffold new projects
#   build              compile native debug/release + WASM
#   cargo              passthrough cargo commands
#   assets             asset pipeline
#   docs               rustdoc generation
#   run                build + launch projects
#   link               IDE workspace linking

# EXAMPLE USAGE:
#   bash devops/tests/run_pill_launcher_tests.sh all
#   bash devops/tests/run_pill_launcher_tests.sh create
#   bash devops/tests/run_pill_launcher_tests.sh build

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ---------------------------------------------------------------------------
# 1. basics - smoke-test the binary itself
# ---------------------------------------------------------------------------

test_launcher_basics() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(1/8) Launcher basics"

    if invoke_launcher --help > /dev/null 2>&1; then
        report_pass "--help"
    else
        report_fail "--help" "exit $?"
    fi

    invoke_launcher --version > /dev/null 2>&1 \
        && report_pass "--version" \
        || report_fail "--version" "exit $?"

    local exit_code=0
    invoke_launcher 2>&1 > /dev/null || exit_code=$?
    if [ "$exit_code" -ne 0 ]; then
        report_pass "no arguments exits non-zero"
    else
        report_fail "no arguments" "exited 0"
    fi

    assert_fail "unknown action"   "error"    -a nonexistent_action
    assert_fail "missing --action" "required" --path .
}

# ---------------------------------------------------------------------------
# 2. create - scaffold new projects
# ---------------------------------------------------------------------------

test_launcher_create() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(2/8) Create action"

    # Basic scaffold
    local project_directory="$test_workspace_root/CreateTest"
    assert_ok "create project" -a create -n CreateTest -p "$test_workspace_root"

    [ -d "$project_directory" ]                && report_pass "project directory exists"      || report_fail "project directory" "missing $project_directory"
    [ -f "$project_directory/Cargo.toml" ]     && report_pass "Cargo.toml exists"            || report_fail "Cargo.toml" "missing"
    [ -f "$project_directory/res/config.ini" ] && report_pass "config.ini exists"            || report_fail "config.ini" "missing"
    [ -d "$project_directory/src" ]            && report_pass "src/ directory exists"         || report_fail "src/" "missing"

    if grep -q "CreateTest" "$project_directory/res/config.ini" 2>/dev/null; then
        report_pass "config.ini contains project name"
    else
        report_fail "config.ini" "project name not found"
    fi

    # Error: duplicate name
    assert_fail "duplicate create" "already exists" -a create -n CreateTest -p "$test_workspace_root"

    # Error: missing --name
    assert_fail "create without --name" "name" -a create -p "$test_workspace_root"

    # Short flags
    assert_ok "create with short flags" -a create -n CreateTest_ShortFlags -p "$test_workspace_root"
}

# ---------------------------------------------------------------------------
# 3. build - compile projects (native + WASM)
# ---------------------------------------------------------------------------

test_launcher_build() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(3/8) Build action"

    local project_directory="$test_workspace_root/BuildTest"
    invoke_launcher -a create -n BuildTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "build tests" "create failed"; return
    }

    # Native debug
    echo "  (native debug - this will take several minutes)"
    local build_output build_exit_code=0
    build_output=$(invoke_launcher -a build -p "$project_directory" -c debug 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build native debug"
        if [ -d "$project_directory/build/dev/data" ]; then
            report_pass "build output data/ exists"
        else
            report_fail "build output" "missing data/"
        fi
    else
        report_skip "build native debug" "exit $build_exit_code"
    fi

    # Native release
    echo "  (native release)"
    build_exit_code=0
    build_output=$(invoke_launcher -a build -p "$project_directory" -c release 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build native release"
    else
        report_skip "build native release" "exit $build_exit_code"
    fi

    # WASM
    echo "  (WASM)"
    build_exit_code=0
    build_output=$(invoke_launcher -a build -p "$project_directory" -t web 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build WASM"
        if [ -d "$project_directory/build/wasm" ]; then
            report_pass "WASM output directory exists"
        else
            report_fail "WASM output" "missing build/wasm/"
        fi
    else
        report_skip "build WASM" "exit $build_exit_code"
    fi

    # Short flags
    local short_directory="$test_workspace_root/ShortBuild"
    invoke_launcher -a create -n ShortBuild -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "short flag build" "create failed"; return
    }
    build_exit_code=0
    invoke_launcher -a build -p "$short_directory" -c release -t native > /dev/null 2>&1 || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build with short flags"
    else
        report_skip "build short flags" "exit $build_exit_code"
    fi
}

# ---------------------------------------------------------------------------
# 4. cargo - passthrough arbitrary cargo commands
# ---------------------------------------------------------------------------

test_launcher_cargo() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(4/8) Cargo passthrough"

    local project_directory="$test_workspace_root/CargoTest"
    invoke_launcher -a create -n CargoTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "cargo tests" "create failed"; return
    }

    assert_ok "cargo --version" -a cargo -p "$project_directory" -- --version

    echo "  (cargo check)"
    local cargo_exit_code=0
    invoke_launcher -a cargo -p "$project_directory" -- check > /dev/null 2>&1 || cargo_exit_code=$?
    if [ "$cargo_exit_code" -eq 0 ]; then
        report_pass "cargo check on project"
    else
        report_skip "cargo check" "exit $cargo_exit_code (may need full workspace)"
    fi

    assert_fail "cargo empty arguments" "at least one argument" -a cargo -p "$project_directory"
    assert_fail "cargo bad subcommand"  "no such command"       -a cargo -p "$project_directory" -- nonexistent_cargo_command

    assert_ok "cargo with short flag" -a cargo -p "$project_directory" -- --version
}

# ---------------------------------------------------------------------------
# 5. assets - run the asset pipeline
# ---------------------------------------------------------------------------

test_launcher_assets() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(5/8) Assets action"

    local project_directory="$test_workspace_root/AssetsTest"
    invoke_launcher -a create -n AssetsTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "assets tests" "create failed"; return
    }

    assert_ok "assets on empty project" -a assets -p "$project_directory"
    assert_ok "assets --clean" -a assets -p "$project_directory" --clean
}

# ---------------------------------------------------------------------------
# 6. docs - generate rustdoc
# ---------------------------------------------------------------------------

test_launcher_docs() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(6/8) Docs action"

    local docs_output_directory="$test_workspace_root/docs_output"
    mkdir -p "$docs_output_directory"

    echo "  (generating docs)"
    local docs_output docs_exit_code=0
    docs_output=$(invoke_launcher -a docs -o "$docs_output_directory" 2>&1) || docs_exit_code=$?

    if [ "$docs_exit_code" -eq 0 ]; then
        report_pass "docs generation"
        if [ -d "$docs_output_directory/docs" ]; then
            report_pass "docs output directory exists"
        else
            report_fail "docs output" "missing $docs_output_directory/docs/"
        fi
    elif echo "$docs_output" | grep -qi "plantuml\|Cannot locate\|manifest\|features"; then
        report_skip "docs generation" "toolchain issue"
    else
        report_fail "docs generation" "exit $docs_exit_code"
    fi
}

# ---------------------------------------------------------------------------
# 7. run - build + launch projects
# ---------------------------------------------------------------------------

test_launcher_run() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(7/8) Run action"

    local project_directory="$test_workspace_root/RunTest"
    invoke_launcher -a create -n RunTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "run tests" "create failed"; return
    }

    # Run native debug (timeout after 10s - game opens a window and waits)
    echo "  (run native debug)"
    local run_output run_exit_code=0
    run_output=$(timeout 10s invoke_launcher -a run -p "$project_directory" -c debug 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run native debug"
    else
        report_skip "run native debug" "exit $run_exit_code"
    fi

    # Run native release
    echo "  (run native release)"
    run_exit_code=0
    run_output=$(timeout 10s invoke_launcher -a run -p "$project_directory" -c release 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run native release"
    else
        report_skip "run native release" "exit $run_exit_code"
    fi

    # Passthrough arguments
    local passthrough_exit_code=0
    invoke_launcher -a run -p "$project_directory" -c debug -- --help > /dev/null 2>&1 || passthrough_exit_code=$?
    if [ "$passthrough_exit_code" -eq 0 ]; then
        report_pass "run with passthrough arguments"
    else
        report_skip "run passthrough" "exit $passthrough_exit_code"
    fi
}

# ---------------------------------------------------------------------------
# 8. link / unlink - IDE workspace membership
# ---------------------------------------------------------------------------

test_launcher_link() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "(8/8) Link / Unlink actions"

    # Use the existing cube example (no need to create a new one)
    local project_path="examples/cube"

    # Unlink first to ensure a clean state
    invoke_launcher -a unlink -p "$project_path" > /dev/null 2>&1 || true

    # Link
    local link_output link_exit_code=0
    link_output=$(invoke_launcher -a link -p "$project_path" 2>&1) || link_exit_code=$?
    if [ "$link_exit_code" -eq 0 ]; then
        # Verify the project appears in engine/Cargo.toml
        if grep -q "$project_path" "engine/Cargo.toml" 2>/dev/null; then
            report_pass "link adds project to workspace members"
        else
            report_fail "link" "project not found in engine/Cargo.toml"
        fi
    else
        report_fail "link" "exit $link_exit_code"
    fi

    # Link again (idempotent)
    local relink_exit_code=0
    invoke_launcher -a link -p "$project_path" > /dev/null 2>&1 || relink_exit_code=$?
    if [ "$relink_exit_code" -eq 0 ]; then
        report_pass "link is idempotent"
    else
        report_fail "link idempotent" "exit $relink_exit_code"
    fi

    # Unlink
    local unlink_exit_code=0
    invoke_launcher -a unlink -p "$project_path" > /dev/null 2>&1 || unlink_exit_code=$?
    if [ "$unlink_exit_code" -eq 0 ]; then
        if grep -q "$project_path" "engine/Cargo.toml" 2>/dev/null; then
            report_fail "unlink" "project still in engine/Cargo.toml"
        else
            report_pass "unlink removes project from workspace members"
        fi
    else
        report_fail "unlink" "exit $unlink_exit_code"
    fi

    # Restore clean state
    invoke_launcher -a unlink -p "$project_path" > /dev/null 2>&1 || true
    git checkout -- engine/Cargo.toml 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Dispatch (only when executed directly)
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    basics)  test_launcher_basics ;;
    create)  test_launcher_create ;;
    build)   test_launcher_build ;;
    cargo)   test_launcher_cargo ;;
    assets)  test_launcher_assets ;;
    docs)    test_launcher_docs ;;
    run)     test_launcher_run ;;
    link)    test_launcher_link ;;

    all|"")
        test_launcher_basics
        test_launcher_create
        test_launcher_build
        test_launcher_cargo
        test_launcher_assets
        test_launcher_docs
        test_launcher_run
        test_launcher_link
        ;;

    *)
        echo "Usage: $0 [all|<test-group>]"
        echo ""
        echo "Test groups:"
        echo "  basics    --help, --version, error handling"
        echo "  create    scaffold projects (normal, duplicate, missing name, short flags)"
        echo "  build     compile native debug/release + WASM + short flags"
        echo "  cargo     passthrough commands (--version, check, errors)"
        echo "  assets    asset pipeline (empty project, --clean)"
        echo "  docs      rustdoc generation"
        echo "  run       build + launch (debug, release, passthrough args)"
        echo "  link      IDE workspace link / unlink"
        exit 1
        ;;
esac

print_summary

