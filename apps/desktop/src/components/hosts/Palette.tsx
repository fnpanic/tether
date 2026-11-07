// Ctrl+K: one box for hosts (connect), saved searches and every command.
import { useMemo, useState, type ReactNode } from "react";
import { ArrowRight, Command, Search } from "lucide-react";
import { PlatformIcon } from "../ui";
import { formatId } from "../../lib/ipc";
import { label, type Host } from "../../lib/hosts/model";
import { fuzzy, search } from "../../lib/hosts/query";
import { Kbd, OnlineDot } from "./bits";

export interface Cmd {
  id: string;
  title: string;
  hint?: string;
  keys?: string;
  icon?: ReactNode;
  run: () => void;
}

type Item = { kind: "host"; h: Host } | { kind: "cmd"; c: Cmd };

export function Palette({ hosts, commands, onClose, onHost }: { hosts: Host[]; commands: Cmd[]; onClose: () => void; onHost: (id: string, kind: "default" | "file-transfer" | "view-only") => void }) {
  const [q, setQ] = useState("");
  const [hi, setHi] = useState(0);
  const items = useMemo<Item[]>(() => {
    const t = q.trim().toLowerCase();
    const cmds = commands
      .map((c) => ({ c, s: t ? fuzzy(`${c.title} ${c.hint ?? ""}`.toLowerCase(), t) : 1 }))
      .filter((x) => x.s > 0)
      .sort((a, b) => b.s - a.s)
      .map(({ c }) => ({ kind: "cmd" as const, c }));
    const hs = t ? search(hosts, q).slice(0, 8).map((h) => ({ kind: "host" as const, h })) : [];
    // Hosts first when the text looks like an ID or a name; commands first for a verb.
    return cmds.length && cmds[0].kind === "cmd" && t && fuzzy(cmds[0].c.title.toLowerCase(), t) >= 1000 ? [...cmds.slice(0, 6), ...hs] : [...hs, ...cmds.slice(0, t ? 6 : 12)];
  }, [q, hosts, commands]);
  const cur = Math.min(hi, Math.max(0, items.length - 1));

  const run = (it: Item | undefined, kind: "default" | "file-transfer" | "view-only" = "default") => {
    if (!it) return;
    onClose();
    if (it.kind === "host") onHost(it.h.id, kind);
    else it.c.run();
  };

  return (
    <div className="animate-fade fixed inset-0 z-50 flex justify-center bg-black/30 pt-[12vh]" onMouseDown={onClose}>
      <div role="dialog" aria-label="Command palette" aria-modal className="animate-rise flex max-h-[70vh] w-[620px] max-w-[92vw] flex-col overflow-hidden rounded-2xl border border-hairline bg-tile shadow-float" onMouseDown={(e) => e.stopPropagation()}>
        <div className="flex h-14 shrink-0 items-center gap-3 border-b border-hairline px-5">
          <Search size={17} className="text-muted" />
          <input
            autoFocus
            value={q}
            onChange={(e) => {
              setQ(e.target.value);
              setHi(0);
            }}
            onKeyDown={(e) => {
              e.stopPropagation();
              if (e.key === "Escape") onClose();
              else if (e.key === "ArrowDown") {
                e.preventDefault();
                setHi((cur + 1) % Math.max(1, items.length));
              } else if (e.key === "ArrowUp") {
                e.preventDefault();
                setHi((cur - 1 + items.length) % Math.max(1, items.length));
              } else if (e.key === "Enter") {
                e.preventDefault();
                run(items[cur], e.shiftKey ? "file-transfer" : e.altKey ? "view-only" : "default");
              }
            }}
            placeholder="Connect to a host, or type a command…"
            aria-label="Command"
            role="combobox"
            aria-expanded
            aria-controls="palette-list"
            aria-activedescendant={items.length ? `pal-${cur}` : undefined}
            spellCheck={false}
            className="h-full flex-1 bg-transparent text-callout outline-none placeholder:text-faint focus-visible:outline-none"
          />
          <Kbd>Esc</Kbd>
        </div>
        <div id="palette-list" role="listbox" className="min-h-0 flex-1 overflow-y-auto p-1.5">
          {items.length === 0 && <div className="px-4 py-6 text-center text-caption text-muted">Nothing matches.</div>}
          {items.map((it, i) => (
            <div
              id={`pal-${i}`}
              role="option"
              aria-selected={i === cur}
              key={it.kind === "host" ? `h${it.h.id}` : `c${it.c.id}`}
              onMouseMove={() => setHi(i)}
              onMouseDown={(e) => {
                e.preventDefault();
                run(it);
              }}
              className={`flex h-10 cursor-default items-center gap-3 rounded-md px-3 text-caption ${i === cur ? "bg-accent/12" : ""}`}
            >
              {it.kind === "host" ? (
                <>
                  <span className="text-muted">
                    <PlatformIcon platform={it.h.os} size={15} />
                  </span>
                  <span className="truncate font-semibold">{label(it.h)}</span>
                  <span className="font-mono text-footnote text-muted">{formatId(it.h.id)}</span>
                  <OnlineDot s={it.h.online} />
                  <span className="flex-1" />
                  {i === cur && (
                    <span className="flex items-center gap-1 text-footnote text-muted">
                      Connect <ArrowRight size={12} />
                    </span>
                  )}
                </>
              ) : (
                <>
                  <span className="grid w-[15px] place-items-center text-muted">{it.c.icon ?? <Command size={14} />}</span>
                  <span className="truncate">{it.c.title}</span>
                  {it.c.hint && <span className="truncate text-footnote text-muted">{it.c.hint}</span>}
                  <span className="flex-1" />
                  {it.c.keys && <Kbd>{it.c.keys}</Kbd>}
                </>
              )}
            </div>
          ))}
        </div>
        <div className="flex shrink-0 items-center gap-3 border-t border-hairline px-4 py-2 text-footnote text-faint">
          <span className="flex items-center gap-1">
            <Kbd>↑</Kbd>
            <Kbd>↓</Kbd> move
          </span>
          <span className="flex items-center gap-1">
            <Kbd>Enter</Kbd> connect / run
          </span>
          <span className="flex items-center gap-1">
            <Kbd>⇧</Kbd>
            <Kbd>Enter</Kbd> files
          </span>
          <span className="flex items-center gap-1">
            <Kbd>Alt</Kbd>
            <Kbd>Enter</Kbd> view only
          </span>
        </div>
      </div>
    </div>
  );
}
