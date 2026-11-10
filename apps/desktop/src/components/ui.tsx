// Shared primitives. Every visual rule here comes from /DESIGN.md.
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { Laptop, Minus, Monitor, Server, Square, X } from "lucide-react";
import { inTauri } from "../lib/ipc";

const isMac = navigator.userAgent.includes("Mac");

async function win() {
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  return getCurrentWindow();
}

/** 44px draggable title bar. macOS keeps native traffic lights at the left. */
export function TitleBar({ children, className = "", dark = false }: { children?: ReactNode; className?: string; dark?: boolean }) {
  return (
    <div data-tauri-drag-region className={`drag flex h-11 shrink-0 items-center gap-3 px-4 ${isMac ? "pl-20" : ""} ${className}`}>
      <div className="flex min-w-0 flex-1 items-center gap-3" data-tauri-drag-region>
        {children}
      </div>
      {!isMac && inTauri && <WindowControls dark={dark} />}
    </div>
  );
}

function WindowControls({ dark }: { dark: boolean }) {
  const btn = `no-drag grid h-8 w-10 place-items-center rounded-md transition ${dark ? "text-white/60 hover:bg-white/10 hover:text-white" : "text-muted hover:bg-parchment hover:text-ink"}`;
  return (
    <div className="no-drag flex items-center">
      <button aria-label="Minimize" className={btn} onClick={async () => (await win()).minimize()}>
        <Minus size={15} />
      </button>
      <button aria-label="Maximize" className={btn} onClick={async () => (await win()).toggleMaximize()}>
        <Square size={12} />
      </button>
      <button aria-label="Close" className={`${btn} hover:!bg-danger hover:!text-white`} onClick={async () => (await win()).close()}>
        <X size={16} />
      </button>
    </div>
  );
}

/** Flat mark: accent tile with two linked screens. */
export function Logo({ size = 22 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 1024 1024" aria-hidden>
      <rect x="64" y="64" width="896" height="896" rx="216" fill="var(--accent)" />
      <rect x="236" y="300" width="352" height="248" rx="44" fill="none" stroke="#fff" strokeWidth="56" />
      <rect x="436" y="476" width="352" height="248" rx="44" fill="#fff" fillOpacity="0.25" stroke="#fff" strokeWidth="56" />
    </svg>
  );
}

/** A selected segment, nav item or filter (DESIGN.md › Segmented control). */
export const SELECTED = "bg-accent text-on-accent font-semibold";

type Variant = "primary" | "secondary" | "utility" | "pearl" | "ghost" | "danger";

