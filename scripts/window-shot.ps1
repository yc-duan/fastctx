# A picture of a process's main window as it draws itself, WebView2's composed content included,
# whatever lies over it on the screen; the whole screen when the process has no window.
#   pwsh scripts/window-shot.ps1 -ProcessId ID -Path FILE.png
param([Parameter(Mandatory)][int]$ProcessId, [Parameter(Mandatory)][string]$Path)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing, System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class WindowShot {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out Rect rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
}
"@
New-Item -ItemType Directory -Force (Split-Path -Parent $Path) | Out-Null
$window = [IntPtr]::Zero
for ($i = 0; $i -lt 20 -and $window -eq [IntPtr]::Zero; $i++) {
    $window = (Get-Process -Id $ProcessId).MainWindowHandle
    if ($window -eq [IntPtr]::Zero) { Start-Sleep -Milliseconds 500 }
}
if ($window -ne [IntPtr]::Zero) {
    $rect = New-Object WindowShot+Rect
    [void][WindowShot]::GetWindowRect($window, [ref]$rect)
    $picture = New-Object System.Drawing.Bitmap ($rect.Right - $rect.Left), ($rect.Bottom - $rect.Top)
    $graphics = [System.Drawing.Graphics]::FromImage($picture)
    $dc = $graphics.GetHdc()
    # PW_RENDERFULLCONTENT: content composed by DirectComposition, as WebView2's, is drawn too.
    $drawn = [WindowShot]::PrintWindow($window, $dc, 2)
    $graphics.ReleaseHdc($dc)
    if (-not $drawn) { throw "the window of process $ProcessId could not be drawn" }
} else {
    $screen = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
    $picture = New-Object System.Drawing.Bitmap $screen.Width, $screen.Height
    [System.Drawing.Graphics]::FromImage($picture).CopyFromScreen($screen.Location, [System.Drawing.Point]::Empty, $screen.Size)
}
$picture.Save($Path)
