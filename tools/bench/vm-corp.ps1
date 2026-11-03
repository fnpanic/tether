# Elevated: turn the benchmark VM into a "corporate" PC and prove Tether still works.
#  - outbound firewall: block everything except the proxy (and DNS)
#  - system proxy (WinINet, per user) -> TLS-inspecting proxy on the host (mitmproxy, Zscaler-style)
#  - proxy CA in LocalMachine\Root, as Zscaler deploys it
#  - new Tether build (443-only policy), reboot, report ID + in-VM network audit
param([string]$Name = 'tether-bench', [string]$Proxy = '172.30.240.1:8080')
Start-Transcript -Path 'C:\vms\vm-corp.log' -Force | Out-Null
try {
  Remove-Item C:\vms\corp-ids.json, C:\vms\corp-audit.json -ErrorAction SilentlyContinue
  # Host: let the VM reach the proxy.
  Remove-NetFirewallRule -DisplayName 'Tether corp proxy' -ErrorAction SilentlyContinue
  New-NetFirewallRule -DisplayName 'Tether corp proxy' -Direction Inbound -Protocol TCP -LocalPort 8080 -RemoteAddress 172.30.240.0/20 -Action Allow | Out-Null

  $cfg = Get-Content 'C:\vms\payload\bench.json' | ConvertFrom-Json
  $cred = New-Object PSCredential('bench', (ConvertTo-SecureString $cfg.password -AsPlainText -Force))
  $s = New-PSSession -VMName $Name -Credential $cred
  Invoke-Command -Session $s -ScriptBlock { Get-Process tether -ErrorAction SilentlyContinue | Stop-Process -Force; Start-Sleep 1 }
  Copy-Item 'C:\vms\payload\tether\tether.exe' -Destination 'C:\Tether\tether.exe' -ToSession $s -Force
  Copy-Item "$env:USERPROFILE\.mitmproxy\mitmproxy-ca-cert.cer" -Destination 'C:\bench\corp-ca.cer' -ToSession $s -Force
  Copy-Item 'C:\install\remote-support\tools\bench\netaudit.ps1' -Destination 'C:\bench\netaudit.ps1' -ToSession $s -Force
  Invoke-Command -Session $s -ArgumentList $Proxy -ScriptBlock {
    param($Proxy)
    $hostIp = $Proxy.Split(':')[0]
    Import-Certificate -FilePath C:\bench\corp-ca.cer -CertStoreLocation Cert:\LocalMachine\Root | Out-Null
    $k = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings'
    Set-ItemProperty $k ProxyEnable 1
    Set-ItemProperty $k ProxyServer $Proxy
    Set-ItemProperty $k ProxyOverride '<local>'
    Get-NetFirewallRule -DisplayName 'Corp *' -ErrorAction SilentlyContinue | Remove-NetFirewallRule
    New-NetFirewallRule -DisplayName 'Corp proxy' -Direction Outbound -Protocol TCP -RemoteAddress $hostIp -RemotePort 8080 -Action Allow | Out-Null
    New-NetFirewallRule -DisplayName 'Corp DNS' -Direction Outbound -Protocol UDP -RemotePort 53 -Action Allow | Out-Null
    Set-NetFirewallProfile -All -DefaultOutboundAction Block
    # Tether's own outbound allow rule from setup would bypass the policy: none exists, but be sure.
    Get-NetFirewallRule -Direction Outbound -ErrorAction SilentlyContinue | Where-Object { $_.DisplayName -like 'Tether*' } | Remove-NetFirewallRule
    "direct 443 blocked: $(-not (Test-NetConnection tether-desk.duckdns.org -Port 443 -WarningAction SilentlyContinue).TcpTestSucceeded)"
    "direct 21116 blocked: $(-not (Test-NetConnection tether-desk.duckdns.org -Port 21116 -WarningAction SilentlyContinue).TcpTestSucceeded)"
    Restart-Computer -Force
  }
  Remove-PSSession $s -ErrorAction SilentlyContinue

  # Wait for reboot + auto-logon + Tether registration.
  Start-Sleep 30
  $s = $null
  for ($i = 0; $i -lt 60 -and -not $s; $i++) {
    try { $s = New-PSSession -VMName $Name -Credential $cred -ErrorAction Stop } catch { Start-Sleep 5 }
  }
  $id = $null
  for ($i = 0; $i -lt 40 -and -not $id; $i++) {
    $id = Invoke-Command -Session $s -ScriptBlock {
      $f = Get-ChildItem "$env:APPDATA\Tether\config\*.toml" -ErrorAction SilentlyContinue | Where-Object Name -notmatch '2|local' | Select-Object -First 1
      if ($f -and (Get-Process tether -ErrorAction SilentlyContinue)) {
        (Select-String -Path $f.FullName -Pattern "^id = '(\d+)'").Matches | ForEach-Object { $_.Groups[1].Value } | Select-Object -First 1
      }
    }
    if (-not $id) { Start-Sleep 3 }
  }
  @{ tether = "$id"; ip = '172.30.252.30' } | ConvertTo-Json | Set-Content C:\vms\corp-ids.json
  "VM Tether ID: $id"
  # In-VM audit while the host connects as viewer.
  $audit = Invoke-Command -Session $s -ScriptBlock { powershell -NoProfile -ExecutionPolicy Bypass -File C:\bench\netaudit.ps1 -Process tether -Seconds 150 }
  $audit | Set-Content C:\vms\corp-audit.json
  $audit
  Remove-PSSession $s -ErrorAction SilentlyContinue
} catch {
  "FAILED: $($_.Exception.Message)"
} finally {
  Stop-Transcript | Out-Null
}
