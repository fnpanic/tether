// Generates the Grafana dashboards in ./dashboards/*.json (provisioned from files, docs/monitoring.md).
//   node deploy/monitoring/grafana/build-dashboards.mjs
// Edit here, not in the JSON: every panel shares the same look (units, colours, legends).
import { writeFileSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const OUT = join(dirname(fileURLToPath(import.meta.url)), "dashboards");
const DS = { type: "prometheus", uid: "prometheus" };
const N = 'node=~"$node"';
const C = { green: "#56A64B", yellow: "#F2CC0C", orange: "#FF9830", red: "#E02F44", blue: "#3274D9", purple: "#A352CC", teal: "#4ECDC4", grey: "#8E8E8E" };

let id = 0;
let y = 0;
let x = 0;
let rowH = 0;
let panels = [];

function place(w, h) {
  if (x + w > 24) {
    x = 0;
    y += rowH;
    rowH = 0;
  }
  const g = { x, y, w, h };
  x += w;
  rowH = Math.max(rowH, h);
  return g;
}

function row(title) {
  if (x) {
    y += rowH;
  }
  x = 0;
  rowH = 0;
  panels.push({ type: "row", title, id: ++id, collapsed: false, gridPos: { x: 0, y, w: 24, h: 1 }, panels: [] });
  y += 1;
}

const targets = (qs) =>
  qs.map(([expr, legend, extra], i) => ({ datasource: DS, expr, legendFormat: legend ?? "", refId: String.fromCharCode(65 + i), ...(extra || {}) }));

function ts(title, qs, o = {}) {
  panels.push({
    type: "timeseries",
    title,
    id: ++id,
    description: o.desc ?? "",
    datasource: DS,
    gridPos: place(o.w ?? 12, o.h ?? 8),
    targets: targets(qs),
    fieldConfig: {
      defaults: {
        unit: o.unit ?? "short",
        min: o.min ?? 0,
        max: o.max,
        decimals: o.decimals,
        color: { mode: o.colorMode ?? "palette-classic" },
        custom: {
          drawStyle: o.bars ? "bars" : "line",
          lineInterpolation: "smooth",
          lineWidth: 2,
          fillOpacity: o.fill ?? 18,
          gradientMode: "opacity",
          showPoints: "never",
          spanNulls: true,
          stacking: { mode: o.stack ? "normal" : "none", group: "A" },
          axisSoftMin: 0,
          thresholdsStyle: { mode: o.thresholds ? "line+area" : "off" },
        },
        thresholds: o.thresholds ?? { mode: "absolute", steps: [{ color: C.green, value: null }] },
      },
      overrides: (o.colors || []).map(([name, color]) => ({
        matcher: { id: "byName", options: name },
        properties: [{ id: "color", value: { mode: "fixed", fixedColor: color } }],
      })),
    },
    options: {
      legend: { displayMode: o.legendList ? "list" : "table", placement: o.legendList ? "bottom" : "right", calcs: o.legendList ? [] : ["lastNotNull", "max"] },
      tooltip: { mode: "multi", sort: "desc" },
    },
  });
}

function stat(title, qs, o = {}) {
  panels.push({
    type: "stat",
    title,
    id: ++id,
    description: o.desc ?? "",
    datasource: DS,
    gridPos: place(o.w ?? 4, o.h ?? 5),
    targets: targets(qs.map(([e, l, x]) => [e, l, { instant: true, ...(x || {}) }])),
    fieldConfig: {
      defaults: {
        unit: o.unit ?? "short",
        decimals: o.decimals,
        color: { mode: "thresholds" },
        thresholds: o.thresholds ?? { mode: "absolute", steps: [{ color: o.color ?? C.blue, value: null }] },
        mappings: o.mappings ?? [],
        noValue: o.noValue ?? "–",
      },
      overrides: [],
    },
    options: {
      reduceOptions: { calcs: ["lastNotNull"], fields: "", values: false },
      colorMode: o.colorMode ?? "background",
      graphMode: o.spark ? "area" : "none",
      justifyMode: "center",
      textMode: o.textMode ?? "auto",
      orientation: "auto",
      wideLayout: true,
    },
  });
}

function gauge(title, qs, o = {}) {
  panels.push({
    type: "gauge",
    title,
    id: ++id,
    description: o.desc ?? "",
    datasource: DS,
    gridPos: place(o.w ?? 4, o.h ?? 6),
    targets: targets(qs.map(([e, l]) => [e, l, { instant: true }])),
    fieldConfig: {
      defaults: {
        unit: o.unit ?? "percentunit",
        min: 0,
        max: o.max ?? 1,
        decimals: o.decimals ?? 0,
        color: { mode: "thresholds" },
        thresholds: o.thresholds ?? pct(0.7, 0.85),
      },
      overrides: [],
    },
    options: { reduceOptions: { calcs: ["lastNotNull"], fields: "", values: false }, showThresholdMarkers: true, showThresholdLabels: false },
  });
}

function stateTimeline(title, qs, o = {}) {
  panels.push({
    type: "state-timeline",
    title,
    id: ++id,
    description: o.desc ?? "",
    datasource: DS,
    gridPos: place(o.w ?? 24, o.h ?? 7),
    targets: targets(qs),
    fieldConfig: {
      defaults: {
        color: { mode: "thresholds" },
        thresholds: o.thresholds ?? { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.green, value: 1 }] },
        mappings: o.mappings ?? upDown,
        custom: { fillOpacity: 80, lineWidth: 0 },
      },
      overrides: [],
    },
    options: { showValue: "never", rowHeight: 0.8, mergeValues: true, alignValue: "left", legend: { showLegend: false } },
  });
}

