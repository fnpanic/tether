import { test } from "node:test";
import assert from "node:assert/strict";
import { access, can, canSign, pageAllowed, ROLES, type Page, type Role } from "./roles.ts";

const signedIn = (manage: Role[]) => ({ offered: true, problem: null, busy: false, signed_in: true, manage, note: null, alarm: null, relays: [] });
const PAGES: Page[] = ["home", "policy", "devices", "sessions", "activity", "team", "releases", "page", "custom", "diag", "keys"];

test("the vault alone (break-glass) works as before: every page, signing", () => {
  const a = access(true, { ...signedIn([]), signed_in: false });
  assert.ok(canSign(a) && can(a, "operate") && PAGES.every((p) => pageAllowed(p, a)));
  assert.ok(!can(a, "propose-policy"), "proposals are for people signed in without the keys");
});

test("each role reaches exactly its row of design 12.4", () => {
  const rows: Record<Role, Page[]> = {
    auditor: ["home", "policy", "devices", "sessions", "activity", "releases"],
    operator: ["home", "policy", "devices", "sessions", "activity", "releases", "diag"],
    "policy-editor": ["home", "policy", "devices", "sessions", "activity", "team", "releases"],
    publisher: ["home", "policy", "devices", "sessions", "activity", "releases"],
    "key-holder": ["home", "policy", "devices", "sessions", "activity", "team", "releases"],
  };
  for (const r of ROLES) {
    const a = access(false, signedIn([r]));
    assert.deepEqual(PAGES.filter((p) => pageAllowed(p, a)), rows[r], r);
    assert.ok(!canSign(a), `${r} without the keys never signs`);
    assert.equal(can(a, "operate"), r === "operator", r);
    assert.equal(can(a, "propose-policy"), r === "policy-editor", r);
    assert.equal(can(a, "propose-release"), r === "publisher", r);
  }
});

test("the keys sign while signed in only for a key holder", () => {
  for (const r of ROLES) assert.equal(canSign(access(true, signedIn([r]))), r === "key-holder", r);
  const auditor = access(true, signedIn(["auditor"]));
  assert.ok(!can(auditor, "sign") && !pageAllowed("keys", auditor) && pageAllowed("activity", auditor));
  // No role at all: nothing.
  assert.ok(PAGES.every((p) => !pageAllowed(p, access(false, signedIn([])))));
});
