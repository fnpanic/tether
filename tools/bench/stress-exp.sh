#!/bin/bash
# stress-exp.sh <label> <codec> <relay:0|1> <minutes>: fresh viewer to the Mac, set codec/route, sample, log viewer memory.
S=/c/vms/night
B=/c/install/remote-support/tools/bench; L=$1; C=$2; R=$3; M=$4; O=$S/stress/$L
powershell -NoProfile -Command 'Get-CimInstance Win32_Process | Where-Object { $_.Name -eq "tether.exe" -and $_.CommandLine -like "*password-env MACPW*" } | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }; exit 0'
sleep 4
for i in 1 2 3; do o=$(cd $S && TECH='C:\cargo-target\tether-rel\release\tether.exe' PORT=9271 PROF=mac-perf DESK=tether-mac57 timeout 120 node mac56.mjs start 2>&1 | tail -1); echo "$o" | grep -q thomass && break; sleep 4; done
node $B/inv.mjs 9271 session_codec "{\"key\":\"\$KEY\",\"codec\":\"$C\"}"
[ "$R" = 1 ] && { node $B/inv.mjs 9271 session_reconnect '{"key":"$KEY","forceRelay":true}'; sleep 12; }
sleep 5
( for i in $(seq 1 $((M * 6))); do powershell -NoProfile -Command '$p = Get-CimInstance Win32_Process | Where-Object { $_.CommandLine -like "*password-env MACPW*" -and $_.Name -eq "tether.exe" } | Select -First 1; if ($p) { $w = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.ProcessId)" | Measure-Object -Property WorkingSetSize -Sum; "{0} viewer {1} MB, webview {2} MB" -f (Get-Date -f HH:mm:ss), [int]($p.WorkingSetSize/1MB), [int]($w.Sum/1MB) } else { "{0} viewer gone" -f (Get-Date -f HH:mm:ss) }'; sleep 10; done ) > $O-mem.log 2>&1 &
node $B/stress-sampler.mjs --port 9271 --seconds $((M * 60)) --every 250 --out $O.jsonl
wait
python $B/stress-report.py $O.jsonl; tail -3 $O-mem.log
