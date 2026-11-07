// node --test src/lib/releases.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { describeRamp, groupReleases, rampPresets, rolloutState } from "./releases.ts";

test("one group per release, manifest separate, newest first", () => {
  const f = (path: string, size = 1) => ({ channel: "stable", path, size, modified: size });
  const g = groupReleases([
    f("technician/darwin-universal/Tether_0.5.1_universal.app.tar.gz", 10),
    f("technician/darwin-universal/Tether_0.5.1_universal.app.tar.gz.sig"),
    f("technician/darwin-universal/Tether_0.5.1_universal.dmg", 20),
    f("technician/darwin-universal/Tether_0.5.1_universal.dmg.sig"),
    f("technician/darwin-universal/latest.json"),
    f("technician/windows-x86_64/Tether_0.5.0_x64-setup.exe"),
    f("technician/windows-x86_64/Tether_0.10.0_x64-setup.exe"),
    f("customer/windows-x86_64/TetherQS-0.5.1.exe"),
    f("customer/windows-x86_64/TetherQS-0.5.1.exe.sig"),
  ]);
  assert.deepEqual(
    g.map((x) => [x.dir, x.version, x.files.length]),
    [
      ["customer/windows-x86_64", "0.5.1", 2],
      ["technician/darwin-universal", null, 1],
      ["technician/darwin-universal", "0.5.1", 4],
      ["technician/windows-x86_64", "0.10.0", 1],
      ["technician/windows-x86_64", "0.5.0", 1],
    ],
  );
  assert.equal(g[2].size, 32);
});

test("ramp presets read as steps", () => {
  assert.equal(describeRamp(rampPresets.standard), "1 % at once → 10 % after 1 h → 50 % after 6 h → 100 % after 1 d");
  assert.equal(describeRamp(rampPresets.fast), "10 % at once → 100 % after 1 h");
  assert.equal(describeRamp(rampPresets.now), "Every device at once");
  assert.equal(describeRamp([]), "");
});

test("rollout state", () => {
  const e = { key: "technician/windows-x86_64", version: "1", percent: 10, halted: false, security: false, mandatory: false, forced: false, start: 0, steps: [], cdn: false };
  assert.equal(rolloutState(e), "10 % of devices");
  assert.equal(rolloutState({ ...e, halted: true }), "Halted");
  assert.equal(rolloutState({ ...e, percent: 100 }), "Everyone");
  assert.equal(rolloutState({ ...e, percent: 0 }), "Not started");
  assert.equal(rolloutState({ ...e, security: true }), "Security release: everyone");
});
