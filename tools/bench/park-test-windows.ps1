# Keep test windows off the user's screen: every second, move top-level windows of test builds
# (exe under C:\cargo-target\ or C:\install\.claude\) far outside the visible desktop, without
# activating them. The user's installed Tether (C:\Program Files\Tether) is never touched.
# CDP automation keeps working on parked windows; on-screen capture of them does not.
#   powershell -NoProfile -ExecutionPolicy Bypass -File park-test-windows.ps1 [-Seconds 0]
param([int]$Seconds = 0, [string[]]$Roots = @('C:\cargo-target\', 'C:\install\.claude\'))
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
public static class Park {
  public delegate bool EnumProc(IntPtr h, IntPtr l);
  [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc f, IntPtr l);
  [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
  [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
  [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("user32.dll")] static extern bool SetWindowPos(IntPtr h, IntPtr after, int x, int y, int cx, int cy, uint flags);
  [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
  public static List<object[]> Windows() {
    var list = new List<object[]>();
    EnumWindows((h, l) => {
      if (IsWindowVisible(h)) {
        uint pid; GetWindowThreadProcessId(h, out pid);
        RECT r; GetWindowRect(h, out r);
        list.Add(new object[] { h, (int)pid, r.L, r.T, r.R - r.L, r.B - r.T });
      }
      return true;
    }, IntPtr.Zero);
    return list;
  }
  // SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE
  public static void Move(IntPtr h, int x, int y) { SetWindowPos(h, IntPtr.Zero, x, y, 0, 0, 0x0001 | 0x0004 | 0x0010); }
}
"@
$end = if ($Seconds -gt 0) { (Get-Date).AddSeconds($Seconds) } else { [DateTime]::MaxValue }
$paths = @{}
while ((Get-Date) -lt $end) {
  foreach ($w in [Park]::Windows()) {
    $h, $procId, $x, $y = $w[0], $w[1], $w[2], $w[3]
    if ($x -le -10000) { continue }            # already parked
    if (-not $paths.ContainsKey($procId)) {
      $paths[$procId] = try { (Get-Process -Id $procId -ErrorAction Stop).Path } catch { '' }
    }
    $p = $paths[$procId]
    if ($p -and ($Roots | Where-Object { $p.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) })) {
      [Park]::Move($h, -20000, $y)
    }
  }
  Start-Sleep -Milliseconds 700
}