function bargauge(title, qs, o = {}) {
  panels.push({
    type: "bargauge",
    title,
    id: ++id,
    description: o.desc ?? "",
    datasource: DS,
    gridPos: place(o.w ?? 12, o.h ?? 8),
    targets: targets(qs.map(([e, l]) => [e, l, { instant: true }])),
    fieldConfig: {
      defaults: { unit: o.unit ?? "percentunit", min: 0, max: o.max ?? 1, decimals: o.decimals ?? 0, color: { mode: "thresholds" }, thresholds: o.thresholds ?? pct(0.7, 0.85) },
      overrides: [],
    },
    options: { orientation: "horizontal", displayMode: "gradient", valueMode: "color", showUnfilled: true, reduceOptions: { calcs: ["lastNotNull"], fields: "", values: false } },
  });
}

function table(title, qs, o = {}) {
  panels.push({
    type: "table",
    title,
    id: ++id,
    datasource: DS,
    gridPos: place(o.w ?? 24, o.h ?? 6),
    targets: targets(qs.map(([e, l]) => [e, l, { instant: true, format: "table" }])),
    fieldConfig: { defaults: { noValue: "none" }, overrides: [] },
    options: { showHeader: true, cellHeight: "sm" },
    transformations: [{ id: "organize", options: { excludeByName: { Time: true, Value: true, __name__: true, alertstate: true } } }],
  });
}

const pct = (warn, crit) => ({ mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.orange, value: warn }, { color: C.red, value: crit }] });
const upDown = [{ type: "value", options: { 0: { text: "DOWN", color: C.red }, 1: { text: "UP", color: C.green } } }];

const nodeVar = {
  name: "node",
  label: "Node",
  type: "query",
  datasource: DS,
  query: { query: "label_values(up, node)", refId: "node" },
  definition: "label_values(up, node)",
  refresh: 2,
  includeAll: true,
  multi: true,
  allValue: ".*",
  current: { selected: true, text: ["All"], value: ["$__all"] },
  sort: 1,
};

function dashboard(uid, title, description, build) {
  id = 0;
  y = 0;
  x = 0;
  rowH = 0;
  panels = [];
  build();
  const d = {
    uid,
    title,
    description,
    tags: ["tether"],
    editable: true,
    graphTooltip: 1,
    refresh: "30s",
    time: { from: "now-6h", to: "now" },
    timezone: "browser",
    schemaVersion: 41,
    version: 1,
    fiscalYearStartMonth: 0,
    liveNow: false,
    templating: { list: [nodeVar] },
    annotations: {
      list: [
        { builtIn: 1, datasource: { type: "grafana", uid: "-- Grafana --" }, enable: true, hide: true, iconColor: "rgba(0, 211, 255, 1)", name: "Annotations & Alerts", type: "dashboard" },
        {
          datasource: DS,
          enable: false, // toggle on to mark process restarts
          iconColor: "rgba(255, 152, 48, 0.5)",
          name: "Restarts",
          expr: `changes(process_start_time_seconds{${N}}[2m]) > 0`,
          step: "60s",
          titleFormat: "{{job}} restarted on {{node}}",
          useValueForTime: false,
        },
      ],
    },
    links: [{ title: "Tether", type: "dashboards", tags: ["tether"], asDropdown: true, includeVars: true, keepTime: true }],
    panels,
  };
  writeFileSync(join(OUT, `${uid}.json`), JSON.stringify(d, null, 2) + "\n");
}

mkdirSync(OUT, { recursive: true });

