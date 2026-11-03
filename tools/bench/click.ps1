param([int]$X, [int]$Y)
# Bench helper: move the cursor to logical (X, Y) and left-click via SendInput.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public class CK {
  [DllImport("user32.dll")] public static extern bool SetCursorPos(int x, int y);
  [DllImport("user32.dll")] public static extern void mouse_event(uint f, uint dx, uint dy, uint d, UIntPtr e);
}
"@
[void][CK]::SetCursorPos($X, $Y)
Start-Sleep -Milliseconds 50
[CK]::mouse_event(0x0002, 0, 0, 0, [UIntPtr]::Zero)
[CK]::mouse_event(0x0004, 0, 0, 0, [UIntPtr]::Zero)
"clicked"
