# Temporary diagnosis of the native Windows ssh bootstrap (see diag-winssh.yml). Round 2:
# PowerShell 5.1 reads only through $input (its host's reader owns the redirected stdin);
# the bridge stand-in (pwsh, which does not read stdin itself) runs from cmd after it exits.
$ErrorActionPreference = 'Continue'
$key = "$env:USERPROFILE\.ssh\marsad_winssh"
$work = Join-Path $env:RUNNER_TEMP 'diag'
New-Item -ItemType Directory -Force $work | Out-Null
function Enc($s) { [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($s)) }
function Ascii($s) { [Text.Encoding]::ASCII.GetBytes($s) }

$candidate = @'
$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'
trap { $m=[Text.Encoding]::UTF8.GetBytes("Marsad bootstrap: $_`n"); $e=[Console]::OpenStandardError(); $e.Write($m,0,$m.Length); $e.Flush(); exit 1 }
$o=[Console]::OpenStandardOutput()
function Say($t) { $x=[Text.Encoding]::ASCII.GetBytes("$t`n"); $o.Write($x,0,$x.Length); $o.Flush() }
$d=@DIR@; $b=Join-Path $d 'marsad-agent.exe'; $v='none'
if (Test-Path $b) { $v=((& $b --version) -join '') -replace ' ','' }
Say "MARSAD-BOOT Windows $env:PROCESSOR_ARCHITECTURE $v"
$in=$input.GetEnumerator()
function Next { if (-not $in.MoveNext()) { throw 'the connection closed during the bootstrap' }; [string]$in.Current }
$cmd=(Next).Split(' ')
if ($cmd[0] -eq 'UPLOAD') {
  New-Item -ItemType Directory -Force $d | Out-Null
  $n=[long]$cmd[1]; $t="$b.new.$PID"; $f=[IO.File]::Create($t)
  try { while ($n -gt 0) { $x=[Convert]::FromBase64String((Next)); $f.Write($x,0,$x.Length); $n-=$x.Length } } finally { $f.Close() }
  if (Test-Path $b) { & $b stop | Out-Null; Move-Item -Force $b "$b.old.$PID" }
  Move-Item -Force $t $b
} elseif ($cmd[0] -eq 'RESTART') { & $b stop | Out-Null } elseif ($cmd[0] -ne 'RUN') { throw "unknown command $($cmd[0])" }
'@
$bridge = Enc @'
$i=[Console]::OpenStandardInput(); $o=[Console]::OpenStandardOutput()
$s=''; while ($true) { $c=$i.ReadByte(); if ($c -lt 0 -or $c -eq 10) { break }; $s+=[char]$c }
$x=[Text.Encoding]::ASCII.GetBytes("BRIDGE-GOT $s`n"); $o.Write($x,0,$x.Length); $o.Flush()
'@

function Upload([long]$n) {
  $r = [byte[]]::new($n); [Random]::new(7).NextBytes($r)
  $ms = [IO.MemoryStream]::new()
  $h = Ascii "UPLOAD $n`n"; $ms.Write($h, 0, $h.Length)
  for ($off = 0; $off -lt $n; $off += 49152) {
    $l = Ascii ([Convert]::ToBase64String($r, $off, [Math]::Min(49152, $n - $off)) + "`n")
    $ms.Write($l, 0, $l.Length)
  }
  , $ms.ToArray()
}

