# Temporary diagnosis of the native Windows ssh bootstrap (see diag-winssh.yml).
$ErrorActionPreference = 'Continue'
$key = "$env:USERPROFILE\.ssh\marsad_winssh"
$work = Join-Path $env:RUNNER_TEMP 'diag'
New-Item -ItemType Directory -Force $work | Out-Null
$src = Get-Content -Raw crates/marsad-agent/src/hub/transport.rs
$boot = ([regex]::Match($src, 'POWERSHELL_BOOTSTRAP: &str = r#"(.*?)"#;', 'Singleline')).Groups[1].Value -replace "`r", ''
Write-Host "bootstrap script: $($boot.Length) chars"

function Enc($s) { [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($s)) }
$payloads = @{}
function Payload([long]$n) {
  if (-not $payloads.ContainsKey($n)) {
    $h = [Text.Encoding]::ASCII.GetBytes("UPLOAD $n`n")
    $b = [byte[]]::new($h.Length + $n)
    [Array]::Copy($h, $b, $h.Length)
    $r = [byte[]]::new($n); [Random]::new(7).NextBytes($r); [Array]::Copy($r, 0, $b, $h.Length, $n)
    $payloads[$n] = $b
  }
  $payloads[$n]
}
function Prelude($name) {
  $log = "$work\$name.log"
  "`$__log='$log'; function L(`$t) { [IO.File]::AppendAllText(`$__log, ([DateTime]::Now.ToString('HH:mm:ss.fff') + ' ' + `$t + [char]10)) }`ntrap { L ('TRAP: ' + `$_); break }`nL 'start'`n"
}
# The product script, instrumented: a log line after every line, progress in the loop.
function Instrumented($name, $dir) {
  $s = $boot.Replace("Join-Path `$env:LOCALAPPDATA 'Marsad\bin'", "'$dir'").Replace('@HOST_ID@', 'diag')
  $s = $s.Replace('$n-=$r }', '$n-=$r; $__t+=$r; if ($__t -ge $__nx) { L "read $__t"; $__nx+=4194304 } }')
  $lines = $s.Split("`n") | Where-Object { $_ -ne '' }
  $k = 0
  (Prelude $name) + '$__t=0; $__nx=0' + "`n" + (($lines | ForEach-Object { $k++; "$_`nL 'after line $k'" }) -join "`n")
}
$readOnly = @'
$o=[Console]::OpenStandardOutput(); $i=[Console]::OpenStandardInput()
function Say($t) { $x=[Text.Encoding]::ASCII.GetBytes("$t`n"); $o.Write($x,0,$x.Length); $o.Flush() }
function ReadLine { $s=''; while ($true) { $c=$i.ReadByte(); if ($c -lt 0 -or $c -eq 10) { return $s }; $s+=[char]$c } }
Say "MARSAD-BOOT Windows diag none"
$l = ReadLine; L "line [$l]"
$n=[long]$l.Split(' ')[1]; $tot=0; $buf=New-Object byte[] 65536; $next=0
while ($n -gt 0) { $r=$i.Read($buf,0,[Math]::Min($buf.Length,$n)); if ($r -le 0) { L "short read: eof after $tot"; exit 70 }; $n-=$r; $tot+=$r; if ($tot -ge $next) { L "read $tot"; $next+=4194304 } }
L "done $tot"
Say 'MARSAD-OK'
exit 0
'@
$stderrUnderStop = @'
$ErrorActionPreference='Stop'
& where.exe no_such_thing_xyz | Out-Null
L "after where, exit $LASTEXITCODE"
[Console]::Out.WriteLine('ALIVE'); [Console]::Out.Flush()
'@
$missingExe = @'
$ErrorActionPreference='Stop'
& 'C:\no\such\marsad-agent.exe' stop | Out-Null
L 'after missing exe'
[Console]::Out.WriteLine('ALIVE'); [Console]::Out.Flush()
'@

function Snapshot($since) {
  Get-CimInstance Win32_Process | Where-Object { $_.CreationDate -gt $since -and $_.Name -notin @('conhost.exe', 'WmiPrvSE.exe') } |
    ForEach-Object { "    pid $($_.ProcessId) ppid $($_.ParentProcessId) $($_.Name) $(([string]$_.CommandLine).Substring(0, [Math]::Min(90, ([string]$_.CommandLine).Length)))" }
}

