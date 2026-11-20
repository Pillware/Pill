#!/usr/bin/env python3

# REQUIREMENTS: Python 3.8+, the Rust toolchain (cargo, rustc) on PATH, and
#               cargo-about 0.9 (`cargo install cargo-about --locked
#               --features cli`). For --dotnet-runtime, the `dotnet` command on
#               PATH. The crates must already be downloaded (the script runs
#               cargo-about offline), which any build of the same manifest
#               does.

# DESCRIPTION: Writes the third-party license notices for one build into a
#   `licenses/` directory. A distributed binary contains the Rust crates it was
#   compiled from, the Rust standard library and, for a C# project shipped with
#   NativeAOT, the .NET runtime; their licenses (MIT, Apache-2.0, BSD, MPL-2.0,
#   Unicode-3.0, OFL-1.1 and others) require their notices to travel with the
#   binary.
#
#   cargo-about resolves the crates the given package compiles for the given
#   target and features, leaving out build and dev dependencies, and checks
#   every license against the accepted list in `modules/about.toml`. A crate
#   under any other license, or a third-party crate with no license at all,
#   fails the generation, so it is noticed before the build ships. Pill's own
#   crates (path dependencies) are not third-party and are skipped.
#
#   The directory receives:
#     THIRD_PARTY_LICENSES.html          every crate, grouped by license text,
#                                        with a link to its source
#     RUST_STANDARD_LIBRARY_LICENSES.html the toolchain's own notice file for
#                                        the standard library
#     DOTNET_LICENSE.txt,                the .NET runtime's license and notices
#     DOTNET_THIRD_PARTY_NOTICES.txt     (only with --dotnet-runtime)
#
#   Exit codes: 0 written, 1 failed (a license problem or a tool error), 3
#   cargo-about is not installed, so a calling build can warn and continue.

# USAGE: python devops/tools/generate_third_party_licenses.py
#          --manifest-path <Cargo.toml>  manifest of the package that becomes
#                                        the binary
#          --output-directory <dir>      where `licenses/` is created
#          [--target <triple>]           target triple, repeatable (default:
#                                        the host)
#          [--features "<a b>"]          features to enable
#          [--all-features]              enable every feature
#          [--no-default-features]       disable the package's default features
#          [--workspace]                 every workspace member, not only the
#                                        manifest's package (a license check
#                                        across the whole workspace)
#          [--dotnet-runtime]            also copy the .NET runtime's notices
#          [--title <text>]              heading of the HTML page

# EXAMPLE USAGE:
#   python devops/tools/generate_third_party_licenses.py \
#       --manifest-path modules/frontends/pill_standalone/Cargo.toml \
#       --no-default-features --features shipping \
#       --output-directory build/licenses_check
#   python devops/tools/generate_third_party_licenses.py \
#       --manifest-path build/pill_web_app/Cargo.toml \
#       --target wasm32-unknown-unknown --output-directory build/web
#   python devops/tools/generate_third_party_licenses.py \
#       --manifest-path modules/Cargo.toml --workspace --all-features \
#       --target x86_64-pc-windows-msvc --target wasm32-unknown-unknown \
#       --output-directory build/build_meta/license_check

# --- SCRIPT ---

# Standard library
import argparse
import html
import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Dict, List, Optional

# cargo-about's diagnostics use box-drawing characters, which the Windows
# console's default code page cannot print.
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")
if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")

# The cargo-about configuration holding the accepted licenses.
ABOUT_CONFIGURATION = Path("modules") / "about.toml"
# The directory created inside --output-directory.
LICENSES_DIRECTORY_NAME = "licenses"
HTML_FILE_NAME = "THIRD_PARTY_LICENSES.html"
RUST_NOTICE_FILE_NAME = "RUST_STANDARD_LIBRARY_LICENSES.html"
DOTNET_LICENSE_FILE_NAME = "DOTNET_LICENSE.txt"
DOTNET_NOTICES_FILE_NAME = "DOTNET_THIRD_PARTY_NOTICES.txt"
# Exit code telling a calling build that the tool is missing, not broken.
EXIT_TOOL_MISSING = 3


class LicenseGenerationError(Exception):
    """A license problem or a failed tool run that must stop the build."""


def repository_root() -> Path:
    """The repository root, located from this script."""
    return Path(__file__).resolve().parent.parent.parent


# The host's target triple from `rustc -vV`. Without an explicit target,
# cargo-about would list the crates of every platform, not the ones a host
# build compiles.
def host_target() -> str:
    completed = subprocess.run(["rustc", "-vV"], capture_output=True, text=True)
    for line in completed.stdout.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    raise LicenseGenerationError("could not read the host target from rustc -vV")


