# A picture of a process's main window as it draws itself, WebView2's composed content included,
# whatever lies over it on the screen; the whole screen when the process has no window. It fails
# when there is no window, or when the window's middle is all one colour: nothing was drawn there.
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
if ($window -eq [IntPtr]::Zero) { throw "process $ProcessId has no window; the screen is in $Path" }
# The window's frame and borders aside, a page that was drawn has more than one colour.
$colours = New-Object System.Collections.Generic.HashSet[int]
for ($x = [int]($picture.Width * 0.2); $x -lt $picture.Width * 0.8; $x += 4) {
    for ($y = [int]($picture.Height * 0.2); $y -lt $picture.Height * 0.8; $y += 4) {
        [void]$colours.Add($picture.GetPixel($x, $y).ToArgb())
    }
}
if ($colours.Count -lt 2) { throw "the window of process $ProcessId drew nothing: its middle is one colour" }
