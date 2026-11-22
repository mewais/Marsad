# Temporary diagnosis of the native Windows ssh bootstrap (see diag-winssh.yml). Round 3:
# the product's command line (PowerShell <NUL compiles the C# boot program, which speaks the
# protocol), with pwsh (which does not read stdin itself) standing in for the bridge.
$ErrorActionPreference = 'Continue'
$key = "$env:USERPROFILE\.ssh\marsad_winssh"
$work = Join-Path $env:RUNNER_TEMP 'diag'
New-Item -ItemType Directory -Force $work | Out-Null
function Enc($s) { [Convert]::ToBase64String([Text.Encoding]::Unicode.GetBytes($s)) }
function Ascii($s) { [Text.Encoding]::ASCII.GetBytes($s) }

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
  $psi.Arguments = "-T -p 2223 -i `"$key`" -o IdentitiesOnly=yes -o BatchMode=yes $env:USERNAME@127.0.0.1 `"$($remote.Replace('"', '\"'))`""
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
  Get-CimInstance Win32_Process | Where-Object { $_.CreationDate -gt $since -and ($_.Name -in @('powershell.exe', 'pwsh.exe', 'ssh.exe', 'cmd.exe', 'where.exe') -or $_.Name -like 'marsad-*') } |
    ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }
  [void]$to.Wait(2000); [void]$te.Wait(2000)
  $o = [Text.Encoding]::UTF8.GetString($out.ToArray()); $e = [Text.Encoding]::UTF8.GetString($err.ToArray())
  Write-Host "  stdout: $($o.Substring(0, [Math]::Min(300, $o.Length)))"
  if ($e) { Write-Host "  stderr: $($e.Substring([Math]::Max(0, $e.Length - 600)))" }
}

$boot = (Get-Content -Raw .github/diag/boot-cmd.txt).Trim()
$bridgePart = '"%LOCALAPPDATA%\Marsad\bin\marsad-agent.exe" bridge --host-id diag'
if (-not $boot.EndsWith($bridgePart)) { throw 'unexpected command line' }
$remote = $boot.Substring(0, $boot.Length - $bridgePart.Length) + "pwsh -NoProfile -NonInteractive -EncodedCommand $bridge"
Write-Host "remote command: $($remote.Length) chars"
$bin = "$env:LOCALAPPDATA\Marsad\bin"
function Raw([byte[]]$data) { $h = Ascii "UPLOAD $($data.Length)`n"; $r = [byte[]]::new($h.Length + $data.Length); [Array]::Copy($h, $r, $h.Length); [Array]::Copy($data, 0, $r, $h.Length, $data.Length); , $r }
function Rand([long]$n) { $r = [byte[]]::new($n); [Random]::new(7).NextBytes($r); , $r }
$hello = Ascii "HELLO`n"
function Show { Get-ChildItem $bin -ErrorAction SilentlyContinue | ForEach-Object { "  $($_.Name) $($_.Length)" } | Write-Host }
function Clean { Get-ChildItem $bin -Filter 'marsad-agent.exe*' -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction Continue }
foreach ($n in 1024, 1048576, 31457280) {
  Clean
  Run2 "install-$n" $remote @(@($null, 'MARSAD-BOOT', $false), @((Raw (Rand $n)), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false)) 120
  Show
}
Run2 'run-on-a-broken-helper' $remote @(@($null, 'never', $false)) 30
Clean
$where = [IO.File]::ReadAllBytes("$env:WINDIR\System32\where.exe")
Run2 'install-where' $remote @(@($null, 'MARSAD-BOOT', $false), @((Raw $where), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
Run2 'run' $remote @(@($null, 'MARSAD-BOOT', $false), @((Ascii "RUN`n"), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
Run2 'restart' $remote @(@($null, 'MARSAD-BOOT', $false), @((Ascii "RESTART`n"), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
Run2 'update' $remote @(@($null, 'MARSAD-BOOT', $false), @((Raw $where), 'MARSAD-OK', $false), @($hello, 'BRIDGE-GOT HELLO', $false))
Show
Run2 'unknown-command' $remote @(@($null, 'MARSAD-BOOT', $false), @((Ascii "BOGUS`n"), 'never', $false)) 20
$half = [byte[]]::new(300000); [Array]::Copy((Raw (Rand 1048576)), $half, 300000)
Run2 'closed-mid-upload' $remote @(@($null, 'MARSAD-BOOT', $false), @($half, 'never', $true)) 30
Show
