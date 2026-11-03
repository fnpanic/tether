param([string]$Title = '', [string]$Process = '', [int]$X = 40, [int]$Y = 40, [int]$W = 900, [int]$H = 420)
# Bench helper: make a visible window topmost at a fixed position.
# Match by window-title substring and/or owning process name (process match avoids title-encoding issues).
Add-Type @"
using System;
using System.Text;
using System.Runtime.InteropServices;
public class TM {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] public static extern int GetWindowText(IntPtr h, StringBuilder s, int n);
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] public static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int w, int hh, uint f);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
}
"@
$procIds = @()
if ($Process) { $procIds = @(Get-Process -Name $Process -ErrorAction SilentlyContinue | ForEach-Object Id) }
$found = $null
[TM]::EnumWindows({ param($h, $l)
  if (-not [TM]::IsWindowVisible($h)) { return $true }
  $sb = New-Object System.Text.StringBuilder 256; [void][TM]::GetWindowText($h, $sb, 256)
  $t = $sb.ToString()
  if ($t.Length -eq 0) { return $true }
  $p = 0; [void][TM]::GetWindowThreadProcessId($h, [ref]$p)
  $okProc = (-not $Process) -or ($procIds -contains [int]$p)
  $okTitle = (-not $Title) -or $t.Contains($Title)
  if ($okProc -and $okTitle) { $script:found = $h; return $false }
  $true }, [IntPtr]::Zero) | Out-Null
if (-not $found) { Write-Output "notfound"; exit 1 }
[void][TM]::SetWindowPos($found, [IntPtr](-1), $X, $Y, $W, $H, 0x0040)
[void][TM]::SetForegroundWindow($found)
Write-Output "ok"
