{{/* Caddy config of the proxy; does not depend on replica counts. */}}
{{- define "tr.caddyfile" -}}
{{- $fn := include "tr.fullname" . -}}
{
  admin off
  auto_https off
  # WebSocket sessions stay open while Caddy shuts down, up to the pod's grace period.
  grace_period 25s
}
(routes) {
  # Per-pod routing: both peers of a session must land on the same hbbr pod, a host's
  # replies on the same hbbs pod. The ordinal maps to the per-pod Service; no streaming
  # timeout (sessions last hours), no buffering.
  @hbbs path_regexp hbbs ^/ws/id/([0-9]{1,3})$
  handle @hbbs {
    reverse_proxy {{ $fn }}-hbbs-{re.hbbs.1}:21118 {
      flush_interval -1
    }
  }
  handle /ws/id {
    reverse_proxy {{ $fn }}-hbbs-0:21118 {
      flush_interval -1
    }
  }
  @hbbr path_regexp hbbr ^/ws/relay/([0-9]{1,3})$
  handle @hbbr {
    reverse_proxy {{ $fn }}-hbbr-{re.hbbr.1}:21119 {
      flush_interval -1
    }
  }
  # Public key hbbs signs with (all hbbs pods share the key), for Tether Manage's rotation check.
  handle /rendezvous/pubkey {
    rewrite * /pubkey
    reverse_proxy {{ $fn }}-hbbs-0:21122
  }
  {{- if .Values.api.enabled }}
  handle /api/* {
    reverse_proxy {{ $fn }}-api:21120
  }
  handle /q/* {
    reverse_proxy {{ $fn }}-api:21120
  }
  handle /download/* {
    reverse_proxy {{ $fn }}-api:8081
  }
  handle / {
    reverse_proxy {{ $fn }}-api:8081
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
  import routes
}
{{- if .Values.proxy.tls.secretName }}
:8443 {
  tls /tls/tls.crt /tls/tls.key
  header Strict-Transport-Security "max-age=31536000"
  import routes
}
{{- end }}
{{- end -}}
