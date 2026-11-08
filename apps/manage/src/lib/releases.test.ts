// node --test src/lib/releases.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { compareVersion, customFields, customRollout, defaultCustom, describeRamp, groupReleases, rampPresets, rampSteps, rolloutProblem, rolloutState, perRelay, releaseToast, Tally } from "./releases.ts";

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
    "custom:0=5,1e2=100",
    "custom:0=5,0x10=100",
    "custom:0=5,2=100.0",
    "custom:0=5,2=1e2",
    "custom:0=5,Infinity=100",
    "custom:0=5,+2=100",
    "custom:0=5,2=+100",
  ]) {
    assert.ok(rolloutProblem(bad), bad);
    assert.equal(rampSteps(bad), null, bad);
  }
  assert.match(rolloutProblem(customRollout({ first: 5, h10: 2, h50: 2, h100: 48 })) ?? "", /later/);
});

const A = "http://127.0.0.1:1", B = "http://127.0.0.1:2", C = "http://127.0.0.1:3";
const ok = (server: string, detail: any = { live: true, url: "u" }) => ({ server, ok: true, detail });
const bad = (server: string, error: string) => ({ server, ok: false, error, detail: null });

test("a build that failed everywhere is split per relay, errors with '; ' inside kept whole", () => {
  const m = perRelay(`${A}: The upload stalled: x; y; ${B}: not reachable (z); its index`, [A, B]);
  assert.deepEqual([...m!], [[A, "The upload stalled: x; y"], [B, "not reachable (z); its index"]]);
  assert.equal(perRelay("Could not read the file", [A, B]), null);
});

test("Cancel at a build boundary: the live build is named, no 'Not every relay'", () => {
  const t = new Tally([A, B, C], []);
  t.results("QS 0.9.40", [ok(A), ok(B), ok(C)]);
  t.error("Mac 0.9.40", "Cancelled. Nothing was published; …");
  t.cancel("Win 0.9.40");
  const rows = t.rows();
  assert.ok(rows.every((r) => r.ok && r.detail.published[0] === "QS 0.9.40" && /cancelled/.test(r.detail.warning)));
  assert.equal(t.summary(), "Cancelled after QS 0.9.40; QS 0.9.40 is live on every relay. The builds after it were not published.");
  assert.ok(!/^Published /.test(t.toast("0.9.40")));
});

test("silent relays are skipped for the later builds and each row has only its own error", () => {
  const t = new Tally([A, B, C], []);
  t.error("QS", `${A}: The upload stalled: a; ${B}: The upload stalled: b; ${C}: Could not read`);
  assert.deepEqual(t.skip, [A, B]);
  t.results("Win", [bad(A, "skipped (not reachable)"), bad(B, "skipped (not reachable)"), ok(C, { live: true, warning: "legacy latest.json not updated: x" })]);
  const [ra, , rc] = t.rows();
  assert.equal(ra.error, "QS: The upload stalled: a\nWin: skipped (not reachable)");
  assert.ok(!rc.ok && rc.detail.published[0] === "Win" && /legacy latest.json/.test(rc.detail.warning));
  assert.match(t.summary(), /^Not every relay has it\. Win is live on 127\.0\.0\.1:3\. /);
});

test("the single-publish toast matches the result", () => {
  assert.equal(releaseToast("1.0", "stable", [ok(A), ok(B)]), "Released 1.0 to stable");
  assert.equal(releaseToast("1.0", "stable", [ok(A), bad(B, "x"), ok("CDN")]), "Released 1.0 to stable on 1 of 2 relays only");
  assert.equal(releaseToast("1.0", "stable", [ok(A), ok(B), bad("Cancel", "Too late")]), "Too late to cancel: 1.0 is live on 2 of 2 relays");
  assert.equal(releaseToast("1.0", "stable", [ok(A, { live: true, warning: "w" })]), "Released 1.0 to stable; some relays need attention");
});

test("toasts after a Cancel name what happened, not a plain success", () => {
  const t = new Tally([A], []);
  t.results("QS", [ok(A), { server: "Cancel", ok: false, error: "Too late", detail: null }]);
  assert.equal(t.toast("1.0"), "Cancel came too late: it is live where marked done");
  const c = new Tally([A], []);
  c.results("QS", [ok(A)]);
  c.cancel("Win");
  assert.equal(c.toast("1.0"), "Cancelled after QS");
  const n = new Tally([A], []);
  n.cancel("QS");
  assert.equal(n.toast("1.0"), "Cancelled. Nothing was published");
});
