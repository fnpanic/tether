// Keys › Server key: what the relays serve and the guided rotation (docs/key-rotation.md).
import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { Check, Copy } from "lucide-react";
import { useApp } from "../App";
import { api, host, invoke, type Device, type RelayResult } from "../lib/ipc";
import { announceList, confirmed, HELM_RETIRE, helmSwitch, isKey, newest, retireList, retireCommands, step, switchCommands, type LiveKeys, type Rotation, type Step } from "../lib/serverkeys";
import { PublishSheet } from "./publish";
import { Badge, Button, CopyField, ErrorText, IconButton, Label, StatusDot, TextField } from "./ui";

const short = (k?: string) => (k ? `${k.slice(0, 8)}…` : "");

function Commands({ text, label }: { text: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <div className="flex items-start gap-2 rounded-xl border border-hairline bg-parchment py-2 pl-3 pr-1">
      <pre className="min-w-0 flex-1 select-text overflow-x-auto whitespace-pre font-mono text-footnote">{text}</pre>
      <IconButton
        label={`Copy ${label}`}
        onClick={async () => {
          await navigator.clipboard.writeText(text);
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        }}
      >
        {done ? <Check size={16} /> : <Copy size={16} />}
      </IconButton>
    </div>
  );
}

/** How many devices active in the last 24 h applied list `serial`; refreshes every 30 s. */
function Confirmations({ serial }: { serial: number }) {
  const [devices, setDevices] = useState<Device[] | null>(null);
  useEffect(() => {
    const load = () => api.devices().then((r) => setDevices(r.devices), () => {});
    load();
    const t = window.setInterval(load, 30_000);
    return () => clearInterval(t);
  }, [serial]);
  if (!devices) return <p className="text-caption text-muted">Reading devices…</p>;
  const { done, of } = confirmed(devices, serial);
  return (
    <div className="rounded-xl bg-parchment p-3" data-testid="sk-confirmed">
      <p className="text-callout font-semibold">
        {done} of {of} devices active in the last 24 h have key list {serial}
      </p>
      {of > 0 && (
        <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-hairline" aria-hidden>
          <div className="h-full rounded-full bg-accent transition-[width]" style={{ width: `${(done / of) * 100}%` }} />
        </div>
      )}
      <p className="mt-2 text-footnote text-muted">
        Devices fetch the list with every policy check (policy.refresh-minutes, or every minute with Fast apply). Versions before key rotation never report one; they stop working after the switch. Updates every 30 s.
      </p>
    </div>
  );
}

const STEPS: { id: Step; title: string }[] = [
  { id: "generate", title: "Generate the new key" },
  { id: "announce", title: "Announce it to the devices" },
  { id: "switch", title: "Switch the relays to the new key" },
  { id: "retire", title: "Retire the old key on the devices" },
  { id: "relays", title: "Stop accepting the old key on the relays" },
];

