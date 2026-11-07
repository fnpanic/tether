# Tether relay on Kubernetes

Helm chart: `deploy/helm/tether-relay`. Same design as [relay-ha.md](relay-ha.md), with each pod
acting as a node:

```
client ──wss──► LB / ingress (TLS) ──► proxy (Caddy, Deployment ×2)
                                         /ws/id/<n>    → Service <rel>-hbbs-<n>  → pod hbbs-<n>
                                         /ws/id        → hbbs-0 (single-server clients)
                                         /ws/relay/<n> → Service <rel>-hbbr-<n>  → pod hbbr-<n>
                                         /api, /q, /download, / → api pod's Caddy :8081
                                           → tether-api 127.0.0.1:21120 (optional, PVC)
hbbs-<n> -r "<publicUrl>/ws/relay/<i>|http://<rel>-hbbr-<i>:21121/readyz", for every i
```

**Why per-pod addressing.** hbbr pairs the two halves of a session only when both connect to the
same process. A host's RelayResponse has to reach the hbbs process that holds the viewer's
connection. Load balancing either one breaks sessions. So every pod is a separate server with a
stable URL:
- StatefulSets give stable ordinals.
- Each ordinal has its own Service with `publishNotReadyAddresses: true`, so a draining pod stays
  reachable for its open sessions and half-made pairs.
- The proxy maps the path ordinal to that Service. The mapping is a regex placeholder, so scaling
  never changes or restarts the proxy.

hbbs hands out full URLs (`wss://relay.example.com/ws/relay/1`). The client already uses
`ws(s)://` relay strings as they are (`check_port`/`check_ws`), and hosts reply to the hbbs URL
they registered with. For the client side see "Client requirements" below.

## Install

```bash
# Image: static musl binaries -> distroless (≈8 MB)
wsl -d Ubuntu-22.04 -e bash -lc 'bash scripts/build-relay.sh'        # binaries in ~/ha-build/bin
docker build -f server/Dockerfile.static -t <registry>/tether-relay:<tag> <dir with the binaries>

kubectl create namespace tether
kubectl -n tether create secret generic tether-relay-key --from-file=id_ed25519=/var/lib/tether/id_ed25519
helm -n tether install tether-relay deploy/helm/tether-relay \
  --set image.repository=<registry>/tether-relay --set image.tag=<tag> \
  --set publicUrl=wss://relay.example.com --set key.existingSecret=tether-relay-key \
  --set proxy.service.type=LoadBalancer --set proxy.tls.secretName=relay-example-com-tls \
  --set api.enabled=true --set api.adminPubKey=<Tether Manage admin public key>
```

Use the production key pair (the same as on the VMs) or clients will not trust the server.

## Values

| Key | Default | Meaning |
| --- | --- | --- |
| `publicUrl` | `wss://relay.example.com` | Base URL clients reach the proxy at; relay URLs handed out are `<publicUrl>/ws/relay/<n>`. |
| `key.existingSecret` / `key.privateKey` | – | Secret with key `id_ed25519`, or the key itself. Required. |
| `key.accept` | – | More public keys hbbs and hbbr accept (`KEYS_ACCEPT`, comma-separated) during a server key rotation ([key-rotation.md](key-rotation.md)). |
| `hbbs.replicas` | 2 | Independent rendezvous servers at `/ws/id/<n>`. |
| `hbbs.drainSeconds` | 2 | Not-ready time before a terminating hbbs exits. |
| `hbbs.extraRelays` | `[]` | Relays outside the cluster, `<address>[\|<health URL>]` (e.g. the VMs). |
| `hbbs.persistence.*` | on, 256Mi | Peer DB (ID → key binding) per pod. |
| `hbbr.replicas` | 2 | Relay pods at `/ws/relay/<n>`; ≥ 2 so one can drain. |
| `hbbr.drainSeconds` | 3600 | Drain window. `terminationGracePeriodSeconds` = drain + 30. |
| `hbbr.env` | `{}` | `TOTAL_BANDWIDTH`, `SINGLE_BANDWIDTH`, `LIMIT_SPEED` (Mb/s). |
| `proxy.enabled` | true | Caddy front end. Off: use your own ingress (`examples/ingress-nginx.yaml`, timeouts 3600 s). |
| `proxy.service.*` | ClusterIP :80 | `LoadBalancer`/`NodePort`. TLS on `tlsPort` when `proxy.tls.secretName` is set (e.g. cert-manager). |
| `api.enabled` | false | tether-api + file server sidecar on a PVC (`api.persistence.size` 5Gi; uploads need 2 GiB free). |
| `podDisruptionBudget.maxUnavailable` | 1 | For hbbs and hbbr (node drains evict one pod at a time). |
| `*.resources` | 50m/32Mi requests, 1 CPU/256Mi limits | Sized for small nodes; for 10 000 devices / 3 000 sessions use `examples/scale-10k.yaml` (hbbs 768Mi, proxy 3Gi, 4 hbbr; [scaling.md](scaling.md)). |

Pods run as non-root (65532) with a read-only root filesystem, all capabilities dropped (Caddy
keeps `NET_BIND_SERVICE`, which its binary needs to start) and RuntimeDefault seccomp.
Probes: `/readyz` (readiness, every 2 s) and `/healthz` (liveness) on the health ports
21121/21122. Pods prefer to spread over nodes (soft anti-affinity).

The WebSocket ports (hbbs 21118, hbbr 21119) listen on 127.0.0.1 by default (`WS_BIND`), because
hbbs and hbbr take the client IP from `X-Forwarded-For`/`X-Real-IP`. The proxy runs in its own pod
here, so the chart sets `WS_BIND=0.0.0.0`: keep these ports on the cluster network (ClusterIP
Services, a NetworkPolicy if other workloads share the cluster), never on a LoadBalancer.

