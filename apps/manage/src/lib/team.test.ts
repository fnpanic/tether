import { test } from "node:test";
import assert from "node:assert/strict";
import { fingerprint, keyProblem, nameProblem, parseKey, sameRoster } from "./team.ts";

// Same vector as tether_trust::team::tests::fingerprint_vector (key bytes 0..32).
const KEY = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

test("fingerprint matches the Rust one", async () => {
  const raw = parseKey(KEY)!;
  assert.deepEqual([...raw], [...Array(32).keys()]);
  assert.equal(await fingerprint(raw), "630D-CD29-66C4-3366");
});

test("keys and names", () => {
  assert.ok(parseKey(` ${KEY.slice(0, 20)}\n${KEY.slice(20)} `), "whitespace ignored");
  for (const bad of ["", "AAAA", KEY.slice(0, 43), KEY.replace("=", "A"), "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHg==", KEY.replace("A", "-")]) assert.equal(parseKey(bad), null, bad);
  assert.equal(keyProblem(KEY, []), null);
  assert.match(keyProblem(KEY, [{ name: "Anna", key: KEY, write: false, audit: false }])!, /already/);
  assert.match(keyProblem("nope", [])!, /Not a technician key/);
  assert.equal(nameProblem("Anna Berger"), null);
  assert.equal(nameProblem("ä".repeat(64)), null);
  for (const bad of ["", " Anna", "Anna ", "x".repeat(65), "A\tB"]) assert.ok(nameProblem(bad), JSON.stringify(bad));
  const m = { name: "A", key: KEY, write: true, audit: false };
  assert.ok(sameRoster({ members: [m] }, { members: [{ ...m }] }));
  assert.ok(!sameRoster({ members: [m] }, { members: [{ ...m, audit: true }] }));
});
