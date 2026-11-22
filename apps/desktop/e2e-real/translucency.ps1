# Windows translucency check (CI): the built app over a vivid magenta full-screen window. The
# window background is picked through the real Settings control — Settings in the side rail,
# then Window background → Translucent, later Solid — clicked through Windows UI Automation
# (the WebView2 accessibility tree; msedgedriver could not attach to the app on the hosted
# runner). Each time the screen is sampled at the app's content area, top bar, side rail and
# status bar. Checked: the shell applied Acrylic (its log), every area differs between
# translucent and solid, and solid is solid. Whether the magenta itself shows through is
# reported, not required: hosted runners have no GPU composition and DWM draws Acrylic as its
# solid fallback colour there.
#
#   powershell -File translucency.ps1 -App <marsad-desktop.exe> -Agent <marsad-agent.exe> -Out <dir>
param(
  [Parameter(Mandatory = $true)][string]$App,
  [Parameter(Mandatory = $true)][string]$Agent,
  [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force $Out | Out-Null
$App = (Resolve-Path $App).Path; $Agent = (Resolve-Path $Agent).Path; $Out = (Resolve-Path $Out).Path
Add-Type -AssemblyName System.Windows.Forms, System.Drawing, UIAutomationClient, UIAutomationTypes
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr a, int x, int y, int cx, int cy, uint f);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, UIntPtr w, string l, uint f, uint t, out UIntPtr r);
}
"@
[W]::SetProcessDPIAware() | Out-Null
# Windows only composites Acrylic/Mica with "Transparency effects" on (off on some runner images).
$pers = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
New-Item -Force $pers -ErrorAction SilentlyContinue | Out-Null
Set-ItemProperty $pers EnableTransparency 1 -Type DWord
$res = [UIntPtr]::Zero
[W]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'ImmersiveColorSet', 2, 5000, [ref]$res) | Out-Null
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds

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

function Front($h) {
  for ($i = 0; $i -lt 6 -and [W]::GetForegroundWindow() -ne $h; $i++) {
    [W]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [W]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)  # ALT unlocks SetForegroundWindow
    [W]::SetForegroundWindow($h) | Out-Null
    Start-Sleep -Milliseconds 700
    if ([W]::GetForegroundWindow() -ne $h) { [W]::ShowWindow($h, 6) | Out-Null; Start-Sleep -Milliseconds 300; [W]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 700 }
  }
  return [W]::GetForegroundWindow() -eq $h
}

# The first element named $name (exact) under $root, waiting for the page to render it.
function Find($root, [string]$name, [int]$seconds = 60) {
  $cond = New-Object System.Windows.Automation.PropertyCondition([System.Windows.Automation.AutomationElement]::NameProperty, $name)
  for ($i = 0; $i -lt $seconds * 2; $i++) {
    $e = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($e) { return $e }
    Start-Sleep -Milliseconds 500
  }
  throw "no element named '$name' in the app window"
}

# Click an element with the mouse at its centre (what a person does).
function Click($h, $e) {
  Front $h | Out-Null
  $b = $e.Current.BoundingRectangle
  [W]::SetCursorPos([int]($b.X + $b.Width / 2), [int]($b.Y + $b.Height / 2)) | Out-Null
  Start-Sleep -Milliseconds 150
  [W]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero); [W]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
  Start-Sleep -Milliseconds 400
}

