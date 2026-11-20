#!/usr/bin/env bash

# REQUIREMENTS: bash 4+, Python 3.8+, a nightly Rust toolchain (the pipeline
#               runs `cargo +nightly rustdoc` to emit rustdoc JSON).

# DESCRIPTION: Thin runner for the Markdown documentation pipeline. All of the
#   logic lives in devops/docs/generate_documentation_markdown.py, which first
#   produces rustdoc JSON via generate_documentation.py and then renders it
#   into a tree of Markdown pages. This script only locates the repository
#   root and forwards its arguments, so the pipeline behaves identically
#   whether it is invoked here or directly:
#
#     python devops/docs/generate_documentation_markdown.py
#
#   Output locations (the local docs_build directory and its sidebar data
#   file) are configured inside that Python script, not here.

# USAGE: bash devops/ci_cd/generate_documentation.sh [--help]
#
#   Every argument is passed straight through to the Python script.

# EXAMPLE USAGE:
#   bash devops/ci_cd/generate_documentation.sh
#
# Exit status: whatever the Python pipeline returns; 0 on success.

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# The repository root is two levels up: devops/ci_cd -> devops -> root.
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Shortcuts for colored section headers
BOLD='\033[1m'
CYAN='\033[0;36m'
RED='\033[0;31m'
NC='\033[0m'

DOCUMENTATION_SCRIPT="devops/docs/generate_documentation_markdown.py"

cd "$PROJECT_ROOT"

if [ "${1:-}" = "-h" ] || [ "${1:-}" = "--help" ]; then
    sed -n '/^# USAGE/,/^# --- SCRIPT ---/p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
fi

if [ ! -f "$DOCUMENTATION_SCRIPT" ]; then
    echo -e "${RED}FATAL: $DOCUMENTATION_SCRIPT not found under $PROJECT_ROOT${NC}" >&2
    exit 1
fi

echo ""
echo -e "${BOLD}${CYAN}===============================================================================${NC}"
echo -e "${BOLD}${CYAN}Markdown documentation generation${NC}"
echo "  script: $DOCUMENTATION_SCRIPT"
echo "  this may take a while"
echo ""

set +e
python "$DOCUMENTATION_SCRIPT" "$@"
documentation_exit=$?
set -e

echo ""
echo -e "${BOLD}${CYAN}===============================================================================${NC}"
if [ "$documentation_exit" -eq 0 ]; then
    echo -e "${BOLD}${CYAN}Documentation generation PASSED${NC}"
else
    echo -e "${BOLD}${RED}Documentation generation FAILED (exit $documentation_exit)${NC}"
fi
echo -e "${BOLD}${CYAN}===============================================================================${NC}"

exit "$documentation_exit"
