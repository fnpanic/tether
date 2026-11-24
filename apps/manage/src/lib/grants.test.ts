import { test } from "node:test";
import assert from "node:assert/strict";
import { afterRevoke, grantBadge, grantOf, needsGrant, noGrant, pendingText, revokeOutcome, toRenew, withGrant, AGENT, GRANT, type GrantInfo } from "./grants.ts";

const NOW = 1_800_000_000;
const g = (o: Partial<GrantInfo>): GrantInfo => ({ status: "valid", id: "abcdefghABCDEFGH", exp: NOW + 200 * 86400, customer: null, made_here: true, renew: false, why: null, ...o });

test("on-request and install need a grant; manual, remove, off and unset do not", () => {
  assert.equal(needsGrant({ [AGENT]: { state: "on", value: "install" } }), "install");
  assert.equal(needsGrant({ [AGENT]: { state: "locked", value: "on-request" } }), "on-request");
  for (const v of ["manual", "remove"]) assert.equal(needsGrant({ [AGENT]: { state: "on", value: v } }), null);
  assert.equal(needsGrant({ [AGENT]: { state: "off" } }), null);
  assert.equal(needsGrant({}), null);
});

test("the grant goes into the layer and out again on revoke, health.agent back to manual", () => {
  const layer = withGrant({ [AGENT]: { state: "locked", value: "install" }, "permission.clipboard": { state: "off" } }, "tok");
  assert.equal(grantOf(layer), "tok");
  assert.deepEqual(afterRevoke(layer), { [AGENT]: { state: "locked", value: "manual" }, "permission.clipboard": { state: "off" } });
  // A layer that does not need the grant keeps its health.agent.
  assert.deepEqual(afterRevoke({ [AGENT]: { state: "on", value: "remove" }, [GRANT]: { state: "on", value: "tok" } }), { [AGENT]: { state: "on", value: "remove" } });
});

test("badges: valid, renew below 60 days, expired, revoked, other key", () => {
  assert.deepEqual(grantBadge(g({}), NOW), { text: "Valid", tone: "ok" });
  assert.deepEqual(grantBadge(g({ renew: true, exp: NOW + 30 * 86400 }), NOW), { text: "Expires in 30 days: renew", tone: "warn" });
  assert.deepEqual(grantBadge(g({ status: "expired" }), NOW), { text: "Expired", tone: "danger" });
  assert.deepEqual(grantBadge(g({ status: "revoked" }), NOW), { text: "Revoked", tone: "danger" });
  assert.match(grantBadge(g({ status: "foreign" }), NOW).text, /admin key: renew/);
  assert.deepEqual(toRenew({ layers: { "profile:technician": g({}), "group:a": g({ renew: true }), "group:b": g({ status: "expired" }), "group:c": g({ status: "replaced" }) }, pending: [] }).map(([k]) => k), ["group:a", "group:b"]);
});

test("badges: not valid, and replaced by a newer grant that was not published", () => {
  assert.deepEqual(grantBadge(g({ status: "invalid", why: "bad signature" }), NOW), { text: "Not valid", tone: "danger" });
  assert.deepEqual(grantBadge(g({ status: "replaced" }), NOW), { text: "Replaced by a newer grant (not published)", tone: "warn" });
});

test("no grant: needed when health.agent needs one, a group uses its profile's grant", () => {
  assert.deepEqual(noGrant({}), { line: "no grant", badge: null });
  assert.deepEqual(noGrant({ [AGENT]: { state: "on", value: "install" } }), { line: "no grant", badge: "Needed: health.agent is install" });
  const profile = { [AGENT]: { state: "on" as const, value: "on-request" } };
  // A group without its own health.agent inherits the profile's value.
  assert.deepEqual(noGrant({}, profile), { line: "no grant", badge: "Needed: health.agent is on-request" });
  assert.deepEqual(noGrant({ [AGENT]: { state: "on", value: "manual" } }, profile), { line: "no grant", badge: null });
  assert.deepEqual(noGrant({ [AGENT]: { state: "on", value: "install" } }, withGrant(profile, "tok")), { line: "no grant of its own: uses the profile's grant", badge: null });
});

test("pending lines and the revoke result name the grant and the relays", () => {
  const host = (s: string) => s.replace("https://", "");
  const p = { id: "OLD", layer: "group:x", state: "replaced" as const, relays: ["https://a", "https://b"], waiting: ["https://b"] };
  assert.equal(pendingText(p, host), "Old grant OLD is still valid: it is revoked on every relay once all of them serve the published policy. The new policy is missing on b.");
  assert.equal(pendingText({ ...p, state: "revoked", relays: ["https://b"], waiting: [] }, host, "group x (deleted)"), "Revoked grant OLD of group x (deleted) not yet revoked on b.");
  const rs = [
    { server: "https://a", ok: true, detail: { id: "NEW", note: "Grant NEW revoked" } },
    { server: "https://a", ok: true, detail: { id: "OLD", note: "Grant OLD revoked" } },
    { server: "https://b", ok: false, error: "Grant OLD not revoked: down", detail: { id: "OLD" } },
  ];
  assert.equal(revokeOutcome(rs, host, true), "Grant NEW: revoked on a. Grant OLD: revoked on a; not on b (down): use Revoke again. The published policy still sends it to machines: publish the draft so machines stop trying.");
  assert.equal(revokeOutcome([], host), "Nothing left to revoke on the relays.");
});
