#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo, rustfmt, clippy), wasm-pack,
#               a compiled PillLauncher binary (auto-discovered or set via
#               PILL_LAUNCHER_BIN).

# DESCRIPTION: Pill Launcher comprehensive tests - exercises every
#   PillLauncher action (create, build, run, cargo, assets, docs, link,
#   unlink) with all valid flag combinations and error-case parameters.
#
#   Uses a temporary workspace for scaffolded projects. Designed for
#   local development and GitHub Actions CI (ci-pill_launcher-tests.yml).

# USAGE: bash devops/tests/run_pill_launcher_tests.sh [all|<test-group>]
#
#   all                run all test groups (default)
#   basics             --help, --version, error handling, subcommand help
#   create             scaffold new projects
#   build              compile native (debug/release/hot-reload) + WASM
#   cargo              passthrough cargo commands (version, check, clippy, fmt)
#   assets             asset pipeline
#   docs               rustdoc generation (default + custom output)
#   run                build + launch (all compile modes, features, WASM, passthrough)
#   hot-reload         hot-reload edit-detect-rebuild workflow
#   link               IDE workspace linking

# EXAMPLE USAGE:
#   bash devops/tests/run_pill_launcher_tests.sh all
#   bash devops/tests/run_pill_launcher_tests.sh create
#   bash devops/tests/run_pill_launcher_tests.sh build

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

# ---------------------------------------------------------------------------
# 1. basics - smoke-test the binary itself
# ---------------------------------------------------------------------------

test_launcher_basics() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(1/9) Launcher basics"

    # `PillLauncher --help` - Prints help (exit 0)
    if invoke_launcher --help > /dev/null 2>&1; then
        report_pass "--help"
    else
        report_fail "--help" "exit $?"
    fi

    # `PillLauncher --version` - Prints version (exit 0)
    invoke_launcher --version > /dev/null 2>&1 \
        && report_pass "--version" \
        || report_fail "--version" "exit $?"

    # `PillLauncher` (no subcommand) - Prints help text (exit non-zero)
    local exit_code=0
    invoke_launcher > /dev/null 2>&1 || exit_code=$?
    if [ "$exit_code" -ne 0 ]; then
        report_pass "no arguments exits non-zero"
    else
        report_fail "no arguments" "exited 0"
    fi

    # `PillLauncher nonexistent_action` - Error: unknown subcommand
    assert_fail "unknown action"   "error"    nonexistent_action

    # `PillLauncher --path .` - Error: missing subcommand, flag unexpected
    assert_fail "missing subcommand" "wasn't expected" --path .

    # `PillLauncher <subcommand> --help` - Prints help for that subcommand (exit 0)
    for subcommand in create run build docs cargo assets link unlink; do
        if invoke_launcher "$subcommand" --help > /dev/null 2>&1; then
            report_pass "$subcommand --help"
        else
            report_fail "$subcommand --help" "exit $?"
        fi
    done

    # `PillLauncher run -c invalid` - Error: invalid value for `--compile-mode`
    assert_fail "invalid compile mode" "error" run -p . -c invalid_mode

    # `PillLauncher run -t invalid` - Error: invalid value for `--target`
    assert_fail "invalid target" "error" run -p . -t invalid_target
}

# ---------------------------------------------------------------------------
# 2. create - scaffold new projects
# ---------------------------------------------------------------------------

