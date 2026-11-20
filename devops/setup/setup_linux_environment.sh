#!/usr/bin/env bash

# REQUIREMENTS: bash 4+, curl, ca-certificates, unzip. The system-package step
#               needs apt-get plus root (the script uses sudo when it is not
#               already root) and only runs on Debian/Ubuntu; every other step
#               installs per-user and needs no privileges. Run from anywhere;
#               the script locates the repository root itself.

# DESCRIPTION: Prepares a fresh Linux machine to build and run this workspace
#   with:
#       export PROJECT_PATH=../devops/tests/projects/managed
#       cd modules
#       cargo run --package pill_standalone --features rendering
#
#   It is the Linux counterpart of setup_windows_environment.ps1 and performs
#   the same jobs, mapped onto this platform:
#
#   1. System libraries. The windowed host dlopens the windowing and GPU stack
#      at runtime and links the toolchain's shared `libstd` at load time, so a
#      desktop needs the Vulkan loader plus an ICD (or the GL/EGL fallback),
#      the X11/Wayland client libraries, and the C toolchain the build scripts
#      shell out to. The managed host additionally starts the .NET runtime,
#      which needs ICU and OpenSSL. Two crates in the graph also link system
#      libraries at build time through pkg-config - gilrs (gamepads) against
#      libudev, cpal (the audio extension) against alsa - so their `-dev`
#      packages must be present, or the build fails before anything runs. The
#      script installs only what is missing.
#
#   2. Rust toolchain (rustup). A plain `cargo run` needs it; the Windows
#      script installs it with winget, this one with rustup's own installer
#      into ~/.cargo.
#
#   3. Shader compiler (Slang). The renderer-data crate's build script cooks
#      every `.hlsl` through `slangc`, and the dev host's shader watcher
#      re-cooks edits the same way; nothing builds without it on PATH. Debian
#      has no Slang package, so the official release zip - at the version
#      pinned below (keep it equal to the Windows script's) - is installed into
#      ~/.local and added to PATH in ~/.bashrc.
#
#   4. .NET 8 SDK. This is the Linux stand-in for the Windows script's MSVC
#      step: the managed (`PROJECT_PATH=.../*.csproj`) backend builds the
#      project with `dotnet build` and boots CoreCLR through hostfxr. The host
#      looks for `libhostfxr.so` under DOTNET_ROOT, ~/.dotnet,
#      /usr/share/dotnet and /usr/lib/dotnet, and spawns the `dotnet`
#      executable from PATH - so the SDK is installed into ~/.dotnet (no root
#      needed) and DOTNET_ROOT/PATH are exported and appended to ~/.bashrc so
#      later shells inherit them.
#
#   5. An optional VS Code debugging setup: the editor (Microsoft's .deb, on
#      apt systems), the extensions the launch profiles and editing depend on,
#      and the dotnet-trace global tool.
#
#   6. Offline cargo registry. The host builds every extension and the project
#      with `cargo build --offline`, so a reload never waits on the registry
#      index. That only works once every dependency - including the pinned
#      `trait_type_map` git dependency - is already cached, so the script runs
#      `cargo fetch` in modules/ once while it still has the network.
#
#   Before any step runs, the script prints this plan with everything it may
#   install and asks once whether to include the VS Code debugging setup, so
#   the rest of the run needs no input. Without a terminal to ask on, the
#   optional step is skipped unless a flag selects it.
#
#   Like the Windows script, this one is idempotent: every step detects what is
#   already present and does nothing.

# USAGE: devops/setup/setup_linux_environment.sh [--no-fetch] [--vscode-debugging | --no-vscode-debugging]
#          (no arguments)          Prepare system packages, Rust, the .NET 8 SDK
#                                  and the offline cargo cache; ask whether to
#                                  add the VS Code debugging setup
#          --no-fetch              Skip the one-time online `cargo fetch` (useful
#                                  when the cache is already warm or the network
#                                  is down)
#          --vscode-debugging      Include the VS Code debugging setup without asking
#          --no-vscode-debugging   Skip the VS Code debugging setup without asking

# EXAMPLE USAGE:
#   bash devops/setup/setup_linux_environment.sh
#   export PROJECT_PATH=../devops/tests/projects/managed
#   cd modules && cargo run --package pill_standalone --features rendering

# --- SCRIPT ---

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
MODULES_DIR="$REPO_ROOT/modules"

# Coloured status lines, matching the rest of devops/.
GREEN=$'\033[0;32m'
YELLOW=$'\033[1;33m'
RED=$'\033[0;31m'
NC=$'\033[0m'

