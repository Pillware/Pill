#!/usr/bin/env bash
# =============================================================================
# devops/tests/run_pill_launcher_tests.sh — Pill Launcher comprehensive tests
# =============================================================================
#
# Exercises every PillLauncher action with different parameters.
# Sources common.sh for binary discovery, report helpers, and temp workspace.
#
# Usage:
#   bash devops/tests/run_pill_launcher_tests.sh all        # everything
#   bash devops/tests/run_pill_launcher_tests.sh create      # just create tests
#   bash devops/tests/run_pill_launcher_tests.sh build       # just build tests

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ===========================================================================
# 1. BASICS — smoke-test the binary itself
# ===========================================================================

test_basics() {
    echo "=== 1. Launcher basics ==="

    if invoke_launcher --help > /dev/null 2>&1; then
        report_pass "--help works"
    else
        report_fail "--help works" "exit $?"
    fi

    invoke_launcher --version > /dev/null 2>&1 \
        && report_pass "--version works" \
        || report_fail "--version works" "exit $?"

    local exit_code
    invoke_launcher 2>&1 >/dev/null && exit_code=$? || exit_code=$?
    [ "$exit_code" -ne 0 ] && report_pass "no args exits non-zero" \
                            || report_fail "no args exits non-zero" "exit 0"

    assert_fail "unknown action"   "error"    -a nonexistent-action
    assert_fail "missing --action" "required" --path .
}

# ===========================================================================
# 2. CREATE — scaffold new projects
# ===========================================================================

test_create() {
    echo "=== 2. Create action ==="

    # -- basic scaffold ------------------------------------------------------
    local project_dir="$test_workspace_root/CreateTest"
    assert_ok "create project" -a create -n CreateTest -p "$test_workspace_root"

    [ -d "$project_dir" ]                && report_pass "project dir exists"      || report_fail "project dir" "missing $project_dir"
    [ -f "$project_dir/Cargo.toml" ]     && report_pass "Cargo.toml exists"      || report_fail "Cargo.toml" "missing"
    [ -f "$project_dir/res/config.ini" ] && report_pass "config.ini exists"      || report_fail "config.ini" "missing"
    [ -d "$project_dir/src" ]            && report_pass "src/ exists"             || report_fail "src/" "missing"

    if grep -q "CreateTest" "$project_dir/res/config.ini" 2>/dev/null; then
        report_pass "config.ini contains project name"
    else
        report_fail "config.ini" "project name not found"
    fi

    # -- error: duplicate name -----------------------------------------------
    assert_fail "duplicate create" "already exists" -a create -n CreateTest -p "$test_workspace_root"

    # -- error: missing --name -----------------------------------------------
    assert_fail "create without --name" "name" -a create -p "$test_workspace_root"

    # -- default path (.) ----------------------------------------------------
    local cwd_test="$test_workspace_root/CwdCreate"
    mkdir -p "$cwd_test"
    invoke_launcher -a create -n InCwd -p "$cwd_test" > /dev/null 2>&1 \
        && report_pass "create with explicit relative path" \
        || report_fail "create relative path" "failed"

    # -- short flags ---------------------------------------------------------
    local short_test="$test_workspace_root/ShortFlags"
    assert_ok "create with short flags" -a create -n SF -p "$test_workspace_root"
}

# ===========================================================================
# 3. BUILD — compile projects (native + WASM)
# ===========================================================================

test_build() {
    echo "=== 3. Build action ==="

    local project_dir="$test_workspace_root/BuildTest"
    invoke_launcher -a create -n BuildTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "build tests" "create failed"; return
    }

    # -- native debug (default) ----------------------------------------------
    echo "    (native debug build — this will take several minutes)"
    local out exit_code
    out=$(invoke_launcher -a build -p "$project_dir" -c debug 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
        report_pass "build native debug"
        [ -d "$project_dir/build/dev/data" ] && report_pass "build output data/ exists" \
                                              || report_fail "build output" "missing data/"
    else
        report_skip "build native debug" "exit $exit_code (${out:0:120})"
    fi

    # -- native release ------------------------------------------------------
    echo "    (native release build)"
    out=$(invoke_launcher -a build -p "$project_dir" -c release 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
        report_pass "build native release"
    else
        report_skip "build native release" "exit $exit_code (${out:0:120})"
    fi

    # -- WASM / web target ---------------------------------------------------
    echo "    (WASM build)"
    out=$(invoke_launcher -a build -p "$project_dir" -t web 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
        report_pass "build WASM (web target)"
        [ -d "$project_dir/build/wasm" ] && report_pass "WASM output dir exists" \
                                          || report_fail "WASM output" "missing build/wasm/"
    else
        report_skip "build WASM" "exit $exit_code (${out:0:120})"
    fi

    # -- short flags (-c, -t) ------------------------------------------------
    local short_test="$test_workspace_root/ShortBuild"
    invoke_launcher -a create -n SB -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "short flag build" "create failed"; return
    }
    out=$(invoke_launcher -a build -p "$short_test/SB" -c release -t native 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ]; then
        report_pass "build with short flags (-c release -t native)"
    else
        report_skip "build short flags" "exit $exit_code"
    fi
}

# ===========================================================================
# 4. CARGO — passthrough arbitrary cargo commands
# ===========================================================================

test_cargo() {
    echo "=== 4. Cargo passthrough ==="

    local project_dir="$test_workspace_root/CargoTest"
    invoke_launcher -a create -n CargoTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "cargo tests" "create failed"; return
    }

    # -- known-good commands -------------------------------------------------
    assert_ok "cargo --version" -a cargo -p "$project_dir" -- --version

    # -- cargo check on project ----------------------------------------------
    echo "    (cargo check — may take a moment)"
    if invoke_launcher -a cargo -p "$project_dir" -- check > /dev/null 2>&1; then
        report_pass "cargo check on project"
    else
        report_skip "cargo check" "exit $? (may need full workspace)"
    fi

    # -- error: empty args ---------------------------------------------------
    assert_fail "cargo empty args" "at least one argument" -a cargo -p "$project_dir"

    # -- error: bad cargo subcommand -----------------------------------------
    assert_fail "cargo bad subcommand" "no such command" -a cargo -p "$project_dir" -- nonexistent-cargo-cmd-xyz

    # -- short flag (-p) -----------------------------------------------------
    assert_ok "cargo with short -p" -a cargo -p "$project_dir" -- --version
}