# steps: @(@(bytes-or-null, expected-text, close-stdin-after?), ...)
function Run2($name, $remote, $steps, $timeout = 60) {
  $since = Get-Date
  $psi = [Diagnostics.ProcessStartInfo]::new('ssh')
  $psi.Arguments = "-T -p 2223 -i `"$key`" -o IdentitiesOnly=yes -o BatchMode=yes $env:USERNAME@127.0.0.1 `"$remote`""
  $psi.RedirectStandardInput = $true; $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
  $psi.UseShellExecute = $false
  $p = [Diagnostics.Process]::Start($psi)
  $out = [IO.MemoryStream]::new(); $err = [IO.MemoryStream]::new()
  $to = $p.StandardOutput.BaseStream.CopyToAsync($out); $te = $p.StandardError.BaseStream.CopyToAsync($err)
  $sw = [Diagnostics.Stopwatch]::StartNew(); $report = @(); $status = 'PASS'; $from = 0
  foreach ($s in $steps) {
    $bytes, $want, $close = $s
    $tw = $null; if ($bytes) { $tw = $p.StandardInput.BaseStream.WriteAsync($bytes, 0, $bytes.Length) }
    $closed = $false; $hit = $false
    while ($sw.Elapsed.TotalSeconds -lt $timeout) {
      if ($tw -and $tw.IsCompleted) { $p.StandardInput.BaseStream.Flush(); $tw = $null }
      if ($close -and -not $tw -and -not $closed) { $p.StandardInput.Close(); $closed = $true }
      $text = [Text.Encoding]::UTF8.GetString($out.ToArray())
      $at = $text.IndexOf($want, $from)
      if ($at -ge 0) { $from = $at + $want.Length; $hit = $true; break }
      if ($p.HasExited) { break }
      Start-Sleep -Milliseconds 50
    }
    if ($hit) { $report += "'$want' at $([Math]::Round($sw.Elapsed.TotalSeconds,1))s" }
    else { $status = if ($p.HasExited) { "EXIT before '$want'" } else { "HANG waiting for '$want'" }; break }
  }
  if ($status -eq 'PASS') { [void]$p.WaitForExit(5000) }
  $exit = if ($p.HasExited) { "exit $($p.ExitCode) at $([Math]::Round($sw.Elapsed.TotalSeconds,1))s" } else { 'not exited' }
  Write-Host "=== $name : $status ($($report -join ', '); $exit)"
  if (-not $p.HasExited) { try { $p.Kill($true) } catch {} }
  Get-CimInstance Win32_Process | Where-Object { $_.CreationDate -gt $since -and $_.Name -in @('powershell.exe', 'pwsh.exe', 'ssh.exe', 'cmd.exe', 'where.exe') } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  [void]$to.Wait(2000); [void]$te.Wait(2000)
  $o = [Text.Encoding]::UTF8.GetString($out.ToArray()); $e = [Text.Encoding]::UTF8.GetString($err.ToArray())
  Write-Host "  stdout: $($o.Substring(0, [Math]::Min(300, $o.Length)))"
  if ($e) { Write-Host "  stderr: $($e.Substring([Math]::Max(0, $e.Length - 600)))" }
}

function Remote($dir, $flags = '') {
  $s = Enc ($candidate.Replace('@DIR@', "'$dir'"))
  "powershell -NoProfile -NonInteractive $flags -EncodedCommand $s && echo MARSAD-OK&& pwsh -NoProfile -NonInteractive -EncodedCommand $bridge"
}
$hello = Ascii "HELLO`n"
foreach ($n in 1024, 1048576, 31457280) {
  $dir = "$work\fresh-$n"
  Run2 "install-$n" (Remote $dir) @(@($null, 'MARSAD-BOOT', $false), @((Upload $n), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false)) 120
  if (Test-Path "$dir\marsad-agent.exe") { Write-Host "  installed size: $((Get-Item "$dir\marsad-agent.exe").Length)" }
}
# update: an existing helper (where.exe stands in) is replaced
$upd = "$work\update"; New-Item -ItemType Directory -Force $upd | Out-Null
Copy-Item "$env:WINDIR\System32\where.exe" "$upd\marsad-agent.exe"
Run2 'update-1M' (Remote $upd) @(@($null, 'MARSAD-BOOT', $false), @((Upload 1048576), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
Get-ChildItem $upd | ForEach-Object { "  $($_.Name) $($_.Length)" } | Write-Host
Run2 'run' (Remote $upd) @(@($null, 'MARSAD-BOOT', $false), @((Ascii "RUN`n"), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
# errors come back as one plain line, the bridge is not started
Run2 'unknown-command' (Remote $upd) @(@($null, 'MARSAD-BOOT', $false), @((Ascii "BOGUS`n"), 'never', $false)) 20
Run2 'unknown-command-OutputFormatText' (Remote $upd '-OutputFormat Text') @(@($null, 'MARSAD-BOOT', $false), @((Ascii "BOGUS`n"), 'never', $false)) 20
$half = Upload 1048576; $half = $half[0..300000]
Run2 'closed-mid-upload' (Remote "$work\fresh-closed") @(@($null, 'MARSAD-BOOT', $false), @([byte[]]$half, 'never', $true)) 30
Run2 'unwritable-dir' (Remote 'C:\Windows\win.ini\sub') @(@($null, 'MARSAD-BOOT', $false), @((Upload 1024), 'never', $false)) 30