info() { printf '%s==>%s %s\n' "$GREEN" "$NC" "$*"; }
warn() { printf '%s[warn]%s %s\n' "$YELLOW" "$NC" "$*" >&2; }
fail() { printf '%s[error]%s %s\n' "$RED" "$NC" "$*" >&2; exit 1; }

# Commands every build spawns. Kept separate from the library list only so the
# warning can name which half is missing; both are covered by APT_PACKAGES.
BUILD_COMMANDS=(cc pkg-config curl git)

# Shared libraries the running host or a loaded extension links at load time.
RUNTIME_LIBRARIES=(
    libvulkan.so.1          # Vulkan loader (wgpu's primary backend)
    libGL.so.1              # GL fallback when no Vulkan ICD is present
    libEGL.so.1
    libxkbcommon.so.0       # keyboard handling (winit)
    libwayland-client.so.0  # Wayland windowing
    libX11.so.6             # X11 windowing
    libXcursor.so.1
    libudev.so.1            # gamepads (gilrs, linked into the windowed binary)
    libasound.so.2          # audio (cpal, linked into the audio extension)
    libicuuc.so             # ICU - the .NET runtime's globalization layer
    libssl.so.3             # OpenSSL 3 - the .NET runtime's crypto layer
)

# pkg-config modules build scripts probe before compiling. These come from the
# `-dev` packages below and fail the build, not the run, when missing: the
# runtime libraries above can all be present and a build still stops here.
PKG_CONFIG_MODULES=(
    libudev                 # gilrs-core (gamepads)
    alsa                    # alsa-sys (audio through cpal)
)

# Debian Bookworm package names providing the commands and libraries above,
# plus the C toolchain and certificate store the build scripts assume, unzip
# for the Slang release, and the runtime and `-dev` packages for the two
# libraries the gamepad and audio crates link (libudev, alsa). Names that a
# different distribution does not know (Ubuntu's libicu/libssl versions
# differ) are filtered out at install time rather than failing the whole run.
APT_PACKAGES=(
    build-essential pkg-config curl git ca-certificates unzip
    libvulkan1 mesa-vulkan-drivers libgl1 libegl1 libgles2
    libxkbcommon0 libwayland-client0 libwayland-cursor0 libwayland-egl1
    libx11-6 libxcursor1 libxi6 libxrandr2 libxcb1
    libudev1 libudev-dev libasound2 libasound2-dev
    libicu72 libssl3 zlib1g
)

# Run a command with root, reusing the caller's own privileges when they are
# already root and sudo otherwise. Returns 127 when neither is available, so
# the caller can fall back to printing the command instead of hanging on an
# interactive password prompt.
as_root() {
    if [[ "${EUID}" -eq 0 ]]; then
        "$@"
    elif command -v sudo > /dev/null 2>&1; then
        sudo "$@"
    else
        return 127
    fi
}

# Print the runtime libraries from the given list that ldconfig cannot resolve.
missing_runtime_libraries() {
    local library
    for library in "$@"; do
        ldconfig -p 2> /dev/null | grep -q "$library" || printf '%s\n' "$library"
    done
}

# Print the pkg-config modules from the given list that pkg-config cannot
# resolve. Without pkg-config itself nothing is printed - the command check
# already flags that, and the install covers the modules anyway.
missing_pkg_config_modules() {
    command -v pkg-config > /dev/null 2>&1 || return 0
    local module
    for module in "$@"; do
        pkg-config --exists "$module" 2> /dev/null || printf '%s\n' "$module"
    done
}

# --- 1. System packages ------------------------------------------------------

