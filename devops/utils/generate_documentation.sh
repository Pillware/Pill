#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo), PlantUML (optional), a compiled
#               PillLauncher binary (auto-discovered or set via PILL_LAUNCHER_BIN).

# DESCRIPTION: Generate rustdoc documentation for all Pill crates via
#   PillLauncher.  Supports two output profiles: project_dev (public API with
#   project + internal features) and engine_dev (private items + pill_core).

# USAGE: bash devops/utils/generate_documentation.sh

# EXAMPLE USAGE:
#   bash devops/utils/generate_documentation.sh

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../common.sh
source "$SCRIPT_DIR/../common.sh"

# All paths in this script are relative to the project root.
cd "$PROJECT_ROOT"

# ---------------------------------------------------------------------------
# Documentation generation
# ---------------------------------------------------------------------------

generate_documentation() {
    echo ""
    echo "------------------------------------------------------------------"
    echo "Generating Pill documentation"
    local docs_output_directory="$test_workspace_root/documentation_output"
    mkdir -p "$docs_output_directory"

    echo "Running docs generation - this may take a while"
    local documentation_output documentation_exit_code=0
    documentation_output=$(invoke_launcher docs -o "$docs_output_directory" 2>&1) || documentation_exit_code=$?

    if [ "$documentation_exit_code" -eq 0 ]; then
        report_pass "documentation generated"
        if [ -d "$docs_output_directory/docs" ]; then
            report_pass "documentation output directory exists"
        else
            report_fail "documentation output" "missing $docs_output_directory/docs/"
        fi
    elif echo "$documentation_output" | grep -qi "plantuml\|Cannot locate\|manifest"; then
        report_skip "documentation generation" "PlantUML not installed or manifest issue"
    else
        report_fail "documentation generation" "exit $documentation_exit_code"
    fi
}

# ---------------------------------------------------------------------------
# Dispatch
# ---------------------------------------------------------------------------

if [[ "${BASH_SOURCE[0]}" != "${0}" ]]; then
    return 0
fi

generate_documentation
print_summary
