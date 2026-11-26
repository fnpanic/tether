import { test } from "node:test";
import assert from "node:assert/strict";
import { gained, lockedOut, sameDoc, standardRoles, teamChanges, type TeamDoc } from "./teamdoc.ts";

const KEY = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";
const base = (): TeamDoc => ({ serial: 3, oidc: null, roles: {}, deny: [], oidc_only: false, members: [{ name: "Bridge", key: KEY, write: false, audit: false, sbom: true, agent: false, customers: ["*"], service: true }, { name: "Anna", key: "x", write: true, audit: false, sbom: true, agent: true, customers: ["*"], service: false }] });
const texts = (from: TeamDoc | null, to: TeamDoc) => teamChanges(from, to).map((c) => c.text);

test("a key holder sees every change in words, security-relevant ones first", () => {
  const to = base();
  to.oidc = { issuer: "https://login.microsoftonline.com/t/v2.0", tenant: "t", audience: "api", client_id: "app", scope: "api://x/access" };
  to.roles = { "Tether.Manage.Auditor": { manage: "auditor" }, "Tether.Customer.4711": { customers: ["4711"] } };
  to.deny = ["oid-1"];
  to.oidc_only = true;
  to.members[1] = { ...to.members[1], write: false };
  assert.deepEqual(texts(base(), to), [
    "Sign-in on: issuer `https://login.microsoftonline.com/t/v2.0`, tenant `t`, audience `api`, client `app`, scope `api://x/access`",
    "Role `Tether.Manage.Auditor` added: Tether Manage: auditor",
    "Role `Tether.Customer.4711` added: team member; customers 4711 (gains customers 4711)",
    "Denied: `oid-1` (their sessions end at once)",
    "Sign-in only: keys work only for service entries (1 entries lose access)",
    "Anna: sbom, agent; customers *",
  ]);
  assert.deepEqual(teamChanges(base(), to).map((c) => [c.risk, c.confirm]), [[true, true], [true, false], [true, false], [true, false], [true, false], [false, false]], "losing write is no risk");
  assert.deepEqual(lockedOut(to), ["Anna"], "oidc_only locks out every roster person");
  assert.ok(sameDoc(to, structuredClone(to)) && !sameDoc(base(), to));
  const back = texts(to, base());
  assert.ok(back.includes("Sign-in off: every signed-in session ends on the relays") && back.includes("No longer denied: `oid-1` (may sign in again)") && back.includes("Role `Tether.Manage.Auditor` removed: sessions with only this role end"));
});

test("gains of write, sbom, agent and customers, a KeyHolder mapping and service switches are flagged; the tick for sign-in and KeyHolder", () => {
  const from = base();
  from.oidc = { issuer: "https://i/v2.0", tenant: "t", audience: "api", client_id: "app", scope: "s" };
  from.roles = { "Tether.Technician": { customers: ["47*"] }, "Tether.Ops": { manage: "operator" } };
  const to = structuredClone(from);
  to.oidc!.tenant = "evil";
  to.roles["Tether.Technician"] = { customers: ["47*", "48*"], agent: true };
  to.roles["Tether.Ops"] = { manage: "key-holder" };
  to.members[0] = { ...to.members[0], service: false };
  to.members[1] = { ...to.members[1], customers: ["4711"] };
  to.members.push({ name: "Script", key: "y", write: false, audit: false, sbom: false, agent: false, customers: [], service: false });
  assert.deepEqual(
    teamChanges(from, to).map((x) => [x.text, x.risk, x.confirm]),
    [
      ["Sign-in tenant: `t` → `evil` (every signed-in session ends)", true, true],
      ["Role `Tether.Technician`: team member; customers 47* → agent; customers 47*, 48* (gains agent, customers 48*)", true, false],
      ["Role `Tether.Ops`: Tether Manage: operator → Tether Manage: key-holder", true, true],
      ["Bridge: sbom; customers * (no longer a service entry)", true, false],
      ["Anna: write, sbom, agent; customers 4711", false, false],
      ["Add Script (read only; customers none)", false, false],
    ],
  );
  assert.deepEqual(gained(null, { customers: [] }), []);
  assert.deepEqual(gained({ customers: ["47*"] }, { customers: ["*"], write: true }), ["write", "every customer"]);
  assert.deepEqual(gained({ customers: ["*"] }, { customers: ["4711"] }), []);
  // A role without customers reaches every customer.
  assert.equal(teamChanges({ ...from, roles: {} }, { ...from, roles: { "Tether.X": {} } })[0].text, "Role `Tether.X` added: team member (gains every customer)");
});

test("a first document from nothing (or from a plain roster) lists every entry and role as added", () => {
  const d = base();
  d.roles = { "Tether.Manage.KeyHolder": { manage: "key-holder" } };
  const c = teamChanges(null, d);
  assert.deepEqual(
    c.map((x) => x.text),
    ["Role `Tether.Manage.KeyHolder` added: Tether Manage: key-holder", "Add Bridge (sbom; customers *; service) (gains sbom, every customer)", "Add Anna (write, sbom, agent; customers *) (gains write, sbom, agent, every customer)"],
  );
  assert.ok(c[0].confirm && c.every((x) => x.risk));
  const r = standardRoles();
  assert.deepEqual(Object.values(r).filter((g) => g.manage).map((g) => g.manage), ["auditor", "operator", "policy-editor", "publisher", "key-holder"]);
  assert.ok(Object.values(r).every((g) => !g.manage || Object.keys(g).length === 1), "a Manage role grants no team permission");
});