// ---------------------------------------------------------------------------------------------
dashboard("tether-overview", "Tether · Overview", "Both relay nodes at a glance: health, sessions, devices, bandwidth, failover.", () => {
  row("Health");
  stat("Node up", [[`min by (node) (up{job="node",${N}})`, "{{node}}"]], { mappings: upDown, thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.green, value: 1 }] }, w: 4 });
  stat("Relay ready (public probe)", [[`min by (instance) (probe_success{probe="readyz",${N}})`, "{{instance}}"]], {
    desc: "https://<node>/readyz/relay probed from the other node.",
    mappings: upDown,
    thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.green, value: 1 }] },
    w: 5,
  });
  stat("Relay choice", [[`max by (node) (hbbs_relay_failover{${N}})`, "{{node}}"]], {
    desc: "What each hbbs hands out: the primary relay, or backups because no primary is healthy.",
    mappings: [{ type: "value", options: { 0: { text: "Primary", color: C.green }, 1: { text: "FAILOVER: backup", color: C.orange } } }],
    thresholds: { mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.orange, value: 1 }] },
    w: 5,
  });
  stat("Firing alerts", [[`count(ALERTS{alertstate="firing",alertname!="Watchdog"}) or vector(0)`, ""]], {
    thresholds: { mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.red, value: 1 }] },
    w: 3,
  });
  stat("Certificate expires in", [[`min by (instance) (probe_ssl_earliest_cert_expiry{${N}} - time())`, "{{instance}}"]], {
    unit: "s",
    decimals: 0,
    thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.orange, value: 14 * 86400 }, { color: C.green, value: 30 * 86400 }] },
    w: 7,
  });
  row("Now");
  stat("Relayed sessions", [[`sum(hbbr_sessions{${N}})`, ""]], { color: C.blue, spark: true, w: 4 });
  stat("Online hosts (per hbbs)", [[`max by (node) (hbbs_peers_online{${N}})`, "{{node}}"]], { color: C.purple, w: 5, desc: "Hosts register with every rendezvous server: each node counts the same hosts." });
  stat("Devices active 24 h", [[`max by (node) (tether_api_devices_active_24h{${N}})`, "{{node}}"]], { color: C.teal, w: 5 });
  stat("Relay bandwidth", [[`sum(rate(hbbr_relayed_bytes_total{${N}}[5m])) * 8`, ""]], { unit: "bps", color: C.blue, spark: true, w: 5 });
  stat("TURN allocations", [[`sum(hbbs_turn_allocations{${N}})`, ""]], { color: C.purple, spark: true, w: 5 });
  row("Trends");
  ts("Relayed sessions", [[`sum by (node) (hbbr_sessions{${N}})`, "{{node}}"]], { stack: true });
  ts("Relay bandwidth", [[`sum by (node) (rate(hbbr_relayed_bytes_total{${N}}[$__rate_interval])) * 8`, "{{node}}"]], { unit: "bps" });
  ts("Online hosts", [[`max by (node) (hbbs_peers_online{${N}})`, "{{node}}"]]);
  ts("Network out (host)", [[`sum by (node) (rate(node_network_transmit_bytes_total{device!~"lo|veth.*|docker.*|podman.*",${N}}[$__rate_interval])) * 8`, "{{node}}"]], { unit: "bps" });
  stateTimeline("Relay health as seen by each hbbs", [[`max by (node, relay) (hbbs_relay_healthy{${N}})`, "{{node}} → {{relay}}"]], { h: 6 });
  table("Firing alerts", [[`ALERTS{alertstate="firing",alertname!="Watchdog"}`, ""]], { w: 24, h: 6 });
});

