# Start a program on a separate, invisible desktop of this session (no window or focus on the
# user's screen). Prints the PID. Environment is inherited from this PowerShell process.
#   hidden-run.ps1 -Exe C:\path\tether.exe -Args '--connect 123 --password-env X'
# -Show N: STARTF_USESHOWWINDOW with wShowWindow N, e.g. 0 (SW_HIDE) like the service starts `--cm`.
param([Parameter(Mandatory)][string]$Exe, [string]$Args = '', [string]$Desktop = 'tether-direct-test', [int]$Show = -1)
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class HiddenDesk {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct STARTUPINFO { public int cb; public string lpReserved; public string lpDesktop; public string lpTitle;
    public int dwX, dwY, dwXSize, dwYSize, dwXCountChars, dwYCountChars, dwFillAttribute, dwFlags; public short wShowWindow, cbReserved2;
    public IntPtr lpReserved2, hStdInput, hStdOutput, hStdError; }
  [StructLayout(LayoutKind.Sequential)]
  public struct PROCESS_INFORMATION { public IntPtr hProcess, hThread; public int dwProcessId, dwThreadId; }
  [DllImport("user32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern IntPtr CreateDesktop(string name, IntPtr dev, IntPtr mode, int flags, uint access, IntPtr sa);
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern bool CreateProcess(string app, string cmd, IntPtr pa, IntPtr ta, bool inherit, int flags, IntPtr env, string dir, ref STARTUPINFO si, out PROCESS_INFORMATION pi);
  public static int Run(string desk, string exe, string args, int show) {
    IntPtr d = CreateDesktop(desk, IntPtr.Zero, IntPtr.Zero, 0, 0x10000000u, IntPtr.Zero); // GENERIC_ALL
    if (d == IntPtr.Zero) throw new Exception("CreateDesktop " + Marshal.GetLastWin32Error());
    var si = new STARTUPINFO(); si.cb = Marshal.SizeOf(si); si.lpDesktop = desk;
    if (show >= 0) { si.dwFlags = 1; si.wShowWindow = (short)show; } // STARTF_USESHOWWINDOW
    PROCESS_INFORMATION pi;
    if (!CreateProcess(exe, "\"" + exe + "\" " + args, IntPtr.Zero, IntPtr.Zero, false, 0, IntPtr.Zero, null, ref si, out pi))
      throw new Exception("CreateProcess " + Marshal.GetLastWin32Error());
    return pi.dwProcessId; // desktop handle stays open with this process until it exits
  }
}
'@
[HiddenDesk]::Run($Desktop, $Exe, $Args, $Show)
