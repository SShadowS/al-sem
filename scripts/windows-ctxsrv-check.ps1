<#
scripts/windows-ctxsrv-check.ps1 - the context server (alsem mcp / engine-host) on
WINDOWS, end to end on the pinned CDO baseline (design
docs/superpowers/specs/2026-11-01-context-server-design.md, section 13 "Windows
measurements"). Everything in the context server was built and measured on Linux;
this is the run that checks it on the owner's machine.

Run it from the repo root (U:/Git/al-call-hierarchy), on the branch to check, in
PowerShell 7 (pwsh) or Windows PowerShell 5.1:

  pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ctxsrv-check.ps1
  pwsh -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ctxsrv-check.ps1 `
       -Workspace U:/Git/DO-cdo-baseline/Cloud -OutDir target/windows-ctxsrv-check

From Git Bash: powershell.exe -NoProfile -ExecutionPolicy Bypass -File scripts/windows-ctxsrv-check.ps1

What it does, in order, each step logged into ONE folder (default
target/windows-ctxsrv-check/) with a summary.txt:

  1. Builds alsem with the release-fast profile (build.log).
  2. Runs the test subsets that touch the platform code, in the debug profile (the
     fault hooks need it): the lib tests of engine::host, engine::resident,
     engine::session, mcp, app_image and release_alloc, and the engine_host and
     mcp_server members of the cli suite (tests-*.log). -SkipTests skips this.
  3. An MCP transcript against `alsem mcp --workspace <baseline>`: initialize,
     tools/list, query (cold, then warm), query with maxInlineBytes 16 (a stdout
     over the inline cap), explain twice, prove, digest as JSON (the large one)
     and as text, status, refresh. Per call: wall ms, the answer's kind and
     outcome, how the stdout was delivered (inline / file / refused) and its size,
     and the engine child's working set and peak working set after it
     (transcript.tsv, calls/*.json). The query, explain, prove and digest answers
     are compared with a cold `alsem` CLI run of the same command (cli/*); a
     difference is a failure. -SkipCliCompare skips the CLI runs.
  4. The manifest check (the per-request freshness check, design target <= 10 ms,
     1.2-1.7 ms on Linux): a second server with ALSEM_TRACE=1 answers one cold and
     six warm queries; the child's `manifest.observe` spans after its build are the
     checks (manifest.txt, trace/).
  5. Confirms the baseline is unchanged (git status before and after) and that the
     server wrote no file into its output folder (on Windows answers are
     inline-only: design section 10, fail closed).

Other parameters: -Routine (default IssueReminder, a CDO codeunit routine),
-AlsemBinary <path> (use that binary, no build), -SkipBuild, -CallTimeoutSec.

Exit code: 0 when every step passed; 1 when any failed (summary.txt lists them);
2 on a usage error. A manifest check over its 10 ms target is reported as WARN, not
a failure. It never writes into the baseline. Send back the whole output folder.
#>
[CmdletBinding()]
param(
    [string]$Workspace = 'U:/Git/DO-cdo-baseline/Cloud',
    [string]$OutDir = 'target/windows-ctxsrv-check',
    [string]$Routine = 'IssueReminder',
    [int]$CallTimeoutSec = 900,
    [string]$AlsemBinary = '',
    [switch]$SkipTests,
    [switch]$SkipBuild,
    [switch]$SkipCliCompare
)

$ErrorActionPreference = 'Continue'

if (-not (Test-Path -LiteralPath $Workspace)) {
    Write-Error "windows-ctxsrv-check: workspace path does not exist: $Workspace"
    exit 2
}
if (-not (Test-Path -LiteralPath 'Cargo.toml')) {
    Write-Error 'windows-ctxsrv-check: run it from the repo root (no Cargo.toml here)'
    exit 2
}
$Workspace = (Resolve-Path -LiteralPath $Workspace).Path
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$OutDir = (Resolve-Path -LiteralPath $OutDir).Path
foreach ($d in 'calls', 'cli', 'trace') {
    New-Item -ItemType Directory -Force -Path (Join-Path $OutDir $d) | Out-Null
}
$summary = Join-Path $OutDir 'summary.txt'
Set-Content -LiteralPath $summary -Value '' -Encoding utf8
$script:failures = New-Object System.Collections.Generic.List[string]
$script:warnings = New-Object System.Collections.Generic.List[string]

function Say([string]$text) {
    Write-Host $text
    Add-Content -LiteralPath $summary -Value $text -Encoding utf8
}
function Fail([string]$text) {
    $script:failures.Add($text)
    Say "FAIL: $text"
}
function Warn([string]$text) {
    $script:warnings.Add($text)
    Say "WARN: $text"
}

# A program run to completion: stdout and stderr to files, judged by its exit code
# (never through a pipe).
function Invoke-Logged([string]$exe, [string[]]$arguments, [string]$log) {
    $p = Start-Process -FilePath $exe -ArgumentList $arguments -NoNewWindow -Wait -PassThru `
        -RedirectStandardOutput $log -RedirectStandardError "$log.stderr"
    return $p.ExitCode
}

