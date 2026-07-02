#!/usr/bin/env bash

# REQUIREMENTS: Rust toolchain (cargo), PlantUML (optional), a compiled
#               PillLauncher binary.  Set PILL_LAUNCHER_BIN to override
#               auto-discovery, or ensure it is on PATH.

# DESCRIPTION: Generate rustdoc documentation for all Pill engine crates.
#   Produces two doc sets under <output>/generated/:
#     project_dev — public API (project + internal features)
#     engine_dev  — private items + pill_core
#   PlantUML diagrams are pre-rendered if PlantUML is installed.

# USAGE: bash devops/utils/generate_documentation.sh [-o <output_dir>]
#
#   -o <directory>    Output directory (default: ../docs)

# EXAMPLE USAGE:
#   bash devops/utils/generate_documentation.sh
#   bash devops/utils/generate_documentation.sh -o /tmp/pill-docs

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=../common.sh
source "$SCRIPT_DIR/../common.sh"

# ---- helpers ---------------------------------------------------------------

say()  { echo -e "${GREEN}[INFO]${NC} $*"; }
die()  { echo -e "${RED}[FATAL]${NC} $*" >&2; exit 1; }

# ---- parse args ------------------------------------------------------------

OUTPUT_DIR="../docs"
while getopts "o:h" opt; do
    case "$opt" in
        o) OUTPUT_DIR="$OPTARG" ;;
        h) echo "Usage: $0 [-o <output_dir>]"; exit 0 ;;
        *) echo "Usage: $0 [-o <output_dir>]"; exit 1 ;;
    esac
done

# ---- main ------------------------------------------------------------------

say "Project root : $PROJECT_ROOT"
say "Launcher     : $pill_launcher_bin"
say "Output       : $OUTPUT_DIR"

mkdir -p "$OUTPUT_DIR"

say "Generating documentation (this may take a while)..."
if invoke_launcher docs -o "$OUTPUT_DIR"; then
    if [ -d "$OUTPUT_DIR/generated" ]; then
        say "Documentation generated successfully → $OUTPUT_DIR/generated/"
    else
        die "Launcher reported success but $OUTPUT_DIR/generated/ is missing"
    fi
else
    die "Documentation generation failed"
fi