// ---------------------------------------------------------------------------------------------
dashboard("tether-relay", "Tether · Relay (hbbr)", "Relayed sessions, pairing and traffic per node.", () => {
  row("Sessions");
  stat("Sessions now", [[`sum by (node) (hbbr_sessions{${N}})`, "{{node}}"]], { color: C.blue, spark: true, w: 6 });
  stat("Waiting for peer", [[`sum by (node) (hbbr_pair_waiting{${N}})`, "{{node}}"]], { color: C.purple, w: 6 });
  stat("Draining", [[`max by (node) (hbbr_draining{${N}})`, "{{node}}"]], {
    mappings: [{ type: "value", options: { 0: { text: "no", color: C.green }, 1: { text: "DRAINING", color: C.orange } } }],
    thresholds: { mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.orange, value: 1 }] },
    w: 6,
  });
  stat("Bandwidth now", [[`sum by (node) (rate(hbbr_relayed_bytes_total{${N}}[5m])) * 8`, "{{node}}"]], { unit: "bps", color: C.teal, w: 6 });
  ts("Sessions by transport", [[`sum by (node, transport) (hbbr_sessions{${N}})`, "{{node}} {{transport}}"]], { stack: true });
  ts("New pairs / min", [[`sum by (node, transport) (rate(hbbr_pairs_total{${N}}[$__rate_interval])) * 60`, "{{node}} {{transport}}"]], { bars: true, fill: 70 });
  row("Traffic");
  ts("Relayed traffic", [[`sum by (node) (rate(hbbr_relayed_bytes_total{${N}}[$__rate_interval])) * 8`, "{{node}}"]], { unit: "bps", desc: "Each relayed byte is received from one side and sent to the other once: traffic in = traffic out." });
  ts("Relayed volume per hour", [[`sum by (node) (increase(hbbr_relayed_bytes_total{${N}}[1h]))`, "{{node}}"]], { unit: "bytes", bars: true, fill: 70 });
  row("Pairing problems and limits");
  ts(
    "Per minute",
    [
      [`sum by (node) (rate(hbbr_pair_timeouts_total{${N}}[$__rate_interval])) * 60`, "{{node}} peer never came"],
      [`sum by (node) (rate(hbbr_pair_refused_draining_total{${N}}[$__rate_interval])) * 60`, "{{node}} refused (draining)"],
      [`sum by (node) (rate(hbbr_downgrades_total{${N}}[$__rate_interval])) * 60`, "{{node}} downgraded (bandwidth limit)"],
      [`sum by (node) (rate(hbbr_blocked_total{${N}}[$__rate_interval])) * 60`, "{{node}} blocklisted IP"],
    ],
    { w: 24 }
  );
  row("Process");
  ts("CPU", [[`rate(process_cpu_seconds_total{job="hbbr",${N}}[$__rate_interval])`, "{{node}}"]], { unit: "percentunit", w: 8 });
  ts("Memory (RSS)", [[`process_resident_memory_bytes{job="hbbr",${N}}`, "{{node}}"]], { unit: "bytes", w: 8 });
  ts("Open files / threads", [[`process_open_fds{job="hbbr",${N}}`, "{{node}} fds"], [`process_threads{job="hbbr",${N}}`, "{{node}} threads"]], { w: 8 });
});

// ---------------------------------------------------------------------------------------------
dashboard("tether-signalling", "Tether · Signalling (hbbs)", "Registrations, punch holes, relay choice and TURN per node.", () => {
  row("Hosts");
  stat("Online hosts", [[`sum by (node) (hbbs_peers_online{${N}})`, "{{node}}"]], { color: C.purple, spark: true, w: 6 });
  stat("Over TCP / WebSocket", [[`sum by (node) (hbbs_peers_signalling{transport="tcp_ws",${N}})`, "{{node}}"]], { color: C.blue, w: 6 });
  stat("Open WebSocket connections", [[`sum by (node) (hbbs_connections{transport="ws",${N}})`, "{{node}}"]], { color: C.teal, w: 6 });
  stat("Peers in memory", [[`sum by (node) (hbbs_peers_in_memory{${N}})`, "{{node}}"]], { color: C.grey, w: 6 });
  ts("Online hosts by signalling path", [[`sum by (node, transport) (hbbs_peers_signalling{${N}})`, "{{node}} {{transport}}"]], { stack: true });
  ts("Open connections", [[`sum by (node, transport) (hbbs_connections{${N}})`, "{{node}} {{transport}}"]]);
  ts("Registrations / s", [[`sum by (node, transport) (rate(hbbs_register_peer_total{${N}}[$__rate_interval]))`, "{{node}} {{transport}}"]], { unit: "reqps" });
  ts("RegisterPk results / min", [[`sum by (node, result) (rate(hbbs_register_pk_total{${N}}[$__rate_interval])) * 60`, "{{node}} {{result}}"]], {
    colors: [],
  });
  row("Connections between peers");
  ts("Punch-hole requests / min", [[`sum by (node, result) (rate(hbbs_punch_hole_total{${N}}[$__rate_interval])) * 60`, "{{node}} {{result}}"]], { bars: true, fill: 70, stack: true });
  ts("Brute-force guard: refused / min", [[`sum by (node, reason) (rate(hbbs_ip_blocked_total{${N}}[$__rate_interval])) * 60`, "{{node}} {{reason}}"]], { bars: true, fill: 70 });
  row("Relay choice");
  stateTimeline("Relay health (3 s checks)", [[`max by (node, relay, role) (hbbs_relay_healthy{${N}})`, "{{node}} → {{relay}} ({{role}})"]], { h: 6 });
  ts("Relay handed out / min", [[`sum by (node, relay) (rate(hbbs_relay_assigned_total{${N}}[$__rate_interval])) * 60`, "{{node}} → {{relay}}"]]);
  ts("Sessions reported by each relay", [[`max by (node, relay) (hbbs_relay_sessions{${N}})`, "{{node}} → {{relay}}"]]);
  stateTimeline("Failover state", [[`max by (node) (hbbs_relay_failover{${N}})`, "{{node}}"]], {
    h: 4,
    w: 12,
    mappings: [{ type: "value", options: { 0: { text: "primary", color: C.green }, 1: { text: "backup", color: C.orange } } }],
    thresholds: { mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.orange, value: 1 }] },
  });
  ts("Failover transitions", [[`sum by (node, to) (increase(hbbs_relay_failover_transitions_total{${N}}[$__rate_interval]))`, "{{node}} to {{to}}"]], { bars: true, fill: 70, h: 4, legendList: true });
  row("TURN");
  ts("Allocations", [[`sum by (node) (hbbs_turn_allocations{${N}})`, "{{node}} open"], [`max by (node) (hbbs_turn_allocations_max{${N}})`, "{{node}} cap"]], { w: 8 });
  ts("TURN traffic", [[`sum by (node, direction) (rate(hbbs_turn_bytes_total{${N}}[$__rate_interval])) * 8`, "{{node}} {{direction}}"]], { unit: "bps", w: 8 });
  ts(
    "Requests and drops / s",
    [
      [`sum by (node) (rate(hbbs_turn_requests_total{${N}}[$__rate_interval]))`, "{{node}} requests"],
      [`sum by (node, reason) (rate(hbbs_turn_dropped_total{${N}}[$__rate_interval]))`, "{{node}} dropped {{reason}}"],
      [`sum by (node) (rate(hbbs_turn_allocations_refused_total{${N}}[$__rate_interval]))`, "{{node}} allocation refused"],
    ],
    { unit: "reqps", w: 8 }
  );
  row("Process");
  ts("CPU", [[`rate(process_cpu_seconds_total{job="hbbs",${N}}[$__rate_interval])`, "{{node}}"]], { unit: "percentunit", w: 8 });
  ts("Memory (RSS)", [[`process_resident_memory_bytes{job="hbbs",${N}}`, "{{node}}"]], { unit: "bytes", w: 8 });
  ts("Open files / threads", [[`process_open_fds{job="hbbs",${N}}`, "{{node}} fds"], [`process_threads{job="hbbs",${N}}`, "{{node}} threads"]], { w: 8 });
});