# A program run to completion with its stdout and stderr captured as UTF-8 text.
function Invoke-Captured([string]$exe, [string[]]$arguments) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $exe
    $psi.Arguments = ($arguments | ForEach-Object { Quote-Arg $_ }) -join ' '
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    $p = [System.Diagnostics.Process]::Start($psi)
    $err = $p.StandardError.ReadToEndAsync()
    $out = $p.StandardOutput.ReadToEnd()
    $p.WaitForExit()
    return [pscustomobject]@{ Stdout = $out; Stderr = $err.Result; Exit = $p.ExitCode }
}

function Quote-Arg([string]$a) {
    if ($a -notmatch '[\s"]') { return $a }
    return '"' + ($a -replace '(\\*)"', '$1$1\"' -replace '(\\+)$', '$1$1') + '"'
}

function Git-Status([string]$dir) {
    $r = Invoke-Captured 'git' @('-C', $dir, 'status', '--porcelain')
    return "$($r.Exit)`n$($r.Stdout)"
}

# ---------------------------------------------------------------------------
# One `alsem mcp` server and its MCP client end (NDJSON JSON-RPC 2.0 over stdio).
# ---------------------------------------------------------------------------
function Start-Mcp([string]$alsem, [string]$outputRoot, [hashtable]$vars, [string]$cwd) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $alsem
    $psi.Arguments = "mcp --workspace $(Quote-Arg $Workspace) --output-root $(Quote-Arg $outputRoot)"
    $psi.UseShellExecute = $false
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.StandardOutputEncoding = [System.Text.UTF8Encoding]::new($false)
    $psi.StandardErrorEncoding = [System.Text.UTF8Encoding]::new($false)
    if ($cwd) { $psi.WorkingDirectory = $cwd }
    # An empty value removes the variable (the child inherits the rest).
    foreach ($k in $vars.Keys) {
        if ($vars[$k] -eq '') { $psi.EnvironmentVariables.Remove($k) }
        else { $psi.EnvironmentVariables[$k] = $vars[$k] }
    }
    $p = [System.Diagnostics.Process]::Start($psi)
    return [pscustomobject]@{
        Proc   = $p
        Stdin  = $p.StandardInput.BaseStream
        Stderr = $p.StandardError.ReadToEndAsync()
        NextId = 1
    }
}

function Send-Line($srv, $obj) {
    $json = $obj | ConvertTo-Json -Compress -Depth 20
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json + "`n")
    $srv.Stdin.Write($bytes, 0, $bytes.Length)
    $srv.Stdin.Flush()
}

# The response to request `id`: (raw line, parsed message). Notifications read on
# the way are skipped.
function Read-Response($srv, [int]$id, [int]$timeoutSec) {
    $deadline = [DateTime]::UtcNow.AddSeconds($timeoutSec)
    while ($true) {
        $left = [int]([Math]::Max(1, ($deadline - [DateTime]::UtcNow).TotalMilliseconds))
        $task = $srv.Proc.StandardOutput.ReadLineAsync()
        if (-not $task.Wait($left)) { throw "no response to request $id within $timeoutSec s" }
        $line = $task.Result
        if ($null -eq $line) { throw "the server closed its output before answering request $id" }
        if ($line.Trim() -eq '') { continue }
        $msg = $line | ConvertFrom-Json
        if (($msg.PSObject.Properties.Name -contains 'id') -and $msg.id -eq $id) {
            return @($line, $msg)
        }
    }
}

function Request($srv, [string]$method, $params) {
    $id = $srv.NextId
    $srv.NextId++
    Send-Line $srv @{ jsonrpc = '2.0'; id = $id; method = $method; params = $params }
    return Read-Response $srv $id $CallTimeoutSec
}

