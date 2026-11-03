# Elevated broker for unattended VM tests (started once by the user, one UAC prompt).
# Runs ONLY the Hyper-V helper scripts C:\vms\vm-*.ps1, on request, and nothing else.
#   request : C:\vms\broker\queue\<id>.json  {"script":"vm-qs-setup.ps1","args":["-Qs","C:\\path\\TetherQS.exe"]}
#   result  : C:\vms\broker\done\<id>.json   {"script","exit","output","started","ended"}
# Stops after $Hours, or when C:\vms\broker\stop exists. Every run is logged to broker.log.
param([int]$Hours = 16)
$ErrorActionPreference = 'Continue'
$root = 'C:\vms\broker'
$queue = Join-Path $root 'queue'
$done = Join-Path $root 'done'
$log = Join-Path $root 'broker.log'
New-Item -ItemType Directory -Force $queue, $done | Out-Null
Remove-Item (Join-Path $root 'stop') -ErrorAction SilentlyContinue
$until = (Get-Date).AddHours($Hours)
function Log($m) { "$(Get-Date -Format s)  $m" | Add-Content $log }
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Log "broker started (admin=$admin, until $until)"
if (-not $admin) { Log 'not elevated: exiting'; exit 1 }
Set-Content (Join-Path $root 'running') "$PID $until"

while ((Get-Date) -lt $until -and -not (Test-Path (Join-Path $root 'stop'))) {
  foreach ($req in Get-ChildItem $queue -Filter *.json -ErrorAction SilentlyContinue) {
    $id = $req.BaseName
    $result = @{ script = ''; exit = -1; output = ''; started = (Get-Date -Format s) }
    try {
      $r = Get-Content $req.FullName -Raw | ConvertFrom-Json
      Remove-Item $req.FullName -Force
      $name = [string]$r.script
      # Allow-list: a plain file name vm-*.ps1 that exists directly in C:\vms (no paths, no ..).
      if ($name -notmatch '^vm-[A-Za-z0-9_-]+\.ps1$' -or -not (Test-Path (Join-Path 'C:\vms' $name) -PathType Leaf)) {
        throw "refused: '$name' is not an allowed C:\vms\vm-*.ps1 script"
      }
      $argList = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', (Join-Path 'C:\vms' $name))
      foreach ($a in @($r.args)) { if ($null -ne $a) { $argList += [string]$a } }
      $result.script = $name
      Log "run $id : $name $(@($r.args) -join ' ')"
      $out = & powershell.exe @argList 2>&1 | Out-String
      $result.exit = $LASTEXITCODE
      $result.output = if ($out.Length -gt 20000) { $out.Substring($out.Length - 20000) } else { $out }
    } catch {
      $result.output = "$($_.Exception.Message)"
      Log "error $id : $($result.output)"
    }
    $result.ended = (Get-Date -Format s)
    $result | ConvertTo-Json -Depth 3 | Set-Content (Join-Path $done "$id.json")
    Log "done $id : exit $($result.exit)"
  }
  Start-Sleep -Milliseconds 500
}
Remove-Item (Join-Path $root 'running') -ErrorAction SilentlyContinue
Log 'broker stopped'
