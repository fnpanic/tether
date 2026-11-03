"""mitmproxy add-on that behaves like a strict corporate proxy / zero-trust gateway (Zscaler-style).

- Only CONNECT to port 443 is allowed; anything else gets 403 (like a 443-only egress policy).
- TLS is intercepted and re-signed with the proxy's CA (TLS inspection), as Zscaler does.
- Every tunnel and WebSocket frame is logged to show what the inspecting proxy can see.

    mitmdump -s corp_proxy.py --listen-host 0.0.0.0 --listen-port 8080
"""

import json
import os
import time

from mitmproxy import ctx, http

LOG = "corp_proxy_log.jsonl"
# Allowed CONNECT ports, e.g. CORP_PORTS=443,80 (default: 443 only).
PORTS = {int(p) for p in os.environ.get("CORP_PORTS", "443").split(",")}


def _log(**kv):
    kv["t"] = round(time.time(), 3)
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(json.dumps(kv) + "\n")


def http_connect(flow: http.HTTPFlow):
    host, port = flow.request.host, flow.request.port
    if port not in PORTS:
        _log(event="blocked", host=host, port=port, reason=f"allowed ports {sorted(PORTS)}")
        flow.response = http.Response.make(403, b"Blocked by corporate policy")
        return
    _log(event="connect", host=host, port=port, client=flow.client_conn.peername[0])


def tls_failed_client(data):
    _log(event="tls_rejected_by_client", host=data.context.server.address[0] if data.context.server.address else None)


def request(flow: http.HTTPFlow):
    if flow.request.scheme == "http" and flow.request.port != 443:
        _log(event="blocked", host=flow.request.host, port=flow.request.port, reason="plain http")
        flow.response = http.Response.make(403, b"Blocked by corporate policy")


def websocket_start(flow: http.HTTPFlow):
    _log(event="websocket_open", host=flow.request.host, path=flow.request.path, tls_inspected=flow.client_conn.tls_established)


def websocket_message(flow: http.HTTPFlow):
    msg = flow.websocket.messages[-1]
    data = msg.content or b""
    # What the inspecting proxy can see of a Tether frame: size and a short prefix only.
    printable = sum(32 <= b < 127 for b in data[:64]) / max(1, min(64, len(data)))
    _log(
        event="ws_frame",
        path=flow.request.path,
        from_client=msg.from_client,
        bytes=len(data),
        printable_ratio=round(printable, 2),
    )


def websocket_end(flow: http.HTTPFlow):
    _log(event="websocket_close", path=flow.request.path, frames=len(flow.websocket.messages))
