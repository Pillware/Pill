#!/usr/bin/env bash

# REQUIREMENTS: Python 3.8+, Rust toolchain (cargo) on PATH. The suite
#               launches the standalone host and drives real source edits, so
#               it needs a working native toolchain (it builds
#               `pill_standalone` first unless --skip-build is given).

# DESCRIPTION: Run the full hot-reload regression net: every suite in
#   devops/tests/ whose docstring carries `SETS: hot-reload`, selected and
#   ordered by devops/tests/run_all.py (static tier first - the harness parsing
#   and log contract checks fail in a second instead of twenty minutes in - then
#   the host suites by name). `python devops/tests/run_all.py --set hot-reload
#   --list` prints the current members; `run_all.py --catalogue` says what each
#   one protects. Every suite runs even after a failure (--keep-going).
#
#   Each suite launches the standalone host and drives live source edits
#   against a real project and an extension, asserting the host's reload
#   behaviour from its console output. The suites temporarily modify - and
#   automatically restore - the projects' project_settings.yaml files,
#   devops/tests/projects/native/src/lib.rs,
#   devops/tests/projects/managed/src/Systems.cs and
#   modules/extensions/pill_spline/modules/runtime/src/lib.rs, plus
#   devops/tests/projects/scene's theme file and project source
#   (the theme suite), so a normal developer workspace is left exactly as it
#   was.
#
#   Expected runtime: ~15-20 minutes (plus the initial host build unless
#   --skip-build is used).
#
#   NOTE: the C# bridge suite needs the .NET SDK on PATH (the host runs
#   `dotnet build` for the managed project).
#
#   Designed for both local development and GitHub Actions CI.

# USAGE: bash devops/ci_cd/run_hot_reload_tests.sh [--skip-build] [--timeout-scale S]
#
#   --skip-build          skip the initial host build (assume pill_standalone
#                         is already built; fastest lane for CI). Passed only
#                         to the suites that accept it; the others always
#                         build what they need.
#   --timeout-scale S     multiply all suite timeouts by S (default 1.5)

# EXAMPLE USAGE:
#   bash devops/ci_cd/run_hot_reload_tests.sh
#   bash devops/ci_cd/run_hot_reload_tests.sh --skip-build
#   bash devops/ci_cd/run_hot_reload_tests.sh --timeout-scale 2.0

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# The repository root is two levels above devops/ci_cd/.
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Shortcuts for colored section headers and failure lines
BOLD='\033[1m'
CYAN='\033[0;36m'
RED='\033[0;31m'
NC='\033[0m'

# All paths in this script are relative to the project root.
cd "$PROJECT_ROOT"

SKIP_BUILD=0
TIMEOUT_SCALE=1.5

while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build)
            SKIP_BUILD=1
            shift
            ;;
        --timeout-scale)
            TIMEOUT_SCALE="$2"
            shift 2
            ;;
        --timeout-scale=*)
            TIMEOUT_SCALE="${1#*=}"
            shift
            ;;
        -h|--help)
            sed -n '/^# USAGE/,/^# --- SCRIPT ---/p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "ERROR: unknown argument '$1'" >&2
            exit 2
            ;;
    esac
done

case "$TIMEOUT_SCALE" in
    ''|*[!0-9.]*) echo "ERROR: --timeout-scale must be a number (got '$TIMEOUT_SCALE')" >&2; exit 2 ;;
esac

echo ""
echo -e "${BOLD}${CYAN}===============================================================================${NC}"
echo -e "${BOLD}${CYAN}Hot-reload regression net (SETS: hot-reload)${NC}"
echo "  timeout scale:  ${TIMEOUT_SCALE}"
if [ "$SKIP_BUILD" = 1 ]; then
    echo "  host build:     skipped where a suite accepts --skip-build"
    extra_args=(--skip-build)
else
    echo "  host build:     yes"
    extra_args=()
fi
echo ""

# run_all.py passes --timeout-scale and --skip-build only to the suites whose
# own parser declares them.
set +e
python devops/tests/run_all.py --set hot-reload --keep-going --timeout-scale "$TIMEOUT_SCALE" "${extra_args[@]}"
overall_exit=$?
set -e

echo ""
echo -e "${BOLD}${CYAN}===============================================================================${NC}"
if [ "$overall_exit" -eq 0 ]; then
    echo -e "${BOLD}${CYAN}Hot-reload regression net PASSED${NC}"
else
    echo -e "${BOLD}${RED}Hot-reload regression net FAILED (exit $overall_exit)${NC}"
fi
echo -e "${BOLD}${CYAN}===============================================================================${NC}"

exit "$overall_exit"
