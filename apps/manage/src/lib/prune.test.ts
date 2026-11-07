// node --test src/lib/prune.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { keptOn, offeredKeys, reasons, selection, type PrunePlan } from "./prune.ts";

const plan: PrunePlan = {
  keep: 2,
  relays: ["https://a", "https://b"],
  index_differs: false,
  offer: { versions: 2, files: 5, bytes: 50 },
  kept_versions: 2,
  groups: [
    {
      channel: "stable",
      dir: "customer/windows-x86_64",
      published: [{ server: "https://a", version: "0.5.24" }],
      versions: [
        { version: "0.5.24", files: ["a", "b", "c"], bytes: 30, why: ["index", "latest.json"], relays: { "https://a": "keep", "https://b": "keep" }, offer: false },
        { version: "0.5.21", files: ["a", "b", "c"], bytes: 30, why: [], relays: { "https://a": "drop", "https://b": "drop" }, offer: true },
        { version: "0.5.20", files: ["a", "b", "c"], bytes: 30, why: ["index"], relays: { "https://a": "drop", "https://b": "keep" }, offer: false },
      ],
    },
    {
      channel: "stable",
      dir: "technician/windows-x86_64",
      published: [],
      versions: [{ version: "0.5.1", files: ["a", "b"], bytes: 20, why: [], relays: { "https://a": "drop" }, offer: true }],
    },
  ],
};

test("every offered version starts ticked; the selection sends only ticked offered ones", () => {
  const all = offeredKeys(plan);
  assert.deepEqual([...all], ["stable/customer/windows-x86_64/0.5.21", "stable/technician/windows-x86_64/0.5.1"]);
  const s = selection(plan, all);
  assert.deepEqual(s.delete, [
    { channel: "stable", dir: "customer/windows-x86_64", version: "0.5.21" },
    { channel: "stable", dir: "technician/windows-x86_64", version: "0.5.1" },
  ]);
  assert.equal(s.files, 5);
  assert.equal(s.bytes, 50);
  // A kept version is never sent, even if its key is ticked.
  const one = selection(plan, new Set(["stable/customer/windows-x86_64/0.5.20", "stable/technician/windows-x86_64/0.5.1"]));
  assert.deepEqual(one.delete.map((d) => d.version), ["0.5.1"]);
});

test("reasons read as words; a relay that protects is named", () => {
  assert.equal(reasons(["index", "latest.json", "recent", "pinned", "x"]), "in the release index · published (latest.json) · uploaded in the last 24 h · on the CDN · x");
  assert.deepEqual(keptOn(plan.groups[0].versions[2]), ["https://b"]);
  assert.deepEqual(keptOn(plan.groups[0].versions[0]), []);
});
