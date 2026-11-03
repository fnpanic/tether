// Settings → Diagnostics: what support needs to know about this install and its network path.
import { useEffect, useState } from "react";
import { invoke } from "../lib/ipc";
import { errorText, isOn, usePolicy } from "../lib/policy";
import { Button, StatusDot, TextField, Toggle } from "../components/ui";

interface Route {
  url: string;
  via_proxy: boolean;
  ok: boolean;
  error: string;
  ms: number;
  at: number;
}

interface Info {
  version: string;
  id: string;
  os: string;
  policy: { serial: number; group: string | null; sources: string[] };
  relay: string;
  proxy: string | null;
  dns: { host: string; addrs?: string[]; error?: string; ms: number };
  routes: Route[];
  errors: string[];
  verboseUntil: number;
  logDir: string;
}

function Line({ k, children }: { k: string; children: React.ReactNode }) {
  return (
    <div className="flex gap-4 border-b border-hairline py-2 text-caption last:border-0">
      <div className="w-36 shrink-0 text-muted">{k}</div>
      <div className="min-w-0 flex-1 break-words font-mono text-footnote leading-5">{children}</div>
    </div>
  );
}

function Routes({ routes }: { routes: Route[] }) {
  if (!routes.length) return <div className="py-2 text-caption text-muted">No connection attempts yet.</div>;
  return (
    <table className="w-full text-left text-footnote">
      <thead className="text-muted">
        <tr>
          <th className="py-1.5 font-normal">Route</th>
          <th className="py-1.5 font-normal">Via</th>
          <th className="py-1.5 text-right font-normal">Time</th>
        </tr>
      </thead>
      <tbody>
        {routes.map((r, i) => (
          <tr key={i} className="border-t border-hairline align-top" title={r.error || undefined}>
            <td className="py-1.5 pr-3">
              <span className="flex items-center gap-2">
                <StatusDot tone={r.ok ? "ok" : "danger"} />
                <span className="font-mono">{r.url}</span>
              </span>
              {r.error && <div className="mt-0.5 pl-3.5 text-danger">{r.error}</div>}
            </td>
            <td className="py-1.5 pr-3">{r.via_proxy ? "Proxy" : "Direct"}</td>
            <td className="py-1.5 text-right tabular-nums">{r.ms} ms</td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}

export function Diagnostics() {
  const eff = usePolicy();
  const [info, setInfo] = useState<Info | null>(null);
  const [test, setTest] = useState<{ routes: Route[] } | null>(null);
  const [testing, setTesting] = useState(false);
  const [ticket, setTicket] = useState("");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const load = () => invoke<Info>("diag_info").then(setInfo).catch(() => {});
  useEffect(() => {
    load();
  }, []);

  const run = async (f: () => Promise<string>) => {
    setBusy(true);
    setMsg(null);
    try {
      setMsg({ ok: true, text: await f() });
    } catch (e) {
      setMsg({ ok: false, text: errorText(e) });
    } finally {
      setBusy(false);
    }
  };
  const verbose = !!info && info.verboseUntil * 1000 > Date.now();

  return (
    <div className="space-y-6">
      <section>
        <Line k="Version">{info?.version ?? "…"}</Line>
        <Line k="ID">{info?.id}</Line>
        <Line k="System">{info?.os}</Line>
        <Line k="Policy">
          {info ? `serial ${info.policy.serial}${info.policy.group ? `, group ${info.policy.group}` : ""} (${info.policy.sources.join(", ")})` : ""}
        </Line>
        <Line k="Server">{info?.relay}</Line>
        <Line k="Proxy">{info ? info.proxy ?? "none, direct connection" : ""}</Line>
        <Line k="DNS">{info ? (info.dns.error ? `failed: ${info.dns.error}` : `${info.dns.addrs?.join(", ")} (${info.dns.ms} ms)`) : ""}</Line>
      </section>

      <section>
        <div className="flex items-center justify-between">
          <h3 className="text-callout font-semibold">Connection routes</h3>
          <Button
            size="sm"
            disabled={testing}
            onClick={async () => {
              setTesting(true);
              setTest(await invoke<{ routes: Route[] }>("diag_test").catch(() => ({ routes: [] })));
              setTesting(false);
              load();
            }}
          >
            {testing ? "Testing…" : "Test connection"}
          </Button>
        </div>
        <p className="mt-1 text-caption text-muted">{test ? "Every route to the server, connected just now:" : "Recent attempts by this window:"}</p>
        <div className="mt-2">
          <Routes routes={test?.routes ?? info?.routes ?? []} />
        </div>
      </section>

      {!!info?.errors.length && (
        <section>
          <h3 className="text-callout font-semibold">Recent errors and warnings</h3>
          <pre className="mt-2 max-h-48 overflow-auto whitespace-pre-wrap rounded-xl bg-parchment p-3 font-mono text-footnote leading-5">{info.errors.join("\n")}</pre>
        </section>
      )}

      <section>
        {isOn(eff, "diagnostics.verbose-logging") && (
          <div className="flex items-center gap-6 border-b border-hairline py-4">
            <div className="flex-1">
              <div className="text-callout">Verbose logging (1 hour)</div>
              <div className="mt-0.5 text-caption text-muted">
                {verbose ? `On until ${new Date(info!.verboseUntil * 1000).toLocaleTimeString()}. ` : ""}Takes effect when Tether next starts.
              </div>
            </div>
            <Toggle label="Verbose logging" checked={verbose} onChange={(on) => invoke<number>("diag_verbose", { on }).then(() => load()).catch((e) => setMsg({ ok: false, text: errorText(e) }))} />
          </div>
        )}
        <div className="flex flex-wrap gap-2 pt-4">
          <Button size="sm" variant="ghost" onClick={() => invoke("diag_open_logs").catch((e) => setMsg({ ok: false, text: errorText(e) }))}>
            Open log folder
          </Button>
          <Button size="sm" disabled={busy} onClick={() => run(async () => `Saved to ${await invoke<string>("diag_export")}`)}>
            Export diagnostics
          </Button>
        </div>
        {isOn(eff, "diagnostics.upload") && (
          <form
            className="mt-4 flex items-center gap-2"
            onSubmit={(e) => {
              e.preventDefault();
              run(async () => {
                await invoke("diag_upload", { ticket });
                setTicket("");
                return "Sent. Your support contact can now see the diagnostics.";
              });
            }}
          >
            <TextField
              value={ticket}
              onChange={(e) => setTicket(e.target.value.toUpperCase().replace(/[^A-Z0-9]/g, "").slice(0, 8))}
              placeholder="Ticket code"
              aria-label="Support ticket code"
              spellCheck={false}
              className="!h-9 w-40 font-mono tracking-[0.15em]"
            />
            <Button size="sm" variant="primary" type="submit" disabled={busy || ticket.length !== 8}>
              Send to support
            </Button>
          </form>
        )}
        {msg && <div className={`mt-3 break-words text-footnote ${msg.ok ? "text-ok" : "text-danger"}`}>{msg.text}</div>}
      </section>
    </div>
  );
}