# Runs cargo-about for one package and returns its JSON report. Raises
# LicenseGenerationError when cargo-about fails, which includes a crate under a
# license missing from the accepted list.
def run_cargo_about(
    manifest_path: Path,
    targets: List[str],
    features: str,
    all_features: bool,
    no_default_features: bool,
    workspace: bool,
) -> Dict:
    with tempfile.TemporaryDirectory() as temporary_directory:
        report_path = Path(temporary_directory) / "report.json"
        command = [
            "cargo",
            "about",
            "generate",
            "--format",
            "json",
            "--offline",
            "--config",
            str(repository_root() / ABOUT_CONFIGURATION),
            "--manifest-path",
            str(manifest_path),
            "--output-file",
            str(report_path),
        ]
        for target in targets or [host_target()]:
            command += ["--target", target]
        if features:
            command += ["--features", features]
        if all_features:
            command.append("--all-features")
        if no_default_features:
            command.append("--no-default-features")
        if workspace:
            command.append("--workspace")
        completed = subprocess.run(
            command,
            cwd=str(manifest_path.parent),
            capture_output=True,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
        # cargo-about warns about every one of Pill's own crates, which carry
        # no license field; those lines would bury the real error.
        if completed.returncode != 0 or not report_path.is_file():
            relevant_lines = [
                line
                for line in completed.stderr.splitlines()
                if "doesn't have a license field" not in line
                and "unable to synthesize license expression" not in line
            ]
            raise LicenseGenerationError(
                "cargo-about failed:\n" + "\n".join(relevant_lines).strip()
            )
        return json.loads(report_path.read_text(encoding="utf-8"))


# A crate from a registry or a git repository is third-party; a path
# dependency is one of Pill's own crates.
def is_third_party(package: Dict) -> bool:
    source = package.get("source") or ""
    return source.startswith("registry+") or source.startswith("git+")


# Refuses a report in which a third-party crate has no license cargo-about
# could determine, because such a crate cannot be shipped with a notice.
def check_every_crate_is_licensed(report: Dict) -> None:
    unlicensed_names = [
        f'{entry["package"]["name"]} {entry["package"]["version"]}'
        for entry in report["crates"]
        if is_third_party(entry["package"]) and not entry.get("license")
    ]
    if unlicensed_names:
        raise LicenseGenerationError(
            "third-party crates with no determinable license: "
            + ", ".join(sorted(unlicensed_names))
        )


# The page where the exact source of a crate can be downloaded. For an MPL-2.0
# crate this is what tells recipients where its source is available.
def crate_source_link(package: Dict) -> str:
    source = package.get("source") or ""
    if source.startswith("registry+"):
        return f'https://crates.io/crates/{package["name"]}/{package["version"]}'
    return package.get("repository") or source


# Renders the report into one self-contained HTML page. `extra_links` are the
# notice files copied beside it, as (title, file name, description) tuples.
def render_html(report: Dict, title: str, extra_links: List[tuple]) -> str:
    licenses = [
        license_entry
        for license_entry in report["licenses"]
        if any(is_third_party(user["crate"]) for user in license_entry["used_by"])
    ]
    crate_count = len(
        {
            user["crate"]["id"]
            for license_entry in licenses
            for user in license_entry["used_by"]
            if is_third_party(user["crate"])
        }
    )
    lines = [
        "<!DOCTYPE html>",
        '<html lang="en">',
        "<head>",
        '<meta charset="utf-8">',
        f"<title>{html.escape(title)}</title>",
        "<style>",
        "body{font-family:system-ui,sans-serif;max-width:60rem;margin:2rem auto;"
        "padding:0 1rem;line-height:1.5;color:#1d1d1f;background:#fff}",
        "pre{white-space:pre-wrap;background:#f4f4f5;padding:1rem;font-size:.85rem}",
        "table{border-collapse:collapse}td,th{padding:.25rem .75rem;text-align:left}",
        "h3{margin-top:2.5rem}",
        "</style>",
        "</head>",
        "<body>",
        f"<h1>{html.escape(title)}</h1>",
        "<p>This software contains the third-party components listed below. "
        "Each is used under the license shown with it.</p>",
    ]

    # Notices that come as their own files, such as the standard library's.
    if extra_links:
        lines.append("<h2>Bundled runtimes</h2>")
        lines.append("<ul>")
        for link_title, file_name, description in extra_links:
            lines.append(
                f'<li><a href="{html.escape(file_name)}">{html.escape(link_title)}'
                f"</a>: {html.escape(description)}</li>"
            )
        lines.append("</ul>")

    # The overview: each license and how many crates use it.
    lines.append(f"<h2>Rust crates ({crate_count})</h2>")
    lines.append("<table><tr><th>License</th><th>Crates</th></tr>")
    for overview_entry in report["overview"]:
        lines.append(
            f'<tr><td>{html.escape(overview_entry["name"])}</td>'
            f'<td>{overview_entry["count"]}</td></tr>'
        )
    lines.append("</table>")

    # One section per distinct license text, with the crates that use it.
    for index, license_entry in enumerate(licenses):
        third_party_users = sorted(
            (user for user in license_entry["used_by"] if is_third_party(user["crate"])),
            key=lambda user: (user["crate"]["name"], user["crate"]["version"]),
        )
        lines.append(
            f'<h3 id="license-{index}">{html.escape(license_entry["name"])}</h3>'
        )
        lines.append("<p>Used by:</p><ul>")
        for user in third_party_users:
            package = user["crate"]
            link = crate_source_link(package)
            label = f'{package["name"]} {package["version"]}'
            lines.append(
                f'<li><a href="{html.escape(link)}">{html.escape(label)}</a></li>'
            )
        lines.append("</ul>")
        lines.append(f'<pre>{html.escape(license_entry["text"])}</pre>')

    lines += ["</body>", "</html>", ""]
    return "\n".join(lines)


# Copies the standard library's notice file from the active toolchain. Returns
# its link tuple, or None when the toolchain does not ship it.
def copy_rust_standard_library_notice(licenses_directory: Path) -> Optional[tuple]:
    completed = subprocess.run(
        ["rustc", "--print", "sysroot"], capture_output=True, text=True
    )
    if completed.returncode != 0:
        return None
    notice_path = (
        Path(completed.stdout.strip()) / "share" / "doc" / "rust" / "COPYRIGHT-library.html"
    )
    if not notice_path.is_file():
        return None
    shutil.copyfile(notice_path, licenses_directory / RUST_NOTICE_FILE_NAME)
    return (
        "Rust standard library",
        RUST_NOTICE_FILE_NAME,
        "compiled into every Rust binary; MIT or Apache-2.0, with the notices "
        "of its own dependencies",
    )


# Copies the .NET runtime's license and third-party notices from the installed
# dotnet root. Raises LicenseGenerationError when they cannot be found, since
# a NativeAOT build embeds that runtime.
def copy_dotnet_notices(licenses_directory: Path) -> List[tuple]:
    dotnet_executable = shutil.which("dotnet")
    if dotnet_executable is None:
        raise LicenseGenerationError("--dotnet-runtime given but dotnet is not on PATH")
    dotnet_root = Path(dotnet_executable).resolve().parent
    copied_links = []
    for source_name, target_name, link_title in (
        ("LICENSE.txt", DOTNET_LICENSE_FILE_NAME, ".NET runtime license"),
        ("ThirdPartyNotices.txt", DOTNET_NOTICES_FILE_NAME, ".NET runtime third-party notices"),
    ):
        source_path = dotnet_root / source_name
        if not source_path.is_file():
            raise LicenseGenerationError(f"missing {source_path}")
        shutil.copyfile(source_path, licenses_directory / target_name)
        copied_links.append(
            (link_title, target_name, "embedded in the NativeAOT library")
        )
    return copied_links


# Writes every notice file for one build. Returns the paths written, relative
# to output_directory.
def generate(
    manifest_path: Path,
    output_directory: Path,
    targets: List[str],
    features: str,
    all_features: bool,
    no_default_features: bool,
    workspace: bool,
    dotnet_runtime: bool,
    title: str,
) -> List[Path]:
    report = run_cargo_about(
        manifest_path, targets, features, all_features, no_default_features, workspace
    )
    check_every_crate_is_licensed(report)

    licenses_directory = output_directory / LICENSES_DIRECTORY_NAME
    if licenses_directory.exists():
        shutil.rmtree(licenses_directory)
    licenses_directory.mkdir(parents=True)

    extra_links = []
    rust_link = copy_rust_standard_library_notice(licenses_directory)
    if rust_link is not None:
        extra_links.append(rust_link)
    if dotnet_runtime:
        extra_links += copy_dotnet_notices(licenses_directory)

    html_path = licenses_directory / HTML_FILE_NAME
    html_path.write_text(render_html(report, title, extra_links), encoding="utf-8")
    return sorted(
        path.relative_to(output_directory) for path in licenses_directory.iterdir()
    )


def main() -> int:
    """Parses the arguments and writes the notices; returns the exit code."""
    parser = argparse.ArgumentParser(description="Write third-party license notices.")
    parser.add_argument("--manifest-path", required=True, type=Path)
    parser.add_argument("--output-directory", required=True, type=Path)
    parser.add_argument("--target", action="append", default=[])
    parser.add_argument("--features", default="")
    parser.add_argument("--all-features", action="store_true")
    parser.add_argument("--no-default-features", action="store_true")
    parser.add_argument("--workspace", action="store_true")
    parser.add_argument("--dotnet-runtime", action="store_true")
    parser.add_argument("--title", default="Third-party licenses")
    arguments = parser.parse_args()

    # A missing tool is reported with its own exit code, so a build can warn.
    if shutil.which("cargo-about") is None:
        print(
            "warning: cargo-about is not installed, so no third-party license "
            "notices were written (cargo install cargo-about --locked "
            "--features cli)",
            file=sys.stderr,
        )
        return EXIT_TOOL_MISSING

    try:
        written_paths = generate(
            arguments.manifest_path.resolve(),
            arguments.output_directory.resolve(),
            arguments.target,
            arguments.features,
            arguments.all_features,
            arguments.no_default_features,
            arguments.workspace,
            arguments.dotnet_runtime,
            arguments.title,
        )
    except LicenseGenerationError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1

    for path in written_paths:
        print(f"third-party licenses: {arguments.output_directory / path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
