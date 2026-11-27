// Virtualised host list: only the rows in view (plus a few) are in the DOM, so 5,000 or 50,000
// hosts scroll and filter at the same speed. Fixed 36px rows (DESIGN.md › List row).
import { memo, useEffect, useLayoutEffect, useRef, useState } from "react";
import { ArrowDown, ArrowUp, Pin } from "lucide-react";
import { PlatformIcon } from "../ui";
import { formatId } from "../../lib/ipc";
import { label, osFamily, type Host, type Store } from "../../lib/hosts/model";
import { healthRank, healthText } from "../../lib/hosts/health";
import { ago, HealthDot, OnlineDot, TagChip } from "./bits";

export const ROW = 36;
const OVERSCAN = 8;

export type SortKey = "smart" | "name" | "id" | "os" | "version" | "last" | "health" | "products" | "agent" | "customer";
export interface Sort {
  key: SortKey;
  desc: boolean;
}

// Columns by the list's own width (window size, detail panel open or not): a column that does
// not fit is dropped, never cut off. Wide: all; medium: no OS/version/last active; narrow: no tags.
const COLS =
  "grid-cols-[32px_16px_minmax(120px,1fr)_112px] @min-[600px]:grid-cols-[32px_16px_minmax(150px,1.3fr)_112px_minmax(120px,1.7fr)] @min-[900px]:grid-cols-[32px_16px_minmax(170px,1.3fr)_112px_minmax(150px,1.7fr)_128px_72px_96px]";
const WIDE = "hidden @min-[900px]:block";
// With health data (deviations: no column chooser): Health from the start (icon only while narrow),
// OS/version/last active from 1050px, Products, Agent and Customer from 1300px.
const HCOLS =
  "grid-cols-[32px_16px_minmax(120px,1fr)_112px_16px] @min-[600px]:grid-cols-[32px_16px_minmax(130px,1.3fr)_112px_minmax(80px,1.7fr)_150px] @min-[1050px]:grid-cols-[32px_16px_minmax(170px,1.3fr)_112px_minmax(150px,1.7fr)_150px_128px_72px_96px] @min-[1300px]:grid-cols-[32px_16px_minmax(170px,1.3fr)_112px_minmax(150px,1.7fr)_150px_minmax(140px,1.2fr)_64px_88px_128px_72px_96px]";
const HWIDE = "hidden @min-[1050px]:block";
const XWIDE = "hidden @min-[1300px]:block";

const NONE = "\uffff"; // sorts after every text: rows without the value go last
const pad = (v: string) => v.split(".").map((x) => x.padStart(5, "0")).join(".");
const productText = (h: Host) => (h.status?.products ?? []).map((p) => [p.name, p.version].filter(Boolean).join(" ")).join(", ");

export function sortHosts(rows: Host[], s: Sort): Host[] {
  if (s.key === "smart") return rows;
  const k = s.key;
  const val = (h: Host): string | number =>
    k === "name"
      ? label(h).toLowerCase()
      : k === "id"
        ? h.id
        : k === "os"
          ? `${osFamily(h.os)} ${h.os}`.toLowerCase()
          : k === "version"
            ? pad(h.version)
            : // Health ascending = worst first (the first click answers "what needs me").
              k === "health"
              ? -(healthRank(h.health) * 1e8 + (h.status?.counts.crit ?? 0) * 1e4 + (h.status?.counts.warn ?? 0))
              : k === "products"
                ? productText(h).toLowerCase() || NONE
                : k === "agent"
                  ? (h.status ? pad(h.status.agent) : NONE)
                  : k === "customer"
                    ? h.status?.customer?.toLowerCase() || NONE
                    : Math.max(h.last, h.seen);
  const d = s.desc ? -1 : 1;
  // Each row's key once, not per comparison (5,000 rows: ~60,000 comparisons).
  return rows
    .map((h) => ({ h, k: val(h) }))
    .sort((a, b) => (a.k < b.k ? -d : a.k > b.k ? d : 0))
    .map((x) => x.h);
}

/** Default order without a search: pinned, online, most recently used. */
export function smartOrder(rows: Host[]): Host[] {
  const rank = (h: Host) => (h.pinned ? 4 : 0) + (h.online === "on" ? 2 : 0);
  return [...rows].sort((a, b) => rank(b) - rank(a) || Math.max(b.last, b.seen) - Math.max(a.last, a.seen) || label(a).localeCompare(label(b)));
}

