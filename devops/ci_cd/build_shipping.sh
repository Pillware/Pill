#!/usr/bin/env bash

# REQUIREMENTS: bash 4+, Rust toolchain (cargo) on PATH. Run from anywhere; the
#               script locates the repository root itself.

# DESCRIPTION: Builds a shipping build in the release profile, which a plain
#   `cargo build --release` cannot do.
#
#   The obstacle is `-C prefer-dynamic` in `modules/.cargo/config.toml`. It is
#   there so the host executable and every extension share one copy of
#   `pill_engine`, which is what keeps its statics, thread-locals and tracing
#   dispatcher single-instance across the DLL boundary. A release binary links
#   everything into one image and needs none of that - and rustc refuses
#   `-C prefer-dynamic` together with the release profile's `lto = "fat"` when
#   targeting Windows, so the flag has to go for a release-profile build to
#   happen at all.
#
#   Cargo offers no way to make `build.rustflags` conditional on the profile.
#   `--config build.rustflags=[]` does not work either: cargo MERGES config
#   arrays under the same key, so an empty list joins with the existing one and
#   changes nothing. Clearing `RUSTFLAGS` in the environment is the only
#   mechanism that does, which is why this is a script rather than a cargo
#   alias - an alias cannot set environment variables.
#
#   Everything else about the build is ordinary cargo. Extra arguments are
#   forwarded, so this is a drop-in replacement for `cargo build --release`.

# USAGE: devops/ci_cd/build_shipping.sh [cargo arguments...]
#          (no arguments)      Build the shipping host (project from
#                              PROJECT_PATH, bundle regenerated, feature
#                              `shipping`; a C# project also gets its
#                              assemblies built and copied)
#          --profile <name>   Build a different release profile
#                             (release-fast, release-with-debug)
#          --project <path>   Project directory (workspace-relative) whose
#                             project_settings.yaml drives the shipping bundle
#                             (defaults to PROJECT_PATH)
#          Any other argument is passed straight to cargo.
#
#   A shipping build of the host is always the shipping posture: release-profile
#   building `pill_standalone` requires `--no-default-features --features
#   shipping`, and the script refuses any other
#   way - as does `pill_standalone`'s build script, for direct cargo builds.
#   A managed (C#) project is additionally built with `dotnet build -c
#   Release` before the host, and its assemblies are copied alongside the
#   shipping binary.

# EXAMPLE USAGE:
#   set PROJECT_PATH=devops/tests/projects/native
#   python devops/ci_cd/build_shipping.sh                        # native shipping build
#   set PROJECT_PATH=devops/tests/projects/managed
#   devops/ci_cd/build_shipping.sh                            # managed (C#) shipping build
#   devops/ci_cd/build_shipping.sh -p pill_engine             # any package, release profile

# --- SCRIPT ---

set -euo pipefail

# The repository root is two levels above this script, so the build works from
# any working directory.
script_directory="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repository_root="$(cd "${script_directory}/../.." && pwd)"
workspace_directory="${repository_root}/modules"

if [[ ! -f "${workspace_directory}/Cargo.toml" ]]; then
    echo "error: no workspace at ${workspace_directory}" >&2
    exit 1
fi

# The project whose `project_settings.yaml` drives the shipping bundle;
# `--project` overrides PROJECT_PATH. The flag is consumed here, not forwarded
# to cargo.
project_path="${PROJECT_PATH:-}"
forwarded_arguments=()
previous=""
for argument in "$@"; do
    if [[ "${previous}" == "--project" ]]; then
        project_path="${argument}"
        previous=""
        continue
    fi
    case "${argument}" in
        --project=*) project_path="${argument#--project=}" ;;
        --project) previous="--project" ;;
        *) forwarded_arguments+=("${argument}") ;;
    esac
    [[ "${argument}" == "--project" ]] && previous="--project"
done
if [[ -n "${previous}" ]]; then
    echo "error: --project requires a value" >&2
    exit 1
fi
set -- "${forwarded_arguments[@]}"

# Returns 0 when the invocation already scopes which packages cargo builds
# (-p/--package/--workspace/--all/--exclude). Without one, the script defaults
# to the shipping host below.
package_scoping_present() {
    local argument
    for argument in "$@"; do
        case "${argument}" in
            -p|--package|--workspace|--all|--exclude) return 0 ;;
            --package=*|--exclude=*) return 0 ;;
        esac
    done
    return 1
}