export function Button({
  variant = "pearl",
  size = "md",
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" | "lg" }) {
  const v: Record<Variant, string> = {
    primary: "rounded-full bg-accent text-on-accent hover:brightness-110",
    secondary: "rounded-full border border-accent text-accent hover:bg-accent/10",
    utility: "rounded-md bg-ink text-canvas hover:opacity-90",
    pearl: "rounded-xl border border-hairline bg-tile-2 text-ink-80 hover:bg-parchment",
    ghost: "rounded-full text-muted hover:bg-parchment hover:text-ink",
    danger: "rounded-xl border border-hairline bg-tile-2 text-danger hover:bg-parchment",
  };
  const s = { sm: "h-8 px-3.5 text-caption", md: "h-9 px-5 text-callout", lg: "h-11 px-[22px] text-callout" }[size];
  return (
    <button
      className={`inline-flex select-none items-center justify-center gap-2 font-normal disabled:pointer-events-none disabled:opacity-40 ${v[variant]} ${s} ${className}`}
      {...rest}
    />
  );
}

export function IconButton({
  label,
  active,
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string; active?: boolean }) {
  return (
    <button
      aria-label={label}
      title={label}
      className={`grid h-9 w-9 shrink-0 place-items-center rounded-full ${
        active ? "bg-accent/12 text-accent" : "text-muted hover:bg-parchment hover:text-ink"
      } ${className}`}
      {...rest}
    />
  );
}

export function Toggle({ checked, onChange, label, disabled }: { checked: boolean; onChange: (v: boolean) => void; label: string; disabled?: boolean }) {
  return (
    <button
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative h-6 w-10 shrink-0 rounded-full disabled:opacity-40 ${checked ? "bg-accent" : "bg-faint"}`}
    >
      <span className={`absolute left-0.5 top-0.5 h-5 w-5 rounded-full bg-white transition-transform duration-150 ${checked ? "translate-x-4" : ""}`} />
    </button>
  );
}

export function Segmented<T extends string>({ value, options, onChange }: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void }) {
  return (
    <div className="flex shrink-0 rounded-full bg-parchment p-0.5" role="radiogroup">
      {options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={`rounded-full border border-transparent px-3.5 py-1.5 text-caption ${value === o.value ? SELECTED : "text-muted hover:text-ink"}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

export function TextField({ className = "", ...rest }: React.InputHTMLAttributes<HTMLInputElement>) {
  return (
    <input
      className={`h-11 w-full rounded-xl border border-hairline bg-parchment px-4 text-callout text-ink outline-none placeholder:text-faint focus-visible:border-focus ${className}`}
      {...rest}
    />
  );
}

/** Transient message at the bottom of the window. */
export function useToast() {
  const [msg, setMsg] = useState<string | null>(null);
  const t = useRef<number>(0);
  // Long messages stay long enough to read: about 60 ms per character, at least 1.8 s.
  const show = (m: string, ms = Math.max(1800, m.length * 60)) => {
    setMsg(m);
    clearTimeout(t.current);
    t.current = window.setTimeout(() => setMsg(null), ms);
  };
  useEffect(() => () => clearTimeout(t.current), []);
  // A popover joins the top layer, above an open modal dialog; shown again on every render, because
  // a dialog opened later would cover it (the top layer is ordered by time of showing). Without
  // popover support (older WKWebView) it stays a fixed layer of the page.
  const raise = (el: HTMLDivElement | null) => {
    if (!el?.showPopover) return;
    try {
      if (el.matches(":popover-open")) el.hidePopover();
      el.showPopover();
    } catch {
      /* not connected yet */
    }
  };
  const node = msg ? (
    <div ref={raise} popover="manual" role="status" className="pointer-events-none fixed inset-x-0 bottom-8 top-auto z-50 m-0 flex h-auto w-auto justify-center overflow-visible border-0 bg-transparent p-0">
      <div className="animate-rise rounded-full bg-ink px-5 py-2.5 text-caption text-canvas shadow-float">{msg}</div>
    </div>
  ) : null;
  return { show, node };
}

/** A native modal dialog: focus stays inside, the page behind is inert, Escape and a click outside
 *  ask to close (no `onClose`: it must be answered), and focus returns to what opened it. Named by
 *  `label`, else by its first `h2`. First focus: `data-autofocus`, else the first control. */
export function Modal({
  children,
  onClose,
  width = 420,
  label,
  alert,
  className = "p-8",
}: {
  children: ReactNode;
  onClose?: () => void;
  width?: number | string;
  label?: string;
  alert?: boolean;
  className?: string;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  const id = useId();
  // Layout effect: close() must run while the dialog is still in the page, or focus does not return.
  useLayoutEffect(() => {
    const d = ref.current!;
    d.showModal();
    d.querySelector<HTMLElement>("[data-autofocus]")?.focus();
    const h = label ? null : d.querySelector("h2");
    if (h) d.setAttribute("aria-labelledby", (h.id ||= id));
    return () => d.close();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps
  return (
    <dialog
      ref={ref}
      role={alert ? "alertdialog" : "dialog"}
      aria-label={label}
      style={{ width }}
      className="animate-rise m-auto max-h-[calc(100vh-2rem)] max-w-[92vw] overflow-y-auto rounded-2xl border border-hairline bg-tile text-ink shadow-float backdrop:bg-black/40"
      onCancel={(e) => {
        e.preventDefault();
        onClose?.();
      }}
      // A second Escape closes a dialog whatever `cancel` said: follow it, or reopen one that needs an answer.
      onClose={() => {
        const d = ref.current;
        if (d && !d.open) onClose ? onClose() : d.showModal();
      }}
      // Only the backdrop has the dialog itself as target (the content fills it).
      onMouseDown={(e) => e.target === e.currentTarget && onClose?.()}
    >
      <div className={className}>{children}</div>
    </dialog>
  );
}

export function PlatformIcon({ platform, size = 18 }: { platform: string; size?: number }) {
  const p = platform.toLowerCase();
  const [Icon, label] = p.includes("mac") ? [Laptop, "macOS"] : p.includes("win") ? [Monitor, "Windows"] : p.includes("linux") ? [Server, "Linux"] : [Monitor, platform || "Unknown"];
  return (
    <span title={label} className="inline-grid place-items-center">
      <Icon size={size} strokeWidth={1.5} />
    </span>
  );
}

export function StatusDot({ tone }: { tone: "ok" | "warn" | "danger" | "muted" }) {
  const c = { ok: "bg-ok", warn: "bg-warn", danger: "bg-danger", muted: "bg-faint" }[tone];
  return <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${c}`} />;
}
