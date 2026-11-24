import { test } from "node:test";
import assert from "node:assert/strict";
import { afterRevoke, grantBadge, grantOf, needsGrant, toRenew, withGrant, AGENT, GRANT, type GrantInfo } from "./grants.ts";

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
  assert.deepEqual(toRenew({ layers: { "profile:technician": g({}), "group:a": g({ renew: true }), "group:b": g({ status: "expired" }) }, pending: [] }).map(([k]) => k), ["group:a", "group:b"]);
});
