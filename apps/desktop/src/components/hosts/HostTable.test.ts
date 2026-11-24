import { describe, expect, it } from "vitest";
import { buildHosts, emptyStore, type StatusSummary } from "../../lib/hosts/model";
import { sortHosts } from "./HostTable";

const NOW = 1_800_000_000;
const sum = (uuid: string, tether_id: string, p: Partial<StatusSummary>): StatusSummary => ({
  uuid,
  tether_id,
  host: tether_id,
  agent: "0.1.0",
  overall: "ok",
  counts: { ok: 3, warn: 0, crit: 0, unknown: 0, setup: 0 },
  products: [],
  faults: [],
  last_seen: NOW,
  next: 300,
  stale: false,
  relay: "r",
  ...p,
});

describe("sortHosts", () => {
  const rows = buildHosts(emptyStore(), [{ id: "444444444", hostname: "NONE", username: "", platform: "", last: NOW }], [], null, NOW, [
    sum("a", "111111111", { overall: "warn", counts: { ok: 1, warn: 2, crit: 0, unknown: 0, setup: 0 }, products: [{ name: "Imaging", version: "3.1" }], agent: "0.1.10" }),
    sum("b", "222222222", { overall: "crit", counts: { ok: 1, warn: 0, crit: 1, unknown: 0, setup: 0 }, products: [{ name: "Records", version: "26.3" }], agent: "0.1.2" }),
    sum("c", "333333333", { stale: true }),
  ]);
  const ids = (key: "health" | "products" | "agent", desc = false) => sortHosts(rows, { key, desc }).map((h) => h.id);
  it("health: worst first ascending", () => {
    expect(ids("health")).toEqual(["222222222", "333333333", "111111111", "444444444"]);
    expect(ids("health", true)).toEqual(["444444444", "111111111", "333333333", "222222222"]);
  });
  it("products and agent: rows without a value last; agent versions numerically", () => {
    expect(ids("products")).toEqual(["111111111", "222222222", "444444444", "333333333"]);
    expect(ids("agent")).toEqual(["333333333", "222222222", "111111111", "444444444"]);
  });
});
