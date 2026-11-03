import { useEffect, useMemo, useState } from "react";
import qrcode from "qrcode-generator";
import { Info, KeyRound, Lock, Monitor, Moon, Network, Palette, Shield, Sun, X } from "lucide-react";
import { api, type Status } from "../lib/ipc";
import { getThemePref, setThemePref, type ThemePref } from "../lib/theme";
import { Button, IconButton, Segmented, TextField, Toggle } from "../components/ui";
import { passwordStrength } from "../lib/security";

type Section = "security" | "access" | "appearance" | "network" | "about";

const NAV: { id: Section; label: string; icon: typeof Shield }[] = [
  { id: "security", label: "Security", icon: Shield },
  { id: "access", label: "Unattended access", icon: KeyRound },
  { id: "appearance", label: "Appearance", icon: Palette },
  { id: "network", label: "Network", icon: Network },
  { id: "about", label: "About", icon: Info },
];

function useOption(key: string, def = "") {
  const [v, setV] = useState(def);
  useEffect(() => {
    api.getOption(key).then((x) => setV(x || def)).catch(() => {});
  }, [key, def]);
  const set = (nv: string) => {
    setV(nv);
    api.setOption(key, nv === def ? "" : nv);
  };
  return [v, set] as const;
}

function Row({ title, desc, children }: { title: string; desc?: string; children: React.ReactNode }) {
  return (
    <div className="flex items-center gap-6 border-b border-hairline py-4 last:border-0">
      <div className="min-w-0 flex-1">
        <div className="text-callout">{title}</div>
        {desc && <div className="mt-0.5 text-caption leading-relaxed text-muted">{desc}</div>}
      </div>
      {children}
    </div>
  );
}

function BoolOption({ k, title, desc }: { k: string; title: string; desc?: string }) {
  // Core options use "N" to disable and empty for the default (enabled).
  const [v, set] = useOption(k, "Y");
  return (
    <Row title={title} desc={desc}>
      <Toggle label={title} checked={v !== "N"} onChange={(on) => set(on ? "Y" : "N")} />
    </Row>
  );
}

function Security() {
  const [approve, setApprove] = useOption("approve-mode", "both");
  return (
    <>
      <div className="flex items-start gap-3 rounded-2xl bg-parchment p-4">
        <Lock size={16} className="mt-0.5 shrink-0 text-ok" />
        <div className="text-caption text-muted">
          <span className="font-semibold text-ink">End-to-end encryption is always on.</span> Every session is encrypted between the two devices. Connections
          that cannot be encrypted are refused, and the relay server cannot see your screen, keystrokes or files.
        </div>
      </div>
      <Row title="How others get in" desc="Choose whether incoming sessions need you to click Accept, a password, or either.">
        <Segmented
          value={approve as "both" | "click" | "password"}
          onChange={setApprove}
          options={[
            { value: "both", label: "Either" },
            { value: "click", label: "Accept only" },
            { value: "password", label: "Password only" },
          ]}
        />
      </Row>
      <BoolOption k="enable-keyboard" title="Allow control" desc="Remote users can use mouse and keyboard." />
      <BoolOption k="enable-clipboard" title="Allow clipboard" />
      <BoolOption k="enable-file-transfer" title="Allow file transfer" />
      <BoolOption k="enable-audio" title="Share audio" />
    </>
  );
}