export function ServerKey() {
  const { state, reload, toast } = useApp();
  const [relays, setRelays] = useState<LiveKeys[] | null>(null);
  const [rot, setRot] = useState<Rotation>({});
  const [current, setCurrent] = useState("");
  const [pass, setPass] = useState("");
  const [expiry, setExpiry] = useState("");
  const [sheet, setSheet] = useState<"announce" | "retire" | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const r = await invoke<{ relays: LiveKeys[]; rotation: Rotation }>("serverkeys_live");
      setRelays(r.relays);
      setRot(r.rotation ?? {});
    } catch (e: any) {
      setError(e.message);
    }
  }, []);
  useEffect(() => void load(), [load]);

  const live = newest(relays ?? []);
  const at = step(rot, live);
  const index = STEPS.findIndex((s) => s.id === at);
  const save = async (r: Rotation) => {
    await invoke("serverkeys_rotation_set", { rotation: r });
    setRot(r);
  };
  const run = async (f: () => Promise<unknown>) => {
    setError(null);
    try {
      await f();
    } catch (e: any) {
      setError(e.message);
    }
  };
  const fromLive = live?.keys[0] ?? "";
  const from = rot.from ?? (current || fromLive);

  const publish = async (keys: ReturnType<typeof retireList>): Promise<RelayResult[]> => {
    const r = await invoke<{ serial: number; relays: RelayResult[] }>("serverkeys_publish", { keys });
    await load();
    return r.relays;
  };

  return (
    <section className="rounded-2xl border border-hairline bg-tile p-6" aria-label="Server key">
      <h2 className="text-headline font-semibold">Server key</h2>
      <p className="mb-3 mt-1 text-caption text-muted">
        hbbs and hbbr sign device identities with the server key (<code className="font-mono">/var/lib/tether/id_ed25519</code>). Devices trust the key built into them
        (<code className="font-mono">TETHER_PUB_KEY</code>) until a server key list signed with the admin key replaces it. Rotating the key needs no reinstall.
      </p>

      <h3 className="mb-2 text-caption font-semibold">Key list on the relays</h3>
      {!relays ? (
        <p className="text-caption text-muted">Checking the relays…</p>
      ) : (
        <ul className="flex flex-col gap-1.5" data-testid="sk-relays">
          {relays.map((r) => (
            <li key={r.server} className="flex flex-wrap items-center gap-2 text-caption">
              <StatusDot tone={!r.reachable ? "danger" : r.verified ? (r.serial === live?.serial ? "ok" : "warn") : r.serial != null ? "danger" : "muted"} />
              <span className="font-semibold">{host(r.server)}</span>
              {r.verified ? (
                <>
                  <span className="text-muted">serial {r.serial}</span>
                  {r.keys?.map((k, i) => (
                    <Badge key={k.key} tone={i === 0 ? "accent" : "muted"} title={k.key}>
                      {short(k.key)}
                      {i === 0 ? " · presented" : ""}
                      {k.expires ? ` · until ${new Date(k.expires * 1000).toLocaleDateString()}` : ""}
                    </Badge>
                  ))}
                  {r.serial !== live?.serial && <Badge tone="warn">Behind</Badge>}
                </>
              ) : (
                <span className={r.reachable && r.serial == null ? "text-muted" : "text-danger"}>{r.note}</span>
              )}
            </li>
          ))}
        </ul>
      )}
      {relays && !live && <p className="mt-2 text-caption text-muted">No list published: devices trust only the key they were built with.</p>}

      <h3 className="mb-3 mt-6 text-caption font-semibold">Rotate the server key</h3>
      <ol className="flex flex-col gap-4" data-testid="sk-steps" data-step={at}>
        {STEPS.map((s, i) => {
          const done = at === "done" || i < index;
          const now = s.id === at;
          return (
            <li key={s.id} className={`rounded-xl border p-4 ${now ? "border-accent/60" : "border-hairline"} ${!now && !done ? "opacity-60" : ""}`} data-testid={`sk-step-${s.id}`}>
              <p className="flex items-center gap-2 text-callout font-semibold">
                <span className={`grid h-5 w-5 place-items-center rounded-full text-footnote ${done ? "bg-ok text-white" : now ? "bg-accent text-white" : "bg-hairline"}`}>{done ? <Check size={12} /> : i + 1}</span>
                {s.title}
              </p>
              {now && s.id === "generate" && (
                <div className="mt-3 flex flex-col gap-3">
                  <Label text="Current server public key" hint="The key the installed apps were built with (TETHER_PUB_KEY, /var/lib/tether/id_ed25519.pub on the relays).">
                    <TextField value={current || fromLive} onChange={(e) => setCurrent(e.target.value.trim())} placeholder="base64, 44 characters" className="font-mono" />
                  </Label>
                  <div className="flex items-end gap-2">
                    <Label text="Passphrase" hint="The new private key is sealed into the key file with it.">
                      <TextField type="password" value={pass} onChange={(e) => setPass(e.target.value)} />
                    </Label>
                    <Button
                      variant="primary"
                      disabled={!isKey(current || fromLive) || !pass}
                      onClick={() =>
                        run(async () => {
                          const to = await invoke<string>("serverkeys_generate", { passphrase: pass });
                          setPass("");
                          await save({ from: current || fromLive, to });
                          await reload();
                          toast("New server key generated and sealed");
                        })
                      }
                    >
                      Generate new key
                    </Button>
                  </div>
                </div>
              )}
              {now && s.id === "announce" && (
                <div className="mt-3 flex flex-col gap-3">
                  <p className="text-caption text-muted">
                    Publishes the list <b>current {short(rot.from)}</b> + <b>new {short(rot.to)}</b>. Devices keep presenting the current key and start to trust the new one. Nothing changes for them yet.
                  </p>
                  <CopyField label="new server public key" value={rot.to ?? ""} />
                  <Label text="Old key stops being trusted on (optional)" hint="Devices then drop it at that time even if they miss the last step. Switch the relays before this date.">
                    <TextField type="date" value={expiry} onChange={(e) => setExpiry(e.target.value)} className="max-w-[220px]" />
                  </Label>
                  <div>
                    <Button variant="primary" onClick={() => setSheet("announce")}>
                      Publish key list…
                    </Button>
                  </div>
                </div>
              )}
              {now && s.id === "switch" && live && (
                <div className="mt-3 flex flex-col gap-3">
                  <Confirmations serial={live.serial} />
                  <p className="text-caption text-muted">
                    When enough devices have the list: install the new key on every relay, one at a time. hbbs and hbbr then sign with the new key and still accept the old one. Devices without the list stop
                    connecting until they fetch it. Copy <code className="font-mono">server/rotate-key.sh</code> next to the key files.
                  </p>
                  <div>
                    <Button
                      onClick={() =>
                        run(async () => {
                          const folder = await open({ directory: true, title: "Folder for id_ed25519 and id_ed25519.pub" });
                          if (typeof folder !== "string") return;
                          await invoke("serverkeys_save_files", { public: rot.to, folder });
                          toast("Key files saved. Delete them after installing.");
                        })
                      }
                    >
                      Save key files…
                    </Button>
                  </div>
                  <Commands label="relay commands" text={switchCommands(state.servers)} />
                  <p className="text-footnote text-muted">Kubernetes (Helm):</p>
                  <Commands label="Helm command" text={helmSwitch(rot.from ?? "")} />
                  <p className="text-caption text-muted">
                    From now on build new installers with <code className="font-mono">TETHER_PUB_KEY={rot.to}</code>.
                  </p>
                  <div>
                    <Button variant="primary" onClick={() => run(() => save({ ...rot, switched: true }))}>
                      Every relay is switched
                    </Button>
                  </div>
                </div>
              )}
              {now && s.id === "retire" && (
                <div className="mt-3 flex flex-col gap-3">
                  <p className="text-caption text-muted">Publishes a list with only the new key. Devices present the new key and stop trusting the old one.</p>
                  <div>
                    <Button variant="primary" onClick={() => setSheet("retire")}>
                      Publish list without the old key…
                    </Button>
                  </div>
                </div>
              )}
              {now && s.id === "relays" && live && (
                <div className="mt-3 flex flex-col gap-3">
                  <Confirmations serial={live.serial} />
                  <p className="text-caption text-muted">When enough devices have this list, stop accepting the old key on every relay. Devices that still present it get "Server key mismatch" until they fetch the list.</p>
                  <Commands label="relay commands" text={retireCommands(state.servers)} />
                  <Commands label="Helm command" text={HELM_RETIRE} />
                  <div>
                    <Button variant="primary" onClick={() => run(() => save({ ...rot, retired: true }))}>
                      Every relay is done
                    </Button>
                  </div>
                </div>
              )}
            </li>
          );
        })}
      </ol>
      {at === "done" && (
        <div className="mt-4 flex flex-wrap items-center gap-3 text-caption" data-testid="sk-done">
          <span>
            Rotation finished. Current key <code className="font-mono">{short(rot.to)}</code>.
          </span>
          <Button size="sm" onClick={() => run(() => save({}))}>
            Start another rotation
          </Button>
        </div>
      )}
      {at !== "generate" && at !== "done" && (
        <p className="mt-3 text-footnote text-faint">
          Rotating {short(from)} → {short(rot.to)}.{" "}
          <button className="underline" onClick={() => run(() => save({}))}>
            Cancel rotation
          </button>{" "}
          (the generated key stays in the key file).
        </p>
      )}
      <div className="mt-3">
        <ErrorText error={error} />
      </div>
      {sheet && (
        <PublishSheet
          title={sheet === "announce" ? "Publish key list: current + new" : "Publish key list: new key only"}
          action="Publish"
          onClose={() => setSheet(null)}
          run={() => publish(sheet === "announce" ? announceList(rot, expiry ? Math.floor(new Date(expiry).getTime() / 1000) : undefined) : retireList(rot))}
          after={(r) => r.some((x) => x.ok) && <p className="mt-4 text-caption text-muted">Devices pick it up with their next policy check.</p>}
        >
          <ul className="flex flex-col gap-1.5 text-caption">
            {(sheet === "announce" ? announceList(rot) : retireList(rot)).map((k, i) => (
              <li key={k.key} className="flex items-center gap-2">
                <Badge tone={i === 0 ? "accent" : "muted"}>{i === 0 ? "presented" : "trusted"}</Badge>
                <code className="font-mono">{k.key}</code>
                {k.key === rot.from ? "current" : "new"}
              </li>
            ))}
          </ul>
        </PublishSheet>
      )}
    </section>
  );
}
