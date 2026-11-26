import { test } from "node:test";
import assert from "node:assert/strict";
import { lockedOut, sameDoc, standardRoles, teamChanges, type TeamDoc } from "./teamdoc.ts";

const KEY = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const base = (): TeamDoc => ({ serial: 3, oidc: null, roles: {}, deny: [], oidc_only: false, members: [{ name: "Bridge", key: KEY, write: false, audit: false, sbom: true, agent: false, customers: ["*"], service: true }, { name: "Anna", key: "x", write: true, audit: false, sbom: true, agent: true, customers: ["*"], service: false }] });

test("a key holder sees every change in words", () => {
  const to = base();
  to.oidc = { issuer: "https://login.microsoftonline.com/t/v2.0", tenant: "t", audience: "api", client_id: "app", scope: "api://x/access" };
  to.roles = { "Tether.Manage.Auditor": { manage: "auditor" }, "Tether.Customer.4711": { customers: ["4711"] } };
  to.deny = ["oid-1"];
  to.oidc_only = true;
  to.members[1] = { ...to.members[1], write: false };
  const c = teamChanges(base(), to);
  assert.deepEqual(c, [
    "Sign-in on: issuer https://login.microsoftonline.com/t/v2.0, tenant t, audience api, client app, scope api://x/access",
    "Role Tether.Manage.Auditor added: Tether Manage: auditor",
    "Role Tether.Customer.4711 added: team member; customers 4711",
    "Denied: oid-1 (their sessions end at once)",
    "Sign-in only: keys work only for service entries (1 entries lose access)",
    "Anna: sbom, agent; customers *",
  ]);
  assert.deepEqual(lockedOut(to), ["Anna"], "oidc_only locks out every roster person");
  assert.ok(sameDoc(to, structuredClone(to)) && !sameDoc(base(), to));
  const back = teamChanges(to, base());
  assert.ok(back.includes("Sign-in off: every signed-in session ends on the relays") && back.includes("No longer denied: oid-1") && back.includes("Role Tether.Manage.Auditor removed: sessions with only this role end"));
});

test("a first document from nothing lists every entry; the standard roles cover design 12.4", () => {
  const c = teamChanges(null, base());
  assert.deepEqual(c, ["Add Bridge (sbom; customers *; service)", "Add Anna (write, sbom, agent; customers *)"]);
  const r = standardRoles();
  assert.deepEqual(Object.values(r).filter((g) => g.manage).map((g) => g.manage), ["auditor", "operator", "policy-editor", "publisher", "key-holder"]);
  assert.ok(Object.values(r).every((g) => !g.manage || Object.keys(g).length === 1), "a Manage role grants no team permission");
});
