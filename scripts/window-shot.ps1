# Saves a PNG of a running app's main window, for checking the interface without a person at the
# screen. The window does not need to be in front.
#
#   powershell -File scripts/window-shot.ps1 -Process paircam -Out shot.png

param(
    [string]$Process = "paircam",
    [Parameter(Mandatory = $true)][string]$Out
)

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;

public static class WindowShot {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }

    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr window, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr window, IntPtr dc, uint flags);
}
"@

[void][WindowShot]::SetProcessDPIAware()

$target = Get-Process -Name $Process -ErrorAction SilentlyContinue |
    Where-Object { $_.MainWindowHandle -ne 0 } |
    Select-Object -First 1
if (-not $target) {
    Write-Error "No window found for process '$Process'."
    exit 1
}

$rect = New-Object WindowShot+RECT
[void][WindowShot]::GetWindowRect($target.MainWindowHandle, [ref]$rect)
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top

$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
$dc = $graphics.GetHdc()
# 2 = PW_RENDERFULLCONTENT, which also captures GPU-composited windows.
[void][WindowShot]::PrintWindow($target.MainWindowHandle, $dc, 2)
$graphics.ReleaseHdc($dc)
$graphics.Dispose()

$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()
Write-Output "$Out ${width}x${height}"
