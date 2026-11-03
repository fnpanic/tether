# Client for the elevated broker: queue one C:\vms\vm-*.ps1 script, wait for it, print its output.
#   powershell -NoProfile -ExecutionPolicy Bypass -File C:\vms\broker\run-elevated.ps1 vm-qs-setup.ps1 -Qs C:\path\TetherQS.exe
# Exit code = the script's exit code (-1 if refused; 2 if the broker is not running; 3 on timeout).
param([Parameter(Mandatory = $true, Position = 0)][string]$Script,
      [Parameter(ValueFromRemainingArguments = $true)][string[]]$Rest,
      [int]$TimeoutSec = 1800)
$root = 'C:\vms\broker'
if (-not (Test-Path (Join-Path $root 'running'))) { Write-Output 'broker not running (start C:\vms\broker\start-broker.cmd as admin)'; exit 2 }
$id = [guid]::NewGuid().ToString('N')
$tmp = Join-Path $root "queue\$id.tmp"
@{ script = $Script; args = @($Rest) } | ConvertTo-Json -Depth 3 | Set-Content $tmp
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
