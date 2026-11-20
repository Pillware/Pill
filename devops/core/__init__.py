"""
Shared code for the devops tooling.

REQUIREMENTS: Python 3.8+ (standard library only).

DESCRIPTION
    Everything the devops scripts have in common. Nothing here executes a
    measurement or asserts anything; it is the layer they all import.

      * `paths`         - devops/repository layout resolution, plus
                          `ensure_devops_on_path()` which every standalone
                          script calls before importing this package.
      * `projects`      - the projects the scripts drive and the files inside
                          them the scripts read or edit.
      * `environment`   - process queries and timestamps.
      * `suite_common`  - host process plumbing: log tokens, the output
                          monitor, atomic source edits, backup/restore.
      * `editor_log_tokens` - the editor driver's log constants.
      * `editor_session` - one editor run for a suite: the per-run sandbox,
                          the launch environment and the character-anchored
                          waits.
      * `scenario`      - the shared scenario runner those suites run their
                          tables on: edit, wait, assert, restore.
      * `test_report`   - the pass/fail/skip tally, the summary block and the
                          size reports.
      * `win_clipboard` - the editor suite's side of the Windows clipboard.
      * `workspace_graph` - the Cargo-manifest reader the architecture guards
                          check.

    A standalone script reaches this package with:

        from pathlib import Path
        import sys
        sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
        from core.paths import ensure_devops_on_path

--- SCRIPT ---
"""