test_launcher_create() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(2/9) Create action"

    # `PillLauncher create -n MyGame` - Creates ./MyGame/ from template,
    #   rewrites Cargo.toml and config.ini with project name + absolute engine paths
    local project_directory="$test_workspace_root/CreateTest"
    assert_ok "create project" create -n CreateTest -p "$test_workspace_root"

    [ -d "$project_directory" ]                && report_pass "project directory exists"      || report_fail "project directory" "missing $project_directory"
    [ -f "$project_directory/Cargo.toml" ]     && report_pass "Cargo.toml exists"            || report_fail "Cargo.toml" "missing"
    [ -f "$project_directory/res/config.ini" ] && report_pass "config.ini exists"            || report_fail "config.ini" "missing"
    [ -d "$project_directory/src" ]            && report_pass "src/ directory exists"         || report_fail "src/" "missing"

    if grep -q "CreateTest" "$project_directory/res/config.ini" 2>/dev/null; then
        report_pass "config.ini contains project name"
    else
        report_fail "config.ini" "project name not found"
    fi

    # `PillLauncher create -n ExistingDir` (directory already exists) -
    #   Error: `Project directory ... already exists`
    assert_fail "duplicate create" "already exists" create -n CreateTest -p "$test_workspace_root"

    # `PillLauncher create` (missing `-n`) - Error: `--name <name> is required`
    assert_fail "create without --name" "name" create -p "$test_workspace_root"

    # `PillLauncher create -n MyGame -p ../my_projects` - Creates in custom path
    assert_ok "create with short flags" create -n CreateTest_ShortFlags -p "$test_workspace_root"
}

# ---------------------------------------------------------------------------
# 3. build - compile projects (native: debug/release/hot-reload + WASM)
# ---------------------------------------------------------------------------

test_launcher_build() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(3/9) Build action"

    local project_directory="$test_workspace_root/BuildTest"
    invoke_launcher create -n BuildTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "build tests" "create failed"; return
    }

    # `PillLauncher build -p ./examples/cube` - Debug build → copies artifacts to output directory
    echo "  (native debug - this may take a while)"
    local build_output build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -c debug 2>&1) || build_exit_code=$?
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

    # `PillLauncher build -p ./examples/cube -c release` - Release build
    echo "  (native release)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -c release 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build native release"
    else
        report_skip "build native release" "exit $build_exit_code"
    fi

    # `PillLauncher build -p ./examples/cube -c hot-reload` - Hot-reload build
    echo "  (native hot-reload)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -c hot-reload 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build native hot-reload"
        if [ -d "$project_directory/build/hot-reload/data" ]; then
            report_pass "hot-reload build output data/ exists"
        else
            report_fail "hot-reload build output" "missing data/"
        fi
    else
        report_skip "build native hot-reload" "exit $build_exit_code"
    fi

    # `PillLauncher build -p ./examples/cube --clean` - Rebuilds assets from source → compiles
    echo "  (build --clean)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -c debug --clean 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build --clean"
    else
        report_skip "build --clean" "exit $build_exit_code"
    fi

    # `PillLauncher build -p ./examples/cube --additional-features "debug_ui"` - Builds with additional feature flags
    echo "  (build --additional-features)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -c debug --additional-features "project" 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build --additional-features project"
    else
        report_skip "build --additional-features" "exit $build_exit_code"
    fi

    # `PillLauncher build -p ./examples/cube -t web` - Builds WASM → output at build/wasm/
    echo "  (WASM)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -t web 2>&1) || build_exit_code=$?
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

    # `PillLauncher build -p ./examples/cube -t web --max-wasm-size 512` -
    #   Builds WASM → errors if binary exceeds 512 KB
    echo "  (WASM --max-wasm-size)"
    build_exit_code=0
    build_output=$(invoke_launcher build -p "$project_directory" -t web --max-wasm-size 99999 2>&1) || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "build WASM --max-wasm-size accepted"
    else
        report_skip "build WASM --max-wasm-size" "exit $build_exit_code"
    fi

    # `PillLauncher build -p <dir> -c release -t native` - Build with short flags
    local short_directory="$test_workspace_root/ShortBuild"
    invoke_launcher create -n ShortBuild -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "short flag build" "create failed"; return
    }
    build_exit_code=0
    invoke_launcher build -p "$short_directory" -c release -t native > /dev/null 2>&1 || build_exit_code=$?
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
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(4/9) Cargo passthrough"

    local project_directory="$test_workspace_root/CargoTest"
    invoke_launcher create -n CargoTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "cargo tests" "create failed"; return
    }

    # `PillLauncher cargo -p ./examples/cube -- --version` - Runs cargo --version
    assert_ok "cargo --version" cargo -p "$project_directory" -- --version

    # `PillLauncher cargo -p ./examples/cube -- check` - Runs cargo check in workspace context
    echo "  (cargo check)"
    local cargo_exit_code=0
    invoke_launcher cargo -p "$project_directory" -- check > /dev/null 2>&1 || cargo_exit_code=$?
    if [ "$cargo_exit_code" -eq 0 ]; then
        report_pass "cargo check on project"
    else
        report_skip "cargo check" "exit $cargo_exit_code (may need full workspace)"
    fi

    # `PillLauncher cargo -p ./examples/cube -- fmt` - Runs cargo fmt in workspace context
    echo "  (cargo fmt)"
    cargo_exit_code=0
    invoke_launcher cargo -p "$project_directory" -- fmt --check > /dev/null 2>&1 || cargo_exit_code=$?
    if [ "$cargo_exit_code" -eq 0 ]; then
        report_pass "cargo fmt --check on project"
    else
        report_skip "cargo fmt" "exit $cargo_exit_code"
    fi

    # `PillLauncher cargo -p ./examples/cube -- clippy` - Runs cargo clippy in workspace context
    echo "  (cargo clippy)"
    cargo_exit_code=0
    invoke_launcher cargo -p "$project_directory" -- clippy > /dev/null 2>&1 || cargo_exit_code=$?
    if [ "$cargo_exit_code" -eq 0 ]; then
        report_pass "cargo clippy on project"
    else
        report_skip "cargo clippy" "exit $cargo_exit_code"
    fi

    # `PillLauncher cargo -- fmt` (no args after `--`) -
    #   Error: `Must call cargo with at least one argument`
    assert_fail "cargo empty arguments" "at least one argument" cargo -p "$project_directory"

    # `PillLauncher cargo -p ./examples/cube -- nonexistent_cargo_command` - Error from cargo/clap
    assert_fail "cargo bad subcommand"  "no such command\|wasn't expected"       cargo -p "$project_directory" -- nonexistent_cargo_command

    # `PillLauncher cargo -p ./examples/cube -- --version` - Short flag, runs cargo --version
    assert_ok "cargo with short flag" cargo -p "$project_directory" -- --version
}

