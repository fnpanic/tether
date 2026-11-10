# Elevated broker for unattended VM tests (started once by the user, one UAC prompt).
# Runs ONLY the Hyper-V helper scripts C:\vms\vm-*.ps1, on request, and nothing else.
#   request : C:\vms\broker\queue\<id>.json  {"script":"vm-qs-setup.ps1","args":["-Qs","C:\\path\\TetherQS.exe"]}
#   result  : C:\vms\broker\done\<id>.json   {"script","exit","output","started","ended"}
# Stops after $Hours, or when C:\vms\broker\stop exists. Every run is logged to broker.log.
#
# Whoever can write a script this broker runs is an administrator. So it refuses to start, and to
# run a script, unless the file and its folder (this file, C:\vms\broker, C:\vms\vm-*.ps1, C:\vms)
# are owned by and writable for Administrators/SYSTEM/TrustedInstaller only. Make it so once, in an
# elevated cmd.exe (S-1-5-32-544 Administrators, S-1-5-18 SYSTEM, S-1-5-32-545 Users, S-1-5-11
# Authenticated Users). After it, files directly in C:\vms (and C:\vms\broker\stop) need an elevated
# process to create or change; the subfolders that exist now stay writable, queue included:
#   for /d %d in (C:\vms\*) do icacls "%d" /grant "*S-1-5-11:(OI)(CI)M"
#   icacls C:\vms /setowner "*S-1-5-32-544"
#   icacls C:\vms /inheritance:r /grant:r "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
#   icacls C:\vms\vm-*.ps1 /setowner "*S-1-5-32-544"
#   icacls C:\vms\vm-*.ps1 /reset
#   icacls C:\vms\broker /setowner "*S-1-5-32-544"
#   icacls C:\vms\broker /inheritance:r /grant:r "*S-1-5-32-544:(OI)(CI)F" "*S-1-5-18:(OI)(CI)F" "*S-1-5-32-545:(OI)(CI)RX"
#   icacls C:\vms\broker /remove:g "*S-1-5-11"
#   icacls C:\vms\broker\* /setowner "*S-1-5-32-544"
#   icacls C:\vms\broker\* /reset
#   icacls C:\vms\broker\queue /grant "*S-1-5-11:(OI)(CI)M"
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
# Why $path (file or folder) can be changed by someone who is not an administrator; nothing if it cannot.
function Unsafe($path, $ok = @('S-1-5-32-544', 'S-1-5-18', 'S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464')) {
  $sid = [Security.Principal.SecurityIdentifier]
  # 0x50000000: GENERIC_WRITE and GENERIC_ALL, as inherited rules can carry them.
  $write = [int][Security.AccessControl.FileSystemRights]'Write, Delete, DeleteSubdirectoriesAndFiles, ChangePermissions, TakeOwnership' -bor 0x50000000
  $acl = Get-Acl -LiteralPath $path -ErrorAction Stop
  $owner = $acl.GetOwner($sid).Value
  if ($ok -notcontains $owner) { return "$path is owned by $owner (an owner can change the permissions)" }
  foreach ($a in $acl.GetAccessRules($true, $true, $sid)) {
    # Inherit-only rules do not apply to $path itself.
    if ($a.AccessControlType -eq 'Allow' -and $ok -notcontains $a.IdentityReference.Value -and
      -not ($a.PropagationFlags -band [Security.AccessControl.PropagationFlags]::InheritOnly) -and ([int]$a.FileSystemRights -band $write)) {
      return "$path is writable by $($a.IdentityReference.Value)"
    }
  }
}
$admin = ([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent()).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
Log "broker started (admin=$admin, until $until)"
if (-not $admin) { Log 'not elevated: exiting'; exit 1 }
$bad = @($PSCommandPath, $PSScriptRoot | ForEach-Object { Unsafe $_ })
if ($bad) { Log "refused to start: $($bad -join '; ') (icacls commands: head of broker.ps1)"; exit 1 }
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
      $bad = @('C:\vms', (Join-Path 'C:\vms' $name) | ForEach-Object { Unsafe $_ })
      if ($bad) { throw "refused: $($bad -join '; ') (icacls commands: head of broker.ps1)" }
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
