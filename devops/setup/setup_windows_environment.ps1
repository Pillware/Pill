# REQUIREMENTS:
#   Windows 10/11 with Windows PowerShell 5.1 or newer, run from an ordinary
#   (non-admin) session. The installers this script starts request elevation
#   through UAC themselves when required.

# DESCRIPTION:
#   Prepares a fresh Windows machine to build and run this workspace with:
#       cargo run --package pill_standalone --features rendering --offline
#
#   It is the Windows counterpart of setup_linux_environment.sh and performs
#   the same jobs, mapped onto this platform:
#
#   1. Rust toolchain (rustup), through winget or rustup's own installer when
#      an enterprise policy blocks winget.
#   2. MSVC build tools and the Windows SDK. The workspace links with rust-lld,
#      but rust-lld still needs the MSVC and Windows SDK import libraries; only
#      link.exe is replaced.
#   3. The Slang shader compiler. The renderer-data crate's build script and
#      the dev host's shader watcher both shell out to `slangc`; without it
#      every build of the renderer fails.
#   4. Smart App Control, which blocks LoadLibrary of the DLLs the host
#      compiles at runtime for hot reload (os error 4551).
#   5. An optional VS Code debugging setup: the editor, the .NET 8 SDK and the
#      extensions the launch profiles depend on.
#   6. The offline cargo registry cache, so `cargo build --offline` never waits
#      on the registry during a reload.
#
#   Before any step runs, the script prints this plan with everything it may
#   install and asks once whether to include the VS Code debugging setup, so
#   the rest of the run needs no input.
#
#   Every step is idempotent: already-installed tools are detected and skipped,
#   so the script can be rerun after fixing whatever failed.

# USAGE: powershell -ExecutionPolicy Bypass -File devops\setup\setup_windows_environment.ps1
#          (no arguments)   Run every step; prompts once, at the start, about
#                           the optional VS Code debugging setup.

# EXAMPLE USAGE:
#   cd devops\setup
#   powershell ./setup_windows_environment.ps1

# --- SCRIPT ---

$ErrorActionPreference = "Stop"

# Invoke-WebRequest draws a progress bar on Windows PowerShell 5.1, which makes
# large downloads several times slower, so turn it off.
$ProgressPreference = "SilentlyContinue"

# Shared state for the [n/6] step marker printed by Write-Step and for the
# failure report in the top-level catch block.
$script:StepNumber = 0
$script:TotalSteps = 6
$script:CurrentStepTitle = 'startup'
$script:CodeExtensionCache = $null

# Prints a green "==> message" status line, matching the rest of devops/.
function Write-Info {
    param([Parameter(Mandatory = $true)][string] $Message)

    Write-Host "==> $Message" -ForegroundColor Green
}

# Prints an empty line and the "==> [n/6] Title" step marker, and remembers the
# step title so a later failure can be reported against it.
function Write-Step {
    param([Parameter(Mandatory = $true)][string] $Title)

    $script:StepNumber++
    $script:CurrentStepTitle = $Title
    Write-Host ""
    Write-Info "[$($script:StepNumber)/$($script:TotalSteps)] $Title"
}

# Prints an indented progress line under the current step.
function Write-Detail {
    param([Parameter(Mandatory = $true)][string] $Message)

    Write-Host "    $Message"
}

# Prints an indented "[ok] ..." line for a completed check or installation.
function Write-Success {
    param([Parameter(Mandatory = $true)][string] $Message)

    Write-Host "    [ok] $Message" -ForegroundColor Green
}

# Prints an indented "[warn] ..." line for a non-fatal problem the user still
# has to handle, for example a missing cdb.exe or Smart App Control being on.
function Write-Caution {
    param([Parameter(Mandatory = $true)][string] $Message)

    Write-Host "    [warn] $Message" -ForegroundColor Yellow
}

function Update-SessionPath {
    $machinePath = [Environment]::GetEnvironmentVariable('Path', 'Machine')
    $userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
    $env:PATH = "$machinePath;$userPath"
}

