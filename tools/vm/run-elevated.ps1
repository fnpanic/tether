# Client for the elevated broker: queue one C:\vms\vm-*.ps1 script, wait for it, print its output.
#   powershell -NoProfile -ExecutionPolicy Bypass -File C:\vms\broker\run-elevated.ps1 vm-qs-setup.ps1 -Qs C:\path\TetherQS.exe
# The first argument is the script name; every other argument is passed to it unchanged (no param
# block here on purpose, so arguments like -Script are not swallowed by this client).
# Exit code = the script's exit code (-1 if refused; 2 if the broker is not running; 3 on timeout).
# Env TETHER_BROKER_TIMEOUT (seconds, default 1800).
if ($args.Count -lt 1) { Write-Output 'usage: run-elevated.ps1 <vm-script.ps1> [args...]'; exit 2 }
$Script = [string]$args[0]
$Rest = @($args | Select-Object -Skip 1 | ForEach-Object { [string]$_ })
$TimeoutSec = if ($env:TETHER_BROKER_TIMEOUT) { [int]$env:TETHER_BROKER_TIMEOUT } else { 1800 }
$root = 'C:\vms\broker'
if (-not (Test-Path (Join-Path $root 'running'))) { Write-Output 'broker not running (start C:\vms\broker\start-broker.cmd as admin)'; exit 2 }
$id = [guid]::NewGuid().ToString('N')
$tmp = Join-Path $root "queue\$id.tmp"
@{ script = $Script; args = $Rest } | ConvertTo-Json -Depth 3 | Set-Content $tmp
Rename-Item $tmp "$id.json"
$doneFile = Join-Path $root "done\$id.json"
$end = (Get-Date).AddSeconds($TimeoutSec)
while (-not (Test-Path $doneFile)) {
  if ((Get-Date) -gt $end) { Write-Output "timeout waiting for $Script"; exit 3 }
  Start-Sleep -Milliseconds 500
}
Start-Sleep -Milliseconds 200
$r = Get-Content $doneFile -Raw | ConvertFrom-Json
Write-Output $r.output
exit [int]$r.exit
