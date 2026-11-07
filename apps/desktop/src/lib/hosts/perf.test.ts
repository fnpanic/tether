// Search latency budget: one frame (16 ms) per keystroke on 5,000 hosts (docs/advanced.md#performance).
import { describe, expect, it } from "vitest";
import { fleet } from "./fleet.test-util";
import { search } from "./query";
import { applyRules } from "./rules";

const QUERIES = ["b", "be", "ber", "berl", "berlin", "ber srv", "#site:berlin", "#customer:acme -#env:prod", "is:online os:win", "(#site:munich | #site:ham) seen:<30d", "anna 0042", "zzzz", "rack 12", "ver:<0.5.21 is:org"];

const median = (xs: number[]) => [...xs].sort((a, b) => a - b)[Math.floor(xs.length / 2)];

describe("performance with 5,000 hosts", () => {
  const { hosts, store } = fleet(5000);
  it("searches within one frame per keystroke", () => {
    for (const q of QUERIES) search(hosts, q); // warm up the JIT
    const per: Record<string, number> = {};
    for (const q of QUERIES) {
      const t: number[] = [];
      for (let i = 0; i < 15; i++) {
        const t0 = performance.now();
        search(hosts, q);
        t.push(performance.now() - t0);
      }
      per[q] = +median(t).toFixed(2);
    }
    console.log("search ms (median of 15) on 5,000 hosts:", per);
    expect(Math.max(...Object.values(per))).toBeLessThan(16);
  });
  it("re-applies rules and re-indexes 5,000 hosts quickly", () => {
    const t0 = performance.now();
    applyRules(hosts, store.rules);
    const ms = performance.now() - t0;
    console.log(`rules + index on 5,000 hosts: ${ms.toFixed(1)} ms`);
    expect(ms).toBeLessThan(100);
  });
});
