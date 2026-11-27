// Small pieces of the Advanced mode UI: tag chips, key hints, status dots, the tag input.
import { useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronDown, ChevronRight, CircleCheck, CircleDashed, CircleDot, CircleX, Minus, TriangleAlert, Unlink2, X } from "lucide-react";
import { normTag, tagColor, type HealthState, type Online, type Store } from "../../lib/hosts/model";

export function TagDot({ tag, meta, size = 7 }: { tag: string; meta: Store["tags"]; size?: number }) {
  return <span aria-hidden className="inline-block shrink-0 rounded-full" style={{ width: size, height: size, background: `var(--tag-${tagColor(tag, meta)})` }} />;
}

/** Neutral chip with a colour dot; `auto` = from a rule (dashed, cannot be removed here). */
export function TagChip({ tag, meta, auto, onRemove, onClick }: { tag: string; meta: Store["tags"]; auto?: boolean; onRemove?: () => void; onClick?: () => void }) {
  const ns = tag.includes(":") ? tag.slice(0, tag.indexOf(":") + 1) : "";
  const body = (
    <>
      <TagDot tag={tag} meta={meta} />
      <span className="truncate">
        {ns && <span className="text-muted">{ns}</span>}
        {tag.slice(ns.length)}
      </span>
    </>
  );
  return (
    <span
      title={auto ? `${tag} (from a rule)` : tag}
      className={`inline-flex h-[22px] max-w-[180px] shrink-0 items-center gap-1.5 rounded-sm border px-1.5 text-footnote text-ink-80 ${auto ? "border-dashed border-hairline" : "border-hairline bg-tile-2"}`}
    >
      {onClick ? (
        <button type="button" className="inline-flex min-w-0 items-center gap-1.5" onClick={onClick}>
          {body}
        </button>
      ) : (
        body
      )}
      {onRemove && (
        <button type="button" aria-label={`Remove tag ${tag}`} className="-mr-0.5 grid h-4 w-4 place-items-center rounded-full text-muted hover:text-ink" onClick={onRemove}>
          <X size={11} />
        </button>
      )}
    </span>
  );
}

export function Kbd({ children }: { children: ReactNode }) {
  return <kbd className="inline-grid h-[18px] min-w-[18px] place-items-center whitespace-nowrap rounded-sm border border-hairline bg-parchment px-1 font-sans text-[11px] leading-none text-muted">{children}</kbd>;
}

export function OnlineDot({ s }: { s: Online }) {
  const [c, t] = s === "on" ? ["bg-ok", "Online"] : s === "off" ? ["bg-faint", "Offline"] : ["border border-faint", "Status unknown"];
  return <span role="img" aria-label={t} title={t} className={`inline-block h-2 w-2 shrink-0 rounded-full ${c}`} />;
}

/** Health of a row: shape and colour differ per state, and the label names it (never colour alone). */
export const HEALTH_LABEL: Record<HealthState, string> = { crit: "Critical", warn: "Warning", stale: "Not reporting", setup: "Setup needed", ok: "OK", none: "No agent", conflict: "Link conflict" };
const HEALTH_ICON = { crit: [CircleX, "text-danger"], warn: [TriangleAlert, "text-warn"], stale: [CircleDashed, "text-muted"], setup: [CircleDot, "text-muted"], ok: [CircleCheck, "text-ok"], none: [Minus, "text-faint"], conflict: [Unlink2, "text-danger"] } as const;

/** `label`: another name for the same shape (a single check's state); "" when the control around it is named. */
export function HealthDot({ s, size = 13, label }: { s: HealthState; size?: number; label?: string }) {
  const [Icon, c] = HEALTH_ICON[s];
  const t = label ?? `Health: ${HEALTH_LABEL[s]}`;
  return (
    <span {...(t ? { role: "img", "aria-label": t, title: t } : { "aria-hidden": true })} className={`inline-grid shrink-0 place-items-center ${c}`}>
      <Icon size={size} strokeWidth={2.25} aria-hidden />
    </span>
  );
}