// ---------------------------------------------------------------------------------------------
const api = (m, extra = "") => `${m}{job="api",${N}${extra}}`;
dashboard("tether-api", "Tether · API", "tether-api requests, latency, errors, workers and data, and the front end (Caddy or nginx) in front of it.", () => {
  row("Now");
  stat("Requests / s", [[`sum(rate(${api("tether_api_requests_total")}[5m]))`, ""]], { unit: "reqps", decimals: 2, color: C.blue, spark: true, w: 4 });
  stat("5xx share (5 m)", [[`(sum(rate(${api("tether_api_requests_total", ',code=~"5.."')}[5m])) or vector(0)) / clamp_min(sum(rate(${api("tether_api_requests_total")}[5m])), 1e-9)`, ""]], {
    unit: "percentunit",
    decimals: 1,
    thresholds: pct(0.01, 0.05),
    w: 4,
  });
  stat("p95 latency (heartbeat)", [[`histogram_quantile(0.95, sum by (le) (rate(${api("tether_api_request_duration_seconds_bucket", ',endpoint="heartbeat"')}[5m])))`, ""]], {
    unit: "s",
    thresholds: { mode: "absolute", steps: [{ color: C.green, value: null }, { color: C.orange, value: 0.25 }, { color: C.red, value: 1 }] },
    w: 4,
  });
  gauge("Workers busy", [[`max(${api("tether_api_workers_active")}) / max(${api("tether_api_workers_max")})`, ""]], { w: 4, h: 5 });
  stat("Devices active 24 h", [[`max by (node) (${api("tether_api_devices_active_24h")})`, "{{node}}"]], { color: C.teal, w: 4 });
  stat("Data dir", [[`max by (node) (${api("tether_api_data_bytes")})`, "{{node}}"]], { unit: "bytes", color: C.grey, w: 4 });
  row("Requests");
  ts("Requests / s by endpoint", [[`sum by (endpoint) (rate(${api("tether_api_requests_total")}[$__rate_interval]))`, "{{endpoint}}"]], { unit: "reqps", stack: true });
  ts("Requests / s by status", [[`sum by (code) (rate(${api("tether_api_requests_total")}[$__rate_interval]))`, "{{code}}"]], { unit: "reqps", stack: true });
  ts(
    "Latency by endpoint (p50 / p95)",
    [
      [`histogram_quantile(0.5, sum by (le, endpoint) (rate(${api("tether_api_request_duration_seconds_bucket", ',endpoint!~"metrics|other"')}[$__rate_interval])))`, "p50 {{endpoint}}"],
      [`histogram_quantile(0.95, sum by (le, endpoint) (rate(${api("tether_api_request_duration_seconds_bucket", ',endpoint!~"metrics|other"')}[$__rate_interval])))`, "p95 {{endpoint}}"],
    ],
    { unit: "s" }
  );
  ts(
    "Errors and limits / min",
    [
      [`sum by (code) (rate(${api("tether_api_requests_total", ',code=~"5.."')}[$__rate_interval])) * 60`, "{{code}}"],
      [`sum(rate(${api("tether_api_busy_rejected_total")}[$__rate_interval])) * 60`, "503 busy (workers full)"],
      [`sum(rate(${api("tether_api_body_timeouts_total")}[$__rate_interval])) * 60`, "body read timeout"],
      [`sum(rate(${api("tether_api_requests_total", ',code="429"')}[$__rate_interval])) * 60`, "429 rate limited"],
    ],
    { bars: true, fill: 70 }
  );
  row("Clients");
  ts("Heartbeats / s", [[`sum by (node) (rate(${api("tether_api_requests_total", ',endpoint="heartbeat",code=~"200|204"')}[$__rate_interval]))`, "{{node}}"]], { unit: "reqps", w: 8 });
  ts("Devices", [[`max by (node) (${api("tether_api_devices_active_24h")})`, "{{node}} active 24 h"], [`max by (node) (${api("tether_api_devices")})`, "{{node}} known"]], { w: 8 });
  ts(
    "Fetches and posts / min",
    [
      [`sum by (endpoint) (rate(${api("tether_api_requests_total", ',endpoint=~"policy|server_keys|custom_download|diag_upload"')}[$__rate_interval])) * 60`, "{{endpoint}}"],
      [`sum(rate(${api("tether_api_requests_total", ',endpoint=~"audit_.*"')}[$__rate_interval])) * 60`, "audit posts"],
    ],
    { w: 8 }
  );
  // Update delivery (docs/update-design.md): G per node in tether-api and hbbs (equal when the
  // push path works), the signed index per channel, hint pushes, index fetches, adoption.
  row("Releases");
  stat("Release generation", [[`max by (node) (${api("tether_api_release_generation")})`, "{{node}} api"], [`max by (node) (hbbs_release_generation{${N}})`, "{{node}} hbbs"]], { color: C.blue, w: 6, desc: "G bumps on every release index or latest.json change; hbbs pushes it to every registered host." });
  stat("Index serial", [[`max by (node, channel) (${api("tether_api_release_index_serial")})`, "{{node}} {{channel}}"]], { color: C.teal, w: 6 });
  stat("Index valid for", [[`min by (channel) (${api("tether_api_release_index_expires_timestamp")}) - time()`, "{{channel}}"]], {
    unit: "s",
    thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.orange, value: 0 }, { color: C.green, value: 7 * 86400 }] },
    w: 6,
  });
  stat("Hosts notified (current G)", [[`max by (node) (hbbs_release_hint_peers_notified{${N}})`, "{{node}}"]], { color: C.purple, w: 6 });
  ts("Release hints sent / min", [[`sum by (node, reason) (rate(hbbs_release_hints_sent_total{${N}}[$__rate_interval])) * 60`, "{{node}} {{reason}}"]], { w: 8, bars: true });
  ts("Release index fetches / s", [[`sum by (node, code) (rate(${api("tether_api_requests_total", ',endpoint="release_index"')}[$__rate_interval]))`, "{{node}} {{code}}"]], { unit: "reqps", w: 8, stack: true });
  ts("Update states (devices, 24 h)", [[`sum by (state) (${api("tether_api_devices_update_state")})`, "{{state}}"]], { w: 8, stack: true });
  ts("Devices by version (24 h)", [[`sum by (profile, version) (${api("tether_api_devices_by_version")})`, "{{profile}} {{version}}"]], { w: 12, stack: true });
  ts("Downloads by version / h", [[`sum by (profile, version) (increase(${api("tether_api_release_downloads_total")}[1h]))`, "{{profile}} {{version}}"]], { w: 12, bars: true });
  row("Workers and storage");
  ts("Workers", [[`max by (node) (${api("tether_api_workers_active")})`, "{{node}} active"], [`max(${api("tether_api_workers_max")})`, "cap"]], { w: 8, colors: [["cap", C.red]] });
  ts("Data dir size", [[`max by (node) (${api("tether_api_data_bytes")})`, "{{node}}"]], { unit: "bytes", w: 8 });
  ts("Free space (data filesystem)", [[`max by (node) (${api("tether_api_data_fs_free_bytes")})`, "{{node}}"]], { unit: "bytes", w: 8 });
  row("Caddy (HTTPS front end)");
  ts("Caddy requests / s by status", [[`sum by (node, code) (rate(caddy_http_request_duration_seconds_count{${N}}[$__rate_interval]))`, "{{node}} {{code}}"]], { unit: "reqps" });
  ts("Caddy p95 response time", [[`histogram_quantile(0.95, sum by (le, node) (rate(caddy_http_request_duration_seconds_bucket{${N}}[$__rate_interval])))`, "{{node}}"]], { unit: "s" });
  ts("tether-api memory (RSS)", [[`process_resident_memory_bytes{job="api",${N}}`, "{{node}} tether-api"], [`process_resident_memory_bytes{job="caddy",${N}}`, "{{node}} caddy"]], { unit: "bytes" });
  ts("Caddy in-flight requests", [[`sum by (node) (caddy_http_requests_in_flight{${N}})`, "{{node}}"]]);
  // nginx-prometheus-exporter (stub_status) on nodes installed with PROXY=nginx; empty elsewhere.
  row("nginx (HTTPS front end, PROXY=nginx)");
  ts("nginx connections", [[`sum by (node) (nginx_connections_active{${N}})`, "{{node}} active"], [`sum by (node) (nginx_connections_waiting{${N}})`, "{{node}} idle (WebSockets, keep-alive)"]], { w: 8 });
  ts("nginx requests and accepts / s", [[`sum by (node) (rate(nginx_http_requests_total{${N}}[$__rate_interval]))`, "{{node}} requests"], [`sum by (node) (rate(nginx_connections_accepted{${N}}[$__rate_interval]))`, "{{node}} accepted"]], { unit: "reqps", w: 8 });
  ts("nginx dropped connections / min", [[`sum by (node) (rate(nginx_connections_accepted{${N}}[$__rate_interval]) - rate(nginx_connections_handled{${N}}[$__rate_interval])) * 60`, "{{node}} (worker_connections full)"]], { w: 8 });
});

