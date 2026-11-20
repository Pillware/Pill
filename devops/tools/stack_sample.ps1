<#
.SYNOPSIS
    Capture a repeatable hang snapshot for a Pill process.

.DESCRIPTION
    Uses the Windows debugger and dbghelp-backed stack walking. It records all
    native thread stacks with symbols from normal and hot-reload build output.
    It never terminates or resumes the target process.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)] [int] $ProcessId,
    [string] $Output = "stack-sample-$ProcessId.txt",
    [string] $SymbolPath = "modules/target/debug;modules/target/hot/build/debug"
)

$process = Get-Process -Id $ProcessId -ErrorAction Stop
$cdb = Get-Command cdb.exe -ErrorAction SilentlyContinue
if ($null -eq $cdb) {
    throw "cdb.exe was not found. Install Debugging Tools for Windows and add it to PATH."
}

$root = (Resolve-Path (Join-Path $PSScriptRoot '../..')).Path
$resolvedSymbols = ($SymbolPath -split ';' | ForEach-Object {
    $candidate = Join-Path $root $_
    if (Test-Path -LiteralPath $candidate) { (Resolve-Path -LiteralPath $candidate).Path }
}) -join ';'

$commands = @(
    ".sympath+ $resolvedSymbols",
    ".reload /f",
    "!runaway",
    "~* k 40",
    "q"
) -join ';'

& $cdb.Source -p $ProcessId -lines -logo $Output -c $commands
if ($LASTEXITCODE -ne 0) {
    throw "cdb failed with exit code $LASTEXITCODE"
}
Write-Host "Stack sample written to $((Resolve-Path -LiteralPath $Output).Path)"