function Access({ status }: { status: Status | null }) {
  const [pw, setPw] = useState("");
  const [pw2, setPw2] = useState("");
  const [msg, setMsg] = useState<{ ok: boolean; text: string } | null>(null);
  const [verify, setVerify] = useOption("verification-method", "both");
  const st = passwordStrength(pw);
  const match = pw === pw2;
  return (
    <>
      <div className="rounded-2xl bg-parchment p-4 text-caption text-muted">
        Unattended access lets you reach this device any time with a permanent password, without anyone in front of it.
        {status && !status.installed && " Installing Tether as a service also lets you reach the sign-in screen and administrator prompts."}
      </div>
      <Row title="Permanent password" desc={st.hint}>
        <form
          className="flex w-[280px] flex-col gap-2"
          onSubmit={async (e) => {
            e.preventDefault();
            try {
              await api.setPermanentPassword(pw);
              setMsg({ ok: true, text: "Password saved." });
              setPw("");
              setPw2("");
            } catch (err) {
              setMsg({ ok: false, text: String(err) });
            }
          }}
        >
          <TextField type="password" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} placeholder="New password" aria-label="New password" className="!h-9" />
          <TextField
            type="password"
            autoComplete="new-password"
            value={pw2}
            onChange={(e) => setPw2(e.target.value)}
            placeholder="Repeat password"
            aria-label="Repeat password"
            aria-invalid={!!pw2 && !match}
            className={`!h-9 ${pw2 && !match ? "!border-danger" : ""}`}
          />
          <Button size="sm" variant="primary" type="submit" disabled={!st.ok || !match}>
            Set password
          </Button>
          {msg && <div className={`text-footnote ${msg.ok ? "text-ok" : "text-danger"}`}>{msg.text}</div>}
        </form>
      </Row>
      <Row title="Accepted passwords">
        <Segmented
          value={verify as "both" | "use-temporary-password" | "use-permanent-password"}
          onChange={setVerify}
          options={[
            { value: "both", label: "Both" },
            { value: "use-temporary-password", label: "One-time" },
            { value: "use-permanent-password", label: "Permanent" },
          ]}
        />
      </Row>
      <TwoFactor />
      {status && !status.installed && (
        <Row title="Install as service" desc="Starts with the computer and stays reachable when nobody is logged in. Requires administrator rights.">
          <Button size="sm" onClick={() => api.installService()}>
            Install
          </Button>
        </Row>
      )}
    </>
  );
}

function Qr({ text, size = 168 }: { text: string; size?: number }) {
  const svg = useMemo(() => {
    const q = qrcode(0, "M");
    q.addData(text);
    q.make();
    return q.createSvgTag({ cellSize: 4, margin: 2, scalable: true });
  }, [text]);
  // SVG is generated locally from our own otpauth URI, not from untrusted input.
  return <div style={{ width: size, height: size }} className="overflow-hidden rounded-md bg-white p-1" dangerouslySetInnerHTML={{ __html: svg }} />;
}

function TwoFactor() {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [uri, setUri] = useState<string | null>(null);
  const [code, setCode] = useState("");
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api.tfaEnabled().then(setEnabled).catch(() => setEnabled(false));
  }, []);
  const secret = uri ? new URLSearchParams(uri.split("?")[1] ?? "").get("secret") : null;
  return (
    <Row title="Two-factor authentication" desc="Incoming sessions also need a 6-digit code from an authenticator app.">
      {enabled ? (
        <Button
          size="sm"
          variant="danger"
          onClick={async () => {
            if (!confirm("Turn off two-factor authentication?")) return;
            await api.tfaDisable();
            setEnabled(false);
          }}
        >
          Turn off
        </Button>
      ) : uri ? (
        <form
          className="flex w-[280px] flex-col items-start gap-3"
          onSubmit={async (e) => {
            e.preventDefault();
            try {
              await api.tfaConfirm(code);
              setEnabled(true);
              setUri(null);
              setErr(null);
            } catch (x) {
              setErr(String(x));
            }
          }}
        >
          <Qr text={uri} />
          {secret && (
            <div className="text-footnote text-muted">
              Or enter the key <span className="select-all font-mono text-ink">{secret}</span>
            </div>
          )}
          <TextField
            inputMode="numeric"
            autoComplete="one-time-code"
            maxLength={6}
            value={code}
            onChange={(e) => setCode(e.target.value.replace(/[^0-9]/g, ""))}
            placeholder="6-digit code"
            aria-label="Authenticator code"
            className="!h-9 font-mono tracking-[0.3em]"
          />
          {err && <div className="text-footnote text-danger">{err}</div>}
          <div className="flex gap-2">
            <Button size="sm" type="button" onClick={() => setUri(null)}>
              Cancel
            </Button>
            <Button size="sm" variant="primary" type="submit" disabled={code.length !== 6}>
              Verify and turn on
            </Button>
          </div>
        </form>
      ) : (
        <Button size="sm" variant="secondary" disabled={enabled === null} onClick={async () => setUri(await api.tfaBegin())}>
          Set up
        </Button>
      )}
    </Row>
  );
}

function Appearance() {
  const [theme, setTheme] = useState<ThemePref>(getThemePref());
  return (
    <Row title="Theme">
      <div className="flex gap-2">
        {(
          [
            ["system", "System", Monitor],
            ["light", "Light", Sun],
            ["dark", "Dark", Moon],
          ] as const
        ).map(([v, label, Icon]) => (
          <button
            key={v}
            onClick={() => {
              setTheme(v);
              setThemePref(v);
            }}
            className={`flex w-24 flex-col items-center gap-2 rounded-xl border p-3 text-caption transition ${theme === v ? "border-accent text-ink" : "border-hairline text-muted hover:text-ink"}`}
          >
            <Icon size={18} />
            {label}
          </button>
        ))}
      </div>
    </Row>
  );
}