// ---------------------------------------------------------------------------------------------
const nd = (m, extra = "") => `${m}{job="node",${N}${extra}}`;
const fsSel = ',fstype!~"tmpfs|devtmpfs|overlay|squashfs|ramfs|efivarfs|vfat",mountpoint!~"/boot.*"';
dashboard("tether-hosts", "Tether · Hosts", "Host metrics of each relay node (node_exporter).", () => {
  row("Now");
  gauge("CPU busy", [[`1 - avg by (node) (rate(${nd("node_cpu_seconds_total", ',mode="idle"')}[5m]))`, "{{node}}"]], { w: 6, thresholds: pct(0.7, 0.9) });
  gauge("Memory used", [[`1 - max by (node) (${nd("node_memory_MemAvailable_bytes")} / ${nd("node_memory_MemTotal_bytes")})`, "{{node}}"]], { w: 6, thresholds: pct(0.8, 0.9) });
  gauge("Root disk used", [[`1 - max by (node) (${nd("node_filesystem_avail_bytes", ',mountpoint="/"')} / ${nd("node_filesystem_size_bytes", ',mountpoint="/"')})`, "{{node}}"]], { w: 6, thresholds: pct(0.75, 0.85) });
  stat("Uptime", [[`min by (node) (time() - ${nd("node_boot_time_seconds")})`, "{{node}}"]], { unit: "s", decimals: 1, color: C.grey, w: 6, desc: "The Hetzner node reboots nightly at 04:00." });
  row("CPU and memory");
  ts("CPU by mode", [[`sum by (node, mode) (rate(${nd("node_cpu_seconds_total", ',mode!="idle"')}[$__rate_interval])) / on (node) group_left count by (node) (${nd("node_cpu_seconds_total", ',mode="idle"')})`, "{{node}} {{mode}}"]], { unit: "percentunit", stack: true, max: 1 });
  ts("Load (1 m) per CPU", [[`max by (node) (${nd("node_load1")}) / count by (node) (${nd("node_cpu_seconds_total", ',mode="idle"')})`, "{{node}}"]], { thresholds: { mode: "absolute", steps: [{ color: "transparent", value: null }, { color: C.red, value: 1 }] } });
  ts("Memory", [[`max by (node) (${nd("node_memory_MemTotal_bytes")} - ${nd("node_memory_MemAvailable_bytes")})`, "{{node}} used"], [`max by (node) (${nd("node_memory_MemTotal_bytes")})`, "{{node}} total"]], { unit: "bytes" });
  ts("Swap used and memory pressure", [[`max by (node) (${nd("node_memory_SwapTotal_bytes")} - ${nd("node_memory_SwapFree_bytes")})`, "{{node}} swap used"]], { unit: "bytes" });
  row("Disk");
  bargauge("Filesystems used", [[`1 - max by (node, mountpoint) (${nd("node_filesystem_avail_bytes", fsSel)} / ${nd("node_filesystem_size_bytes", fsSel)})`, "{{node}} {{mountpoint}}"]], { thresholds: pct(0.75, 0.85) });
  ts("Disk I/O", [[`sum by (node) (rate(${nd("node_disk_read_bytes_total")}[$__rate_interval]))`, "{{node}} read"], [`sum by (node) (rate(${nd("node_disk_written_bytes_total")}[$__rate_interval]))`, "{{node}} write"]], { unit: "Bps" });
  row("Network");
  ts("Network traffic", [
    [`sum by (node) (rate(${nd("node_network_receive_bytes_total", ',device!~"lo|veth.*|docker.*|podman.*"')}[$__rate_interval])) * 8`, "{{node}} in"],
    [`sum by (node) (rate(${nd("node_network_transmit_bytes_total", ',device!~"lo|veth.*|docker.*|podman.*"')}[$__rate_interval])) * 8`, "{{node}} out"],
  ], { unit: "bps" });
  ts("Conntrack entries", [[`max by (node) (${nd("node_nf_conntrack_entries")})`, "{{node}} entries"], [`max by (node) (${nd("node_nf_conntrack_entries_limit")})`, "{{node}} limit"]]);
  ts("TCP connections", [[`max by (node) (${nd("node_netstat_Tcp_CurrEstab")})`, "{{node}} established"], [`max by (node) (${nd("node_sockstat_TCP_tw")})`, "{{node}} time-wait"]]);
  ts("Errors and drops / s", [
    [`sum by (node) (rate(${nd("node_network_receive_drop_total", ',device!="lo"')}[$__rate_interval]))`, "{{node}} rx drop"],
    [`sum by (node) (rate(${nd("node_network_transmit_drop_total", ',device!="lo"')}[$__rate_interval]))`, "{{node}} tx drop"],
    [`sum by (node) (rate(${nd("node_netstat_TcpExt_ListenDrops")}[$__rate_interval]))`, "{{node}} listen drops"],
    [`sum by (node) (rate(${nd("node_netstat_Tcp_RetransSegs")}[$__rate_interval]))`, "{{node}} TCP retransmits"],
  ]);
  row("System");
  ts("Open files", [[`max by (node) (${nd("node_filefd_allocated")})`, "{{node}} allocated"]], { w: 8 });
  stateTimeline("Service units", [[`max by (node, name) (${nd("node_systemd_unit_state", ',state="active",name=~"tether-.*|prometheus.*|alertmanager.*|grafana-server.*|node_exporter.*|blackbox_exporter.*|fail2ban.*|firewalld.*|sshd.*"')})`, "{{node}} {{name}}"]], {
    w: 16,
    h: 12,
    mappings: [{ type: "value", options: { 0: { text: "not active", color: C.red }, 1: { text: "active", color: C.green } } }],
  });
});

