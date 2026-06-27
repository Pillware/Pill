#!/usr/bin/env bash
# =============================================================================
# devops/tests/generate_documentation.sh — Pill Engine documentation generator
# =============================================================================
#
# Generates rustdoc for the engine crates via `PillLauncher -a docs`.
# Sources run_basic_tests.sh for shared helpers (binary discovery, report_*,
# test workspace, etc.).
#
# Usage:
#   bash devops/tests/generate_documentation.sh          # generate docs
#   bash devops/tests/generate_documentation.sh test     # generate + verify
#   bash devops/tests/generate_documentation.sh --help   # this message

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=./common.sh
source "$SCRIPT_DIR/common.sh"

# ===========================================================================
# Documentation generation
# ===========================================================================

generate_docs() {
    echo "=== Generating Pill Engine documentation ==="
    local docs_output_dir="$test_workspace_root/docs-out"
    mkdir -p "$docs_output_dir"

    echo "    (this may take a moment)"
    local launcher_output exit_code
    launcher_output=$(invoke_launcher -a docs -o "$docs_output_dir" 2>&1) && exit_code=$? || exit_code=$?

    if [ "$exit_code" -eq 0 ]; then
        report_pass "docs generation succeeded"
        echo ""
        echo "Documentation generated at: $docs_output_dir/docs/"
        echo "  project_dev/ — public API (game + internal features)"
        echo "  engine_dev/  — private items + pill_core"
    elif echo "$launcher_output" | grep -qi "plantuml\|Cannot locate\|manifest"; then
        report_skip "docs generation" "PlantUML not installed or manifest issue"
    else
        report_fail "docs generation" "exit $exit_code: ${launcher_output:0:200}"
    fi
}

# ===========================================================================
# Dispatch (only when executed directly)
# ===========================================================================

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0  # sourced — expose generate_docs function
fi

case "${1:-test}" in
    test)
        generate_docs
        print_summary
        ;;
    --help|-h|help)
        echo "Usage: $0 [test|--help]"
        echo ""
        echo "Commands:"
        echo "  test    Generate docs and report results (default)"
        echo "  --help  Show this message"
        ;;
    *)
        generate_docs
        print_summary
        ;;
esac