const Row = memo(function Row({
  h,
  top,
  index,
  cursor,
  selected,
  meta,
  now,
  hv,
  onRow,
  onToggle,
  onOpen,
}: {
  hv: boolean;
  h: Host;
  top: number;
  index: number;
  cursor: boolean;
  selected: boolean;
  meta: Store["tags"];
  now: number;
  onRow: (id: string, e: React.MouseEvent) => void;
  onToggle: (id: string) => void;
  onOpen: (id: string) => void;
}) {
  const tags = h.auto.length ? [...h.tags.map((t) => [t, false] as const), ...h.auto.map((t) => [t, true] as const)] : h.tags.map((t) => [t, false] as const);
  return (
    <div
      role="row"
      aria-rowindex={index + 2}
      aria-selected={selected}
      data-id={h.id}
      data-cursor={cursor || undefined}
      onMouseDown={(e) => onRow(h.id, e)}
      onDoubleClick={() => onOpen(h.id)}
      style={{ transform: `translateY(${top}px)` }}
      className={`absolute inset-x-0 top-0 grid h-9 ${hv ? HCOLS : COLS} items-center gap-3 rounded-md px-2 text-caption ${
        selected ? "bg-accent/12" : cursor ? "bg-tile-2" : "hover:bg-tile-2"
      } ${cursor ? "ring-1 ring-inset ring-focus/60" : ""}`}
    >
      <span role="gridcell" className="grid place-items-center">
        <input
          type="checkbox"
          tabIndex={-1}
          aria-label={`Select ${label(h)}`}
          checked={selected}
          onChange={() => onToggle(h.id)}
          onMouseDown={(e) => e.stopPropagation()}
          className="h-3.5 w-3.5 accent-[var(--accent)]"
        />
      </span>
      <span role="gridcell" className="grid place-items-center">
        <OnlineDot s={h.online} />
      </span>
      <span role="gridcell" className="flex min-w-0 items-center gap-2">
        <span className="shrink-0 text-muted">
          <PlatformIcon platform={h.os} size={15} />
        </span>
        <span className="truncate font-semibold" title={label(h)}>
          {label(h)}
        </span>
        {h.name && h.hostname && h.name !== h.hostname && <span className="truncate text-footnote text-muted" title={h.hostname}>
            {h.hostname}
          </span>}
        {h.pinned && <Pin size={12} className="shrink-0 text-muted" aria-label="Pinned" />}
      </span>
      <span role="gridcell" className="truncate font-mono text-footnote tabular-nums text-ink-80">
        {formatId(h.id)}
      </span>
      {/* Wrapping chips in a one-line box: a chip that does not fit drops out whole. */}
      <span role="gridcell" title={tags.map(([t]) => t).join(", ")} className="hidden h-[22px] min-w-0 flex-wrap gap-1 overflow-hidden @min-[600px]:flex">
        {tags.map(([t, auto]) => (
          <TagChip key={t} tag={t} meta={meta} auto={auto} />
        ))}
      </span>
      {hv && (
        <>
          <span role="gridcell" className="flex min-w-0 items-center gap-1.5">
            <HealthDot s={h.health} />
            <span className={`hidden truncate text-footnote @min-[600px]:inline ${h.health === "crit" || h.health === "warn" || h.health === "stale" ? "text-ink-80" : "text-muted"}`} title={healthText(h, now)}>
              {healthText(h, now)}
            </span>
          </span>
          <span role="gridcell" className={`truncate text-footnote text-muted ${XWIDE}`} title={productText(h)}>
            {productText(h) || "—"}
          </span>
          <span role="gridcell" className={`truncate font-mono text-footnote text-muted ${XWIDE}`}>
            {h.status?.agent || "—"}
          </span>
          <span role="gridcell" className={`truncate font-mono text-footnote text-muted ${XWIDE}`}>
            {h.status?.customer || "—"}
          </span>
        </>
      )}
      <span role="gridcell" className={`truncate text-footnote text-muted ${hv ? HWIDE : WIDE}`} title={h.os}>
        {h.os ? `${osFamily(h.os)} ${/\d[\d.]*/.exec(h.os)?.[0] ?? ""}`.trim() : "—"}
      </span>
      <span role="gridcell" className={`truncate font-mono text-footnote text-muted ${hv ? HWIDE : WIDE}`}>
        {h.version || "—"}
      </span>
      <span role="gridcell" className={`truncate text-footnote text-muted ${hv ? HWIDE : WIDE}`} title={h.seen ? `Last heartbeat ${new Date(h.seen * 1000).toLocaleString()}` : undefined}>
        {ago(Math.max(h.last, h.seen), now)}
      </span>
    </div>
  );
});

