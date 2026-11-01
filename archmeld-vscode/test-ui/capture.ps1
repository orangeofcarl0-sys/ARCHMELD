param(
    [Parameter(Mandatory = $true)][string]$Out,
    [int]$ExtensionHostPid = 0,
    [string]$Marker = ''
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Capture {
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hWnd, int nCmdShow);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint flags);
    [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr hWnd, IntPtr after, int x, int y, int cx, int cy, uint flags);
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
}
"@

$processes = Get-CimInstance Win32_Process
$byId = @{}
foreach ($item in $processes) { $byId[[int]$item.ProcessId] = $item }

function Find-OwnerCodeProcess([int]$startPid) {
    $current = $startPid
    $guard = 0
    while ($current -ne 0 -and $guard -lt 30) {
        $item = $byId[$current]
        if (-not $item) { return $null }
        if ($item.Name -eq 'Code.exe') {
            $window = Get-Process -Id $current -ErrorAction SilentlyContinue
            if ($window -and $window.MainWindowHandle -ne [IntPtr]::Zero) { return $window }
        }
        $current = [int]$item.ParentProcessId
        $guard++
    }
    return $null
}

$window = $null
if ($ExtensionHostPid -gt 0) {
    $window = Find-OwnerCodeProcess $ExtensionHostPid
}
if (-not $window -and $Marker) {
    foreach ($item in $processes) {
        if ($item.Name -eq 'Code.exe' -and $item.CommandLine -like "*$Marker*") {
            $candidate = Get-Process -Id $item.ProcessId -ErrorAction SilentlyContinue
            if ($candidate -and $candidate.MainWindowHandle -ne [IntPtr]::Zero) {
                $window = $candidate
                break
            }
        }
    }
}
if (-not $window) {
    Write-Error 'No Code window owning this extension host; refusing to capture an unrelated window'
}

[Win32Capture]::ShowWindow($window.MainWindowHandle, 9) | Out-Null
[Win32Capture]::SetWindowPos($window.MainWindowHandle, [IntPtr](-1), 0, 0, 0, 0, 0x0003) | Out-Null
$shell = New-Object -ComObject WScript.Shell
$shell.AppActivate($window.Id) | Out-Null
[Win32Capture]::SetForegroundWindow($window.MainWindowHandle) | Out-Null
Start-Sleep -Milliseconds 3500
$rect = New-Object Win32Capture+RECT
[Win32Capture]::GetWindowRect($window.MainWindowHandle, [ref]$rect) | Out-Null
$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) {
    Write-Error 'VS Code window has no usable bounds'
}
$bitmap = New-Object System.Drawing.Bitmap $width, $height
$graphics = [System.Drawing.Graphics]::FromImage($bitmap)
# Copy the actual painted pixels after bringing the window to the front;
# PrintWindow on Electron can return the stale startup frame.
$graphics.CopyFromScreen($rect.Left, $rect.Top, 0, 0, $bitmap.Size)
$method = 'foreground-copyscreen'
$bitmap.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png)
$graphics.Dispose()
$bitmap.Dispose()

$sidecar = [ordered]@{
    captured = $true
    code_pid = $window.Id
    title = $window.MainWindowTitle
    method = $method
    width = $width
    height = $height
    out = $Out
}
[Win32Capture]::SetWindowPos($window.MainWindowHandle, [IntPtr](-2), 0, 0, 0, 0, 0x0003) | Out-Null
$sidecar | ConvertTo-Json | Set-Content -Path ($Out + '.json')
Write-Output ("captured pid=" + $window.Id + " title=" + $window.MainWindowTitle + " method=" + $method)
