# TLS-117 W10, a diagnostic (design note docs/superpowers/notes/2026-11-21-tls117-windows-image-key.md,
# section 8): with the data-digest slot mandatory, it shows that stamped slots differ exactly
# when writable sections differ, reports whether the shipped profile also makes a .data-only
# difference visible to the canonical hash (TimeDateStamp / debug directory), and checks that
# two links of the same source never canonicalise equal while their writable sections differ.
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
$before = Get-Item $exe
$beforeTime = $before.LastWriteTimeUtc
$beforeHash = (Get-FileHash $exe -Algorithm SHA256).Hash
Start-Sleep -Seconds 2
(Get-Item 'examples\tls117_probe.rs').LastWriteTime = Get-Date
Build @()
Copy-Item $exe (Join-Path $out 'relink.exe') -Force
# The linker must have run: the output's modification time moved. Its hash is reported; an
# unchanged hash with a moved time means the link is reproducible.
$afterTime = (Get-Item $exe).LastWriteTimeUtc
$afterHash = (Get-FileHash $exe -Algorithm SHA256).Hash
$linked = $afterTime -gt $beforeTime
Write-Output ("W10 relink: linker ran: {0} (mtime {1:o} -> {2:o}); output hash changed: {3}" -f `
    $linked, $beforeTime, $afterTime, ($afterHash -ne $beforeHash))
if (-not $linked) { Write-Output 'FAIL W10 relink: the touch did not relink the probe'; exit 1 }

$probe = Join-Path $out 'base.exe'
& $probe w10-compare variant (Join-Path $out 'base.exe') (Join-Path $out 'variant.exe')
$a = $LASTEXITCODE
& $probe w10-compare relink (Join-Path $out 'base.exe') (Join-Path $out 'relink.exe')
$b = $LASTEXITCODE
if ($a -ne 0 -or $b -ne 0) { exit 1 }
exit 0
