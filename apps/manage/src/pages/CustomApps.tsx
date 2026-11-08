import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { ImageOff, Link2, Plus, RefreshCw } from "lucide-react";
import { useApp } from "../App";
import { invoke, when, type Feature, type Layer, type RelayResult } from "../lib/ipc";
import { byArea, FeatureRow } from "../components/features";
import { PublishSheet, RelayResults } from "../components/publish";
import { Badge, Button, Card, CopyField, ErrorText, IconButton, Label, Modal, PageHeader, TextArea, TextField } from "../components/ui";

interface CustomProfile {
  id: string;
  name: string;
  accent: string;
  welcome: string;
  logo_png: string;
  layer: Layer;
  issued_at: number;
}
interface CustomRow {
  id: string;
  name: string;
  token: string;
  expires: number;
  channel: string;
  created?: number;
  downloads?: number;
  last_download?: number | null;
  link: string;
  status: "active" | "expired" | "not-on-relay";
  /** Logo, colours and settings: only on the PC that created it. */
  local: CustomProfile | null;
}
interface Form {
  id?: string;
  name: string;
  accent: string;
  welcome: string;
  logo_png: string;
  layer: Layer;
  channel: string;
  days: number;
  new_link: boolean;
}

const MAX_LOGO = 96 * 1024;

/** Scale a picture down to a PNG of at most 96 KiB (256 px first, smaller if needed); base64. */
async function toLogo(file: File): Promise<string> {
  // An <img> also decodes SVG (createImageBitmap does not).
  const url = URL.createObjectURL(file);
  const img = new Image();
  try {
    img.src = url;
    await img.decode();
  } catch {
    throw new Error("This file is not a picture.");
  } finally {
    URL.revokeObjectURL(url);
  }
  for (const size of [256, 192, 128, 96, 64]) {
    const s = Math.min(1, size / Math.max(img.naturalWidth || size, img.naturalHeight || size));
    const c = document.createElement("canvas");
    c.width = Math.max(1, Math.round((img.naturalWidth || size) * s));
    c.height = Math.max(1, Math.round((img.naturalHeight || size) * s));
    c.getContext("2d")!.drawImage(img, 0, 0, c.width, c.height);
    const b64 = c.toDataURL("image/png").split(",")[1];
    if ((b64.length * 3) / 4 <= MAX_LOGO) return b64;
  }
  throw new Error("The logo stays above 96 KiB even at 64 px. Use a simpler picture.");
}

const days = (unix: number) => Math.round((unix - Date.now() / 1000) / 86400);

function Logo({ png, accent, size = 40 }: { png?: string; accent?: string; size?: number }) {
  return png ? (
    <img src={`data:image/png;base64,${png}`} alt="" width={size} height={size} className="shrink-0 rounded-md border border-hairline bg-tile object-contain" style={{ width: size, height: size }} />
  ) : (
    <span className="grid shrink-0 place-items-center rounded-md border border-hairline text-muted" style={{ width: size, height: size, background: accent || undefined }}>
      <ImageOff size={16} className={accent ? "text-white" : ""} />
    </span>
  );
}

