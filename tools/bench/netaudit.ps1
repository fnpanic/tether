# Network audit: sample every TCP connection and UDP socket owned by a process name for N seconds,
# then print a JSON summary. Used to prove "only TCP 443 outbound".
#   powershell -File netaudit.ps1 -Process tether -Seconds 60
param([string]$Process = 'tether', [int]$Seconds = 60, [int]$IntervalMs = 200)
$tcp = @{}
$udp = @{}
$deadline = (Get-Date).AddSeconds($Seconds)
$samples = 0
while ((Get-Date) -lt $deadline) {
  $pids = @(Get-Process -Name $Process -ErrorAction SilentlyContinue | ForEach-Object Id)
  if ($pids.Count) {
    Get-NetTCPConnection -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess -and $_.State -ne 'Listen' } | ForEach-Object {
      $key = "$($_.RemoteAddress):$($_.RemotePort)"
      if (-not $tcp.ContainsKey($key)) { $tcp[$key] = "$($_.State)" }
    }
    Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess } | ForEach-Object {
      $key = "LISTEN $($_.LocalAddress):$($_.LocalPort)"
      if (-not $tcp.ContainsKey($key)) { $tcp[$key] = 'Listen' }
    }
    Get-NetUDPEndpoint -ErrorAction SilentlyContinue | Where-Object { $pids -contains $_.OwningProcess } | ForEach-Object {
      $key = "$($_.LocalAddress):$($_.LocalPort)"
      if (-not $udp.ContainsKey($key)) { $udp[$key] = $true }
    }
  }
  $samples++
  Start-Sleep -Milliseconds $IntervalMs
}
$loop = '^(127\.|::1|0\.0\.0\.0|::$)'
$remote = $tcp.Keys | Where-Object { $_ -notlike 'LISTEN*' }
$external = $remote | Where-Object { ($_ -replace ':\d+$', '') -notmatch $loop }
$non443 = $external | Where-Object { $_ -notmatch ':443$' }
[pscustomobject]@{
  samples = $samples
  tcp_external = @($external | Sort-Object -Unique)
  tcp_external_non_443 = @($non443 | Sort-Object -Unique)
  tcp_listen = @($tcp.Keys | Where-Object { $_ -like 'LISTEN*' } | Sort-Object)
  udp_sockets = @($udp.Keys | Sort-Object)
  verdict = if (@($non443).Count -eq 0 -and @($udp.Keys).Count -eq 0) { 'PASS: only TCP 443 outbound, no UDP' } else { 'FAIL' }
} | ConvertTo-Json