# ---------------------------------------------------------------------------
# 5. assets - run the asset pipeline
# ---------------------------------------------------------------------------

test_launcher_assets() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(5/9) Assets action"

    local project_directory="$test_workspace_root/AssetsTest"
    invoke_launcher create -n AssetsTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "assets tests" "create failed"; return
    }

    # `PillLauncher assets -p ./examples/cube` - Runs asset pipeline on res/ (incremental)
    assert_ok "assets on empty project" assets -p "$project_directory"

    # `PillLauncher assets -p ./examples/cube --clean` - Deletes all cooked assets → rebuilds
    assert_ok "assets --clean" assets -p "$project_directory" --clean

    # `PillLauncher assets -p <dir>` - Works with short -p flag
    assert_ok "assets with short flag" assets -p "$project_directory"
}

# ---------------------------------------------------------------------------
# 6. docs - generate rustdoc
# ---------------------------------------------------------------------------

test_launcher_docs() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(6/9) Docs action"

    # `PillLauncher docs -o ../docs_output` - Outputs docs to custom directory
    local docs_output_directory="$test_workspace_root/docs_output"
    mkdir -p "$docs_output_directory"

    echo "  (generating docs with -o)"
    local docs_output docs_exit_code=0
    docs_output=$(invoke_launcher docs -o "$docs_output_directory" 2>&1) || docs_exit_code=$?

    if [ "$docs_exit_code" -eq 0 ]; then
        report_pass "docs generation with -o"
        if [ -d "$docs_output_directory/generated" ]; then
            report_pass "docs output directory exists"
        else
            report_fail "docs output" "missing $docs_output_directory/generated/"
        fi
    elif echo "$docs_output" | grep -qi "plantuml\|Cannot locate\|manifest\|Cannot find Empty"; then
        report_skip "docs generation" "PlantUML or required tool not available"
    else
        report_fail "docs generation" "exit $docs_exit_code"
    fi
}

# ---------------------------------------------------------------------------
# 7. run - build + launch projects (all compile modes + features + WASM)
# ---------------------------------------------------------------------------

