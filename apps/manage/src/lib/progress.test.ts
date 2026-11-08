// node --test src/lib/progress.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { idle, percent, track, type Progress } from "./progress.ts";

const p = (o: Partial<Progress>): Progress => ({ dir: "technician/windows-x86_64", version: "0.5.3", server: "https://a", file: "", sent: 0, total: 0, done: false, error: null, ...o });

test("signing, then one line per relay uploading at once; a relay that fails is listed at once", () => {
  let s = track(idle, p({ server: "", file: "Tether_0.5.3_x64-setup.exe", total: 40 }));
  assert.deepEqual(s.current.map((c) => c.server), [""]);
  s = track(s, p({ file: "Tether_0.5.3_x64-setup.exe", sent: 10, total: 40 }));
  s = track(s, p({ server: "https://b", file: "Tether_0.5.3_x64-setup.exe", sent: 5, total: 40 }));
  s = track(s, p({ file: "Tether_0.5.3_x64-setup.exe", sent: 20, total: 40 }));
  assert.deepEqual(s.current.map((c) => [c.server, c.sent]), [["https://a", 20], ["https://b", 5]]);
  s = track(s, p({ server: "https://b", done: true, error: "No answer" }));
  assert.deepEqual(s.current.map((c) => c.server), ["https://a"]);
  assert.deepEqual(s.finished.map((f) => [f.server, f.error]), [["https://b", "No answer"]]);
  s = track(s, p({ done: true }));
  assert.deepEqual(s.current, []);
  assert.deepEqual(s.finished.map((f) => [f.server, f.error]), [["https://b", "No answer"], ["https://a", null]]);
});

test("percent", () => {
  assert.equal(percent(p({ sent: 1, total: 3 })), 33);
  assert.equal(percent(p({ sent: 40, total: 40 })), 100);
  assert.equal(percent(p({})), 0);
});