export function ago(t: number, now = Date.now() / 1000): string {
  if (!t) return "—";
  const s = Math.max(0, now - t);
  if (s < 90) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  if (s < 86400 * 45) return `${Math.round(s / 86400)} d ago`;
  return new Date(t * 1000).toLocaleDateString();
}

/** Add tags: type, Enter/comma/Tab commits; suggestions from the tags in use. */
export function TagInput({ known, onAdd, placeholder = "Add tag", autoFocus, inputId }: { known: string[]; onAdd: (t: string) => void; placeholder?: string; autoFocus?: boolean; inputId?: string }) {
  const [v, setV] = useState("");
  const [hi, setHi] = useState(0);
  const ref = useRef<HTMLInputElement>(null);
  const sug = useMemo(() => {
    const q = v.trim().toLowerCase().replace(/^#/, "");
    if (!q) return [];
    return known.filter((t) => t.includes(q)).sort((a, b) => Number(!b.startsWith(q)) - Number(!a.startsWith(q)) || a.length - b.length).slice(0, 6);
  }, [v, known]);
  const commit = (t: string) => {
    const n = normTag(t);
    if (n) onAdd(n);
    setV("");
    setHi(0);
  };
  return (
    <div className="relative">
      <input
        id={inputId}
        ref={ref}
        value={v}
        autoFocus={autoFocus}
        onChange={(e) => {
          setV(e.target.value.slice(0, 64));
          setHi(0);
        }}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "ArrowDown" && sug.length) {
            e.preventDefault();
            setHi((hi + 1) % sug.length);
          } else if (e.key === "ArrowUp" && sug.length) {
            e.preventDefault();
            setHi((hi - 1 + sug.length) % sug.length);
          } else if ((e.key === "Enter" || e.key === "," || (e.key === "Tab" && v)) && (v.trim() || sug.length)) {
            e.preventDefault();
            commit(e.key !== "," && sug[hi] && sug[hi].startsWith(v.trim().toLowerCase().replace(/^#/, "")) ? sug[hi] : v);
          } else if (e.key === "Escape") {
            if (v) {
              e.preventDefault();
              setV("");
            } else (e.target as HTMLInputElement).blur();
          }
        }}
        placeholder={placeholder}
        aria-label={placeholder}
        spellCheck={false}
        autoComplete="off"
        className="h-8 w-full rounded-xl border border-hairline bg-parchment px-3 text-caption text-ink outline-none placeholder:text-faint focus-visible:border-focus"
      />
      {sug.length > 0 && (
        <div role="listbox" className="animate-fade absolute inset-x-0 top-9 z-30 overflow-hidden rounded-xl border border-hairline bg-tile p-1 shadow-float">
          {sug.map((t, i) => (
            <button
              type="button"
              role="option"
              aria-selected={i === hi}
              key={t}
              onMouseDown={(e) => {
                e.preventDefault();
                commit(t);
                ref.current?.focus();
              }}
              className={`flex h-8 w-full items-center gap-2 rounded-md px-2 text-left text-caption ${i === hi ? "bg-accent/12" : "hover:bg-tile-2"}`}
            >
              {t}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/** A fold header: a button with a chevron, the label and, at the right, `extra`. */
export function Fold({ label, open, onToggle, extra }: { label: string; open: boolean; onToggle: () => void; extra?: ReactNode }) {
  return (
    <button type="button" aria-expanded={open} onClick={onToggle} className="flex h-7 w-full items-center gap-1 rounded-md text-left text-footnote text-muted hover:text-ink">
      {open ? <ChevronDown size={12} aria-hidden /> : <ChevronRight size={12} aria-hidden />}
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {extra}
    </button>
  );
}

/** A unix time as a medium local date and short time; "" for none. */
export const day = (t: number | null | undefined) => (t ? new Date(t * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : "");
