# Run inside the VM (as bench via PowerShell Direct): start a command in the logged-on console user's
# interactive session through a one-shot scheduled task. Env: PERF_CMD (powershell args), PERF_WAIT.
param($pw)
$user = ((query user 2>$null) -match 'console' | ForEach-Object { ($_ -replace '^\s*>?', '' -split '\s+')[0] }) | Select-Object -First 1
$cmd = (Get-Content C:\bench\perf\cmd.txt -Raw).Trim()
$name = 'perf-' + [guid]::NewGuid().ToString('N').Substring(0, 6)
$a = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument "-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden $cmd"
$p = New-ScheduledTaskPrincipal -UserId $user -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName $name -Action $a -Principal $p -Settings (New-ScheduledTaskSettingsSet -ExecutionTimeLimit (New-TimeSpan -Hours 12)) -Force | Out-Null
Start-ScheduledTask -TaskName $name
"started '$cmd' as $user ($name)"
$wait = 0; if (Test-Path C:\bench\perf\wait.txt) { $wait = [int](Get-Content C:\bench\perf\wait.txt) }
if ($wait -gt 0) {
  $end = (Get-Date).AddSeconds($wait)
  while ((Get-Date) -lt $end -and (Get-ScheduledTask -TaskName $name).State -eq 'Running') { Start-Sleep 1 }
  "state: $((Get-ScheduledTask -TaskName $name).State)"
  Unregister-ScheduledTask -TaskName $name -Confirm:$false -ErrorAction SilentlyContinue
}
Get-Content C:\bench\perf\drag.log -Tail 3 -ErrorAction SilentlyContinue