function Stop-Mcp($srv, [string]$stderrLog) {
    try { $srv.Stdin.Close() } catch { }
    if (-not $srv.Proc.WaitForExit(60000)) {
        Warn "the server did not exit within 60 s of its input closing; killed"
        try { $srv.Proc.Kill() } catch { }
    }
    $srv.Proc.WaitForExit()
    Set-Content -LiteralPath $stderrLog -Value $srv.Stderr.Result -Encoding utf8
    return $srv.Proc.ExitCode
}

function Prop($o, [string]$name) {
    if ($null -eq $o) { return $null }
    if ($o.PSObject.Properties.Name -contains $name) { return $o.$name }
    return $null
}

function Child-Memory([int]$childPid) {
    if ($childPid -le 0) { return @(0, 0, 0) }
    $p = Get-Process -Id $childPid -ErrorAction SilentlyContinue
    if ($null -eq $p) { return @(0, 0, 0) }
    $p.Refresh()
    return @($p.WorkingSet64, $p.PeakWorkingSet64, $p.PrivateMemorySize64)
}

function MiB([long]$b) { return [Math]::Round($b / 1MB, 1) }

# ---------------------------------------------------------------------------
Say "windows-ctxsrv-check: al-sem $(git rev-parse --short HEAD) on $(git rev-parse --abbrev-ref HEAD)"
Say "windows-ctxsrv-check: workspace $Workspace ($((Invoke-Captured 'git' @('-C', $Workspace, 'rev-parse', '--short', 'HEAD')).Stdout.Trim()))"
Say "windows-ctxsrv-check: $([System.Environment]::OSVersion.VersionString); PowerShell $($PSVersionTable.PSVersion); $([System.Environment]::ProcessorCount) logical CPUs"
Say "windows-ctxsrv-check: routine $Routine; output $OutDir"
if (git status --porcelain --untracked-files=no) {
    Warn 'the al-sem tree has uncommitted changes'
}
if (-not $env:TREE_SITTER_AL_PATH -and -not (Test-Path -LiteralPath 'tree-sitter-al')) {
    Warn 'no tree-sitter-al clone here and TREE_SITTER_AL_PATH is unset: the build will stop'
}
$baselineBefore = Git-Status $Workspace

# 1. Build.
$targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { 'target' }
$alsem = Join-Path (Join-Path $targetDir 'release-fast') 'alsem.exe'
if (-not (Test-Path -LiteralPath $alsem) -and (Test-Path -LiteralPath ($alsem -replace '\.exe$', ''))) {
    $alsem = $alsem -replace '\.exe$', ''  # a non-Windows dry run
}
if ($AlsemBinary) {
    $alsem = $AlsemBinary
    $SkipBuild = $true
}
if (-not $SkipBuild) {
    Say 'windows-ctxsrv-check: [1/5] building alsem (release-fast)...'
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $rc = Invoke-Logged 'cargo' @('build', '--profile', 'release-fast', '--bin', 'alsem') (Join-Path $OutDir 'build.log')
    Say "windows-ctxsrv-check: build exit $rc in $([int]$sw.Elapsed.TotalSeconds) s"
    if ($rc -ne 0) {
        Fail 'build failed, see build.log.stderr'
        exit 1
    }
}
if (-not (Test-Path -LiteralPath $alsem)) {
    Fail "no binary at $alsem"
    exit 1
}
$alsem = (Resolve-Path -LiteralPath $alsem).Path

# 2. The platform-touching test subsets (debug profile: the fault hooks).
if (-not $SkipTests) {
    Say 'windows-ctxsrv-check: [2/5] running the test subsets (debug profile)...'
    $suites = @(
        @{ Name = 'lib'; Args = @('test', '-p', 'al-sem', '--lib', '--', 'engine::host', 'engine::resident', 'engine::session', 'mcp::', 'app_image', 'release_alloc') },
        @{ Name = 'cli'; Args = @('test', '-p', 'al-sem', '--test', 'cli', '--', 'engine_host::', 'mcp_server::') }
    )
    foreach ($s in $suites) {
        $log = Join-Path $OutDir "tests-$($s.Name).log"
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $rc = Invoke-Logged 'cargo' $s.Args $log
        $results = Select-String -LiteralPath $log -Pattern '^test result:' | ForEach-Object { $_.Line }
        $failed = Select-String -LiteralPath $log -Pattern '^test .* FAILED$' | ForEach-Object { $_.Line }
        Say "windows-ctxsrv-check: tests $($s.Name): exit $rc in $([int]$sw.Elapsed.TotalSeconds) s; $($results -join '; ')"
        foreach ($f in $failed) { Say "    $f" }
        if ($rc -ne 0) { Fail "tests $($s.Name) failed (exit $rc), see tests-$($s.Name).log(.stderr)" }
    }
}