install_system_packages() {
    local tool
    local missing_commands=()
    for tool in "${BUILD_COMMANDS[@]}"; do
        command -v "$tool" > /dev/null 2>&1 || missing_commands+=("$tool")
    done

    local missing_libraries=()
    mapfile -t missing_libraries < <(missing_runtime_libraries "${RUNTIME_LIBRARIES[@]}")

    local missing_modules=()
    mapfile -t missing_modules < <(missing_pkg_config_modules "${PKG_CONFIG_MODULES[@]}")

    if [[ ${#missing_commands[@]} -eq 0 && ${#missing_libraries[@]} -eq 0 && ${#missing_modules[@]} -eq 0 ]]; then
        info "system commands, runtime libraries and build modules already present"
        return
    fi
    if [[ ${#missing_commands[@]} -gt 0 ]]; then
        warn "missing build commands: ${missing_commands[*]}"
    fi
    if [[ ${#missing_libraries[@]} -gt 0 ]]; then
        warn "missing runtime libraries: ${missing_libraries[*]}"
    fi
    if [[ ${#missing_modules[@]} -gt 0 ]]; then
        warn "missing pkg-config modules (shipped by the -dev packages): ${missing_modules[*]}"
    fi

    if ! command -v apt-get > /dev/null 2>&1; then
        warn "apt-get not found; install the equivalents with your package manager, then re-run:"
        printf '  packages: %s\n' "${APT_PACKAGES[*]}"
        return
    fi

    info "updating the apt package index..."
    if ! as_root apt-get update > /dev/null; then
        warn "could not run 'apt-get update' (needs root/sudo). Install manually, then re-run:"
        printf '  sudo apt-get install -y %s\n' "${APT_PACKAGES[*]}"
        return
    fi

    # Keep only the names this distribution knows, so a differing libicu/libssl
    # version name does not take the whole install down with it.
    local package
    local installable=()
    for package in "${APT_PACKAGES[@]}"; do
        if apt-cache show "$package" > /dev/null 2>&1; then
            installable+=("$package")
        else
            warn "package '$package' is unknown on this distribution; install its equivalent if the runtime needs it"
        fi
    done
    if [[ ${#installable[@]} -eq 0 ]]; then
        return
    fi

    info "installing system packages: ${installable[*]}"
    if ! as_root apt-get install -y "${installable[@]}"; then
        warn "package installation failed; install manually, then re-run:"
        printf '  sudo apt-get install -y %s\n' "${installable[*]}"
        return
    fi
    hash -r
    info "system packages installed"
}

# --- 2. Rust toolchain -------------------------------------------------------

install_rust() {
    if command -v cargo > /dev/null 2>&1; then
        info "Rust toolchain already installed ($(cargo --version))"
        return
    fi
    command -v curl > /dev/null 2>&1 || fail "curl is required to install Rust"
    info "installing Rust (rustup)..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
        | sh -s -- -y --profile minimal --default-toolchain stable
    # rustup installs into ~/.cargo and drops its own env script there; the
    # current shell has to source it before `cargo` is on PATH.
    # shellcheck source=/dev/null
    source "$HOME/.cargo/env"
    hash -r
    info "Rust installed ($(cargo --version))"
}

# --- 3. Shader compiler (Slang) ----------------------------------------------

# The pinned release; keep it equal to setup_windows_environment.ps1. The renderer-data
# crate's build script and the host's shader watcher both shell out to
# `slangc`; the zip is relocatable (its rpath is `$ORIGIN/../lib`), so an
# install that keeps bin/ and lib/ together works from ~/.local without root.
SLANG_VERSION="2025.21.2"

install_slangc() {
    if command -v slangc > /dev/null 2>&1; then
        info "Slang shader compiler already installed ($(command -v slangc))"
        return
    fi
    command -v curl > /dev/null 2>&1 || fail "curl is required to install the Slang shader compiler"
    command -v unzip > /dev/null 2>&1 || fail "unzip is required to extract the Slang release (install it, then re-run)"
    local install_dir="$HOME/.local/share/slang/$SLANG_VERSION"
    info "installing the Slang shader compiler ($SLANG_VERSION) into $install_dir ..."
    local archive
    archive="$(mktemp --suffix=.zip)"
    curl -fsSL "https://github.com/shader-slang/slang/releases/download/v${SLANG_VERSION}/slang-${SLANG_VERSION}-linux-x86_64.zip" -o "$archive"
    rm -rf "$install_dir"
    mkdir -p "$install_dir"
    unzip -q "$archive" -d "$install_dir"
    rm -f "$archive"
    export PATH="$install_dir/bin:$PATH"
    hash -r
    command -v slangc > /dev/null 2>&1 || fail "the Slang install did not produce a working 'slangc' on PATH"

    # Make the compiler visible to future shells, like the .NET SDK below.
    local rc_file="$HOME/.bashrc"
    local marker="# Slang shader compiler, added by devops/setup/setup_linux_environment.sh"
    if [[ ! -f "$rc_file" ]] || ! grep -qF "$marker" "$rc_file"; then
        {
            printf '\n%s\n' "$marker"
            printf 'export PATH="%s/bin:$PATH"\n' "$install_dir"
        } >> "$rc_file"
        info "added $install_dir/bin to PATH in $rc_file"
    fi
    info "Slang shader compiler installed ($(command -v slangc))"
}

# --- 4. .NET 8 SDK -----------------------------------------------------------

# Honour an existing DOTNET_ROOT, otherwise use the location the host already
# searches. Both the install and the PATH entry are derived from this value.
DOTNET_ROOT="${DOTNET_ROOT:-$HOME/.dotnet}"
export DOTNET_ROOT
export PATH="$DOTNET_ROOT:$PATH"

install_dotnet() {
    if command -v dotnet > /dev/null 2>&1 && dotnet --list-sdks 2> /dev/null | grep -q '^8\.'; then
        info ".NET 8 SDK already installed ($(dotnet --version))"
        return
    fi
    command -v curl > /dev/null 2>&1 || fail "curl is required to install the .NET SDK"
    info "installing the .NET 8 SDK into $DOTNET_ROOT ..."
    local installer
    installer="$(mktemp)"
    curl -sSL https://dot.net/v1/dotnet-install.sh -o "$installer"
    bash "$installer" --channel 8.0 --install-dir "$DOTNET_ROOT"
    rm -f "$installer"
    hash -r
    if ! dotnet --list-sdks 2> /dev/null | grep -q '^8\.'; then
        fail "the .NET SDK install did not produce a working 'dotnet' on PATH"
    fi
    info ".NET 8 SDK installed ($(dotnet --version))"
}

# Make the SDK visible to future shells, the way the Windows script refreshes
# the session PATH. Appended once, guarded by a marker comment.
persist_dotnet_environment() {
    local rc_file="$HOME/.bashrc"
    local marker="# Added by devops/setup/setup_linux_environment.sh"
    if [[ -f "$rc_file" ]] && grep -qF "$marker" "$rc_file"; then
        return
    fi
    {
        printf '\n%s\n' "$marker"
        printf 'export DOTNET_ROOT="%s"\n' "$DOTNET_ROOT"
        printf 'export PATH="$DOTNET_ROOT:$PATH"\n'
    } >> "$rc_file"
    info "added DOTNET_ROOT/PATH to $rc_file"
}

# --- 5. Optional VS Code debugging setup -------------------------------------

# Extensions the launch profiles and day-to-day editing rely on: CodeLLDB and
# C/C++ (gdb) debug the native host, the C# extensions attach coreclr to the
# managed runtime. Same list as the Windows script.
VSCODE_EXTENSIONS=(
    rust-lang.rust-analyzer
    ms-vscode.cpptools
    ms-dotnettools.csharp
    ms-dotnettools.csdevkit
    ms-vscode.hexeditor
    tamasfe.even-better-toml
    vadimcn.vscode-lldb
)

# Install VS Code from Microsoft's .deb when the `code` command is missing.
# Returns non-zero, after a warning, when it cannot be installed here, so the
# caller skips the extensions instead of failing the whole setup.
install_vscode() {
    if command -v code > /dev/null 2>&1; then
        info "VS Code already installed"
        return 0
    fi
    if ! command -v apt-get > /dev/null 2>&1; then
        warn "VS Code is not installed and apt-get is not available; install it from https://code.visualstudio.com, then re-run"
        return 1
    fi
    info "installing VS Code (Microsoft's .deb package)..."
    local package_file
    package_file="$(mktemp --suffix=.deb)"
    if ! curl -sSL 'https://update.code.visualstudio.com/latest/linux-deb-x64/stable' -o "$package_file" \
        || ! as_root apt-get install -y "$package_file"; then
        rm -f "$package_file"
        warn "VS Code installation failed; install it from https://code.visualstudio.com, then re-run"
        return 1
    fi
    rm -f "$package_file"
    hash -r
    info "VS Code installed"
}

# Install VS Code, its extensions and the dotnet-trace global tool, then put
# the .NET global tools folder on PATH for later shells.
install_vscode_debugging() {
    if install_vscode; then
        # Query the installed extensions once; each `code` call takes a second.
        local installed_extensions
        installed_extensions="$(code --list-extensions 2> /dev/null || true)"
        local extension
        for extension in "${VSCODE_EXTENSIONS[@]}"; do
            if grep -qixF "$extension" <<< "$installed_extensions"; then
                info "VS Code extension $extension already installed"
            else
                info "installing VS Code extension $extension..."
                code --install-extension "$extension" > /dev/null \
                    || warn "could not install VS Code extension $extension"
            fi
        done
    fi

    # `dotnet tool install` fails when the tool already exists, so check first.
    if dotnet tool list --global 2> /dev/null | grep -q 'dotnet-trace'; then
        info "dotnet-trace already installed"
    else
        info "installing dotnet-trace..."
        dotnet tool install --global dotnet-trace
    fi

    # Global tools land in ~/.dotnet/tools, which dotnet does not put on PATH.
    local rc_file="$HOME/.bashrc"
    local marker="# .NET global tools, added by devops/setup/setup_linux_environment.sh"
    if [[ ! -f "$rc_file" ]] || ! grep -qF "$marker" "$rc_file"; then
        {
            printf '\n%s\n' "$marker"
            printf 'export PATH="$PATH:$HOME/.dotnet/tools"\n'
        } >> "$rc_file"
        info "added ~/.dotnet/tools to PATH in $rc_file"
    fi
}

# --- 6. Offline cargo registry ----------------------------------------------

fetch_cargo_dependencies() {
    info "fetching cargo dependencies (online, one-time)..."
    (cd "$MODULES_DIR" && cargo fetch)
    info "cargo dependency cache populated"
}

# --- main -------------------------------------------------------------------

FETCH=1

# 1 or 0 when a flag decided the VS Code debugging setup, empty to ask.
VSCODE_DEBUGGING=""

usage() {
    sed -n 's/^# USAGE:/  /p; s/^# EXAMPLE USAGE:/  /p' "$0"
    printf '\n'
}

# Print what each step does and what it may install, so the user knows the
# whole plan before the one question and before anything changes.
show_setup_overview() {
    printf '\n%sPill workspace environment setup (Linux)%s\n\n' "$GREEN" "$NC"
    cat << 'EOF'
This script prepares this machine to build and run the engine. It will:

  1. System packages    Install the missing build tools and runtime libraries with apt
                        (C toolchain, pkg-config, git, Vulkan/GL, X11/Wayland, ICU,
                        OpenSSL, gamepad/audio dev files). Uses sudo; on other
                        distributions it only lists them.
  2. Rust toolchain     Install rustup and the stable toolchain into ~/.cargo.
  3. Slang compiler     Install the Slang shader compiler into ~/.local/share/slang
                        and add it to PATH in ~/.bashrc; the renderer's build cooks
                        every .hlsl through `slangc`.
  4. .NET 8 SDK         Install it into ~/.dotnet and add DOTNET_ROOT and PATH to ~/.bashrc.
  5. VS Code debugging  Optional. Install VS Code (with sudo), the dotnet-trace tool and
                        these extensions: rust-analyzer, C/C++, C#, C# Dev Kit, Hex Editor,
                        Even Better TOML and CodeLLDB.
  6. Cargo cache        Download every crate dependency once (cargo fetch), so offline
                        builds during hot reload never wait on the network.

Anything already installed is detected and skipped.

EOF
}

# Settle the VS Code question before any step runs: a flag wins, otherwise ask
# on the terminal (default yes), and with no terminal to ask on, skip it.
choose_vscode_debugging() {
    if [[ -n "$VSCODE_DEBUGGING" ]]; then
        return
    fi
    if [[ ! -t 0 ]]; then
        VSCODE_DEBUGGING=0
        info "no terminal to ask on; skipping the VS Code debugging setup (pass --vscode-debugging to include it)"
        return
    fi
    local answer
    read -r -p "Include the optional VS Code debugging setup (step 5)? [Y/n] " answer
    case "${answer,,}" in
        "" | y | yes) VSCODE_DEBUGGING=1 ;;
        *) VSCODE_DEBUGGING=0 ;;
    esac
    printf '\n'
}

main() {
    local argument
    for argument in "$@"; do
        case "$argument" in
            --no-fetch) FETCH=0 ;;
            --vscode-debugging) VSCODE_DEBUGGING=1 ;;
            --no-vscode-debugging) VSCODE_DEBUGGING=0 ;;
            -h | --help)
                usage
                exit 0
                ;;
            *) fail "unknown argument: $argument (try --help)" ;;
        esac
    done

    show_setup_overview
    choose_vscode_debugging

    info "repository root: $REPO_ROOT"
    install_system_packages
    install_rust
    install_slangc
    install_dotnet
    persist_dotnet_environment
    if [[ "$VSCODE_DEBUGGING" == "1" ]]; then
        install_vscode_debugging
    else
        info "skipping the VS Code debugging setup"
    fi
    if [[ "$FETCH" == "1" ]]; then
        fetch_cargo_dependencies
    fi

    printf '\n%sSetup complete.%s Run the project with:\n' "$GREEN" "$NC"
    printf '  export DOTNET_ROOT="%s"\n' "$DOTNET_ROOT"
    printf '  export PATH="$DOTNET_ROOT:$PATH"\n'
    printf '  export PROJECT_PATH=../devops/tests/projects/managed\n'
    printf '  cd modules\n'
    printf '  cargo run --package pill_standalone --features rendering\n'
    printf '\n'
    printf 'Windows used to need Smart App Control off before the host could load\n'
    printf 'the DLLs it compiles at runtime. Linux has no equivalent, but the run\n'
    printf 'must have a display: the host opens a window, so run it from a desktop\n'
    printf 'session (X11 or Wayland), not a bare TTY.\n'
}

main "$@"
