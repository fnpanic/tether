import { test } from "node:test";
import assert from "node:assert/strict";
import { announceList, confirmed, isKey, newest, retireList, step, switchCommands } from "./serverkeys.ts";

const A = "A".repeat(43) + "=";
const B = "B".repeat(43) + "=";

test("steps follow what the relays serve", () => {
  assert.equal(step({}, null), "generate");
  const r = { from: A, to: B };
  assert.equal(step(r, null), "announce");
  assert.equal(step(r, { keys: [A] }), "announce");
  // Wrong order would make devices present the new key before relays accept it.
  assert.equal(step(r, { keys: [B, A] }), "announce");
  assert.equal(step(r, { keys: [A, B] }), "switch");
  assert.equal(step({ ...r, switched: true }, { keys: [A, B] }), "retire");
  assert.equal(step({ ...r, switched: true }, { keys: [B] }), "relays");
  assert.equal(step({ ...r, switched: true, retired: true }, { keys: [B] }), "done");
});

test("lists and newest verified serial", () => {
  const r = { from: A, to: B };
  assert.deepEqual(announceList(r), [{ key: A }, { key: B }]);
  assert.deepEqual(announceList(r, 1800000000), [{ key: A, expires: 1800000000 }, { key: B }]);
  assert.deepEqual(retireList(r), [{ key: B }]);
  const live = newest([
    { server: "https://a", reachable: true, verified: true, serial: 2, keys: [{ key: A }] },
    { server: "https://b", reachable: true, verified: false, serial: 9, keys: [{ key: B }] },
    { server: "https://c", reachable: true, verified: true, serial: 3, keys: [{ key: A }, { key: B }] },
  ]);
  assert.deepEqual(live, { serial: 3, keys: [A, B] });
  assert.equal(newest([{ server: "https://a", reachable: false }]), null);
});

test("confirmations count active devices on the list", () => {
  const d = (s: number | undefined, ago: number) => ({ uuid: "u", id: "1", profile: "technician", version: "0.5.15", os: "w", serial: 1, ip: "", last_seen: 1000000 - ago, server_keys_serial: s });
  assert.deepEqual(confirmed([d(3, 10), d(2, 10), d(undefined, 10), d(3, 90000)], 3, 1000000), { done: 1, of: 3 });
});

test("relay commands and key format", () => {
  const c = switchCommands(["https://r1.example.org", "http://127.0.0.1:21131"]);
  assert.match(c, /opc@r1\.example\.org:\/tmp\//);
  assert.match(c, /ssh opc@127\.0\.0\.1 'sudo bash \/tmp\/rotate-key\.sh switch \/tmp\/id_ed25519/);
  assert.ok(isKey(A) && !isKey("abc") && !isKey(A + "x"));
});
