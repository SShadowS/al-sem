# install-windows.ps1 - build, stamp and install al-sem's Windows binaries (TLS-117).
#
# The IR cache (ENG-110) has a key on Windows only for a STAMPED binary: `alsem-stamp` writes the
# digest of the binary's writable sections and masked bytes into its data-digest slot after
# linking. Every plain `cargo build` binary is unstamped and gets no key (the cache is simply off
# for it). This script is the owner's local install path:
#   1. builds alsem, aldump, al-call-hierarchy and alsem-stamp with --profile release-ship;
#   2. copies the three engine binaries to a staging folder and stamps each there;
#   3. verifies each stamped file (the slot matches, the IR marker is present);
#   4. copies them to ~\.al-sem\bin\ (alsem.exe, aldump.exe) and to every -LspDir given
#      (al-call-hierarchy.exe and alsem.exe), refusing to overwrite a running binary.
#
# Run from the repository root, in Windows PowerShell 5.1 or pwsh 7, with the Windows build
# settings (CLAUDE.md, "Windows build"): TREE_SITTER_AL_PATH at the commit CLAUDE.md names,
# CMAKE_GENERATOR = "Visual Studio 17 2022", and a short CARGO_TARGET_DIR (e.g. C:/atw).
#
#   powershell -ExecutionPolicy Bypass -File scripts\install-windows.ps1 [-LspDir <dir>]... [-NoBuild]
#
# Exit code 0 only when every binary was built, stamped, verified and copied.

param(
    [string[]] $LspDir = @(),
    [switch] $NoBuild
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$out = Join-Path $target 'release-ship'
$stage = Join-Path $target 'install-stage'
$engines = @('alsem', 'aldump', 'al-call-hierarchy')

# SHA-256 through .NET (Get-FileHash is not always loadable in Windows PowerShell 5.1).
function FileSha256([string] $path) {
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        return [System.BitConverter]::ToString($sha.ComputeHash([System.IO.File]::ReadAllBytes($path))).Replace('-', '')
    } finally {
        $sha.Dispose()
    }
}

# The processes running $dst, if any.
function RunningAt([string] $dst) {
    if (-not (Test-Path $dst)) { return @() }
    $full = (Resolve-Path $dst).Path
    return @(Get-Process -ErrorAction SilentlyContinue |
        Where-Object { try { $_.Path -eq $full } catch { $false } })
}

# Copy $src over $dst and check the copy (running targets were refused before any copy).
function CopyChecked([string] $src, [string] $dst) {
    New-Item -ItemType Directory -Force (Split-Path -Parent $dst) | Out-Null
    Copy-Item $src $dst -Force
    if ((FileSha256 $src) -ne (FileSha256 $dst)) { throw "copy of $src to $dst differs" }
    Write-Output "installed $dst"
}

# Run a native command with $ErrorActionPreference = 'Continue': Windows PowerShell 5.1 turns
# a native command's stderr lines (cargo's progress) into errors that 'Stop' throws on. The
# exit code decides.
function Native([string] $what, [scriptblock] $cmd) {
    $saved = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try { & $cmd 2>&1 | ForEach-Object { "$_" } } finally { $ErrorActionPreference = $saved }
    if ($LASTEXITCODE -ne 0) { throw "$what failed (exit $LASTEXITCODE)" }
}

# Every target as (staged source name, destination).
$bin = Join-Path $HOME '.al-sem\bin'
$targets = @(@('alsem.exe', (Join-Path $bin 'alsem.exe')), @('aldump.exe', (Join-Path $bin 'aldump.exe')))
foreach ($d in $LspDir) {
    $targets += , @('al-call-hierarchy.exe', (Join-Path $d 'al-call-hierarchy.exe'))
    $targets += , @('alsem.exe', (Join-Path $d 'alsem.exe'))
}

# Refuse before building or copying anything when a target is running: a refusal midway would
# leave some targets new and some old (sol r1).
function RefuseRunning {
    $busy = @()
    foreach ($t in $targets) {
        $p = RunningAt $t[1]
        if ($p.Count -gt 0) { $busy += "$($t[1]) (pid $($p.Id -join ', '))" }
    }
    if ($busy.Count -gt 0) { throw "refusing to install: running $($busy -join '; '): stop them first" }
}
RefuseRunning

if (-not $NoBuild) {
    $bins = @()
    foreach ($b in $engines + 'alsem-stamp') { $bins += @('--bin', $b) }
    Native 'cargo build' { cargo build --profile release-ship @bins }
}

$stamp = Join-Path $out 'alsem-stamp.exe'
New-Item -ItemType Directory -Force $stage | Out-Null
foreach ($b in $engines) {
    $built = Join-Path $out "$b.exe"
    $staged = Join-Path $stage "$b.exe"
    Copy-Item $built $staged -Force
    Native "stamping $b.exe" { & $stamp stamp $staged }
    Native "verifying $b.exe" { & $stamp verify $staged }
}

# Again just before copying: the build takes minutes, and a target may have started since.
RefuseRunning
foreach ($t in $targets) {
    CopyChecked (Join-Path $stage $t[0]) $t[1]
}
Write-Output 'install-windows: done (stamped and verified)'
exit 0
