#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo), a compiled PillLauncher binary
#               (auto-discovered or set via PILL_LAUNCHER_BIN).

# DESCRIPTION: Build every Pill example project to verify they all compile.
#   Each example is built via PillLauncher in release mode. Standalone Cargo
#   crates (net_minimal) are built directly with cargo.

# USAGE: bash devops/tests/run_examples_tests.sh [all|<example-path>]
#
#   all                        build all examples (default)
#   examples/cube              build a single example

# EXAMPLE USAGE:
#   bash devops/tests/run_examples_tests.sh all
#   bash devops/tests/run_examples_tests.sh examples/city

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# All paths in this script are relative to the project root.
cd "$PROJECT_ROOT"

# ---------------------------------------------------------------------------
# Example lists
# ---------------------------------------------------------------------------

PILL_EXAMPLES=(
    "examples/cube"
    "examples/floating_pills"
    "examples/italian_brainrot"
    "examples/city"
    "examples/empty"
    "examples/pill_tunel"
)

STANDALONE_CRATES=(
    "examples/net_minimal/client"
    "examples/net_minimal/server"
)

# ---------------------------------------------------------------------------
# Build helpers
# ---------------------------------------------------------------------------

_build_pill_example() {
    local example_path="$1"
    local build_exit_code=0
    invoke_launcher build -p "$example_path" -c release 2>&1 || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "$example_path"
    else
        report_fail "$example_path" "build failed (exit $build_exit_code)"
    fi
}

_build_standalone_crate() {
    local crate_path="$1"
    local build_exit_code=0
    cargo build --manifest-path "$crate_path/Cargo.toml" --release 2>&1 || build_exit_code=$?
    if [ "$build_exit_code" -eq 0 ]; then
        report_pass "$crate_path"
    else
        report_fail "$crate_path" "build failed (exit $build_exit_code)"
    fi
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

build_all_examples() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "Building all Pill example projects (release, shared target dir)"

    # Use a shared cargo target directory so pill_engine (and its
    # dependencies) are reused when features match across examples.
    export PILL_TARGET_DIR="$PWD/engine/target_projects/_examples"

    local example_index=0
    local total_examples=$((${#PILL_EXAMPLES[@]} + ${#STANDALONE_CRATES[@]}))

    # Pill projects
    for example_path in "${PILL_EXAMPLES[@]}"; do
        example_index=$((example_index + 1))
        echo ""
        echo "($example_index/$total_examples) $example_path"
        echo "Building - this may take a moment"
        _build_pill_example "$example_path"
    done

    # Standalone Cargo crates
    for crate_path in "${STANDALONE_CRATES[@]}"; do
        example_index=$((example_index + 1))
        echo ""
        echo "($example_index/$total_examples) $crate_path"
        echo "Building - this may take a moment"
        _build_standalone_crate "$crate_path"
    done
}

build_single_example() {
    local example_path="$1"
    echo ""
    echo "------------------------------------------------------------------"
    echo "Building $example_path (release)"
    echo "Building - this may take a moment"

    if [ -d "$example_path/res" ]; then
        _build_pill_example "$example_path"
    elif [ -f "$example_path/Cargo.toml" ]; then
        _build_standalone_crate "$example_path"
    else
        report_skip "$example_path" "not a valid project (no Cargo.toml or res/ found)"
    fi
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

case "${1:-all}" in
    all|"")
        build_all_examples
        ;;
    *)
        build_single_example "$1"
        ;;
esac

print_summary

