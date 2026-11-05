#!/bin/bash
# usage (WSL, as root): [MODES="tcp turn"] [ENVS=RELAY_CC=bbr] [EXP="VAR=1 ..."] [BIN=...] netem-bench.sh <delay_ms_per_hop> <loss_pct> <secs> [cc]
# Own network namespace "tb" with netem on its loopback (MTU 1500, no offloads), so nothing else is affected.
set -u
B=${BIN:-/home/$(ls /home | head -1)/turn-bench-target/release/turn-bench}
ip netns add tb 2>/dev/null
ip -n tb link set lo up
# Real-path packet sizes: loopback MTU 64K and GSO would carry a whole frame in one netem packet.
ip -n tb link set lo mtu 1500
ip netns exec tb ethtool -K lo tso off gso off gro off 2>/dev/null || echo "ethtool missing"
ip -n tb link add d0 type dummy 2>/dev/null; ip -n tb addr add 10.99.0.1/24 dev d0 2>/dev/null; ip -n tb link set d0 up
ip netns exec tb sysctl -qw net.ipv4.tcp_congestion_control=${4:-cubic}
ip netns exec tb tc qdisc del dev lo root 2>/dev/null
if [ "$1" != 0 ] || [ "$2" != 0 ]; then
  ip netns exec tb tc qdisc add dev lo root netem delay ${1}ms loss ${2}% limit 100000
fi
echo "# hop delay ${1}ms loss ${2}% cc ${4:-cubic}"
for m in ${MODES:-tcp turn}; do ip netns exec tb env ${ENVS:-X=1} timeout 120 $B $m $3 2>/dev/null | tail -1; done
if [ -n "${EXP:-}" ]; then echo -n "exp($EXP) "; ip netns exec tb env $EXP timeout 120 $B turn $3 2>/dev/null | tail -1; fi
ip netns exec tb tc qdisc del dev lo root 2>/dev/null