tether-api trusts `X-Forwarded-For` only from a loopback peer (its per-IP heartbeat, audit and
`/q/` limits). So every API request goes through the Caddy in the api pod (`<rel>-api:8081`),
which keeps the proxy pods' `X-Forwarded-For` (trusted: private ranges) and connects to tether-api
over loopback; the Service has no tether-api port. Before, the proxy sent `/api/*` to tether-api
directly and every client counted as the proxy pod's IP: 30 new devices per minute for the whole
cluster. A bring-your-own ingress points `/api` and `/q` at port 8081 too
(`examples/ingress-nginx.yaml`).

## Rollout procedure

- **hbbr** (`helm upgrade` or `kubectl -n tether rollout restart sts/tether-relay-hbbr`): one pod
  at a time, highest ordinal first. The terminating pod goes not-ready at once; every hbbs stops
  handing it out within 3 s. It keeps its sessions until they end or `drainSeconds` pass, then
  the next pod follows. A full rollout takes at most `replicas × drainSeconds`. To speed it up,
  delete a pod with a shorter `--grace-period`; its remaining sessions re-pair in tens of ms.
- **hbbs**: the rolling restart takes seconds. Relayed sessions do not depend on hbbs. Hosts stay
  reachable through the other hbbs pods and re-register.
- **proxy**: changes only with the Caddy version or its config. A proxy rollout cuts the
  WebSockets passing through the old pods; clients re-pair (measured ≤ 40 ms). Do it rarely.
- Scaling hbbr changes hbbs's relay list, so hbbs rolls; that is harmless (see above).
- Node maintenance: `kubectl drain` respects the PDBs (one hbbs and one hbbr pod at a time) and the
  pods' grace periods.

Silent node death: when a node dies without closing its connections, the relays on it are
re-paired through others as fast as the client detects the silence (measured 1.02 s with 1 s
detection). A **proxy** pod on the dead node is the weak spot. kube-proxy keeps routing new
connections to it until the node is NotReady and the endpoint is removed (about 40 s by default).
Clients behind a cloud LoadBalancer should use `externalTrafficPolicy: Local` with LB health
checks of a few seconds, so traffic only reaches nodes that are up. Keep `proxy.replicas` ≥ 2 on
different nodes.

## Measured (kind)

`tools/bench/results/k8s.json`, 2026-11-03: kind v0.33.0 (Kubernetes 1.37), 1 control plane and
2 workers on Docker Desktop. Chart values `deploy/kind/values-kind.yaml`: hbbs ×2, hbbr ×3
(drainSeconds 90), proxy ×2, api ×1.

The harness (`relay-harness`, a pod in the cluster) does what Tether clients do:
- hosts register over WebSocket with both hbbs pods (WS ping liveness, 2.5 s);
- the viewer races its PunchHoleRequest over both;
- both sides pair on the relay hbbs names, and the host streams 1 KiB frames every 20 ms;
- on a drop, or 1 s of silence, the viewer asks again and both re-pair.

Gap = time from the last frame on the old relay to the first frame on the new one, detection
included.

| Test | Sessions | Reconnects | Max gap |
| --- | --- | --- | --- |
| a1 hbbr `rollout restart` (57 s) during churn of 30 s sessions | 30 | **0** | 0 |
| a2 hbbs `rollout restart` (7 s) during churn of 20 s sessions | 30 | **0** | 0 |
| a3 proxy rollout during live sessions | 6 | 3 | **40 ms** |
| b1 `delete pod hbbr-1 --grace-period=0 --force` with live streams | 6 | 2 | **39 ms** |
| b2 `delete pod hbbr-2 --grace-period=15` (session outlives the grace) | 6 | 2 | **20 ms** (not-ready at once, kept streaming 15 s) |
| b3a worker node frozen (`docker pause`; hbbr-0, hbbr-1, hbbs-0), proxies on the other node | 6 | 5 | **1.02 s** (1 s detection + ~20 ms) |
| b3 same, with one proxy pod on the frozen node | 6 | 4 | 43.7 s (stale kube-proxy endpoint, see above) |

Relay latency inside the cluster: p50 0.3 ms. Within one connection, frames never stopped for
more than 73 ms during any rollout.

Reproduce:

```bash
kind.exe create cluster --config deploy/kind/kind-config.yaml        # proxy at http://localhost:18080
bash deploy/kind/rebuild-image.sh ha4                                # WSL build + image + kind load
kubectl create namespace tether
helm -n tether install tether-relay deploy/helm/tether-relay -f deploy/kind/values-kind.yaml --set image.tag=ha4
bash deploy/kind/harness.sh a1 sessions=6 seconds=150 session_seconds=30 &&
  sleep 10 && kubectl -n tether rollout restart sts/tether-relay-hbbr
kubectl -n tether logs a1        # JSON report once the pod has finished
```

## Client requirements (Tether client)

1. A rendezvous server entry can be a full URL (`wss://relay.example.com/ws/id/1`). It is used
   as-is, and the host sends its RelayResponse to the same entry.
2. Relay strings from hbbs can be `wss://…/ws/relay/<n>`; do not rewrite them. The route racer's
   port-80 fallback should treat a URL entry's host as "own".
3. The `relay-server` option must stay empty, otherwise the client ignores hbbs's relay choice.
   Every URL must be `wss://`: Tether refuses plain `ws://` to anything but loopback.
4. Liveness: WebSocket ping about every 1 s on rendezvous and relay connections, dead after
   about 2 s. hbbs and hbbr answer pings.
5. On relay drop, or when a relay closes right after `RequestRelay` (it is draining), ask again
   at once (race all hbbs) and re-pair.
