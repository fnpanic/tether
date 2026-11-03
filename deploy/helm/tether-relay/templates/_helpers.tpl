{{- define "tr.fullname" -}}
{{- if contains .Chart.Name .Release.Name -}}{{ .Release.Name | trunc 40 | trimSuffix "-" }}{{- else -}}{{ printf "%s-%s" .Release.Name .Chart.Name | trunc 40 | trimSuffix "-" }}{{- end -}}
{{- end -}}

{{- define "tr.labels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version }}
{{- end -}}

{{- define "tr.selector" -}}
app.kubernetes.io/name: {{ .root.Chart.Name }}
app.kubernetes.io/instance: {{ .root.Release.Name }}
app.kubernetes.io/component: {{ .component }}
{{- end -}}

{{- define "tr.image" -}}
{{ .Values.image.repository }}:{{ .Values.image.tag | default .Chart.AppVersion }}
{{- end -}}

{{- define "tr.keySecret" -}}
{{- if .Values.key.existingSecret -}}{{ .Values.key.existingSecret }}{{- else -}}{{ include "tr.fullname" . }}-key{{- end -}}
{{- end -}}

{{/* Container hardening shared by all workloads. */}}
{{- define "tr.containerSecurity" -}}
securityContext:
  runAsNonRoot: true
  runAsUser: 65532
  runAsGroup: 65532
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities: { drop: [ALL] }
{{- end -}}

{{/* Caddy's binary carries the cap_net_bind_service file capability; exec fails without it in the bounding set. */}}
{{- define "tr.caddySecurity" -}}
securityContext:
  runAsNonRoot: true
  runAsUser: 65532
  runAsGroup: 65532
  allowPrivilegeEscalation: false
  readOnlyRootFilesystem: true
  capabilities: { drop: [ALL], add: [NET_BIND_SERVICE] }
{{- end -}}

{{/* hbbs relay list: one entry per hbbr pod, `<public URL>|<in-cluster health URL>`, plus extras. */}}
{{- define "tr.relayList" -}}
{{- $fn := include "tr.fullname" . -}}
{{- $out := list -}}
{{- range $i := until (int .Values.hbbr.replicas) -}}
{{- $out = append $out (printf "%s/ws/relay/%d|http://%s-hbbr-%d:21121/readyz" (trimSuffix "/" $.Values.publicUrl) $i $fn $i) -}}
{{- end -}}
{{- range .Values.hbbs.extraRelays -}}{{- $out = append $out . -}}{{- end -}}
{{- join "," $out -}}
{{- end -}}
