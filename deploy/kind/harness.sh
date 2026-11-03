#!/usr/bin/env bash
# Start the relay harness as a pod in the kind cluster: [NODE=<node>] deploy/kind/harness.sh <name> [key=value...]
# The JSON report is in `kubectl -n tether logs <name>` when the pod has finished.
set -euo pipefail
NAME="$1"; shift
K="kubectl.exe --context kind-tether-ha -n tether"
IMG="$($K get sts tether-relay-hbbr -o jsonpath='{.spec.template.spec.containers[0].image}')"
extra=()
[ -n "${NODE:-}" ] && extra=(--overrides "{\"apiVersion\":\"v1\",\"spec\":{\"nodeName\":\"$NODE\"}}")
$K delete pod "$NAME" --ignore-not-found >/dev/null
$K run "$NAME" --image="$IMG" --image-pull-policy=Never --restart=Never "${extra[@]}" --command -- \
  relay-harness hbbs=ws://tether-relay-proxy/ws/id/0,ws://tether-relay-proxy/ws/id/1 \
  key=3d94eDolyjozz+/Kjzy8Jz6oKAdITJ0QIchN0h2wgkw= "$@" >/dev/null
$K wait --for=condition=Ready "pod/$NAME" --timeout=60s >/dev/null
