/** File-manager style multi-selection over a list of unique names (in display order). */
export interface Selection {
  names: string[]; // selected, in list order
  anchor: string | null; // where a Shift range starts
  cursor: string | null; // the keyboard position (last clicked / moved to)
}

export const emptySelection: Selection = { names: [], anchor: null, cursor: null };

const ordered = (list: string[], keep: Set<string>) => list.filter((n) => keep.has(n));

/** Click: plain selects one, `toggle` (Ctrl/Cmd) adds/removes one, `range` (Shift) selects anchor..name. */
export function clickSelect(list: string[], sel: Selection, name: string, mods: { toggle?: boolean; range?: boolean } = {}): Selection {
  const a = sel.anchor && list.includes(sel.anchor) ? sel.anchor : null;
  if (mods.range && a) {
    const [i, j] = [list.indexOf(a), list.indexOf(name)].sort((x, y) => x - y);
    const span = new Set(list.slice(i, j + 1));
    if (mods.toggle) for (const n of sel.names) span.add(n);
    return { names: ordered(list, span), anchor: a, cursor: name };
  }
  if (mods.toggle) {
    const s = new Set(sel.names);
    if (s.has(name)) s.delete(name);
    else s.add(name);
    return { names: ordered(list, s), anchor: name, cursor: name };
  }
  return { names: [name], anchor: name, cursor: name };
}

/** Arrow keys: move the cursor by `delta`; `extend` (Shift) grows the range from the anchor. */
export function moveSelect(list: string[], sel: Selection, delta: number, extend = false): Selection {
  if (!list.length) return sel;
  const at = sel.cursor ? list.indexOf(sel.cursor) : -1;
  const next = list[at < 0 ? (delta > 0 ? 0 : list.length - 1) : Math.max(0, Math.min(list.length - 1, at + delta))];
  return clickSelect(list, sel, next, { range: extend });
}

export function selectAll(list: string[], sel: Selection): Selection {
  return { names: [...list], anchor: sel.anchor ?? list[0] ?? null, cursor: sel.cursor ?? list[0] ?? null };
}
