// The feature editor row, shared by the policy editor and the custom app settings.
import type { Feature, Rule, RuleState } from "../lib/ipc";
import { Badge, Segmented, Select, TextField, Toggle } from "./ui";

export type Lang = "en" | "de";

const STATES: { value: RuleState | "default"; label: string }[] = [
  { value: "default", label: "Default" },
  { value: "on", label: "On" },
  { value: "off", label: "Off" },
  { value: "locked", label: "Locked" },
  { value: "hidden", label: "Hidden" },
];

/** "Default" selected stays quiet (most rows); any other state is a filled accent segment. */
const stateSelected = (v: string) => (v === "default" ? "bg-tile text-ink font-semibold ring-1 ring-hairline" : "bg-accent text-on-accent font-semibold");

const words = (f: Feature, v: string) => (f.kind === "bool" ? (v === "Y" ? "on" : "off") : v || "empty");

function ValueEditor({ f, value, onChange }: { f: Feature; value: string; onChange: (v: string) => void }) {
  if (f.kind === "bool") return <Toggle label={f.en} checked={value === "Y"} onChange={(v) => onChange(v ? "Y" : "N")} />;
  if (f.kind === "choice")
    return (
      <Select value={value} onChange={(e) => onChange(e.target.value)} aria-label={f.en}>
        {f.choices.map((c) => (
          <option key={c}>{c}</option>
        ))}
      </Select>
    );
  return <TextField aria-label={f.en} type={f.kind === "number" ? "number" : "text"} min={0} value={value} onChange={(e) => onChange(e.target.value)} />;
}

/** Same rule (state and value), so "not published" is exact. */
export const sameRule = (a?: Rule, b?: Rule) => (a?.state ?? "") === (b?.state ?? "") && (a?.value ?? "") === (b?.value ?? "");

export function FeatureRow({ f, rule, lang, onChange, live }: { f: Feature; rule?: Rule; lang: Lang; onChange: (r?: Rule) => void; live?: Rule | null }) {
  const state = rule?.state ?? "default";
  const showValue = rule && (rule.state === "on" || rule.state === "locked");
  // `live` undefined: no comparison (custom apps); null: the live layer does not set it.
  const unpublished = live !== undefined && !sameRule(rule, live ?? undefined);
  return (
    <div
      data-feature={f.id}
      className={`flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-l-[3px] border-b-hairline py-2.5 pl-3 pr-1 last:border-b-0 ${rule ? "border-l-ink bg-tile-2" : "border-l-transparent"}`}
    >
      <div className="min-w-[180px] flex-1">
        <p className="flex flex-wrap items-center gap-2 text-caption" title={lang === "en" ? f.de : f.en}>
          <span className={rule ? "font-semibold" : ""}>{lang === "en" ? f.en : f.de}</span>
          {rule && <Badge tone="ink">Changed from default</Badge>}
          {unpublished && (
            <Badge tone="warn" title={live ? `Live: ${live.state}${live.value != null ? ` ${live.value}` : ""}` : "Live: default"}>
              Not published
            </Badge>
          )}
        </p>
        <p className="font-mono text-footnote text-muted">
          {f.id} · default {words(f, f.default)}
        </p>
      </div>
      <div className="ml-auto flex items-center gap-3">
        {showValue && (
          <div className={f.kind === "bool" ? "" : "w-40"}>
            <ValueEditor f={f} value={rule.value ?? (f.kind === "bool" ? "Y" : f.default)} onChange={(value) => onChange({ ...rule, value })} />
          </div>
        )}
        <Segmented
          label={`State of ${f.en}`}
          value={state}
          options={STATES}
          selected={stateSelected}
          onChange={(s) => onChange(s === "default" ? undefined : { state: s, ...(s === "on" || s === "locked" ? { value: rule?.value ?? (f.kind === "bool" ? "Y" : f.default) } : {}) })}
        />
      </div>
    </div>
  );
}

/** Features grouped by area, for one profile; `only` limits to the given ids. */
export function byArea(features: Feature[], profile: string, only?: (f: Feature) => boolean): [string, Feature[]][] {
  const m = new Map<string, Feature[]>();
  for (const f of features.filter((f) => f.profiles.includes(profile) && (!only || only(f)))) m.set(f.area, [...(m.get(f.area) ?? []), f]);
  return [...m];
}

/** A value as people read it: on/off for Y/N, "–" for empty. */
export const shown = (v: string) => (v === "" ? "–" : v === "Y" ? "on" : v === "N" ? "off" : v);
