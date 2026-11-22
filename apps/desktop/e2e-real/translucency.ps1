# Windows translucency helpers for the real-app WebDriver suite (run.mjs, scenario
# `translucent-window`), which picks the window background in Settings itself:
#
#   translucency.ps1 -Backdrop                 a borderless magenta window covering the screen
#                                              (runs until killed); turns Windows' Transparency
#                                              effects on first (off on some runner images)
#   translucency.ps1 -Sample <name> -Out <dir> bring the running app's window to the front,
#                                              screenshot the screen to <dir>\<name>.png and
#                                              print one JSON line: the pixel colours of the
#                                              app's content area, top bar, side rail and
#                                              status bar, and whether the window was active
param(
  [switch]$Backdrop,
  [string]$Sample,
  [string]$Out
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr h, int c);
  [DllImport("user32.dll")] public static extern void keybd_event(byte k, byte s, uint f, UIntPtr e);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessageTimeout(IntPtr h, uint m, UIntPtr w, string l, uint f, uint t, out UIntPtr r);
}
"@
[W]::SetProcessDPIAware() | Out-Null

if ($Backdrop) {
  # Windows only composites Acrylic/Mica with "Transparency effects" on.
  $pers = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize'
  New-Item -Force $pers -ErrorAction SilentlyContinue | Out-Null
  Set-ItemProperty $pers EnableTransparency 1 -Type DWord
  $res = [UIntPtr]::Zero
  [W]::SendMessageTimeout([IntPtr]0xffff, 0x001A, [UIntPtr]::Zero, 'ImmersiveColorSet', 2, 5000, [ref]$res) | Out-Null
  $f = New-Object System.Windows.Forms.Form
  $f.FormBorderStyle = 'None'; $f.WindowState = 'Maximized'; $f.ShowInTaskbar = $false
  $f.BackColor = [System.Drawing.Color]::FromArgb(255, 0, 255)
  [System.Windows.Forms.Application]::Run($f)
  exit 0
}

if (-not $Sample -or -not $Out) { throw 'use -Backdrop, or -Sample <name> -Out <dir>' }
New-Item -ItemType Directory -Force $Out | Out-Null
$p = Get-Process marsad-desktop | Where-Object { $_.MainWindowHandle -ne [IntPtr]::Zero } | Select-Object -First 1
if (-not $p) { throw 'no marsad-desktop window' }
$h = $p.MainWindowHandle
# Active, or Windows draws the system backdrop as its solid fallback.
for ($i = 0; $i -lt 6 -and [W]::GetForegroundWindow() -ne $h; $i++) {
  [W]::keybd_event(0x12, 0, 0, [UIntPtr]::Zero); [W]::keybd_event(0x12, 0, 2, [UIntPtr]::Zero)  # ALT unlocks SetForegroundWindow
  [W]::SetForegroundWindow($h) | Out-Null
  Start-Sleep -Milliseconds 700
  if ([W]::GetForegroundWindow() -ne $h) { [W]::ShowWindow($h, 6) | Out-Null; Start-Sleep -Milliseconds 300; [W]::ShowWindow($h, 9) | Out-Null; Start-Sleep -Milliseconds 700 }
}
$active = [W]::GetForegroundWindow() -eq $h
Start-Sleep -Seconds 2   # DWM settles the backdrop of the newly active window
$r = New-Object W+RECT
[W]::GetWindowRect($h, [ref]$r) | Out-Null
$screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
$g.Dispose()
$bmp.Save((Join-Path $Out "$Sample.png"), [System.Drawing.Imaging.ImageFormat]::Png)
$w = $r.R - $r.L; $hh = $r.B - $r.T
# Points relative to the window: content (centre-right), the app's top bar (below the native
# caption), the side rail (left strip), the status bar.
$pts = [ordered]@{
  content = @([int]($r.L + $w * 0.62), [int]($r.T + $hh * 0.55));
  topbar  = @([int]($r.L + $w * 0.80), [int]($r.T + 31 + 20));
  rail    = @([int]($r.L + 30),        [int]($r.T + $hh * 0.70));
  status  = @([int]($r.L + $w * 0.85), [int]($r.B - 18));
}
$px = [ordered]@{}
foreach ($k in $pts.Keys) {
  $c = $bmp.GetPixel([Math]::Max(0, [Math]::Min($pts[$k][0], $bmp.Width - 1)), [Math]::Max(0, [Math]::Min($pts[$k][1], $bmp.Height - 1)))
  $px[$k] = @($c.R, $c.G, $c.B)
}
$bmp.Dispose()
[ordered]@{ active = $active; window = @($r.L, $r.T, $r.R, $r.B); pixels = $px } | ConvertTo-Json -Compress -Depth 4
