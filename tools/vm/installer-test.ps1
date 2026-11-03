# Runs in the VM's interactive session (as the logged-on admin user, via interactive.ps1).
# Installer test for "the installer asks to close Tether": with Tether running, run the new installer
# INTERACTIVELY (no /S, no /P), press Enter through its pages, and watch every window text through UI
# Automation for the bundler's "<app> is running ... kill it" box. Result: C:\bench\perf\installer.log
param([int]$Seconds = 180)
$log = 'C:\bench\perf\installer.log'
"start $(Get-Date -Format s)" | Set-Content $log
function L($m) { "$(Get-Date -Format HH:mm:ss) $m" | Add-Content $log }
Add-Type -AssemblyName UIAutomationClient, UIAutomationTypes, System.Windows.Forms
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class W32 {
  [DllImport("user32.dll")] public static extern IntPtr SendMessage(IntPtr h, uint msg, IntPtr w, IntPtr l);
}
"@

$exe = 'C:\Program Files\Tether\tether.exe'
L "installed before: $((Get-Item $exe -ErrorAction SilentlyContinue).VersionInfo.ProductVersion)"
if (-not (Get-Process tether -ErrorAction SilentlyContinue)) { Start-Process $exe; Start-Sleep 8 }
$before = @(Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | ForEach-Object { "$($_.ProcessId):$($_.CommandLine)" })
L "tether processes before: $($before.Count) -> $($before -join ' | ')"

Get-Process Tether-setup, uninstall -ErrorAction SilentlyContinue | Stop-Process -Force
Start-Sleep 1
Copy-Item C:\bench\perf\push.bin C:\bench\perf\Tether-setup.exe -Force
$setup = Start-Process C:\bench\perf\Tether-setup.exe -PassThru
L "installer pid $($setup.Id)"
$prompt = $false
$texts = @{}
$end = (Get-Date).AddSeconds($Seconds)
$root = [System.Windows.Automation.AutomationElement]::RootElement
$any = [System.Windows.Automation.Condition]::TrueCondition
$alive = { @(Get-Process Tether-setup, uninstall, Un_* -ErrorAction SilentlyContinue).Count -gt 0 }
# The installer relaunches itself (the first process exits) and on upgrades runs the old uninstaller:
# follow every installer/uninstaller process by name until all are gone.
while ((& $alive) -and (Get-Date) -lt $end) {
  foreach ($w in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $any)) {
    try {
      $names = @($w.Current.Name) + @($w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $any) | ForEach-Object { $_.Current.Name })
      $t = ($names | Where-Object { $_ }) -join ' / '
      if ($t -match 'is running|kill it|close it|wird ausgef|läuft') { $prompt = $true; L "PROMPT SEEN: $t" }
      if (@(Get-Process Tether-setup, uninstall -ErrorAction SilentlyContinue | ForEach-Object Id) -contains $w.Current.ProcessId -and -not $texts.ContainsKey($t)) { $texts[$t] = 1; L "installer window: $($t.Substring(0, [Math]::Min(200, $t.Length)))" }
    } catch {}
  }
  # Advance the installer's pages by invoking its buttons (UI Automation; only inside this test VM).
  # Never "OK": that would dismiss a "<app> is running" box before it is recorded.
  if (-not $dumped) {
    foreach ($w in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $any)) {
      if (@(Get-Process Tether-setup -ErrorAction SilentlyContinue | ForEach-Object Id) -notcontains $w.Current.ProcessId) { continue }
      $dumped = $true
      foreach ($e in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $any)) {
        L ("element {0} '{1}' enabled={2} patterns={3}" -f $e.Current.ControlType.ProgrammaticName, $e.Current.Name, $e.Current.IsEnabled, (($e.GetSupportedPatterns() | ForEach-Object { $_.ProgrammaticName }) -join ','))
      }
    }
  }
  if (-not $prompt) {
    # NSIS buttons expose no UIA patterns: click them with BM_CLICK on their window handle.
    # Elements vanish when a page changes: an exception here just means "look again next round".
    # The setup window and, on upgrades, the old version's uninstaller it starts.
    $pids = @(Get-Process Tether-setup, uninstall, Un_* -ErrorAction SilentlyContinue | ForEach-Object Id)
    try { foreach ($w in $root.FindAll([System.Windows.Automation.TreeScope]::Children, $any)) {
      if ($pids -notcontains $w.Current.ProcessId) { continue }
      foreach ($b in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants, $any)) {
        $n = $b.Current.Name -replace '&', ''
        $h = [IntPtr]$b.Current.NativeWindowHandle
        if ($b.Current.IsEnabled -and $h -ne [IntPtr]::Zero -and $n -match '^(Next >|Install|Finish|Uninstall|Close)$') {
          [void][W32]::SendMessage($h, 0x00F5, [IntPtr]::Zero, [IntPtr]::Zero)
          L "clicked '$n'"
          Start-Sleep -Milliseconds 1200
          break
        }
      }
    } } catch { L "ui changed: $($_.Exception.GetType().Name)" }
  }
  Start-Sleep -Milliseconds 500
}
L "installer processes gone: $(-not (& $alive))"
Start-Sleep 10
$after = @(Get-CimInstance Win32_Process -Filter "Name='tether.exe'" | ForEach-Object { "$($_.ProcessId):$($_.CommandLine)" })
L "installed after: $((Get-Item $exe).VersionInfo.ProductVersion)"
L "tether processes after: $($after.Count) -> $($after -join ' | ')"
L "RESULT prompt_seen=$prompt"
