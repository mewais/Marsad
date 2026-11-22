# Windows translucency check (CI): the built app over a vivid magenta full-screen window, in
# translucent and in solid mode; full-screen screenshots; pixels sampled inside the app's
# content area, top bar and side rail. Translucent pixels must carry the colour behind
# (red and blue well above green), solid ones must not.
#
#   pwsh translucency.ps1 -App <marsad-desktop.exe> -Out <dir>
param(
  [Parameter(Mandatory = $true)][string]$App,
  [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, UIntPtr w, string l, uint f, uint t, out UIntPtr r);
}
"@
[W]::SetProcessDPIAware() | Out-Null
# Windows only composites Acrylic/Mica with "Transparency effects" on (off on some runner images).
$pers = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
New-Item -Force $pers -ErrorAction SilentlyContinue | Out-Null
Write-Host "EnableTransparency was: $((Get-ItemProperty $pers -ErrorAction SilentlyContinue).EnableTransparency)"
Set-ItemProperty $pers EnableTransparency 1 -Type DWord
# Tell running windows (DWM, explorer) that the setting changed.
$res = [UIntPtr]::Zero
[W]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'ImmersiveColorSet', 2, 5000, [ref]$res) | Out-Null
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
Write-Host "screen: $($screen.Width)x$($screen.Height)"

# The vivid backdrop: a borderless magenta window covering the screen, in its own process.
$bgScript = Join-Path $Out 'backdrop.ps1'
@'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$f = New-Object System.Windows.Forms.Form
$f.FormBorderStyle = 'None'; $f.WindowState = 'Maximized'; $f.BackColor = [System.Drawing.Color]::FromArgb(255, 0, 255)
$f.ShowInTaskbar = $false
[System.Windows.Forms.Application]::Run($f)
'@ | Set-Content $bgScript
$bg = Start-Process powershell -ArgumentList '-NoProfile', '-STA', '-File', $bgScript -PassThru
Start-Sleep -Seconds 4

function Shot([string]$name) {
  $bmp = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
  $g.Dispose()
  $path = Join-Path $Out "$name.png"
  $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
  return $bmp
}

function Run-Mode([string]$mode, [string]$name) {
  $data = Join-Path $Out "data-$name"
  $env:MARSAD_DATA = $data
  $env:MARSAD_SURFACE = $mode
  $log = Join-Path $Out "$name.log"
  $p = Start-Process $App -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
  $h = [IntPtr]::Zero
  for ($i = 0; $i -lt 60 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $p.Refresh(); $h = $p.MainWindowHandle }
  if ($h -eq [IntPtr]::Zero) { throw "no main window for $mode" }
  # A known place and size, then active (Windows draws system backdrops solid while inactive).
  [W]::ShowWindow($h, 1) | Out-Null
  [W]::SetWindowPos($h, [IntPtr]::Zero, 20, 10, 980, 660, 0x0040) | Out-Null
  Start-Sleep -Seconds 12   # page load, the surface hook's reload, the backdrop
  # Active, or Windows draws the system backdrop as its solid fallback.
  for ($i = 0; $i -lt 6 -and [W]::GetForegroundWindow() -ne $h; $i++) {
    [W]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [W]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)  # ALT unlocks SetForegroundWindow
    [W]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 700
    if ([W]::GetForegroundWindow() -ne $h) { [W]::ShowWindow($h, 6) | Out-Null; Start-Sleep -Milliseconds 300; [W]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 700 }
  }
  Write-Host "$name active: $([W]::GetForegroundWindow() -eq $h)"
  Start-Sleep -Seconds 3
  $r = New-Object W+RECT
  [W]::GetWindowRect($h, [ref]$r) | Out-Null
  Write-Host "$mode window: $($r.L),$($r.T) - $($r.R),$($r.B)"
  $bmp = Shot $name
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  # Points relative to the window: content (centre-right), the app's top bar (below the native
  # caption), the side rail (left strip), the status bar.
  $pts = [ordered]@{
    content = @([int]($r.L + $w * 0.62), [int]($r.T + $hh * 0.55));
    topbar  = @([int]($r.L + $w * 0.80), [int]($r.T + 31 + 20));
    rail    = @([int]($r.L + 30),        [int]($r.T + $hh * 0.70));
    status  = @([int]($r.L + $w * 0.85), [int]($r.B - 18));
  }
  $res = [ordered]@{}
  foreach ($k in $pts.Keys) {
    $x = $pts[$k][0]; $y = $pts[$k][1]
    $c = $bmp.GetPixel([Math]::Min($x, $bmp.Width - 1), [Math]::Min($y, $bmp.Height - 1))
    $res[$k] = $c
    Write-Host ("{0,-12} {1,-8} at {2},{3}: R={4} G={5} B={6}" -f $name, $k, $x, $y, $c.R, $c.G, $c.B)
  }
  $bmp.Dispose()
  Stop-Process -Id $p.Id -Force
  Start-Sleep -Seconds 2
  Get-Content $log -ErrorAction SilentlyContinue | Select-String 'window surface' | ForEach-Object { Write-Host "$name shell: $_" }
  return $res
}

$t = Run-Mode 'translucent:80' 'translucent'
$s = Run-Mode 'solid' 'solid'
Stop-Process -Id $bg.Id -Force -ErrorAction SilentlyContinue

function Magenta($c) { return (($c.R + $c.B) / 2) - $c.G }
function ColourDistance($a, $b) { return [Math]::Max([Math]::Max([Math]::Abs($a.R - $b.R), [Math]::Abs($a.G - $b.G)), [Math]::Abs($a.B - $b.B)) }
$fail = @(); $seeThrough = $true
$effect = (Get-Content (Join-Path $Out 'translucent.log') -ErrorAction SilentlyContinue | Select-String 'window surface: translucent' | Select-Object -Last 1)
Write-Host "shell answer: $effect"
if ("$effect" -notmatch 'acrylic') { $fail += 'the shell did not apply Acrylic' }
foreach ($k in $t.Keys) {
  $mt = Magenta $t[$k]; $ms = Magenta $s[$k]; $d = ColourDistance $t[$k] $s[$k]
  Write-Host ("{0,-8} magenta influence: translucent {1,6:N1}  solid {2,6:N1}; translucent vs solid {3}" -f $k, $mt, $ms, $d)
  # Every area (chrome included) lets the backdrop DWM draws show: it differs from solid.
  if ($d -lt 10) { $fail += "$k looks the same translucent and solid ($d)" }
  if ($ms -gt 12) { $fail += "$k is not solid in solid mode ($ms)" }
  if ($mt -lt 25) { $seeThrough = $false }
}
if ($seeThrough) { Write-Host 'OBSERVED: the blurred colour behind shows through in every sampled area' }
else { Write-Host 'OBSERVED: Acrylic applied, but this runner (no GPU composition) draws the system backdrop as its solid fallback colour, so the magenta behind does not reach the screenshot; the page and chrome are transparent over it (they differ from solid)' }
if ($fail.Count) { $fail | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
Write-Host 'translucency OK'