// ---------------------------------------------------------------------------------------------
dashboard("tether-probes", "Tether · Probes", "Public endpoints of each node probed from the other node (blackbox_exporter).", () => {
  row("Now");
  stat("Endpoint up", [[`min by (instance) (probe_success{${N}})`, "{{instance}}"]], { mappings: upDown, thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: C.green, value: 1 }] }, w: 24, h: 6, textMode: "value_and_name" });
  row("History");
  stateTimeline("Probe result", [[`max by (instance, probe) (probe_success{${N}})`, "{{probe}} {{instance}}"]], { h: 9 });
  ts("Probe duration", [[`max by (instance) (probe_duration_seconds{${N}})`, "{{instance}}"]], { unit: "s" });
  ts("HTTP phases (TLS, processing)", [
    [`max by (instance) (probe_http_duration_seconds{phase="tls",${N}})`, "tls {{instance}}"],
    [`max by (instance) (probe_http_duration_seconds{phase="processing",${N}})`, "processing {{instance}}"],
  ], { unit: "s" });
  ts("HTTP status code", [[`max by (instance) (probe_http_status_code{${N}})`, "{{instance}}"]], { legendList: true });
  ts("Certificate expires in", [[`min by (instance) (probe_ssl_earliest_cert_expiry{${N}} - time()) / 86400`, "{{instance}}"]], {
    unit: "d",
    thresholds: { mode: "absolute", steps: [{ color: C.red, value: null }, { color: "transparent", value: 14 }] },
  });
});

console.log(`dashboards written to ${OUT}`);
