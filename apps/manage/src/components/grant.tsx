// The editor row of health.enrol-grant: the layer's grant for agents Tether installs (made with
// the vault's admin key in the backend, never typed in), its id and expiry, Renew and Revoke.
import { useEffect, useState } from "react";
import { ask } from "@tauri-apps/plugin-dialog";
import { host, invoke, when, type Feature, type Layer, type RelayResult } from "../lib/ipc";
import { afterRevoke, grantBadge, grantOf, needsGrant, noGrant, pendingText, revokeOutcome, withGrant, type GrantInfo, type PendingRevoke } from "../lib/grants";
import type { Lang } from "./features";
import { Badge, Button, ErrorText, TextField } from "./ui";

/** Grants still to revoke, each with Revoke again; `named` adds the layer (for a deleted group). */
export function PendingLines({ pending, named, onReload, toast }: { pending: PendingRevoke[]; named?: boolean; onReload: () => void; toast: (m: string) => void }) {
  const [busy, setBusy] = useState(false);
  const again = async () => {
    setBusy(true);
    try {
      toast(revokeOutcome(await invoke<RelayResult[]>("grants_revoke_pending"), host));
    } catch (e: any) {
      toast(e.message);
    } finally {
      setBusy(false);
      onReload();
    }
  };
  return (
    <>
      {pending.map((p) => (
        <p key={p.id} className="flex flex-wrap items-center gap-2 text-footnote text-warn">
          {pendingText(p, host, named ? p.layer.replace(/^group:(.*)$/, "group $1 (deleted)").replace(/^profile:(.*)$/, "the $1 profile") : undefined)}
          <Button size="sm" variant="ghost" disabled={busy} onClick={again}>
            Revoke again
          </Button>
        </p>
      ))}
    </>
  );
}

export function GrantRow({ f, lang, layerKey, layer, profile, info, pending, liveRevoked, onChange, onReload, toast }: {
  f: Feature;
  lang: Lang;
  layerKey: string;
  layer: Layer;
  /** A group's profile layer (its grant is inherited when the group has none). */
  profile?: Layer;
  info?: GrantInfo;
  pending: PendingRevoke[];
  /** The published policy still carries a grant of this layer that is revoked. */
  liveRevoked: boolean;
  onChange: (next: Layer) => void;
  onReload: () => void;
  toast: (m: string) => void;
}) {
  const token = grantOf(layer);
  const needs = needsGrant(layer);
  const [customer, setCustomer] = useState(info?.customer ?? "");
  // Renew keeps the grant's customer unless it is changed here.
  useEffect(() => setCustomer(info?.customer ?? ""), [info?.id]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const now = Math.floor(Date.now() / 1000);
  const badge = info && grantBadge(info, now);
  const none = noGrant(layer, profile);

  const run = async (what: () => Promise<void>) => {
    setBusy(true);
    setError(null);
    try {
      await what();
    } catch (e: any) {
      setError(e.message);
    } finally {
      setBusy(false);
      onReload();
    }
  };
  const create = () =>
    run(async () => {
      const t = await invoke<string>("grant_create", { layer: layerKey, customer, old: token || null });
      onChange(withGrant(layer, t));
      toast(token ? "New grant in the draft. Publish: each relay revokes the old one once it serves the new policy." : "Grant created in the draft. Publish to send it.");
    });
  const revoke = async () => {
    if (!(await ask(`Revoke this grant, and any older grant of this layer not yet revoked everywhere, on every relay now? New installs and re-enrolments with it stop at once; installed agents keep working.${needs ? " The draft sets health.agent of this layer to manual (nothing installed is removed); publish it." : ""}`, { title: "Revoke grant", kind: "warning" }))) return;
    run(async () => {
      const rs = await invoke<RelayResult[]>("grant_revoke", { layer: layerKey, token });
      onChange(afterRevoke(layer));
      toast(revokeOutcome(rs, host, true));
    });
  };

  return (
    <div data-feature={f.id} className={`flex flex-col gap-2 border-b border-l-[3px] border-b-hairline py-2.5 pl-3 pr-1 last:border-b-0 ${token ? "border-l-ink bg-tile-2" : "border-l-transparent"}`}>
      <div className="flex flex-wrap items-center gap-x-4 gap-y-2">
        <div className="min-w-[180px] flex-1">
          <p className="flex flex-wrap items-center gap-2 text-caption" title={lang === "en" ? f.de : f.en}>
            <span className={token ? "font-semibold" : ""}>{lang === "en" ? f.en : f.de}</span>
            {badge && <Badge tone={badge.tone}>{badge.text}</Badge>}
            {!token && none.badge && <Badge tone="warn">{none.badge}</Badge>}
          </p>
          <p className="font-mono text-footnote text-muted">
            {f.id} ·{" "}
            {token
              ? `grant ${info?.id ?? "?"} · expires ${when(info?.exp)}${info?.customer ? ` · customer ${info.customer}` : " · no customer"}${info && !info.made_here ? " · made on another PC" : ""}`
              : none.line}
          </p>
          {info?.why && <p className="text-footnote text-danger">{info.why}</p>}
          {liveRevoked && <p className="text-footnote text-danger">The published policy still has the revoked grant, so machines keep trying to enrol with it: publish the draft so machines stop trying.</p>}
        </div>
        <div className="ml-auto flex items-center gap-2">
          <div className="w-40">
            <TextField aria-label={`Customer reference for the grant of ${f.en}`} placeholder="Customer (optional)" maxLength={32} value={customer} onChange={(e) => setCustomer(e.target.value)} />
          </div>
          <Button size="sm" variant={needs && (!token || info?.status !== "valid" || info.renew) ? "primary" : "secondary"} disabled={busy} onClick={create} title="Makes a grant valid for 365 days with this vault's admin key">
            {token ? "Renew" : "Create grant"}
          </Button>
          {token && (
            <Button size="sm" variant="ghost" disabled={busy} onClick={revoke}>
              Revoke
            </Button>
          )}
        </div>
      </div>
      <PendingLines pending={pending} onReload={onReload} toast={toast} />
      <ErrorText error={error} />
    </div>
  );
}
