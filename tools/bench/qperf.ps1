# Quality bench (docs/performance.md, "Image quality vs AnyDesk"): per-second network bytes and CPU in the VM. C:\bench\perf\qperf-sec.txt = "<seconds> [<start second of minute>]".
# Runs over PowerShell Direct (not in the console session). Prints one JSON line.
$cfg = (Get-Content C:\bench\perf\qperf-sec.txt -Raw).Trim() -split '\s+'
$sec = [int]$cfg[0]
$names = 'TetherQS', 'AnyDesk', 'msedge'
$cpuc = New-Object System.Diagnostics.PerformanceCounter('Processor', '% Processor Time', '_Total'); $cpuc.NextValue() | Out-Null
function snap { $n = Get-NetAdapterStatistics | Measure-Object -Property SentBytes, ReceivedBytes -Sum; $p = @{}; foreach ($k in $names) { $p[$k] = [double](Get-Process $k -ErrorAction SilentlyContinue | Measure-Object -Property CPU -Sum).Sum }; [pscustomobject]@{ t = Get-Date; sent = $n[0].Sum; recv = $n[1].Sum; p = $p } }
snap | Out-Null  # warm up: the first Get-NetAdapterStatistics loads its module (seconds)
if ($cfg.Count -gt 1) { $at = [int]$cfg[1]; while ((Get-Date).Second -ne $at) { Start-Sleep -Milliseconds 50 } }
$first = snap; $prev = $first; $series = @(); $cpus = @()
$start = $first.t.ToString('HH:mm:ss.fff')
for ($i = 0; $i -lt $sec; $i++) {
  Start-Sleep -Milliseconds (1000 - ((Get-Date) - $prev.t).TotalMilliseconds % 1000)
  $cur = snap; $cpus += [math]::Round($cpuc.NextValue(), 1)
  $series += [math]::Round(($cur.sent - $prev.sent) / 1024, 1)
  $prev = $cur
}
$dt = ($prev.t - $first.t).TotalSeconds
$pc = @{}; foreach ($k in $names) { $pc[$k] = [math]::Round((($prev.p[$k] - $first.p[$k]) / $dt) * 100 / [Environment]::ProcessorCount, 1) }
[pscustomobject]@{ start = $start; seconds = [math]::Round($dt, 1); sent_kB = [math]::Round(($prev.sent - $first.sent) / 1024, 1); sent_kbps = [math]::Round(($prev.sent - $first.sent) * 8 / 1000 / $dt, 1); recv_kbps = [math]::Round(($prev.recv - $first.recv) * 8 / 1000 / $dt, 1); cpu_total = [math]::Round(($cpus | Measure-Object -Average).Average, 1); cpu_proc = $pc; sent_kB_per_s = $series; cpu_per_s = $cpus } | ConvertTo-Json -Compress