# Walks up from the script folder until it finds the directory whose modules
# folder holds Cargo.toml. This keeps the script working no matter which
# directory it is launched from, for example devops\setup.
function Find-RepositoryRoot {
    $candidate = if ($PSScriptRoot) { $PSScriptRoot } else { (Get-Location).ProviderPath }
    while (-not [string]::IsNullOrEmpty($candidate)) {
        if (Test-Path -LiteralPath (Join-Path $candidate 'modules\Cargo.toml')) {
            return $candidate
        }
        $parent = Split-Path -Parent $candidate
        if ($parent -eq $candidate) {
            break
        }
        $candidate = $parent
    }
    throw "Could not find the repository root above '$PSScriptRoot': no parent folder contains modules\Cargo.toml. Keep devops\setup inside the repository and rerun the script."
}

# Adds a directory to the current session PATH and persists it in the user PATH,
# skipping entries that are already there. Used for tool locations the
# installers do not always register themselves, such as .NET global tools.
function Add-UserPathEntry {
    param([Parameter(Mandatory = $true)][string] $Directory)

    $sessionEntries = @($env:PATH -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($sessionEntries -notcontains $Directory) {
        $env:PATH = "$Directory;$env:PATH"
    }

    $userEntries = @([Environment]::GetEnvironmentVariable('Path', 'User') -split ';' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
    if ($userEntries -notcontains $Directory) {
        [Environment]::SetEnvironmentVariable('Path', (@($Directory) + $userEntries) -join ';', 'User')
        Write-Detail "Added $Directory to the user PATH for future terminals."
    }
}

function Test-WingetPackage {
    param([Parameter(Mandatory = $true)][string] $Id)

    if (-not $script:WingetEnabled) {
        return $false
    }
    $result = & winget list --id $Id --exact --source winget --disable-interactivity 2>$null
    return ($LASTEXITCODE -eq 0 -and ($result -match [regex]::Escape($Id)))
}

function Install-WingetPackage {
    param(
        [Parameter(Mandatory = $true)][string] $Id,
        [string] $Override
    )

    if (Test-WingetPackage -Id $Id) {
        Write-Success "$Id is already installed."
        return
    }

    if (-not $script:WingetEnabled) {
        throw "Cannot install $Id because Windows Package Manager is disabled by Group Policy. Install it manually or ask your administrator to enable App Installer/Windows Package Manager, then rerun this script."
    }

    Write-Detail "Installing $Id..."
    $arguments = @(
        'install', '--id', $Id, '--exact', '--source', 'winget',
        '--accept-package-agreements', '--accept-source-agreements'
    )
    if ($Override) {
        $arguments += @('--override', $Override)
    }
    & winget @arguments
    if ($LASTEXITCODE -ne 0) {
        throw "winget failed to install $Id with exit code $LASTEXITCODE."
    }
    Update-SessionPath
    Write-Success "$Id installed."
}

function Test-CodeExtension {
    param([Parameter(Mandatory = $true)][string] $Id)

    if (-not (Get-Command code -ErrorAction SilentlyContinue)) {
        return $false
    }
    # Query the installed extensions once per run; the list costs about a
    # second per invocation and is checked for seven extensions.
    if ($null -eq $script:CodeExtensionCache) {
        $script:CodeExtensionCache = @(code --list-extensions 2>$null)
    }
    return ($script:CodeExtensionCache -contains $Id)
}

function Install-CodeExtension {
    param([Parameter(Mandatory = $true)][string] $Id)

    if (Test-CodeExtension -Id $Id) {
        Write-Success "VS Code extension $Id is already installed."
        return
    }

    Write-Detail "Installing VS Code extension $Id..."
    & code --install-extension $Id
    if ($LASTEXITCODE -ne 0) {
        throw "VS Code failed to install extension $Id with exit code $LASTEXITCODE."
    }
    $script:CodeExtensionCache += $Id
}

function Test-VisualStudioCppBuildTools {
    $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
    if (-not (Test-Path -LiteralPath $vswhere)) {
        return $false
    }

    $installationPath = & $vswhere -latest -products * `
        -requires Microsoft.VisualStudio.Workload.VCTools -property installationPath 2>$null
    return (-not [string]::IsNullOrWhiteSpace(($installationPath -join '')))
}

function Test-Cdb {
    $cdbCandidates = @(
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Debuggers\x64\cdb.exe'),
        (Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\Debuggers\x86\cdb.exe'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\Debuggers\x64\cdb.exe'),
        (Join-Path $env:ProgramFiles 'Windows Kits\10\Debuggers\x86\cdb.exe')
    )
    return ($cdbCandidates | Where-Object { Test-Path -LiteralPath $_ } | Measure-Object).Count -gt 0
}

function Install-VisualStudioCppBuildTools {
    $installer = Join-Path ([IO.Path]::GetTempPath()) 'vs_buildtools.exe'
    Write-Detail 'Downloading the official Visual Studio Build Tools installer...'
    Invoke-WebRequest -Uri 'https://aka.ms/vs/17/release/vs_buildtools.exe' -OutFile $installer
    try {
        Write-Detail 'Installing the C++ workload and the Windows 11 SDK (this can take a while)...'
        & $installer --wait --quiet `
            --add Microsoft.VisualStudio.Workload.VCTools `
            --add Microsoft.VisualStudio.Component.Windows11SDK.22621 `
            --includeRecommended
        $installerExitCode = $LASTEXITCODE
        if ($installerExitCode -eq 3010) {
            # 3010 means the install succeeded but a reboot is still pending.
            Write-Caution 'The Build Tools installer reports that a reboot is required to finish.'
        }
        elseif ($installerExitCode -ne 0) {
            throw "Visual Studio Build Tools installer failed with exit code $installerExitCode."
        }
    }
    finally {
        Remove-Item -LiteralPath $installer -Force -ErrorAction SilentlyContinue
    }
}

# The renderer-data crate cooks every .hlsl through `slangc` at build time,
# and the dev host's shader watcher re-cooks edits the same way. The release
# installed below is pinned here and in setup_linux_environment.sh (keep the
# two equal), from the official release zip.
$script:SlangVersion = '2025.21.2'

function Test-SlangCompiler {
    return [bool](Get-Command slangc.exe -ErrorAction SilentlyContinue)
}

function Install-SlangCompiler {
    $installDirectory = Join-Path $env:LOCALAPPDATA "Programs\slang\$($script:SlangVersion)"
    $slangcPath = Join-Path $installDirectory 'bin\slangc.exe'
    if (Test-Path -LiteralPath $slangcPath) {
        Add-UserPathEntry -Directory (Join-Path $installDirectory 'bin')
        Write-Success "Slang shader compiler $($script:SlangVersion) is already installed."
        return
    }

    $archive = Join-Path ([IO.Path]::GetTempPath()) "slang-$($script:SlangVersion).zip"
    Write-Detail "Downloading the Slang shader compiler $($script:SlangVersion)..."
    Invoke-WebRequest -Uri "https://github.com/shader-slang/slang/releases/download/v$($script:SlangVersion)/slang-$($script:SlangVersion)-windows-x86_64.zip" -OutFile $archive
    try {
        New-Item -ItemType Directory -Force -Path $installDirectory | Out-Null
        Expand-Archive -LiteralPath $archive -DestinationPath $installDirectory -Force
    }
    finally {
        Remove-Item -LiteralPath $archive -Force -ErrorAction SilentlyContinue
    }
    if (-not (Test-Path -LiteralPath $slangcPath)) {
        throw "The Slang archive did not contain bin\slangc.exe at the expected path '$slangcPath'."
    }
    Add-UserPathEntry -Directory (Join-Path $installDirectory 'bin')
    Write-Success "Slang shader compiler $($script:SlangVersion) installed."
}

function Test-DotNet8Sdk {
    if (-not (Get-Command dotnet -ErrorAction SilentlyContinue)) {
        return $false
    }
    $sdks = @(dotnet --list-sdks 2>$null)
    return ($sdks -match '^8\.')
}

function Install-VsCodeAndDotNetFallbacks {
    if (Get-Command code -ErrorAction SilentlyContinue) {
        Write-Success 'Visual Studio Code is already installed.'
    }
    else {
        $codeInstaller = Join-Path ([IO.Path]::GetTempPath()) 'VSCodeUserSetup.exe'
        Write-Detail 'Downloading the official VS Code installer because Winget installation is unavailable...'
        Invoke-WebRequest -Uri 'https://update.code.visualstudio.com/latest/win32-x64-user/stable' -OutFile $codeInstaller
        try {
            $codeProcess = Start-Process -FilePath $codeInstaller -ArgumentList '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART' -Wait -PassThru
            if ($codeProcess.ExitCode -ne 0) {
                throw "VS Code installer failed with exit code $($codeProcess.ExitCode)."
            }
        }
        finally {
            Remove-Item -LiteralPath $codeInstaller -Force -ErrorAction SilentlyContinue
        }
        Update-SessionPath
        Write-Success 'Visual Studio Code installed.'
    }

    if (Test-DotNet8Sdk) {
        Write-Success '.NET 8 SDK is already installed.'
    }
    else {
        $dotnetInstaller = Join-Path ([IO.Path]::GetTempPath()) 'dotnet-install.ps1'
        Write-Detail 'Downloading the official .NET SDK installer because Winget installation is unavailable...'
        Invoke-WebRequest -Uri 'https://dot.net/v1/dotnet-install.ps1' -OutFile $dotnetInstaller
        try {
            & $dotnetInstaller -Channel 8.0 -InstallDir (Join-Path $env:USERPROFILE '.dotnet')
            if ($LASTEXITCODE -ne 0) {
                throw "dotnet-install failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            Remove-Item -LiteralPath $dotnetInstaller -Force -ErrorAction SilentlyContinue
        }
        Add-UserPathEntry -Directory (Join-Path $env:USERPROFILE '.dotnet')
        Write-Success '.NET 8 SDK installed.'
    }
}

function Read-YesNo {
    param(
        [Parameter(Mandatory = $true)][string] $Prompt,
        [bool] $Default = $false
    )

    $suffix = if ($Default) { '[Y/n]' } else { '[y/N]' }
    $answer = Read-Host "$Prompt $suffix"
    if ([string]::IsNullOrWhiteSpace($answer)) {
        return $Default
    }
    return $answer.Trim().ToLowerInvariant() -in @('y', 'yes')
}

# Prints what each step does and what it may install, so the user knows the
# whole plan before the one question and before anything changes.
function Show-SetupOverview {
    Write-Host ''
    Write-Host 'Pill workspace environment setup (Windows)' -ForegroundColor Green
    Write-Host ''
    Write-Host 'This script prepares this machine to build and run the engine. It will:'
    Write-Host ''
    Write-Host '  1. Rust toolchain     Install rustup and the stable MSVC toolchain'
    Write-Host '                        (through winget, or rustup-init.exe when winget is blocked).'
    Write-Host '  2. C++ Build Tools    Install the Visual Studio Build Tools C++ workload and the'
    Write-Host '                        Windows 11 SDK, which the linker needs. Asks for elevation (UAC).'
    Write-Host '  3. Slang compiler     Install the Slang shader compiler, which cooks every .hlsl'
    Write-Host '                        the renderer builds.'
    Write-Host '  4. Smart App Control  Check that it is off. Nothing is installed: it can only be'
    Write-Host '                        turned off by hand in Windows Security, and the script says how.'
    Write-Host '  5. VS Code debugging  Optional. Install VS Code, the .NET 8 SDK, the dotnet-trace'
    Write-Host '                        tool and these extensions: rust-analyzer, C/C++, C#, C# Dev Kit,'
    Write-Host '                        Hex Editor, Even Better TOML and CodeLLDB.'
    Write-Host '  6. Cargo cache        Download every crate dependency once (cargo fetch), so offline'
    Write-Host '                        builds during hot reload never wait on the network.'
    Write-Host ''
    Write-Host 'Anything already installed is detected and skipped.'
    Write-Host ''
}

try {
    # --- Overview and the one question ---------------------------------------
    # Asked before any step runs, so the rest of the setup is unattended.
    Show-SetupOverview
    $installVsCodeDebugging = Read-YesNo -Prompt 'Include the optional VS Code debugging setup (step 5)?' -Default $true
    Write-Host ''

    # --- Preflight ---------------------------------------------------------
    $repoRoot = Find-RepositoryRoot
    Write-Info "repository root: $repoRoot"

    # Winget is used when available; enterprise policies sometimes block its
    # install operation while it still answers --version, which the per-step
    # fallbacks below handle.
    $script:WingetEnabled = $false
    if (Get-Command winget -ErrorAction SilentlyContinue) {
        & winget --version 2>$null | Out-Null
        $script:WingetEnabled = ($LASTEXITCODE -eq 0)
    }
    if ($script:WingetEnabled) {
        Write-Detail 'Windows Package Manager (winget) is available.'
    }
    else {
        Write-Caution 'Windows Package Manager is unavailable or disabled by Group Policy; direct installer fallbacks will be used instead.'
    }

    # --- 1. Rust toolchain --------------------------------------------------
    # Rustup installs the stable-x86_64-pc-windows-msvc toolchain by default.
    Write-Step 'Rust toolchain'
    $cargoExecutable = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe'
    if (Test-Path -LiteralPath $cargoExecutable) {
        Write-Success 'Rust is already installed.'
    }
    elseif ($script:WingetEnabled) {
        Install-WingetPackage -Id 'Rustlang.Rustup'
        Update-SessionPath
    }
    else {
        # Rustup's official installer is the fallback when an enterprise policy
        # disables Winget. It does not require administrator rights.
        $rustupInstaller = Join-Path ([IO.Path]::GetTempPath()) 'rustup-init.exe'
        Write-Detail 'Downloading the official Rust installer because Winget is blocked...'
        Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $rustupInstaller
        try {
            & $rustupInstaller -y --default-toolchain stable-x86_64-pc-windows-msvc
            if ($LASTEXITCODE -ne 0) {
                throw "rustup failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            Remove-Item -LiteralPath $rustupInstaller -Force -ErrorAction SilentlyContinue
        }
        Update-SessionPath
        Write-Success 'Rust toolchain installed.'
    }

    # Fail here with a readable message instead of failing in the cargo fetch
    # step with a bare "cargo not found".
    if (-not (Test-Path -LiteralPath $cargoExecutable)) {
        $cargoCommand = Get-Command cargo.exe -ErrorAction SilentlyContinue
        if ($cargoCommand) {
            $cargoExecutable = $cargoCommand.Source
        }
        else {
            throw "cargo.exe was not found at '$cargoExecutable' and is not on the PATH. Install Rust from https://rustup.rs and rerun this script."
        }
    }

    # --- 2. MSVC build tools + Windows SDK ----------------------------------
    # Needed to link even though the workspace uses rust-lld as the linker:
    # rust-lld still needs the MSVC and Windows SDK import libraries.
    Write-Step 'Visual Studio C++ Build Tools and Windows SDK'
    if (Test-VisualStudioCppBuildTools) {
        Write-Success 'C++ workload is already installed.'
    }
    else {
        # Use the official bootstrapper directly. It works even when an
        # enterprise policy permits querying Winget but blocks its install
        # operation.
        Install-VisualStudioCppBuildTools
        Write-Success 'Visual Studio C++ Build Tools and Windows SDK installed.'
    }

    # cdb.exe is distributed by the separate Windows SDK installer, not
    # necessarily by the Visual Studio Build Tools installer. It is needed only
    # by the manual stack-sampling check, so report it without stopping setup.
    if (Test-Cdb) {
        Write-Success 'Windows debugging tools (cdb.exe) are already installed.'
    }
    else {
        Write-Caution 'cdb.exe is not installed. Download the Windows SDK from https://developer.microsoft.com/en-us/windows/downloads/windows-sdk/ and select only Debugging Tools for Windows. This is required for stack_sample.ps1.'
    }

    # --- 3. Slang shader compiler -------------------------------------------
    # The renderer-data crate's build script cooks every .hlsl through
    # `slangc`; without it on PATH every renderer build fails, and the host's
    # shader watcher cannot re-cook edited shaders at runtime either.
    Write-Step 'Slang shader compiler'
    if (Test-SlangCompiler) {
        Write-Success 'Slang shader compiler is already installed.'
    }
    else {
        Install-SlangCompiler
    }

    # --- 4. Smart App Control -----------------------------------------------
    # SAC blocks LoadLibrary of the DLLs this workspace compiles on the fly for
    # hot-reloaded modules and projects ("An Application Control policy has
    # blocked this file", os error 4551). It can only be turned off through the
    # Windows Security UI while it is still in evaluation mode: it cannot be
    # scripted (the registry key is protected even from an elevated process),
    # and once fully enforced it can only be turned back off by reinstalling
    # Windows.
    Write-Step 'Windows Smart App Control'
    $sacState = (Get-ItemProperty -Path "HKLM:\SYSTEM\CurrentControlSet\Control\CI\Policy" -Name "VerifiedAndReputablePolicyState" -ErrorAction SilentlyContinue).VerifiedAndReputablePolicyState
    if ($sacState -ne 0) {
        Write-Caution 'Smart App Control is still ON. Go to Settings > Privacy & security > Windows Security > App & browser control > Smart App Control settings and turn it Off, then rerun this script.'
        Write-Caution 'If the Off option is greyed out, SAC is fully enforced and can only be disabled by reinstalling Windows.'
    }
    else {
        Write-Success 'Smart App Control is off.'
    }

    # --- 5. Optional VS Code debugging setup --------------------------------
    Write-Step 'VS Code debugging setup (optional)'
    if (-not $installVsCodeDebugging) {
        Write-Detail 'Skipping the VS Code debugging setup, as chosen at the start.'
    }
    else {
        # VS Code's cppvsdbg provider is required by the native launch profiles;
        # coreclr is supplied by the C# extension. CodeLLDB supports the
        # alternate launch profile and the remaining extensions support
        # editing and inspection.
        if ($script:WingetEnabled) {
            try {
                Install-WingetPackage -Id 'Microsoft.VisualStudioCode'
                Install-WingetPackage -Id 'Microsoft.DotNet.SDK.8'
            }
            catch {
                Write-Caution "Winget installation was blocked: $($_.Exception.Message)"
                Install-VsCodeAndDotNetFallbacks
            }
        }
        else {
            Install-VsCodeAndDotNetFallbacks
        }

        $debugExtensions = @(
            'rust-lang.rust-analyzer',
            'ms-vscode.cpptools',
            'ms-dotnettools.csharp',
            'ms-dotnettools.csdevkit',
            'ms-vscode.hexeditor',
            'tamasfe.even-better-toml',
            'vadimcn.vscode-lldb'
        )
        if (-not (Get-Command code -ErrorAction SilentlyContinue)) {
            Write-Caution 'The VS Code "code" command is not on PATH, so extensions cannot be installed automatically. Open VS Code once and run "Shell Command: Install code command in PATH" from the Command Palette, then rerun this script.'
        }
        else {
            foreach ($extensionId in $debugExtensions) {
                Install-CodeExtension -Id $extensionId
            }
        }

        # "dotnet tool install" fails when the tool already exists, so check
        # the global tool list instead of retrying the install.
        $dotnetToolsDirectory = Join-Path $env:USERPROFILE '.dotnet\tools'
        $globalTools = @(dotnet tool list --global 2>$null)
        if ($globalTools -match 'dotnet-trace') {
            Write-Success 'dotnet-trace is already installed.'
        }
        else {
            Write-Detail 'Installing dotnet-trace...'
            & dotnet tool install --global dotnet-trace
            if ($LASTEXITCODE -ne 0) {
                throw "dotnet-trace installation failed with exit code $LASTEXITCODE."
            }
            Write-Success 'dotnet-trace installed.'
        }

        # The dotnet CLI does not always update PATH itself, so persist the
        # tools directory for future terminals.
        Add-UserPathEntry -Directory $dotnetToolsDirectory
    }

    # --- 6. Offline cargo registry -------------------------------------------
    # The --offline run needs every dependency downloaded once; this also
    # fetches the pinned `trait_type_map` git dependency.
    Write-Step 'Cargo dependency cache'
    $modulesDirectory = Join-Path $repoRoot 'modules'
    Update-SessionPath
    Push-Location -LiteralPath $modulesDirectory
    try {
        Write-Detail 'Fetching crate and git dependencies (online, one-time)...'
        & $cargoExecutable fetch
        if ($LASTEXITCODE -ne 0) {
            throw "cargo fetch failed with exit code $LASTEXITCODE."
        }
        Write-Success 'Cargo dependency cache populated.'
    }
    finally {
        Pop-Location
    }

    Write-Host ""
    Write-Host "Setup complete!" -ForegroundColor Green
}
catch {
    Write-Host ""
    if ($script:StepNumber -gt 0) {
        Write-Host "[error] Setup failed in step $($script:StepNumber)/$($script:TotalSteps) ($($script:CurrentStepTitle)):" -ForegroundColor Red
    }
    else {
        Write-Host "[error] Setup failed during the startup checks:" -ForegroundColor Red
    }
    Write-Host "  $($_.Exception.Message) (script line $($_.InvocationInfo.ScriptLineNumber))" -ForegroundColor Red
    Write-Host ""
    Write-Host 'Fix the problem above, then rerun this script; finished steps are detected and skipped.'
    exit 1
}
