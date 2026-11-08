// node --test src/lib/releases.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { compareVersion, customFields, customRollout, defaultCustom, describeRamp, groupReleases, rampPresets, rampSteps, rolloutProblem, rolloutState } from "./releases.ts";

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

test("the TetherQS gzip copy belongs to its release, not a row of its own", () => {
  const f = (path: string) => ({ channel: "stable", path, size: 1, modified: 1 });
  const g = groupReleases([
    f("customer/windows-x86_64/TetherQS-0.5.24.exe"),
    f("customer/windows-x86_64/TetherQS-0.5.24.exe.sig"),
    f("customer/windows-x86_64/TetherQS-0.5.24.exe.gz"),
    f("customer/windows-x86_64/TetherQS-0.5.4-qa.exe.gz"),
    f("customer/windows-x86_64/latest.json"),
  ]);
  assert.deepEqual(
    g.map((x) => [x.version, x.files.length]),
    [
      [null, 1],
      ["0.5.24", 3],
      ["0.5.4-qa", 1],
    ],
  );
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

// Mirrors tether_trust::version tests (ordering).
test("compareVersion: the clients' version order", () => {
  const lt = (a: string, b: string) => {
    assert.equal(compareVersion(a, b), -1, `${a} < ${b}`);
    assert.equal(compareVersion(b, a), 1, `${b} > ${a}`);
  };
  lt("0.3.0", "0.3.1");
  lt("0.3.1-test", "0.3.1");
  lt("0.9.9", "0.10.0");
  assert.equal(compareVersion("1.0", "1.0.0"), 0);
  lt("0.5.0-test.9", "0.5.0-test.10");
  lt("0.5.0-test.10", "0.5.0-test.10.1");
  lt("0.5.0-1", "0.5.0-beta");
  lt("0.5.0-alpha", "0.5.0-beta");
  lt("0.5.0-test.10", "0.5.0");
  assert.equal(compareVersion("0.5.0-rc.2", "0.5.0-rc.2"), 0);
  lt("0.5.0", "unknown");
  const f = (v: string) => ({ channel: "beta", path: `technician/windows-x86_64/Tether_${v}_x64-setup.exe`, size: 1, modified: 1 });
  assert.deepEqual(groupReleases([f("0.5.0-test.9"), f("0.5.0-test.10"), f("0.5.0")]).map((g) => g.version), ["0.5.0", "0.5.0-test.10", "0.5.0-test.9"]);
});

test("custom ramps: built from the fields, checked as release::rollout does", () => {
  assert.equal(customRollout(defaultCustom), "custom:0=5,2=10,12=50,48=100");
  assert.equal(customRollout({ first: 20, h10: 1, h50: 3, h100: 6 }), "custom:0=20,3=50,6=100", "steps the first wave reaches are left out");
  assert.equal(customRollout({ first: 100, h10: 1, h50: 2, h100: 3 }), "custom:0=100");
  assert.deepEqual(customFields("custom:0=20,3=50,6=100"), { first: 20, h10: 0, h50: 3, h100: 6 });
  assert.equal(customFields("custom:0=1,1=2,3=100"), null, "not a shape of the fields");
  assert.equal(customFields("fast"), null);
  assert.deepEqual(rampSteps("custom:0=5,1.5=25,12=100"), [
    [0, 5],
    [5400, 25],
    [43200, 100],
  ]);
  assert.deepEqual(rampSteps("standard"), rampPresets.standard);
  assert.deepEqual(rampSteps(""), rampPresets.standard);
  assert.equal(describeRamp(rampSteps(customRollout(defaultCustom))), "5 % at once → 10 % after 2 h → 50 % after 12 h → 100 % after 2 d");
  for (const ok of ["standard", "fast", "now", "", "custom:0=100", "custom:0=1,0.01=100"]) assert.equal(rolloutProblem(ok), null, ok);
  // The same refusals as release.rs custom_ramps_are_validated.
  for (const bad of [
    "bogus",
    "custom:",
    "custom:1=10,2=100",
    "custom:0=0,1=100",
    "custom:0=10,1=101",
    "custom:0=10,1=50",
    "custom:0=10,2=50,1=100",
    "custom:0=50,1=50,2=100",
    "custom:0=10,721=100",
    "custom:0=10,-1=100",
    "custom:0=10,x=100",
    "custom:0=10;1=100",
    "custom:0=10,0.001=100",
    "custom:0=1.5,1=100",
  ]) {
    assert.ok(rolloutProblem(bad), bad);
    assert.equal(rampSteps(bad), null, bad);
  }
  assert.match(rolloutProblem(customRollout({ first: 5, h10: 2, h50: 2, h100: 48 })) ?? "", /later/);
});
