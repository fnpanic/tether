import { describe, expect, it } from "vitest";
import { permissionLine, PERM_TEXT, signInView, type SignIn } from "./signin";
import { agentMenu } from "./agent";

const base: SignIn = { offered: true, busy: false, signed_in: false, relays: [] };
const signedIn: SignIn = { ...base, signed_in: true, name: "Software Technician", upn: "software@tether.test", roles: ["Tether.Technician", "Tether.Software"], exp: 1_795_000_000, write: false, audit: false, sbom: true, agent: false, customers: ["*"], relays: [{ relay: "relay.example.com", ok: true, text: "Signed in." }] };

describe("sign-in part of the Team dialog", () => {
  it("is not shown before the check or when no relay offers sign-in (the key roster as before)", () => {
    expect(signInView(undefined, true).show).toBe(false);
    expect(signInView({ ...base, offered: null }, true).show).toBe(false);
    expect(signInView({ ...base, offered: false }, true).show).toBe(false);
  });

  it("offers Sign in, with what a roster key already has", () => {
    const v = signInView(base, true);
    expect([v.show, v.title, v.action, v.warn]).toEqual([true, "Not signed in", "sign-in", []]);
    expect(v.lines[0]).toContain("key is on your team's list");
    expect(signInView(base, false).lines[0]).toBe("Sign in with your organisation's account to use team features.");
  });

  it("shows the person, roles, permissions and expiry, and Sign out", () => {
    const v = signInView(signedIn, false);
    expect([v.title, v.action, v.warn]).toEqual(["Signed in as Software Technician (software@tether.test)", "sign-out", []]);
    expect(v.lines[0]).toBe("Roles: Tether.Technician, Tether.Software.");
    expect(v.lines[1]).toBe("May software lists and software search. Not: change shared tags and links, read a computer's session log, install or remove the health monitoring agent.");
    expect(v.lines[2]).toMatch(/^Session until .+; renewed automatically\.$/);
  });

  it("shows the name without hidden characters", () => {
    expect(signInView({ ...signedIn, name: "Anna\u202e\u200bnimdA", upn: "anna@x.test\u2066" }, false).title).toBe("Signed in as AnnanimdA (anna@x.test)");
  });

  it("names each relay that refused while signed in at another", () => {
    const v = signInView({ ...signedIn, relays: [...signedIn.relays, { relay: "relay2.example.com", ok: false, text: "Your account is blocked for Tether by your administrator." }] }, false);
    expect(v.warn).toEqual(["relay2.example.com: Your account is blocked for Tether by your administrator."]);
  });

  it("states: waiting for the browser, provider unreachable, refused, no role, settings not usable, signed out", () => {
    expect(signInView({ ...base, busy: true }, false)).toMatchObject({ title: "Signing in…", action: "cancel" });
    expect(signInView({ ...base, note: "provider: The sign-in provider cannot be reached: timed out" }, false).warn).toEqual(["The sign-in provider cannot be reached: timed out"]);
    const noRole = "relay.example.com: Your account has no Tether role. Ask your administrator to assign one.";
    expect(signInView({ ...base, note: noRole, relays: [{ relay: "relay.example.com", ok: false, text: "x" }] }, false).warn).toEqual([noRole]);
    // Without a note (a later check), each relay's refusal.
    expect(signInView({ ...base, relays: [{ relay: "relay.example.com", ok: false, text: "This server does not offer sign-in." }] }, false).warn).toEqual(["relay.example.com: This server does not offer sign-in."]);
    const problem = "Sign-in refused: the servers a and b name different sign-in settings.";
    expect(signInView({ ...base, offered: false, problem }, false)).toMatchObject({ show: true, title: "Sign-in is not available", warn: [problem], action: null });
    const out = signInView({ ...base, note: "Signed out." }, false);
    expect([out.warn, out.lines.at(-1)]).toEqual([[], "Signed out."]);
  });

  it("a customer scope is named; a role without extras", () => {
    expect(permissionLine({ write: false, audit: false, sbom: false, agent: false, customers: ["4711", "47*"] })).toBe("May see the host list and health reports. Not: change shared tags and links, read a computer's session log, software lists and software search, install or remove the health monitoring agent. Only computers of customers 4711, 47*.");
    expect(permissionLine({ write: true, audit: true, sbom: true, agent: true })).toBe("May change shared tags and links, read a computer's session log, software lists and software search, install or remove the health monitoring agent.");
  });
});

describe("the session's agent menu item", () => {
  it("is disabled with the reason when the team role does not allow it", () => {
    expect(agentMenu(false, false, false)).toEqual({ disabled: true, hint: PERM_TEXT.agent.missing });
    expect(agentMenu(false, false, true).disabled).toBe(false);
  });
});