function Run($name, $exe, $script, [byte[]]$payload, $marker = 'MARSAD-OK', $timeout = 30, [switch]$closeStdin) {
  $remote = "$exe -NoProfile -NonInteractive -EncodedCommand $(Enc $script)"
  $since = Get-Date
  $psi = [Diagnostics.ProcessStartInfo]::new('ssh')
  $psi.Arguments = "-T -p 2223 -i `"$key`" -o IdentitiesOnly=yes -o BatchMode=yes $env:USERNAME@127.0.0.1 `"$remote`""
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $p = [Diagnostics.Process]::Start($psi)
  $out = [IO.MemoryStream]::new(); $err = [IO.MemoryStream]::new()
  $to = $p.StandardOutput.BaseStream.CopyToAsync($out)
  $te = $p.StandardError.BaseStream.CopyToAsync($err)
  $sw = [Diagnostics.Stopwatch]::StartNew()
  $tw = $null; if ($payload) { $tw = $p.StandardInput.BaseStream.WriteAsync($payload, 0, $payload.Length) }
  $wrote = $null; $seen = $null; $status = 'HANG'
  while ($sw.Elapsed.TotalSeconds -lt $timeout) {
    if ($tw -and $tw.IsCompleted -and -not $wrote) {
      $wrote = $sw.Elapsed.TotalSeconds
      if ($tw.IsFaulted) { $wrote = "faulted: $($tw.Exception.InnerException.Message)" }
      $p.StandardInput.BaseStream.Flush()
      if ($closeStdin) { $p.StandardInput.Close() }
    }
    if (-not $payload -and $closeStdin -and -not $wrote) { $p.StandardInput.Close(); $wrote = 0 }
    $text = [Text.Encoding]::UTF8.GetString($out.ToArray())
    if (-not $seen -and $text.Contains($marker)) { $seen = $sw.Elapsed.TotalSeconds; $status = 'PASS' }
    if ($p.HasExited) { if (-not $seen) { $status = 'EXIT' }; break }
    if ($seen -and $sw.Elapsed.TotalSeconds -gt $seen + 3) { break }
    Start-Sleep -Milliseconds 100
  }
  $alive = ''
  if (-not $p.HasExited) { $alive = "ssh still running at $([int]$sw.Elapsed.TotalSeconds)s; processes:`n" + ((Snapshot $since) -join "`n") }
  $exit = if ($p.HasExited) { "exit $($p.ExitCode) at $([Math]::Round($sw.Elapsed.TotalSeconds,1))s" } else { 'not exited' }
  Write-Host "=== $name : $status (write done: $wrote; marker at: $seen; $exit)"
  if ($alive) { Write-Host $alive }
  if (-not $p.HasExited) { try { $p.Kill($true) } catch {} }
  Get-CimInstance Win32_Process | Where-Object { $_.CreationDate -gt $since -and $_.Name -in @('powershell.exe', 'pwsh.exe', 'where.exe', 'ssh.exe', 'cmd.exe') } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  [void]$to.Wait(2000); [void]$te.Wait(2000)
  $o = [Text.Encoding]::UTF8.GetString($out.ToArray()); $e = [Text.Encoding]::UTF8.GetString($err.ToArray())
  Write-Host "  stdout ($($o.Length)): $($o.Substring(0, [Math]::Min(300, $o.Length)))"
  if ($e) { Write-Host "  stderr: $($e.Substring([Math]::Max(0, $e.Length - 800)))" }
  if (Test-Path "$work\$name.log") { Write-Host '  log:'; Get-Content "$work\$name.log" | Select-Object -Last 25 | ForEach-Object { "    $_" } | Write-Host }
}

Write-Host '--- does powershell 5.1 accept -InputFormat None?'
powershell -NoProfile -NonInteractive -InputFormat None -Command 'echo hi-5.1'; Write-Host "exit $LASTEXITCODE"
pwsh -NoProfile -NonInteractive -InputFormat None -Command 'echo hi-7'; Write-Host "exit $LASTEXITCODE"
$PSVersionTable.PSVersion.ToString()
powershell -NoProfile -Command '$PSVersionTable.PSVersion.ToString()'

$PS = 'powershell -InputFormat None'
$exist = "$work\v-exist"; New-Item -ItemType Directory -Force $exist | Out-Null
Copy-Item "$env:WINDIR\System32\where.exe" "$exist\marsad-agent.exe"

# read-only: isolate the stdin path
Run 'ro-ps51-IFnone-1K' $PS ((Prelude 'ro-ps51-IFnone-1K') + $readOnly) (Payload 1024)
Run 'ro-ps51-IFnone-1M' $PS ((Prelude 'ro-ps51-IFnone-1M') + $readOnly) (Payload 1048576)
Run 'ro-ps51-IFnone-30M' $PS ((Prelude 'ro-ps51-IFnone-30M') + $readOnly) (Payload 31457280) -timeout 90
Run 'ro-ps51-noIF-1M' 'powershell' ((Prelude 'ro-ps51-noIF-1M') + $readOnly) (Payload 1048576)
Run 'ro-ps51-IFnone-1M-closed' $PS ((Prelude 'ro-ps51-IFnone-1M-closed') + $readOnly) (Payload 1048576) -closeStdin
Run 'ro-pwsh-1M' 'pwsh' ((Prelude 'ro-pwsh-1M') + $readOnly) (Payload 1048576)
Run 'ro-pwsh-30M' 'pwsh' ((Prelude 'ro-pwsh-30M') + $readOnly) (Payload 31457280) -timeout 90
# the product script: first install ($b missing) and update ($b exists)
Run 'cur-missing-1K' $PS (Instrumented 'cur-missing-1K' "$work\v-missing1") (Payload 1024)
Run 'cur-missing-1M' $PS (Instrumented 'cur-missing-1M' "$work\v-missing2") (Payload 1048576)
Run 'cur-missing-30M' $PS (Instrumented 'cur-missing-30M' "$work\v-missing3") (Payload 31457280) -timeout 90
Run 'cur-missing-1M-noIF' 'powershell' (Instrumented 'cur-missing-1M-noIF' "$work\v-missing4") (Payload 1048576)
Run 'cur-exist-1M' $PS (Instrumented 'cur-exist-1M' $exist) (Payload 1048576)
# error behaviour
Run 'stderr-under-stop' $PS ((Prelude 'stderr-under-stop') + $stderrUnderStop) $null 'ALIVE' 20
Run 'missing-exe-open-stdin' $PS ((Prelude 'missing-exe-open-stdin') + $missingExe) $null 'ALIVE' 20
Run 'missing-exe-closed-stdin' $PS ((Prelude 'missing-exe-closed-stdin') + $missingExe) $null 'ALIVE' 20 -closeStdin
Write-Host '--- sshd log tail'
Get-Content 'C:\ProgramData\ssh\logs\sshd.log' -Tail 40 -ErrorAction Continue
