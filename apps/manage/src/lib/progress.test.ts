// node --test src/lib/progress.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { idle, percent, track, type Progress } from "./progress.ts";

const p = (o: Partial<Progress>): Progress => ({ dir: "technician/windows-x86_64", version: "0.5.3", server: "https://a", file: "", sent: 0, total: 0, done: false, error: null, ...o });

test("latest event is current, finished relays stay listed with their errors", () => {
  let s = track(idle, p({ server: "", file: "Tether_0.5.3_x64-setup.exe", total: 40 }));
  s = track(s, p({ file: "Tether_0.5.3_x64-setup.exe", sent: 10, total: 40 }));
  assert.equal(s.current?.sent, 10);
  s = track(s, p({ done: true }));
  s = track(s, p({ server: "https://b", file: "Tether_0.5.3_x64-setup.exe", total: 40 }));
  s = track(s, p({ server: "https://b", done: true, error: "HTTP 507" }));
  assert.equal(s.current, null);
  assert.deepEqual(s.finished.map((f) => [f.server, f.error]), [["https://a", null], ["https://b", "HTTP 507"]]);
});

test("percent", () => {
  assert.equal(percent(p({ sent: 1, total: 3 })), 33);
  assert.equal(percent(p({ sent: 40, total: 40 })), 100);
  assert.equal(percent(p({})), 0);
});