# 3. The MCP transcript.
Say 'windows-ctxsrv-check: [3/5] MCP transcript on the baseline...'
$mcpOut = Join-Path $OutDir 'mcp-out'
New-Item -ItemType Directory -Force -Path $mcpOut | Out-Null
$tsv = Join-Path $OutDir 'transcript.tsv'
Set-Content -LiteralPath $tsv -Value "step`tmethod`twall_ms`tkind`toutcome`tdelivery`tstdout_bytes`tchild_ws_mib`tchild_peak_ws_mib`tchild_private_mib`tengine_compute_ms" -Encoding utf8
$script:childPid = 0
$script:answers = @{}

function Record([string]$step, [string]$method, [long]$ms, [string]$line, $msg) {
    Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'calls') "$step.json") -Value $line -Encoding utf8
    $result = Prop $msg 'result'
    $answer = Prop $result 'structuredContent'
    $kind = Prop $answer 'kind'
    $outcome = Prop $answer 'outcome'
    $delivery = ''
    $bytes = ''
    $text = Prop $answer 'stdoutText'
    $file = Prop $answer 'stdoutFile'
    if ($null -ne $text) {
        $delivery = 'inline'
        $bytes = [System.Text.Encoding]::UTF8.GetByteCount([string]$text)
    } elseif ($null -ne $file) {
        $delivery = "file $($file.path)"
        $bytes = $file.bytes
    } elseif ($kind -eq 'failed') {
        $delivery = 'refused'
    }
    if ($null -ne (Prop $msg 'error')) {
        $kind = 'rpc-error'
        $outcome = $msg.error.message
    }
    $compute = Prop (Prop $answer 'timing') 'computeMs'
    $mem = Child-Memory $script:childPid
    Add-Content -LiteralPath $tsv -Encoding utf8 -Value (@(
            $step, $method, $ms, $kind, $outcome, $delivery, $bytes,
            (MiB $mem[0]), (MiB $mem[1]), (MiB $mem[2]), $compute) -join "`t")
    Say ("  {0,-16} {1,8} ms  {2,-7} {3,-12} {4} {5}  child WS {6} MiB (peak {7} MiB)" -f `
            $step, $ms, $kind, $outcome, $delivery, $bytes, (MiB $mem[0]), (MiB $mem[1]))
    return $answer
}

function Call-Tool($srv, [string]$step, [string]$tool, $arguments) {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    try {
        $r = Request $srv 'tools/call' @{ name = $tool; arguments = $arguments }
    } catch {
        Fail "$step ($tool): $_"
        return $null
    }
    $ms = $sw.ElapsedMilliseconds
    return Record $step 'tools/call' $ms $r[0] $r[1]
}

$serverEnv = @{ 'ALSEM_ENGINE_TEST_HOOK' = '' }
$srv = Start-Mcp $alsem $mcpOut $serverEnv $null
try {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = Request $srv 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'windows-ctxsrv-check'; version = '1' } }
    $ms = $sw.ElapsedMilliseconds
    Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'calls') 'initialize.json') -Value $r[0] -Encoding utf8
    Say "  initialize       $ms ms  protocol $($r[1].result.protocolVersion)"
    if ($r[1].result.protocolVersion -ne '2025-06-18') { Fail "initialize: protocol $($r[1].result.protocolVersion)" }
    Send-Line $srv @{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = @{} }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $r = Request $srv 'tools/list' @{}
    $names = @($r[1].result.tools | ForEach-Object { $_.name })
    Say "  tools/list       $($sw.ElapsedMilliseconds) ms  $($names -join ', ')"
    foreach ($t in 'alsem_query_effects', 'alsem_query_touches', 'alsem_explain', 'alsem_prove', 'alsem_digest', 'alsem_status', 'alsem_refresh') {
        if ($names -notcontains $t) { Fail "tools/list lacks $t" }
    }

    $q = @{ routine = $Routine; deterministic = $true }
    $script:answers['query'] = Call-Tool $srv 'query-cold' 'alsem_query_effects' $q
    # The child's pid, for its memory, once it runs.
    $st = Request $srv 'tools/call' @{ name = 'alsem_status'; arguments = @{} }
    $eng = @($st[1].result.structuredContent.engines)
    if ($eng.Count -ge 1 -and $null -ne (Prop $eng[0] 'engine')) {
        $script:childPid = [int]$eng[0].engine.pid
        Say "  engine child pid $script:childPid"
    } else {
        Fail 'status names no running engine child'
    }
    Call-Tool $srv 'query-warm' 'alsem_query_effects' $q | Out-Null
    Call-Tool $srv 'query-warm-2' 'alsem_query_effects' $q | Out-Null
    $over = Call-Tool $srv 'query-over-cap' 'alsem_query_effects' @{ routine = $Routine; deterministic = $true; maxInlineBytes = 16 }
    $script:answers['explain'] = Call-Tool $srv 'explain' 'alsem_explain' @{ routines = @($Routine); format = 'json' }
    Call-Tool $srv 'explain-warm' 'alsem_explain' @{ routines = @($Routine); format = 'json' } | Out-Null
    $script:answers['prove'] = Call-Tool $srv 'prove' 'alsem_prove' @{ routine = $Routine; question = 'may-commit'; deterministic = $true }
    $script:answers['digest-json'] = Call-Tool $srv 'digest-json' 'alsem_digest' @{ routines = @($Routine); deterministic = $true; format = 'json' }
    $script:answers['digest'] = Call-Tool $srv 'digest-human' 'alsem_digest' @{ routines = @($Routine); deterministic = $true; format = 'human' }
    $outEnv = Call-Tool $srv 'out-refused' 'alsem_prove' @{ routine = $Routine; question = 'may-commit'; out = 'answers/prove.json' }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $st = Request $srv 'tools/call' @{ name = 'alsem_status'; arguments = @{} }
    Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'calls') 'status.json') -Value $st[0] -Encoding utf8
    $sc = $st[1].result.structuredContent
    $e0 = @($sc.engines)[0]
    $memStatus = Prop (Prop $e0 'child') 'memory'
    Say "  status           $($sw.ElapsedMilliseconds) ms  spawns $($e0.spawns) deaths $($e0.deaths) completed $($e0.completed)"
    if ($null -ne $memStatus) {
        Say "  child memory status: $($memStatus | ConvertTo-Json -Compress -Depth 5)"
        if ((Prop $memStatus 'rssBytes') -eq 0) { Fail 'the child reports rssBytes 0: its Windows meter reads nothing' }
    }
    if ([int]$e0.deaths -ne 0) { Fail "the engine child died $($e0.deaths) time(s); stderr tail: $($e0.stderrTail)" }
    if ([int]$e0.spawns -ne 1) { Fail "the engine child was spawned $($e0.spawns) times, expected 1" }
    Call-Tool $srv 'refresh' 'alsem_refresh' @{ wait = $true } | Out-Null

    # What each answer must be.
    foreach ($k in 'query', 'explain', 'prove', 'digest-json', 'digest') {
        $a = $script:answers[$k]
        if ($null -eq $a -or (Prop $a 'kind') -ne 'cli') {
            Fail "$k is not a CLI answer: $(if ($a) { $a | ConvertTo-Json -Compress -Depth 4 } else { 'none' })"
        }
    }
    if ($null -ne $over) {
        if ((Prop $over 'kind') -ne 'cli') {
            Fail "a stdout over the inline cap was not delivered: $($over | ConvertTo-Json -Compress -Depth 4)"
        } elseif ($null -eq (Prop $over 'stdoutText')) {
            Warn 'a stdout over the inline cap came as a file: this platform wrote one (expected inline-only on Windows)'
        }
    }
    $dj = $script:answers['digest-json']
    if ($null -ne $dj -and (Prop $dj 'kind') -eq 'cli') {
        $n = if ($null -ne (Prop $dj 'stdoutText')) { [System.Text.Encoding]::UTF8.GetByteCount([string]$dj.stdoutText) } else { $dj.stdoutFile.bytes }
        $how = if ($null -ne (Prop $dj 'stdoutText')) { 'inline (stdoutText)' } else { 'as a file' }
        Say "  the JSON digest ($n bytes) was delivered $how, with a summary: $($null -ne (Prop $dj 'summary'))"
        if ($null -eq (Prop $dj 'summary')) { Fail 'the digest answer has no summary' }
    }
    if ($null -ne $outEnv) {
        if ((Prop $outEnv 'outcome') -eq 'refusedPath') {
            Say "  an 'out' file was refused (inline-only): $(Prop $outEnv 'message')"
        } elseif ((Prop $outEnv 'kind') -eq 'cli') {
            Warn "an 'out' file was written ($($outEnv.stdoutFile.path)): this platform has an output folder"
        } else {
            Fail "an 'out' file gave neither a file nor refusedPath: $($outEnv | ConvertTo-Json -Compress -Depth 4)"
        }
    }
} catch {
    Fail "MCP transcript: $_"
} finally {
    $rc = Stop-Mcp $srv (Join-Path $OutDir 'mcp-server.stderr')
    Say "  server exit $rc"
    if ($rc -ne 0) { Fail "the MCP server exited $rc, see mcp-server.stderr" }
}

# The same commands as cold CLI runs: the MCP answer must be their stdout exactly.
if (-not $SkipCliCompare) {
    Say 'windows-ctxsrv-check: comparing with cold CLI runs...'
    $cli = @{
        'query'       = @('query', 'effects', $Workspace, '--routine', $Routine, '--deterministic')
        'explain'     = @('explain', $Workspace, '--routine', $Routine, '--format', 'json')
        'prove'       = @('prove', $Workspace, $Routine, 'may-commit', '--deterministic')
        'digest-json' = @('digest', $Workspace, '--routine', $Routine, '--deterministic', '--format', 'json')
        'digest'      = @('digest', $Workspace, '--routine', $Routine, '--deterministic', '--format', 'human')
    }
    foreach ($k in 'query', 'explain', 'prove', 'digest-json', 'digest') {
        $a = $script:answers[$k]
        if ($null -eq $a -or (Prop $a 'kind') -ne 'cli') { continue }
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $run = Invoke-Captured $alsem $cli[$k]
        $ms = $sw.ElapsedMilliseconds
        Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'cli') "$k.stdout") -Value $run.Stdout -Encoding utf8 -NoNewline
        Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'cli') "$k.stderr") -Value $run.Stderr -Encoding utf8 -NoNewline
        $mcp = if ($null -ne (Prop $a 'stdoutText')) {
            [string]$a.stdoutText
        } else {
            [System.IO.File]::ReadAllText($a.stdoutFile.path, [System.Text.UTF8Encoding]::new($false))
        }
        $same = ($mcp -ceq $run.Stdout) -and ([int]$a.exitCode -eq $run.Exit)
        Say ("  {0,-12} cold CLI {1,7} ms, exit {2}: {3}" -f $k, $ms, $run.Exit, $(if ($same) { 'identical' } else { 'DIFFERENT' }))
        if (-not $same) {
            Set-Content -LiteralPath (Join-Path (Join-Path $OutDir 'cli') "$k.mcp-stdout") -Value $mcp -Encoding utf8 -NoNewline
            Fail "${k}: the MCP answer differs from the cold CLI (cli/$k.stdout vs cli/$k.mcp-stdout; exit $($a.exitCode) vs $($run.Exit))"
        }
    }
}

# 4. The manifest check: the child's manifest.observe spans (ALSEM_TRACE=1). The
#    first build observes twice (before and after its read, every file hashed);
#    every later `fresh` request observes once, reusing hashes by stat: those are
#    the checks.
Say 'windows-ctxsrv-check: [4/5] manifest check timing (ALSEM_TRACE=1)...'
$traceDir = Join-Path $OutDir 'trace'
$mcpOut2 = Join-Path $OutDir 'mcp-out-trace'
New-Item -ItemType Directory -Force -Path $mcpOut2 | Out-Null
$srv = Start-Mcp $alsem $mcpOut2 @{ 'ALSEM_TRACE' = '1'; 'ALSEM_TRACE_FILE' = ''; 'ALSEM_ENGINE_TEST_HOOK' = '' } $traceDir
$tracePid = 0
try {
    Request $srv 'initialize' @{ protocolVersion = '2025-06-18'; capabilities = @{}; clientInfo = @{ name = 'windows-ctxsrv-check'; version = '1' } } | Out-Null
    Send-Line $srv @{ jsonrpc = '2.0'; method = 'notifications/initialized'; params = @{} }
    for ($i = 0; $i -lt 7; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        $r = Request $srv 'tools/call' @{ name = 'alsem_query_effects'; arguments = @{ routine = $Routine; deterministic = $true } }
        Say "  traced query $i  $($sw.ElapsedMilliseconds) ms"
    }
    $st = Request $srv 'tools/call' @{ name = 'alsem_status'; arguments = @{} }
    $tracePid = [int]@($st[1].result.structuredContent.engines)[0].engine.pid
} catch {
    Fail "traced transcript: $_"
} finally {
    Stop-Mcp $srv (Join-Path $OutDir 'mcp-server-trace.stderr') | Out-Null
}
$traceFile = Join-Path $traceDir "alsem-trace-$tracePid.json"
$manifestTxt = Join-Path $OutDir 'manifest.txt'
if ($tracePid -gt 0 -and (Test-Path -LiteralPath $traceFile)) {
    # The trace is one JSON array on one line; each event is a flat object (an E
    # event carries one nested `args` object). Pair each B with the next E on the
    # same thread; `ts` is in microseconds.
    $open = @{}
    $durations = New-Object System.Collections.Generic.List[double]
    $content = [System.IO.File]::ReadAllText($traceFile)
    foreach ($m in [regex]::Matches($content, '\{(?:[^{}]|\{[^{}]*\})*\}')) {
        $ev = $m.Value
        if ($ev -notmatch '"name":"manifest\.observe"') { continue }
        $ph = [regex]::Match($ev, '"ph":"(\w)"').Groups[1].Value
        $tid = [regex]::Match($ev, '"tid":(\d+)').Groups[1].Value
        $ts = [double][regex]::Match($ev, '"ts":([0-9.]+)').Groups[1].Value
        if ($ph -eq 'B') { $open[$tid] = $ts }
        elseif ($ph -eq 'E' -and $open.ContainsKey($tid)) {
            $durations.Add(($ts - $open[$tid]) / 1000.0)
            $open.Remove($tid)
        }
    }
    $all = ($durations | ForEach-Object { '{0:N2}' -f $_ }) -join ', '
    Set-Content -LiteralPath $manifestTxt -Value "manifest.observe durations (ms), in order: $all" -Encoding utf8
    Say "  manifest.observe (ms, in order): $all"
    if ($durations.Count -ge 3) {
        $checks = @($durations | Select-Object -Skip 2 | Sort-Object)
        $median = $checks[[int][Math]::Floor($checks.Count / 2)]
        $line = ('manifest check (warm, {0} samples): min {1:N2} ms, median {2:N2} ms, max {3:N2} ms (target <= 10 ms; Linux 1.2-1.7 ms)' -f `
                $checks.Count, $checks[0], $median, $checks[-1])
        Add-Content -LiteralPath $manifestTxt -Value $line -Encoding utf8
        Say "  $line"
        if ($median -gt 10) { Warn "the manifest check's median $('{0:N2}' -f $median) ms is over the 10 ms target" }
    } else {
        Fail "the trace holds $($durations.Count) manifest.observe span(s); expected 2 for the build and 1 per later query"
    }
} else {
    Fail "no child trace file ($traceFile)"
}

# 5. Nothing written where it must not be.
Say 'windows-ctxsrv-check: [5/5] the baseline and the output folders...'
$baselineAfter = Git-Status $Workspace
if ($baselineAfter -ne $baselineBefore) {
    Fail "the baseline's git status changed during the run:`n--- before`n$baselineBefore`n--- after`n$baselineAfter"
} else {
    Say '  the baseline is unchanged (git status)'
}
foreach ($root in $mcpOut, $mcpOut2) {
    $files = @(Get-ChildItem -LiteralPath $root -Recurse -File -ErrorAction SilentlyContinue)
    if ($files.Count -eq 0) {
        Say "  $root holds no file (inline-only)"
    } else {
        Warn "$root holds $($files.Count) file(s): $(($files | Select-Object -First 5 | ForEach-Object { $_.FullName }) -join ', ')"
    }
}

Say ''
Say "windows-ctxsrv-check: $($script:failures.Count) failure(s), $($script:warnings.Count) warning(s)"
foreach ($f in $script:failures) { Say "  FAIL $f" }
foreach ($w in $script:warnings) { Say "  WARN $w" }
Say "windows-ctxsrv-check: done. Send back the folder: $OutDir"
if ($script:failures.Count -gt 0) { exit 1 }
exit 0
