// Shared primitives, copied from apps/desktop/src/components/ui.tsx. Visual rules: /DESIGN.md.
import { type ButtonHTMLAttributes, type ReactNode, useEffect, useRef, useState } from "react";
import { Check, Copy } from "lucide-react";

type Variant = "primary" | "secondary" | "utility" | "pearl" | "ghost" | "danger";

export function Button({
  variant = "pearl",
  size = "md",
  className = "",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { variant?: Variant; size?: "sm" | "md" | "lg" }) {
  const v: Record<Variant, string> = {
    primary: "rounded-full bg-accent text-white hover:brightness-110",
    secondary: "rounded-full border border-accent text-accent hover:bg-accent/10",
    utility: "rounded-md bg-ink text-canvas hover:opacity-90",
    pearl: "rounded-xl border border-hairline bg-tile-2 text-ink-80 hover:bg-parchment",
    ghost: "rounded-full text-muted hover:bg-parchment hover:text-ink",
    danger: "rounded-xl border border-hairline bg-tile-2 text-danger hover:bg-parchment",
  };
  const s = { sm: "h-8 px-3.5 text-caption", md: "h-9 px-5 text-callout", lg: "h-11 px-[22px] text-callout" }[size];
  return (
    <button
      className={`inline-flex shrink-0 select-none items-center justify-center gap-2 font-normal disabled:pointer-events-none disabled:opacity-40 ${v[variant]} ${s} ${className}`}
      {...rest}
    />
  );
}

export function IconButton({ label, className = "", ...rest }: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      aria-label={label}
      title={label}
      className={`grid h-9 w-9 shrink-0 place-items-center rounded-full text-muted hover:bg-parchment hover:text-ink ${className}`}
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
      className={`relative h-6 w-10 shrink-0 rounded-full disabled:opacity-40 ${checked ? "bg-accent" : "bg-hairline"}`}
    >
      <span className={`absolute top-0.5 h-5 w-5 rounded-full bg-white transition-all duration-150 ${checked ? "left-[18px]" : "left-0.5"}`} style={{ boxShadow: "0 1px 3px rgba(0,0,0,.25)" }} />
    </button>
  );
}

export function Segmented<T extends string>({ value, options, onChange, label }: { value: T; options: { value: T; label: string }[]; onChange: (v: T) => void; label?: string }) {
  return (
    <div className="flex shrink-0 rounded-full bg-parchment p-0.5" role="radiogroup" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          role="radio"
          aria-checked={value === o.value}
          onClick={() => onChange(o.value)}
          className={`rounded-full px-3 py-1 text-caption ${value === o.value ? "border border-hairline bg-tile text-ink" : "border border-transparent text-muted hover:text-ink"}`}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}

const field = "w-full rounded-xl border border-hairline bg-parchment px-3 text-caption text-ink outline-none placeholder:text-faint focus-visible:border-focus";

export function TextField({ className = "", ...rest }: React.InputHTMLAttributes<HTMLInputElement>) {
  return <input className={`h-9 ${field} ${className}`} {...rest} />;
}

export function TextArea({ className = "", ...rest }: React.TextareaHTMLAttributes<HTMLTextAreaElement>) {
  return <textarea className={`py-2 ${field} ${className}`} {...rest} />;
}

export function Select({ className = "", ...rest }: React.SelectHTMLAttributes<HTMLSelectElement>) {
  return <select className={`h-9 ${field} ${className}`} {...rest} />;
}

export function Label({ text, hint, children }: { text: string; hint?: string; children: ReactNode }) {
  return (
    <label className="flex flex-col gap-1.5">
      <span className="text-caption font-semibold">{text}</span>
      {children}
      {hint && <span className="text-footnote text-muted">{hint}</span>}
    </label>
  );
}

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return <section className={`rounded-2xl border border-hairline bg-tile p-6 ${className}`}>{children}</section>;
}

export function PageHeader({ title, sub, children }: { title: string; sub?: string; children?: ReactNode }) {
  return (
    <header className="mb-6 flex items-end justify-between gap-4">
      <div>
        <h1 className="text-title font-semibold">{title}</h1>
        {sub && <p className="mt-1 text-callout text-muted">{sub}</p>}
      </div>
      <div className="flex items-center gap-2">{children}</div>
    </header>
  );
}

export function ErrorText({ error }: { error: string | null }) {
  if (!error) return null;
  return (
    <p role="alert" className="flex items-start gap-2 text-caption text-danger">
      <StatusDot tone="danger" className="mt-[7px]" />
      <span className="select-text">{error}</span>
    </p>
  );
}

export function StatusDot({ tone, className = "" }: { tone: "ok" | "warn" | "danger" | "muted"; className?: string }) {
  const c = { ok: "bg-ok", warn: "bg-warn", danger: "bg-danger", muted: "bg-faint" }[tone];
  return <span className={`inline-block h-1.5 w-1.5 shrink-0 rounded-full ${c} ${className}`} />;
}

export function CopyField({ value, label }: { value: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <div className="flex items-center gap-2 rounded-xl border border-hairline bg-parchment py-1 pl-3 pr-1">
      <code className="min-w-0 flex-1 select-text truncate font-mono text-footnote" title={value}>
        {value}
      </code>
      <IconButton
        label={`Copy ${label}`}
        onClick={async () => {
          await navigator.clipboard.writeText(value);
          setDone(true);
          setTimeout(() => setDone(false), 1200);
        }}
      >
        {done ? <Check size={16} /> : <Copy size={16} />}
      </IconButton>
    </div>
  );
}

/** Transient message at the bottom of the window. */
export function useToast() {
  const [msg, setMsg] = useState<string | null>(null);
  const t = useRef<number>(0);
  const show = (m: string) => {
    setMsg(m);
    clearTimeout(t.current);
    t.current = window.setTimeout(() => setMsg(null), 2400);
  };
  useEffect(() => () => clearTimeout(t.current), []);
  const node = msg ? (
    <div role="status" className="pointer-events-none fixed inset-x-0 bottom-8 z-50 flex justify-center">
      <div className="animate-rise rounded-full bg-ink px-5 py-2.5 text-caption text-canvas shadow-float">{msg}</div>
    </div>
  ) : null;
  return { show, node };
}

export function Modal({ children, onClose, width = 420 }: { children: ReactNode; onClose?: () => void; width?: number }) {
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose?.();
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="animate-fade fixed inset-0 z-40 grid place-items-center bg-black/40" onMouseDown={onClose}>
      <div role="dialog" aria-modal style={{ width }} className="animate-rise max-w-[92vw] rounded-2xl border border-hairline bg-tile p-8 shadow-float" onMouseDown={(e) => e.stopPropagation()}>
        {children}
      </div>
    </div>
  );
}

/** Table shell with the list-row look. */
export function Table({ head, children, empty }: { head: string[]; children: ReactNode; empty?: string | false }) {
  return (
    <div className="overflow-auto rounded-2xl border border-hairline bg-tile">
      <table className="w-full border-collapse text-left text-caption">
        <thead>
          <tr className="border-b border-hairline text-footnote text-muted">
            {head.map((h) => (
              <th key={h} className="px-4 py-2.5 font-normal">
                {h}
              </th>
            ))}
          </tr>
        </thead>
        <tbody className="[&>tr:hover]:bg-tile-2 [&>tr]:border-b [&>tr]:border-hairline [&>tr:last-child]:border-0 [&_td]:px-4 [&_td]:py-2">{children}</tbody>
      </table>
      {empty && <p className="px-4 py-8 text-center text-caption text-muted">{empty}</p>}
    </div>
  );
}
