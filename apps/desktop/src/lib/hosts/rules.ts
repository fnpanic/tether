// Tag rules: "hosts matching <query> get <tag>". One pass over hosts without rule tags, so rules
// cannot feed each other (no order dependence, no loops).
import { indexHost, normTag, type Host, type Rule } from "./model";
import { matches, parse } from "./query";

export function applyRules(hosts: Host[], rules: Rule[], now = Date.now() / 1000): void {
  const live = rules
    .filter((r) => !r.off)
    .map((r) => ({ tag: normTag(r.tag), node: parse(r.when).node }))
    .filter((r): r is { tag: string; node: NonNullable<ReturnType<typeof parse>["node"]> } => !!r.tag && !!r.node);
  for (const h of hosts) {
    h.auto = [];
    indexHost(h);
  }
  if (!live.length) return;
  for (const h of hosts) {
    const auto: string[] = [];
    for (const r of live) if (!h.tags.includes(r.tag) && !auto.includes(r.tag) && matches(r.node, h, now)) auto.push(r.tag);
    h.auto = auto;
  }
  for (const h of hosts) if (h.auto.length) indexHost(h);
}
