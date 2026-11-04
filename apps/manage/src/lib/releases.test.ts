// node --test src/lib/releases.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { groupReleases } from "./releases.ts";

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
