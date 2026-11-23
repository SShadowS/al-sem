# TLS-117 W10 (design note docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md,
# section 8): does the shipped build profile make a .data-only difference visible to the
# canonical hash (through the TimeDateStamp / debug directory), and do two links of the same
# source never canonicalise equal while their writable sections differ?
#
# Run from the repository root, in PowerShell, with TREE_SITTER_AL_PATH set:
#   powershell -ExecutionPolicy Bypass -File examples\tls117_w10.ps1
# It builds the probe three times with `--profile release-ship` (full LTO: expect several minutes
# each), copies each build to <target>\tls117-w10\, and prints one PASS/FAIL line per check.
# Exit code 0 only when both checks pass.

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root
$target = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $root 'target' }
$exe = Join-Path $target 'release-ship\examples\tls117_probe.exe'
$out = Join-Path $target 'tls117-w10'
New-Item -ItemType Directory -Force $out | Out-Null

function Build([string[]] $extra) {
    & cargo build --profile release-ship --example tls117_probe @extra
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed: $extra" }
}

# 1. The base build.
Build @()
Copy-Item $exe (Join-Path $out 'base.exe') -Force
# 2. The same source with one static's initial value changed (feature w10-variant).
Build @('--features', 'w10-variant')
Copy-Item $exe (Join-Path $out 'variant.exe') -Force
# 3. The base source again, relinked: touching the probe's source forces a rebuild and link.
(Get-Item 'examples\tls117_probe.rs').LastWriteTime = Get-Date
Build @()
Copy-Item $exe (Join-Path $out 'relink.exe') -Force

$probe = Join-Path $out 'base.exe'
& $probe w10-compare variant (Join-Path $out 'base.exe') (Join-Path $out 'variant.exe')
$a = $LASTEXITCODE
& $probe w10-compare relink (Join-Path $out 'base.exe') (Join-Path $out 'relink.exe')
$b = $LASTEXITCODE
if ($a -ne 0 -or $b -ne 0) { exit 1 }
exit 0
