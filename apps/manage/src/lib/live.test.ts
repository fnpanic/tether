import { test } from "node:test";
import assert from "node:assert/strict";
import { incomplete, liveLine } from "./live.ts";

const A = "https://a.example", B = "https://b.example";
const relay = (server: string, o: object = {}) => ({ server, reachable: true, error: null, serial: 4, issued_at: 1, verified: true, note: null, ...o });
const live = (relays: any[], serial: number | null = 4) => ({ relays, serial, issued_at: 1, effective: {}, unconfigured: [], bundle: null });
const NO_ANSWER = "No answer from the server in time.";

test("every relay answered", () => {
  assert.deepEqual(liveLine(live([relay(A), relay(B)]), null), { tone: "ok", text: "Live: serial 4 on 2/2 relays" });
  assert.equal(incomplete(live([relay(A), relay(B)]), null), false);
});

test("no relay could be read: could not check, not 'nothing published yet'", () => {
  const l = live([relay(A, { reachable: false, error: NO_ANSWER, serial: null, verified: false, note: NO_ANSWER }), relay(B, { reachable: false, error: NO_ANSWER, serial: null, verified: false, note: NO_ANSWER })], null);
  assert.deepEqual(liveLine(l, null), { tone: "danger", text: `Live: could not check (a.example: ${NO_ANSWER}; b.example: ${NO_ANSWER})` });
  assert.equal(incomplete(l, null), true);
});

test("nothing published: every relay answered 404", () => {
  const l = live([relay(A, { serial: null, verified: false, note: "No policy published" }), relay(B, { serial: null, verified: false, note: "No policy published" })], null);
  assert.deepEqual(liveLine(l, null), { tone: "ok", text: "Live: nothing published yet" });
  assert.equal(incomplete(l, null), false);
});

test("one relay silent: the serial of the other, behind, asked again", () => {
  const l = live([relay(A), relay(B, { reachable: false, error: NO_ANSWER, serial: null, verified: false, note: NO_ANSWER })]);
  assert.deepEqual(liveLine(l, null), { tone: "warn", text: "Live: serial 4 on 1/2 relays" });
  assert.equal(incomplete(l, null), true);
});

test("first check failed outright, or a later one failed", () => {
  assert.deepEqual(liveLine(null, "Unlock the keys first."), { tone: "danger", text: "Live: could not check (Unlock the keys first.)" });
  assert.deepEqual(liveLine(null, null), { tone: "muted", text: "Live: checking…" });
  assert.equal(liveLine(live([relay(A)]), "x").tone, "danger");
  assert.equal(incomplete(live([relay(A)]), "x"), true);
});