# Resolve the project root up front so the shipping default below can pick the
# posture matching the project's scripting language: a `*.csproj` in the root
# means managed (C#), anything else means native Rust.
project_root=""
managed_project=0
if [[ -n "${project_path}" ]]; then
    case "${project_path}" in
        /*) project_root="${project_path}" ;;
        *)
            # The host resolves PROJECT_PATH against the working directory, so
            # `../devops/tests/projects/native` works from the workspace dir; try that
            # first, then the repository root, so both spellings work.
            if [[ -d "${PWD}/${project_path}" ]]; then
                project_root="${PWD}/${project_path}"
            else
                project_root="${repository_root}/${project_path}"
            fi
            ;;
    esac
    if compgen -G "${project_root}"/*.csproj >/dev/null 2>&1; then
        managed_project=1
    fi
fi

# A plain invocation defaults to the shipping host: pill_standalone built with
# `--no-default-features --features shipping`, native and managed projects
# alike (the generated bundle carries the backend).
host_default=()
if ! package_scoping_present "$@"; then
    host_default=(--package pill_standalone --no-default-features --features shipping)
fi
set -- "${host_default[@]}" "$@"

# Build output lives with the project: cargo's target dir under
# build/build_meta/pill_build_data, and dated artifact copies under build/<date>.
target_directory=""
artifacts_directory=""
if [[ -n "${project_path}" ]]; then
    target_directory="${project_root}/build/build_meta/pill_build_data"
    artifacts_directory="${project_root}/build/$(date +%d-%m-%Y_%H-%M)"
    # The project's recorded artifact name (validated and written by the
    # generator into the shared build scratch dir); it names the copied binary
    # and PDB.
    build_binary_name=""
    if [[ -f "${repository_root}/build/build_meta/build_binary_name.txt" ]]; then
        build_binary_name="$(cat "${repository_root}/build/build_meta/build_binary_name.txt")"
    fi
    if [[ -z "${build_binary_name}" ]]; then
        echo "error: build/build_meta/build_binary_name.txt missing (the generator did not record a build binary name)" >&2
        exit 1
    fi
fi

# A shipping build of the host is always the shipping posture: hot reload is a
# development tool and must not ship. Refuse any invocation that would compile
# `pill_standalone` without `shipping`. `pill_standalone`'s
# build script enforces the same rule for direct cargo builds; this fails first
# with the same guidance.

# Returns 0 when the invocation would compile `pill_standalone`: it is named by
# `-p`/`--package`, selected by `--workspace`/`--all`, or no package scoping is
# given at all (cargo then builds every workspace member). `--exclude
# pill_standalone` opts it out in every case.
host_will_build() {
    local host_targeted=0
    local host_excluded=0
    local workspace_requested=0
    local packages_scoped=0
    local previous=""
    local argument
    for argument in "$@"; do
        case "${previous}" in
            -p|--package)
                packages_scoped=1
                [[ "${argument}" == "pill_standalone" ]] && host_targeted=1
                ;;
            --exclude)
                [[ "${argument}" == "pill_standalone" ]] && host_excluded=1
                ;;
        esac
        case "${argument}" in
            --workspace|--all) workspace_requested=1 ; packages_scoped=1 ;;
            -p|--package|--exclude) previous="${argument}" ;;
            *) previous="" ;;
        esac
    done
    [[ ${host_excluded} -eq 1 ]] && return 1
    [[ ${host_targeted} -eq 1 ]] && return 0
    # Scoped to specific packages (not `--workspace`) and the host is not among
    # them: the host is not built.
    [[ ${packages_scoped} -eq 1 && ${workspace_requested} -eq 0 ]] && return 1
    return 0
}

# Returns 0 when any of the comma-separated feature names in the first argument
# appear across `--features`/`-F`/`--features=...` arguments.
features_selected() {
    local names_string="$1"
    shift
    local names=()
    local name
    IFS=',' read -ra names <<< "${names_string}"
    local previous=""
    local argument
    for argument in "$@"; do
        local value=""
        if [[ "${previous}" == "--features" || "${previous}" == "-F" ]]; then
            value="${argument}"
        elif [[ "${argument}" == --features=* ]]; then
            value="${argument#--features=}"
        fi
        if [[ -n "${value}" ]]; then
            local item
            IFS=',' read -ra items <<< "${value}"
            for item in "${items[@]}"; do
                for name in "${names[@]}"; do
                    [[ "${item}" == "${name}" ]] && return 0
                done
            done
        fi
        previous="${argument}"
    done
    return 1
}

# Returns 0 when the named flag appears among the arguments.
flag_selected() {
    local flag="$1"
    shift
    local argument
    for argument in "$@"; do
        [[ "${argument}" == "${flag}" ]] && return 0
    done
    return 1
}

if host_will_build "$@"; then
    if features_selected "shipping" "$@"; then
        # The shipping posture links the project in; the default `dev`
        # feature would otherwise stay on, so it must be turned off.
        if ! flag_selected "--no-default-features" "$@"; then
            echo "error: the shipping posture (\`shipping\`) needs \`--no-default-features\`, otherwise the default \`dev\` feature stays on and the binary ships the development host." >&2
            exit 1
        fi
        if features_selected "dev" "$@"; then
            echo "error: a shipping build cannot combine \`shipping\` with \`dev\`." >&2
            exit 1
        fi
        # Regenerate the shipping bundle from the project's settings file so
        # the static build always reflects `project_settings.yaml`.
        if [[ -z "${project_path}" ]]; then
            echo "error: no project path: set PROJECT_PATH or pass --project (the shipping bundle is generated from the project's project_settings.yaml)." >&2
            exit 1
        fi
        if ! python "${repository_root}/devops/tools/generate_shipping_bundle.py" "${project_path}"; then
            echo "error: shipping bundle generation failed" >&2
            exit 1
        fi
        # A managed shipping build loads a prebuilt assembly: produce it with
        # dotnet before the host is linked. The bundle declared the modules in
        # Rust, so this dotnet build is the whole managed side of the binary.
        if [[ ${managed_project} -eq 1 ]]; then
            managed_manifest="$(find "${project_root}" -maxdepth 1 -name '*.csproj' | head -n 1)"
            if [[ -z "${managed_manifest}" ]]; then
                echo "error: no .csproj in managed project root ${project_root}" >&2
                exit 1
            fi
            if ! command -v dotnet >/dev/null 2>&1; then
                echo "error: a C# shipping build needs the .NET SDK (dotnet) on PATH" >&2
                exit 1
            fi
            echo "Building the managed project assembly (dotnet build -c Release)."
            ( cd "${repository_root}" && dotnet build "${managed_manifest}" -c Release --nologo )
        fi
    else
        echo "error: a shipping build of pill_standalone is always the shipping posture - hot reload is a development tool and must not ship." >&2
        echo "  build it as: devops/ci_cd/build_shipping.sh --package pill_standalone --no-default-features --features shipping" >&2
        echo "  or scope the build away from the host (e.g. -p pill_engine or --exclude pill_standalone)." >&2
        exit 1
    fi
fi

# `--profile` may already be among the forwarded arguments; adding `--release`
# as well would make cargo reject the pair.
profile_already_chosen=0
for argument in "$@"; do
    if [[ "${argument}" == "--profile" || "${argument}" == --profile=* ]]; then
        profile_already_chosen=1
        break
    fi
done

profile_arguments=()
if [[ ${profile_already_chosen} -eq 0 ]]; then
    profile_arguments=(--release)
fi

echo "Building the release profile with RUSTFLAGS cleared."
echo "  workspace: ${workspace_directory}"

# An empty RUSTFLAGS replaces `build.rustflags` outright rather than merging
# with it, which is exactly what removing `-C prefer-dynamic` requires. `--offline`
# avoids the registry package-cache lock (which rust-analyzer can hold) stalling
# the build; every dependency is a path dep or already cached. A shipping build
# also redirects cargo's target dir into the project's build/build_meta.
cd "${workspace_directory}"
if [[ -n "${target_directory}" ]]; then
    RUSTFLAGS="" CARGO_TARGET_DIR="${target_directory}" cargo build --offline "${profile_arguments[@]}" "$@"
else
    RUSTFLAGS="" cargo build --offline "${profile_arguments[@]}" "$@"
fi

# Copy the finished shipping artifacts into the dated output directory (only
# reached when the build above succeeded - a failure exits via `set -e`). The
# binary and PDB are renamed to the project's recorded name.
if [[ -n "${artifacts_directory}" ]]; then
    mkdir -p "${artifacts_directory}"
    binary_extension=".exe"
    if [[ ! -f "${target_directory}/release/pill_standalone.exe" ]]; then
        binary_extension=""
    fi
    for pair in \
        "${target_directory}/release/pill_standalone${binary_extension}|${build_binary_name}${binary_extension}" \
        "${target_directory}/release/pill_standalone.pdb|${build_binary_name}.pdb"; do
        source="${pair%%|*}"
        target="${pair#*|}"
        if [[ -f "${source}" ]]; then
            cp -f "${source}" "${artifacts_directory}/${target}"
            echo "  artifact: ${target}"
        fi
    done
    for sidecar in "${target_directory}"/release/std-*.dll; do
        if [[ -f "${sidecar}" ]]; then
            cp -f "${sidecar}" "${artifacts_directory}/"
            echo "  artifact: $(basename "${sidecar}")"
        fi
    done
    # The managed side of a C# shipping build: the project assembly and the
    # C# runtime it references, recorded alongside the shipping binary.
    if [[ ${managed_project} -eq 1 && -n "${managed_manifest:-}" ]]; then
        managed_assembly_name="$(basename "${managed_manifest}" .csproj)"
        for source in \
            "${project_root}/bin/Release/net8.0/${managed_assembly_name}.dll" \
            "${project_root}/bin/Release/net8.0/${managed_assembly_name}.pdb" \
            "${workspace_directory}/csharp/pill_csharp_runtime/bin/Release/net8.0/csharp_runtime.dll" \
            "${workspace_directory}/csharp/pill_csharp_runtime/bin/Release/net8.0/csharp_runtime.runtimeconfig.json"; do
            if [[ -f "${source}" ]]; then
                cp -f "${source}" "${artifacts_directory}/"
                echo "  artifact: $(basename "${source}")"
            fi
        done
    fi
    echo "artifacts: ${artifacts_directory}"
fi