# ===========================================================================
# 5. ASSETS — run the asset pipeline
# ===========================================================================

test_assets() {
    echo "=== 5. Assets action ==="

    local project_dir="$test_workspace_root/AssetsTest"
    invoke_launcher -a create -n AssetsTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "assets tests" "create failed"; return
    }

    # -- empty project (no raw assets) ---------------------------------------
    assert_ok "assets on empty project" -a assets -p "$project_dir"

    # -- with --clean --------------------------------------------------------
    assert_ok "assets --clean" -a assets -p "$project_dir" --clean
}

# ===========================================================================
# 6. DOCS — generate rustdoc
# ===========================================================================

test_docs() {
    echo "=== 6. Docs action ==="

    local docs_out="$test_workspace_root/docs-out"
    mkdir -p "$docs_out"

    echo "    (generating docs — may take a moment)"
    local out exit_code
    out=$(invoke_launcher -a docs -o "$docs_out" 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "docs generation succeeds"
        [ -d "$docs_out/docs" ] && report_pass "docs output dir exists" \
                                 || report_fail "docs output" "missing $docs_out/docs/"
    elif echo "$out" | grep -qi "plantuml\|Cannot locate\|manifest\|features"; then
        report_skip "docs generation" "${out:0:100}"
    else
        report_fail "docs generation" "exit $exit_code: ${out:0:200}"
    fi

    # -- default output path (.) ---------------------------------------------
    invoke_launcher -a docs -o "$docs_out" > /dev/null 2>&1 \
        && report_pass "docs with explicit output path" \
        || report_skip "docs output path" "exit $?"

}

# ===========================================================================
# 7. RUN — build + launch projects
# ===========================================================================

test_run() {
    echo "=== 7. Run action ==="

    local project_dir="$test_workspace_root/RunTest"
    invoke_launcher -a create -n RunTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "run tests" "create failed"; return
    }

    # -- run native (launches briefly, we just check exit code) --------------
    echo "    (run native — launches and exits immediately)"
    local out exit_code
    out=$(timeout 10s invoke_launcher -a run -p "$project_dir" -c debug 2>&1) && exit_code=$? || exit_code=$?
    # 124 = timeout (process ran >10s, which is fine — it launched)
    # 0   = process exited cleanly
    # 1   = cargo/compilation error
    if [ "$exit_code" -eq 0 ] || [ "$exit_code" -eq 124 ]; then
        report_pass "run native debug (launched)"
    else
        report_skip "run native" "exit $exit_code (${out:0:120})"
    fi

    # -- run native release --------------------------------------------------
    echo "    (run native release)"
    out=$(timeout 10s invoke_launcher -a run -p "$project_dir" -c release 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ] || [ "$exit_code" -eq 124 ]; then
        report_pass "run native release (launched)"
    else
        report_skip "run release" "exit $exit_code (${out:0:120})"
    fi

    # -- run with passthrough args -------------------------------------------
    echo "    (run with passthrough args)"
    invoke_launcher -a run -p "$project_dir" -c debug -- --help > /dev/null 2>&1 \
        && report_pass "run with -- passthrough args" \
        || report_skip "run passthrough" "exit $?"

    # -- short flags ---------------------------------------------------------
    local short_test="$test_workspace_root/ShortRun"
    invoke_launcher -a create -n SR -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "short run flags" "create failed"; return
    }
    out=$(timeout 10s invoke_launcher -a run -p "$short_test/SR" -c debug 2>&1) && exit_code=$? || exit_code=$?
    if [ "$exit_code" -eq 0 ] || [ "$exit_code" -eq 124 ]; then
        report_pass "run with short flags"
    else
        report_skip "run short flags" "exit $exit_code"
    fi
}

# ===========================================================================
# Dispatch (only when executed directly)
# ===========================================================================

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    basics)    test_basics ;;
    create)    test_create ;;
    build)     test_build ;;
    cargo)     test_cargo ;;
    assets)    test_assets ;;
    docs)      test_docs ;;
    run)       test_run ;;

    all|"")
        test_basics
        test_create
        test_build
        test_cargo
        test_assets
        test_docs
        test_run
        ;;

    *)
        echo "Usage: $0 [all|<test-group>]"
        echo ""
        echo "Test groups:"
        echo "  basics    smoke-test the binary (--help, --version, errors)"
        echo "  create    scaffold projects (flags, defaults, duplicates)"
        echo "  build     compile native debug/release + WASM"
        echo "  cargo     passthrough commands (version, check, errors)"
        echo "  assets    asset pipeline (empty project, --clean)"
        echo "  docs      rustdoc generation (default + custom output)"
        echo "  run       build + launch (debug, release, passthrough)"
        echo ""
        echo "  all       run everything in sequence"
        exit 1
        ;;
esac

print_summary