export function HostTable({
  rows,
  cursor,
  selected,
  meta,
  sort,
  onSort,
  onRow,
  onToggle,
  onToggleAll,
  onOpen,
  health = false,
}: {
  /** Health data exists: the Health, Products and Agent columns are shown. */
  health?: boolean;
  rows: Host[];
  cursor: string | null;
  selected: Set<string>;
  meta: Store["tags"];
  sort: Sort;
  onSort: (s: Sort) => void;
  onRow: (id: string, e: React.MouseEvent) => void;
  onToggle: (id: string) => void;
  onToggleAll: () => void;
  onOpen: (id: string) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [top, setTop] = useState(0);
  const [height, setHeight] = useState(600);
  // Minute resolution, so memoised rows do not re-render on every parent render.
  const now = Math.floor(Date.now() / 60000) * 60;

  useLayoutEffect(() => {
    const el = ref.current!;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    setHeight(el.clientHeight);
    return () => ro.disconnect();
  }, []);

  // Keep the cursor row in view when the keyboard moves it.
  const ci = cursor ? rows.findIndex((h) => h.id === cursor) : -1;
  useEffect(() => {
    const el = ref.current;
    if (!el || ci < 0) return;
    const y = ci * ROW;
    if (y < el.scrollTop) el.scrollTop = y;
    else if (y + ROW > el.scrollTop + el.clientHeight) el.scrollTop = y + ROW - el.clientHeight;
  }, [ci]);

  // A shorter result list (typing) scrolls back into range.
  useEffect(() => {
    const el = ref.current;
    if (el && el.scrollTop > Math.max(0, rows.length * ROW - el.clientHeight)) el.scrollTop = Math.max(0, rows.length * ROW - el.clientHeight);
  }, [rows.length]);

  const first = Math.max(0, Math.floor(top / ROW) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((top + height) / ROW) + OVERSCAN);
  const all = rows.length > 0 && rows.every((h) => selected.has(h.id));
  const hv = health;
  const wide = hv ? "hidden @min-[1050px]:flex" : "hidden @min-[900px]:flex";

  const head = (k: SortKey, text: string, cls = "") => (
    <button
      type="button"
      role="columnheader"
      aria-sort={sort.key === k ? (sort.desc ? "descending" : "ascending") : "none"}
      onClick={() => onSort(sort.key === k ? (sort.desc ? { key: "smart", desc: false } : { key: k, desc: true }) : { key: k, desc: false })}
      className={`${cls || "flex"} items-center gap-1 truncate text-left text-footnote font-semibold text-muted hover:text-ink`}
    >
      {text}
      {sort.key === k && (sort.desc ? <ArrowDown size={11} /> : <ArrowUp size={11} />)}
    </button>
  );

  return (
    <div role="grid" aria-label="Hosts" aria-rowcount={rows.length + 1} aria-multiselectable className="@container flex min-h-0 flex-1 flex-col">
      <div role="row" aria-rowindex={1} className={`grid h-8 shrink-0 ${hv ? HCOLS : COLS} items-center gap-3 border-b border-hairline px-4`}>
        <span className="grid place-items-center">
          <input type="checkbox" aria-label="Select all shown" checked={all} onChange={onToggleAll} tabIndex={-1} className="h-3.5 w-3.5 accent-[var(--accent)]" />
        </span>
        <span />
        {head("name", "Name")}
        {head("id", "ID")}
        <span className="hidden text-footnote font-semibold text-muted @min-[600px]:block">Tags</span>
        {hv && (
          <>
            {head("health", "Health", "hidden @min-[600px]:flex")}
            {head("products", "Products", "hidden @min-[1300px]:flex")}
            {head("agent", "Agent", "hidden @min-[1300px]:flex")}
            {head("customer", "Customer", "hidden @min-[1300px]:flex")}
          </>
        )}
        {head("os", "OS", wide)}
        {head("version", "Version", wide)}
        {head("last", "Last active", wide)}
      </div>
      <div ref={ref} data-testid="host-scroll" onScroll={(e) => setTop(e.currentTarget.scrollTop)} className="relative min-h-0 flex-1 overflow-y-auto px-2 py-1">
        <div style={{ height: rows.length * ROW }} className="relative">
          {rows.slice(first, last).map((h, i) => (
            <Row
              key={h.id}
              h={h}
              index={first + i}
              top={(first + i) * ROW}
              cursor={h.id === cursor}
              selected={selected.has(h.id)}
              meta={meta}
              now={now}
              hv={hv}
              onRow={onRow}
              onToggle={onToggle}
              onOpen={onOpen}
            />
          ))}
        </div>
      </div>
    </div>
  );
}