function NetworkSection() {
  const [server, setServer] = useOption("custom-rendezvous-server");
  const [relay, setRelay] = useOption("relay-server");
  const [key, setKey] = useOption("key");
  const [draft, setDraft] = useState<{ server: string; relay: string; key: string } | null>(null);
  const d = draft ?? { server, relay, key };
  const field = (label: string, k: keyof typeof d, ph: string) => (
    <label className="block">
      <span className="text-caption text-muted">{label}</span>
      <input
        value={d[k]}
        spellCheck={false}
        onChange={(e) => setDraft({ ...d, [k]: e.target.value })}
        placeholder={ph}
        className="mt-1 h-9 w-full rounded-xl border border-hairline bg-parchment px-3.5 font-mono text-caption outline-none focus-visible:border-focus"
      />
    </label>
  );
  return (
    <>
      <div className="rounded-2xl bg-parchment p-4 text-caption leading-relaxed text-muted">
        Tether uses your organisation's built-in server. Override it only if your administrator asked you to.
      </div>
      <div className="mt-4 space-y-3">
        {field("ID server", "server", "built-in")}
        {field("Relay server", "relay", "same as ID server")}
        {field("Public key", "key", "built-in")}
      </div>
      <div className="mt-4 flex justify-end gap-2">
        {draft && (
          <Button size="sm" variant="ghost" onClick={() => setDraft(null)}>
            Discard
          </Button>
        )}
        <Button
          size="sm"
          variant="primary"
          disabled={!draft}
          onClick={() => {
            setServer(d.server.trim());
            setRelay(d.relay.trim());
            setKey(d.key.trim());
            setDraft(null);
          }}
        >
          Apply
        </Button>
      </div>
    </>
  );
}

function About({ status }: { status: Status | null }) {
  return (
    <div className="space-y-3 text-caption leading-relaxed text-muted">
      <div>
        <span className="text-ink">Tether</span> {status?.version ?? ""}
      </div>
      <p>Fast, private remote desktop. Built on the RustDesk open-source core.</p>
      <p>
        Licensed under the GNU Affero General Public License v3. You can get the complete source code of this build, including our changes, from your
        administrator or the project repository.
      </p>
    </div>
  );
}

export function Settings({ onClose, status }: { onClose: () => void; status: Status | null }) {
  const [sec, setSec] = useState<Section>("security");
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="animate-fade fixed inset-0 z-30 flex bg-black/40" onMouseDown={onClose}>
      <div
        className="animate-rise m-auto flex h-[min(600px,92vh)] w-[min(860px,94vw)] overflow-hidden rounded-2xl border border-hairline bg-tile shadow-float"
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-label="Settings"
      >
        <nav className="flex w-56 shrink-0 flex-col gap-0.5 border-r border-hairline bg-parchment p-3">
          <div className="px-3 pb-3 pt-2 text-callout font-semibold">Settings</div>
          {NAV.map(({ id, label, icon: Icon }) => (
            <button
              key={id}
              onClick={() => setSec(id)}
              className={`flex items-center gap-2.5 h-9 rounded-md px-3 text-left text-caption transition ${sec === id ? "bg-accent/12 text-ink" : "text-muted hover:bg-tile-2 hover:text-ink"}`}
            >
              <Icon size={16} />
              {label}
            </button>
          ))}
        </nav>
        <div className="flex min-w-0 flex-1 flex-col">
          <div className="flex items-center justify-between px-7 pb-2 pt-5">
            <h2 className="text-title font-semibold">{NAV.find((n) => n.id === sec)!.label}</h2>
            <IconButton label="Close settings" onClick={onClose}>
              <X size={18} />
            </IconButton>
          </div>
          <div className="min-h-0 flex-1 overflow-y-auto px-7 pb-7">
            {sec === "security" && <Security />}
            {sec === "access" && <Access status={status} />}
            {sec === "appearance" && <Appearance />}
            {sec === "network" && <NetworkSection />}
            {sec === "about" && <About status={status} />}
          </div>
        </div>
      </div>
    </div>
  );
}
