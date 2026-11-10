import { test } from "node:test";
import assert from "node:assert/strict";
import { announceList, confirmed, currentKey, fromProblem, isKey, newest, notSwitched, retireCommands, retireList, step, switchCommands } from "./serverkeys.ts";

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
  // docs/key-rotation.md: a 0700 folder in the login user's home, never /tmp; the private key
  // is removed also when the switch fails, and the script's exit code is kept.
  assert.equal(
    switchCommands(["https://r1.example.org"]),
    "ssh opc@r1.example.org 'install -d -m 0700 ~/tether-key'\n" +
      "scp id_ed25519 id_ed25519.pub rotate-key.sh opc@r1.example.org:tether-key/\n" +
      "ssh opc@r1.example.org 'sudo bash ~/tether-key/rotate-key.sh switch ~/tether-key/id_ed25519; rc=$?; rm -f ~/tether-key/id_ed25519 ~/tether-key/id_ed25519.pub; exit $rc'",
  );
  assert.match(c, /ssh opc@127\.0\.0\.1 'sudo bash ~\/tether-key\/rotate-key\.sh switch /);
  assert.equal(retireCommands(["https://r1.example.org"]), "ssh opc@r1.example.org 'sudo bash ~/tether-key/rotate-key.sh retire'");
  assert.ok(!c.includes("/tmp") && !c.includes("&& rm"));
  assert.ok(isKey(A) && !isKey("abc") && !isKey(A + "x"));
});

test("a new rotation starts from the key the relays sign with", () => {
  const C = "C".repeat(43) + "=";
  // Cancel after the switch: the live list is [A, B] and the relays already sign with B.
  const relays = [
    { server: "https://a", reachable: true, verified: true, serial: 2, keys: [{ key: A }, { key: B }], signing: B },
    { server: "https://b", reachable: true, verified: true, serial: 2, keys: [{ key: A }, { key: B }], signing: B },
  ];
  const live = newest(relays);
  assert.equal(currentKey(relays, live), B, "not the list's first key A");
  assert.deepEqual(announceList({ from: currentKey(relays, live), to: C }).map((k) => k.key), [B, C], "the list keeps the key in use");
  assert.match(fromProblem(A, relays)!, /a \(BBBBBBBB…\), b \(BBBBBBBB…\)/);
  assert.equal(fromProblem(B, relays), null);
  // Relays that do not tell: the list's presented key, and nothing to object to.
  const quiet = relays.map(({ signing, ...r }) => r);
  assert.equal(currentKey(quiet, live), A);
  assert.equal(fromProblem(A, quiet), null);
  // Half switched: no key covers every relay.
  const mixed = [relays[0], { ...relays[1], signing: A }];
  assert.equal(currentKey(mixed, live), "");
  assert.match(fromProblem(A, mixed)!, /different keys/);
  assert.match(fromProblem(B, mixed)!, /different keys/);
});

test("the switch step needs every relay to sign with the new key", () => {
  const relays = [
    { server: "https://a", reachable: true, signing: B },
    { server: "https://b", reachable: true, signing: A },
    { server: "https://c", reachable: false, signing_note: "update hbbs" },
  ];
  assert.deepEqual(notSwitched(relays, B).map((r) => r.server), ["https://b", "https://c"]);
  assert.deepEqual(notSwitched(relays.slice(0, 1), B), []);
  assert.equal(notSwitched(relays.slice(0, 1), undefined).length, 1);
});