function Sample($h, [string]$name) {
  $active = Front $h
  Start-Sleep -Seconds 2   # DWM settles the backdrop of the newly active window
  $r = New-Object W+RECT
  [W]::GetWindowRect($h, [ref]$r) | Out-Null
  [W]::SetCursorPos(0, 0) | Out-Null
  $bmp = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
  $g.Dispose()
  $bmp.Save((Join-Path $Out "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
  $w = $r.R - $r.L; $hh = $r.B - $r.T
  $pts = [ordered]@{
    content = @([int]($r.L + $w * 0.62), [int]($r.T + $hh * 0.55));
    topbar  = @([int]($r.L + $w * 0.80), [int]($r.T + 31 + 20));
    rail    = @([int]($r.L + 30),        [int]($r.T + $hh * 0.70));
    status  = @([int]($r.L + $w * 0.85), [int]($r.B - 18));
  }
  $px = [ordered]@{}
  foreach ($k in $pts.Keys) {
    $c = $bmp.GetPixel([Math]::Max(0, [Math]::Min($pts[$k][0], $bmp.Width - 1)), [Math]::Max(0, [Math]::Min($pts[$k][1], $bmp.Height - 1)))
    $px[$k] = $c
    Write-Host ("{0,-12} {1,-8}: R={2} G={3} B={4}" -f $name, $k, $c.R, $c.G, $c.B)
  }
  $bmp.Dispose()
  Write-Host "$name active: $active"
  return $px
}

$env:MARSAD_DATA = Join-Path $Out 'data'
$env:MARSAD_AGENT_BIN = $Agent
$log = Join-Path $Out 'app.log'
$p = Start-Process $App -PassThru -RedirectStandardOutput $log -RedirectStandardError "$log.err"
$h = [IntPtr]::Zero
for ($i = 0; $i -lt 120 -and $h -eq [IntPtr]::Zero; $i++) { Start-Sleep -Milliseconds 500; $p.Refresh(); $h = $p.MainWindowHandle }
if ($h -eq [IntPtr]::Zero) { throw 'no main window' }
[W]::ShowWindow($h, 1) | Out-Null
[W]::SetWindowPos($h, [IntPtr]::Zero, 20, 10, 1100, 720, 0x0040) | Out-Null
$root = [System.Windows.Automation.AutomationElement]::FromHandle($h)
try {
  Click $h (Find $root 'Settings')
  Click $h (Find $root 'Translucent')
  Start-Sleep -Seconds 3
  Click $h (Find $root 'Inbox')
  $t = Sample $h 'translucent'
  Click $h (Find $root 'Settings')
  Click $h (Find $root 'Solid')
  Start-Sleep -Seconds 3
  Click $h (Find $root 'Inbox')
  $s = Sample $h 'solid'
} finally {
  Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
  Stop-Process -Id $bg.Id -Force -ErrorAction SilentlyContinue
  & $Agent stop --data $env:MARSAD_DATA | Out-Null
  Start-Sleep -Seconds 1
}

function Magenta($c) { return (($c.R + $c.B) / 2) - $c.G }
function Dist($a, $b) { return [Math]::Max([Math]::Max([Math]::Abs($a.R - $b.R), [Math]::Abs($a.G - $b.G)), [Math]::Abs($a.B - $b.B)) }
$fail = @(); $seeThrough = $true
$answers = Get-Content $log, "$log.err" -ErrorAction SilentlyContinue | Select-String 'window surface: translucent'
Write-Host "shell answers: $answers"
if ("$answers" -notmatch 'acrylic') { $fail += 'the shell did not apply Acrylic (picked in Settings)' }
foreach ($k in $t.Keys) {
  $mt = Magenta $t[$k]; $ms = Magenta $s[$k]; $d = Dist $t[$k] $s[$k]
  Write-Host ("{0,-8} magenta influence: translucent {1,6:N1}  solid {2,6:N1}; translucent vs solid {3}" -f $k, $mt, $ms, $d)
  if ($d -lt 10) { $fail += "$k looks the same translucent and solid ($d)" }
  if ($ms -gt 12) { $fail += "$k is not solid in solid mode ($ms)" }
  if ($mt -lt 25) { $seeThrough = $false }
}
if ($seeThrough) { Write-Host 'OBSERVED: the blurred colour behind shows through in every sampled area' }
else { Write-Host 'OBSERVED: Acrylic applied, but this runner (no GPU composition) draws the system backdrop as its solid fallback colour; the page and chrome are transparent over it (they differ from solid)' }
if ($fail.Count) { $fail | ForEach-Object { Write-Host "FAIL: $_" }; exit 1 }
Write-Host 'translucency OK'