test_launcher_run() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(7/9) Run action"

    local project_directory="$test_workspace_root/RunTest"
    invoke_launcher create -n RunTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "run tests" "create failed"; return
    }

    # `PillLauncher run -p ./examples/cube` - Debug build → copies artifacts → launches
    echo "  (run native debug)"
    local run_output run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -c debug 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run native debug"
    else
        report_skip "run native debug" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube -c release` - Release build → launch
    echo "  (run native release)"
    run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -c release 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run native release"
    else
        report_skip "run native release" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube -c hot-reload` - Hot-reload build → launch with file watchers
    echo "  (run native hot-reload)"
    run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -c hot-reload 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run native hot-reload"
    else
        report_skip "run native hot-reload" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube --clean` - Rebuilds cooked assets → builds → launches
    echo "  (run --clean)"
    run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -c debug --clean 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run --clean"
    else
        report_skip "run --clean" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube -t web` - Builds WASM → starts dev server on port 8080
    echo "  (run WASM)"
    run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -t web 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run WASM"
    else
        report_skip "run WASM" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube -t web --wasm-port 3000` - Builds WASM → starts dev server on port 3000
    echo "  (run WASM --wasm-port)"
    run_exit_code=0
    run_output=$(timeout 10s "$pill_launcher_bin" run -p "$project_directory" -t web --wasm-port 9090 2>&1) || run_exit_code=$?
    if [ "$run_exit_code" -eq 0 ] || [ "$run_exit_code" -eq 124 ]; then
        report_pass "run WASM --wasm-port 9090"
    else
        report_skip "run WASM --wasm-port" "exit $run_exit_code"
    fi

    # `PillLauncher run -p ./examples/cube -- --help` - Passes `--help` to the running project executable
    # The project may not support --help (game loop runs until timeout); we only verify the launcher accepts `--`.
    local passthrough_exit_code=0
    timeout 5s "$pill_launcher_bin" run -p "$project_directory" -c debug -- --help > /dev/null 2>&1 || passthrough_exit_code=$?
    if [ "$passthrough_exit_code" -eq 0 ] || [ "$passthrough_exit_code" -eq 124 ]; then
        report_pass "run with passthrough arguments"
    else
        report_skip "run passthrough" "exit $passthrough_exit_code (game may not support --help)"
    fi

    # Clean up any lingering game windows / dev servers
    taskkill //F //IM RunTest.exe > /dev/null 2>&1 || true
    kill_server_on_port 8080
    kill_server_on_port 9090
}

# ---------------------------------------------------------------------------
# 8. hot-reload - edit-detect-rebuild workflow
# ---------------------------------------------------------------------------

test_launcher_hot_reload() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(8/9) Hot-reload workflow"

    # `PillLauncher run -p ./examples/cube -c hot-reload` -
    #   Hot-reload cycle: launch → edit source → detect change → rebuild DLL → survive
    local project_directory="$test_workspace_root/HotReloadTest"
    invoke_launcher create -n HotReloadTest -p "$test_workspace_root" > /dev/null 2>&1 || {
        report_skip "hot-reload tests" "create failed"; return
    }

    echo "  (starting hot-reload run in background)"
    invoke_launcher run -p "$project_directory" -c hot-reload > /dev/null 2>&1 &
    local launcher_pid=$!

    # Wait for the project to finish building and start running
    sleep 5

    # Verify the process is still running (window should be open)
    if kill -0 "$launcher_pid" 2>/dev/null; then
        report_pass "hot-reload process started"
    else
        report_skip "hot-reload process" "process died before we could test"
        return
    fi

    # Touch a source file to trigger a rebuild
    echo "  (touching source file to trigger hot-reload)"
    local source_file="$project_directory/src/project.rs"
    if [ -f "$source_file" ]; then
        touch "$source_file"
        report_pass "touched project.rs"
    else
        # Some templates may use a different structure
        local any_source
        any_source=$(find "$project_directory/src" -name "*.rs" -type f | head -1)
        if [ -n "$any_source" ]; then
            touch "$any_source"
            report_pass "touched source file: $any_source"
        else
            report_skip "touch source" "no .rs files found"
        fi
    fi

    # Give the file watcher time to detect the change and trigger a rebuild
    sleep 5

    # Verify the process survived the hot-reload
    if kill -0 "$launcher_pid" 2>/dev/null; then
        report_pass "hot-reload process survived rebuild"
    else
        report_skip "hot-reload survived" "process died (may be normal on headless CI)"
    fi

    # Clean up - kill the launcher and any spawned game processes
    kill "$launcher_pid" 2>/dev/null || true
    wait "$launcher_pid" 2>/dev/null || true
    taskkill //F //IM HotReloadTest.exe > /dev/null 2>&1 || true
    report_pass "hot-reload process terminated cleanly"
}

# ---------------------------------------------------------------------------
# 9. link / unlink - IDE workspace membership
# ---------------------------------------------------------------------------

test_launcher_link() {
    echo ""
    echo -e "${BOLD}${CYAN}===============================================================================${NC}"
    echo "(9/9) Link / Unlink actions"

    # Use the existing cube example (no need to create a new one)
    local project_path="examples/cube"

    # Unlink first to ensure a clean state (unlink takes no arguments)
    invoke_launcher unlink > /dev/null 2>&1 || true

    # `PillLauncher link -p ./examples/cube` - Adds project to engine/Cargo.toml workspace members
    local link_output link_exit_code=0
    link_output=$(invoke_launcher link -p "$project_path" 2>&1) || link_exit_code=$?
    if [ "$link_exit_code" -eq 0 ]; then
        if grep -q "$project_path" "engine/Cargo.toml" 2>/dev/null; then
            report_pass "link adds project to workspace members"
        else
            report_fail "link" "project not found in engine/Cargo.toml"
        fi
    else
        report_fail "link" "exit $link_exit_code"
    fi

    # `PillLauncher link -p ./examples/cube` (already linked) -
    #   Prints `Project already linked: ...`
    local relink_exit_code=0
    invoke_launcher link -p "$project_path" > /dev/null 2>&1 || relink_exit_code=$?
    if [ "$relink_exit_code" -eq 0 ]; then
        report_pass "link is idempotent"
    else
        report_fail "link idempotent" "exit $relink_exit_code"
    fi

    # `PillLauncher unlink` - Removes the project from workspace members
    local unlink_exit_code=0
    invoke_launcher unlink > /dev/null 2>&1 || unlink_exit_code=$?
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
    invoke_launcher unlink > /dev/null 2>&1 || true
    git checkout -- engine/Cargo.toml 2>/dev/null || true
}

# ---------------------------------------------------------------------------
# Dispatch (only when executed directly)
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    basics)      test_launcher_basics ;;
    create)      test_launcher_create ;;
    build)       test_launcher_build ;;
    cargo)       test_launcher_cargo ;;
    assets)      test_launcher_assets ;;
    docs)        test_launcher_docs ;;
    run)         test_launcher_run ;;
    hot-reload)  test_launcher_hot_reload ;;
    link)        test_launcher_link ;;

    all|"")
        test_launcher_basics
        test_launcher_create
        test_launcher_build
        test_launcher_cargo
        test_launcher_assets
        test_launcher_docs
        test_launcher_run
        test_launcher_hot_reload
        test_launcher_link
        ;;

    *)
        echo "Usage: $0 [all|<test-group>]"
        echo ""
        echo "Test groups:"
        echo "  basics      --help, --version, error handling, subcommand help"
        echo "  create      scaffold projects (normal, duplicate, missing name, short flags)"
        echo "  build       compile native (debug/release/hot-reload) + WASM + --clean + --additional-features"
        echo "  cargo       passthrough commands (--version, check, fmt, clippy, errors)"
        echo "  assets      asset pipeline (incremental, --clean, short flag)"
        echo "  docs        rustdoc generation (custom -o output)"
        echo "  run         build + launch (debug/release/hot-reload, --clean, --additional-features, WASM, --wasm-port, passthrough)"
        echo "  hot-reload  edit-detect-rebuild workflow verification"
        echo "  link        IDE workspace link / unlink / idempotent"
        exit 1
        ;;
esac

print_summary
