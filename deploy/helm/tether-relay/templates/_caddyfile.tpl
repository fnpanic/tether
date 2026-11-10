{{/* Caddy config of the proxy; does not depend on replica counts. */}}
{{- define "tr.caddyfile" -}}
{{- $fn := include "tr.fullname" . -}}
{{- $trusted := .Values.proxy.trustedProxies -}}
{{- /* The client as the services see it: the peer, or behind trusted proxies the address they report. */ -}}
{{- $ip := ternary "{client_ip}" "{remote_host}" (not (empty $trusted)) -}}
{
  admin off
  auto_https off
  # WebSocket sessions stay open while Caddy shuts down, up to the pod's grace period.
  grace_period 25s
  {{- if $trusted }}
  # proxy.trustedProxies: only from these is X-Forwarded-For believed (the load balancer in front).
  # Strict: the client is the last address a trusted proxy added, not the first in the header
  # (which the client writes itself).
  servers {
    trusted_proxies static {{ join " " $trusted }}
    trusted_proxies_strict
  }
  {{- end }}
}
(routes) {
  # Per-pod routing: both peers of a session must land on the same hbbr pod, a host's
  # replies on the same hbbs pod. The ordinal maps to the per-pod Service; no streaming
  # timeout (sessions last hours), no buffering. X-Real-IP (read by hbbr) is overwritten with
  # the peer Caddy saw; hbbs takes the last X-Forwarded-For hop, which Caddy sets the same way.
  # With proxy.trustedProxies both are the client those proxies report, as one address (a
  # client-sent X-Forwarded-For entry never reaches the services).
  @hbbs path_regexp hbbs ^/ws/id/([0-9]{1,3})$
  handle @hbbs {
    reverse_proxy {{ $fn }}-hbbs-{re.hbbs.1}:21118 {
      header_up X-Real-IP {{ $ip }}
      {{- if $trusted }}
      header_up X-Forwarded-For {client_ip}
      {{- end }}
      flush_interval -1
    }
  }
  handle /ws/id {
    reverse_proxy {{ $fn }}-hbbs-0:21118 {
      header_up X-Real-IP {{ $ip }}
      {{- if $trusted }}
      header_up X-Forwarded-For {client_ip}
      {{- end }}
      flush_interval -1
    }
  }
  @hbbr path_regexp hbbr ^/ws/relay/([0-9]{1,3})$
  handle @hbbr {
    reverse_proxy {{ $fn }}-hbbr-{re.hbbr.1}:21119 {
      header_up X-Real-IP {{ $ip }}
      {{- if $trusted }}
      header_up X-Forwarded-For {client_ip}
      {{- end }}
      flush_interval -1
    }
  }
  # Public key hbbs signs with (all hbbs pods share the key), for Tether Manage's rotation check.
  handle /rendezvous/pubkey {
    rewrite * /pubkey
    reverse_proxy {{ $fn }}-hbbs-0:21122
  }
  {{- if .Values.api.enabled }}
  # Through the api pod's Caddy, which hands tether-api the client IP from X-Forwarded-For
  # (tether-api trusts it only from loopback: direct, every client would be this pod's IP).
  # /q/*, /download/* and /: the api pod's Caddy sends the files (templates/api.yaml).
  @api path /api/* /q/* /download/* /
  handle @api {
    # The first 600 KiB of a body are read before tether-api is connected: a slow client waits in
    # Caddy, not in a tether-api worker (which reads at most 512 KiB + 1 into memory; `512k`
    # would be 512 000 bytes). Larger uploads stream through after that.
    reverse_proxy {{ $fn }}-api:8081 {
      request_buffers 600KiB
      {{- if $trusted }}
      header_up X-Forwarded-For {client_ip}
      {{- end }}
    }
  }
  {{- end }}
  handle /proxy-healthz {
    respond "ok" 200
  }
  handle {
    respond "Tether relay" 200
  }
  header -Server
}
:8080 {
  {{- if .Values.proxy.tls.secretName }}
  # TLS is served on 8443: plain HTTP answers the probes and redirects everything else.
  handle /proxy-healthz {
    respond "ok" 200
  }
  handle {
    redir https://{host}{{ if ne (int .Values.proxy.service.tlsPort) 443 }}:{{ .Values.proxy.service.tlsPort }}{{ end }}{uri} 308
  }
  {{- else }}
  import routes
  {{- end }}
}
{{- if .Values.proxy.tls.secretName }}
:8443 {
  tls /tls/tls.crt /tls/tls.key
  header Strict-Transport-Security "max-age=31536000"
  import routes
}
{{- end }}
{{- end -}}
