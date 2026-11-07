// Home view: Simple (ID box + recent cards) or Advanced (host inventory, docs/advanced.md).
// The user's choice is a per-PC preference; policy `ui.advanced` can hide (off) or force it.
import { useEffect, useState } from "react";
import { usePolicy, type Effective } from "./policy";

export type Mode = "simple" | "advanced";
export type ModePolicy = "off" | "available" | "forced";

const KEY = "tether.mode";

export function getModePref(): Mode {
  try {
    return localStorage.getItem(KEY) === "advanced" ? "advanced" : "simple";
  } catch {
    return "simple";
  }
}

const subs = new Set<() => void>();

export function setModePref(m: Mode) {
  try {
    localStorage.setItem(KEY, m);
  } catch {
    /* storage unavailable: this window only */
  }
  subs.forEach((f) => f());
}

export function modePolicy(eff: Effective | null): ModePolicy {
  const v = eff?.values["ui.advanced"];
  return v === "off" || v === "forced" ? v : "available";
}

export function resolveMode(policy: ModePolicy, pref: Mode): Mode {
  return policy === "off" ? "simple" : policy === "forced" ? "advanced" : pref;
}

export function useMode(): { mode: Mode; policy: ModePolicy; set: (m: Mode) => void } {
  const eff = usePolicy();
  const [pref, setPref] = useState(getModePref);
  useEffect(() => {
    const f = () => setPref(getModePref());
    subs.add(f);
    const st = (e: StorageEvent) => e.key === KEY && f();
    window.addEventListener("storage", st);
    return () => {
      subs.delete(f);
      window.removeEventListener("storage", st);
    };
  }, []);
  const policy = modePolicy(eff);
  return { mode: resolveMode(policy, pref), policy, set: setModePref };
}