function Editor({ form, set, features, onReview, onClose }: { form: Form; set: (f: Form) => void; features: Feature[]; onReview: () => void; onClose: () => void }) {
  const [logoError, setLogoError] = useState<string | null>(null);
  // Name, colour and welcome text have their own fields above.
  const areas = byArea(features, "customer", (f) => !["customer.title", "customer.accent-color", "customer.welcome"].includes(f.id));
  const ok = form.name.trim() && form.days >= 1 && form.days <= 366 && (!form.accent || /^#[0-9a-fA-F]{6}$/.test(form.accent));
  return (
    <Modal
      onClose={onClose}
      width={860}
      title={
        <>
          <h2 className="text-headline font-semibold">{form.id ? `Edit ${form.name}` : "New custom app"}</h2>
          <p className="mt-1 text-caption text-muted">A TetherQS download with your customer's name, logo, colour and settings, behind a private link.</p>
        </>
      }
      footer={
        <div className="flex justify-end gap-2">
          <Button onClick={onClose}>Cancel</Button>
          <Button variant="primary" disabled={!ok} onClick={onReview}>
            Review…
          </Button>
        </div>
      }
    >
      <div className="grid grid-cols-2 gap-x-6 gap-y-4">
        <Label text="Name" hint="Window title and company name in TetherQS; also the download's file name. 1–64 characters.">
          <TextField autoFocus maxLength={64} value={form.name} onChange={(e) => set({ ...form, name: e.target.value })} placeholder="ACME Support" />
        </Label>
        <Label text="Logo" hint="PNG, JPEG or SVG; scaled to at most 256 × 256 px and 96 KiB.">
          <div className="flex items-center gap-3">
            <Logo png={form.logo_png} accent={form.accent} size={44} />
            {/* App-styled picker: the native file button is labelled in the OS language. */}
            <label className="inline-flex h-8 cursor-default items-center rounded-xl border border-hairline bg-tile-2 px-3.5 text-caption text-ink-80 hover:bg-parchment has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-focus">
              {form.logo_png ? "Replace picture…" : "Choose picture…"}
            <input
              type="file"
              accept="image/png,image/jpeg,image/svg+xml,image/webp"
              className="sr-only"
              onChange={async (e) => {
                const f = e.target.files?.[0];
                if (!f) return;
                setLogoError(null);
                try {
                  set({ ...form, logo_png: await toLogo(f) });
                } catch (err: any) {
                  setLogoError(err.message ?? "Not a picture.");
                }
              }}
            />
            </label>
            {form.logo_png && (
              <Button size="sm" variant="ghost" onClick={() => set({ ...form, logo_png: "" })}>
                Remove
              </Button>
            )}
          </div>
          <ErrorText error={logoError} />
        </Label>
        <Label text="Accent colour" hint="Buttons and highlights in TetherQS. Empty: the policy's colour.">
          <div className="flex items-center gap-2">
            <input type="color" aria-label="Pick accent colour" className="h-9 w-12 shrink-0 rounded-md border border-hairline bg-parchment" value={form.accent || "#0071e3"} onChange={(e) => set({ ...form, accent: e.target.value })} />
            <TextField value={form.accent} maxLength={7} placeholder="#0071e3" onChange={(e) => set({ ...form, accent: e.target.value.trim() })} />
          </div>
        </Label>
        <Label text="Update channel" hint="The TetherQS build the link serves.">
          <TextField value={form.channel} onChange={(e) => set({ ...form, channel: e.target.value })} />
        </Label>
        <div className="col-span-2">
          <Label text="Welcome text" hint="Shown above the ID and password. Empty: the policy's text. At most 400 characters.">
            <TextArea rows={2} maxLength={400} value={form.welcome} onChange={(e) => set({ ...form, welcome: e.target.value })} />
          </Label>
        </div>
        <Label text="Link valid for (days)" hint={`1–366. Until ${new Date(Date.now() + form.days * 86400_000).toLocaleDateString()}.`}>
          <TextField type="number" min={1} max={366} value={form.days} onChange={(e) => set({ ...form, days: Number(e.target.value) })} />
        </Label>
        {form.id && (
          <label className="flex items-start gap-2 self-center text-caption">
            <input type="checkbox" className="mt-0.5" checked={form.new_link} onChange={(e) => set({ ...form, new_link: e.target.checked })} />
            <span>Issue a new link (the current link stops working)</span>
          </label>
        )}
        <div className="col-span-2 mt-2">
          <h3 className="text-caption font-semibold">Settings for everyone using this download</h3>
          <p className="mb-2 text-footnote text-muted">Customer-module features only. They apply on top of the customer profile of the policy; groups and machine policy still win.</p>
          {areas.map(([area, list]) => (
            <div key={area} className="mt-3">
              <h4 className="mb-1 text-footnote font-semibold capitalize text-muted">{area}</h4>
              {list.map((f) => (
                <FeatureRow
                  key={f.id}
                  f={f}
                  rule={form.layer[f.id]}
                  lang="en"
                  onChange={(r) => {
                    const layer = { ...form.layer };
                    if (r) layer[f.id] = r;
                    else delete layer[f.id];
                    set({ ...form, layer });
                  }}
                />
              ))}
            </div>
          ))}
        </div>
      </div>
    </Modal>
  );
}

export function CustomApps() {
  const { toast } = useApp();
  const [rows, setRows] = useState<CustomRow[] | null>(null);
  const [features, setFeatures] = useState<Feature[]>([]);
  const [form, setForm] = useState<Form | null>(null);
  const [step, setStep] = useState<"edit" | "review" | null>(null);
  const [link, setLink] = useState<string | null>(null);
  const [results, setResults] = useState<RelayResult[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () => invoke<CustomRow[]>("custom_list").then(setRows, (e) => setError(e.message));
  useEffect(() => {
    load();
    invoke<Feature[]>("features").then(setFeatures);
  }, []);

  const fromRow = (r: CustomRow, extra: Partial<Form> = {}): Form => ({
    id: r.id,
    name: r.local!.name,
    accent: r.local!.accent,
    welcome: r.local!.welcome,
    logo_png: r.local!.logo_png,
    layer: r.local!.layer,
    channel: r.channel,
    days: Math.max(1, Math.min(366, days(r.expires))),
    new_link: false,
    ...extra,
  });

  const save = async (f: Form) => {
    const r = await invoke<{ id: string; link: string; relays: RelayResult[] }>("custom_save", { args: { ...f, id: f.id ?? null, channel: f.channel.trim() } });
    setLink(r.link);
    load();
    return r.relays;
  };

  /** Quick actions on a row: same command, no editor. */
  const quick = async (f: Form, done: string) => {
    setError(null);
    try {
      setResults(await save(f));
      toast(done);
    } catch (e: any) {
      setError(e.message);
    }
  };

  return (
    <>
      <PageHeader title="Custom apps" sub="Branded TetherQS downloads for one customer each: name, logo, colour and settings, behind a private link that expires.">
        <IconButton label="Refresh" onClick={load}>
          <RefreshCw size={16} />
        </IconButton>
        <Button
          variant="primary"
          onClick={() => {
            setForm({ name: "", accent: "", welcome: "", logo_png: "", layer: {}, channel: "stable", days: 30, new_link: false });
            setStep("edit");
          }}
        >
          <Plus size={16} /> New custom app
        </Button>
      </PageHeader>
      <Card className="mb-6 flex items-start gap-3 py-4">
        <Link2 size={18} className="mt-0.5 shrink-0 text-accent" />
        <p className="text-caption text-muted">
          <span className="font-semibold text-ink">The link is private.</span> Anyone who has it can download this app until it expires, so send it only to the customer it is for. It is 256-bit random, not listed anywhere and cannot be guessed; an unknown, expired or revoked link answers the same "not found". Revoke it or issue a new link at any time. The one link serves Windows and macOS: a Mac browser gets the zipped TetherQS.app, everything else the .exe; add <code>/mac</code> or <code>/windows</code> to the link to choose.
        </p>
      </Card>
      <ErrorText error={error} />
      <div className="flex flex-col gap-3">
        {rows?.map((r) => {
          const left = days(r.expires);
          return (
            <Card key={r.id} className="flex flex-wrap items-center gap-x-5 gap-y-3 py-4">
              <Logo png={r.local?.logo_png} accent={r.local?.accent} />
              <div className="w-48 min-w-0">
                <p className="truncate text-callout font-semibold">{r.name}</p>
                <p className="flex items-center gap-1.5 text-footnote text-muted">
                  {r.local?.accent && <span className="inline-block h-2.5 w-2.5 rounded-full" style={{ background: r.local.accent }} />}
                  {r.channel} · {r.local ? `${Object.keys(r.local.layer).length} settings` : "created on another PC"}
                </p>
              </div>
              <div className="min-w-[260px] flex-1">
                <CopyField label={`link of ${r.name}`} value={r.link} />
              </div>
              <div className="w-40 text-caption">
                {r.status === "active" ? <Badge tone={left <= 7 ? "warn" : "ok"}>{left <= 0 ? "Expires today" : `${left} days left`}</Badge> : r.status === "expired" ? <Badge tone="danger">Expired</Badge> : <Badge tone="warn">Not on the relay</Badge>}
                <p className="mt-1 text-footnote text-muted">until {when(r.expires)}</p>
              </div>
              <div className="w-28 text-caption">
                <p className="font-semibold">{r.downloads ?? 0} download{r.downloads === 1 ? "" : "s"}</p>
                <p className="text-footnote text-muted">{r.last_download ? `last ${when(r.last_download)}` : "none yet"}</p>
              </div>
              <div className="flex gap-1.5">
                <Button size="sm" disabled={!r.local} title={r.local ? undefined : "Created on another PC: its settings are not here"} onClick={() => (setForm(fromRow(r)), setStep("edit"))}>
                  Edit
                </Button>
                <Button size="sm" disabled={!r.local} onClick={() => quick(fromRow(r, { days: Math.min(366, Math.max(left, 0) + 30) }), `${r.name}: link valid 30 days longer`)}>
                  +30 days
                </Button>
                <Button
                  size="sm"
                  disabled={!r.local}
                  onClick={async () => {
                    if (await ask(`Issue a new link for ${r.name}? The current link stops working at once.`, { title: "New link", kind: "warning" })) quick(fromRow(r, { new_link: true }), `${r.name}: new link issued`);
                  }}
                >
                  New link
                </Button>
                <Button
                  size="sm"
                  variant="danger"
                  onClick={async () => {
                    if (!(await ask(`Revoke ${r.name}? The link stops working at once on every relay. Apps already downloaded keep working.`, { title: "Revoke", kind: "warning" }))) return;
                    try {
                      setResults(await invoke<RelayResult[]>("custom_delete", { id: r.id }));
                      setLink(null);
                      load();
                    } catch (e: any) {
                      setError(e.message);
                    }
                  }}
                >
                  Revoke
                </Button>
              </div>
            </Card>
          );
        })}
        {rows && !rows.length && <p className="py-10 text-center text-caption text-muted">No custom apps yet.</p>}
      </div>

      {form && step === "edit" && <Editor form={form} set={setForm} features={features} onClose={() => setStep(null)} onReview={() => setStep("review")} />}
      {form && step === "review" && (
        <PublishSheet
          title={form.id ? `Save ${form.name}` : `Create ${form.name}`}
          action={form.id ? "Save" : "Create"}
          run={() => save(form)}
          onClose={() => {
            setStep(null);
            setLink(null);
          }}
          after={() =>
            link && (
              <div className="mt-5">
                <p className="mb-2 text-caption font-semibold">Private download link</p>
                <CopyField label="private link" value={link} />
                <p className="mt-2 text-footnote text-muted">Send it only to {form.name}. It works on every relay until {new Date(Date.now() + form.days * 86400_000).toLocaleDateString()}.</p>
              </div>
            )
          }
        >
          <div className="flex items-center gap-4">
            <Logo png={form.logo_png} accent={form.accent} size={56} />
            <div>
              <p className="text-callout font-semibold">{form.name.trim()}</p>
              <p className="text-caption text-muted">
                {form.accent || "policy colour"} · channel {form.channel} · {Object.keys(form.layer).length} settings · link valid {form.days} days
                {form.id ? (form.new_link ? " · new link" : " · same link") : ""}
              </p>
            </div>
          </div>
          {form.welcome.trim() && <p className="mt-3 rounded-xl bg-parchment p-3 text-caption">{form.welcome.trim()}</p>}
          <p className="mt-3 text-caption text-muted">Signed with the admin key; TetherQS checks the signature itself. Stored on every relay with the same link.</p>
          <div className="mt-3">
            <Button size="sm" variant="ghost" onClick={() => setStep("edit")}>
              Back to editing
            </Button>
          </div>
        </PublishSheet>
      )}
      {results && (
        <Modal
          onClose={() => setResults(null)}
          title={<h2 className="text-headline font-semibold">Result per relay</h2>}
          footer={
            <div className="flex justify-end">
              <Button
                variant="primary"
                onClick={() => {
                  setResults(null);
                  setLink(null);
                }}
              >
                Done
              </Button>
            </div>
          }
        >
          <RelayResults results={results} />
          {link && (
            <div className="mt-4">
              <CopyField label="private link" value={link} />
            </div>
          )}
        </Modal>
      )}
    </>
  );
}
