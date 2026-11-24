import { describe, expect, it } from "vitest";
import { agentConfirm, agentDate, agentMenu, agentView, practiceLine, type AgentAnswer } from "./agent";

const T = 1_795_000_000;
const ans = (state: string, p: Partial<AgentAnswer> = {}): AgentAnswer => ({ state, version: "", installed_by: "", detail: "", at: 0, ...p });

describe("session dialog: one sentence and its buttons per state", () => {
  it("each state the host can answer", () => {
    const rows: [AgentAnswer, string, string[]][] = [
      [ans("not-installed"), "The health monitoring agent is not installed on the remote computer.", ["install"]],
      [ans("installed", { version: "0.2.0", installed_by: "policy", at: T }), `Installed 0.2.0 by Tether on ${agentDate(T)} (policy).`, ["remove"]],
      [ans("installed", { version: "0.2.0", installed_by: "session" }), "Installed 0.2.0 by Tether (technician in a session).", ["remove"]],
      [ans("installed-by-hand", { version: "0.1.4" }), "Installed by hand 0.1.4 (managed outside Tether).", ["remove"]],
      [ans("removed-locally", { at: T }), `Removed on the remote computer on ${agentDate(T)} by someone at that computer. Tether does not install it again by itself.`, ["reinstall"]],
      [ans("removed-by-tether", { at: T }), `Removed by Tether on ${agentDate(T)}.`, ["install"]],
      [ans("failed", { detail: "download refused" }), "The last action on the remote computer did not succeed.", ["again", "install"]],
      [ans("running"), "Still running on the remote computer. Ask again in a moment to see the result.", ["again"]],
      [ans("busy"), "Tether on the remote computer is busy with the agent right now. Ask again in a moment.", ["again"]],
      [ans("not-allowed"), "Not allowed by the remote computer's policy.", []],
      [ans("needs-service"), "Needs Tether's service on the remote computer (Tether installed as a service).", []],
      [ans("not-supported"), "The remote computer does not support this: the health monitoring agent is for Windows computers with Tether installed.", []],
      [ans("too-old"), "Tether on the remote computer is too old (needs 0.7.0).", []],
      [ans("<b>x</b>", { detail: "something new" }), "The remote computer gave an answer this version of Tether does not know.", ["again"]],
    ];
    for (const [a, text, offers] of rows) {
      const v = agentView(a);
      expect([a.state, v.text, v.offers]).toEqual([a.state, text, offers]);
    }
  });
  it("running is never shown as failed, and asks again", () => {
    const v = agentView(ans("running", { detail: "failed" }), "install");
    expect([v.problem, v.detail, v.offers, /fail/i.test(v.text)]).toEqual([false, "", ["again"], false]);
  });
  it("an install's own result says it is linked and when the first report comes", () => {
    const v = agentView(ans("installed", { version: "0.2.0", installed_by: "session", detail: "installed 0.2.0, service running" }), "install");
    expect(v.text).toBe("Installed 0.2.0, linked to this host; first report within five minutes.");
    expect(v.detail).toBe("installed 0.2.0, service running");
    expect(agentView(ans("removed-by-tether"), "remove").text).toBe("Removed: the agent's service, folders and enrolment are gone.");
  });
  it("the state decides the wording; detail is an added, clipped line and never replaces it", () => {
    const long = "x".repeat(5000);
    const v = agentView(ans("failed", { detail: long, version: "9".repeat(500) }));
    expect(v.text).toBe("The last action on the remote computer did not succeed.");
    expect(v.detail.length).toBe(300);
    expect(agentView(ans("installed", { version: "1".repeat(500) })).text.length).toBeLessThan(80);
    // A refusal's own text (host wording, "this computer") is not shown: the sentence is the viewer's.
    expect(agentView(ans("not-allowed", { detail: "not allowed by this computer's policy" })).detail).toBe("");
    // The unknown state's name is not shown at all.
    expect(agentView(ans("<img src=x onerror=alert(1)>")).text).not.toContain("img");
    // Fields of the wrong type (a hostile or broken answer) read as empty.
    expect(agentView({ state: "installed", version: 5, installed_by: {}, detail: null, at: "x" } as unknown as AgentAnswer).text).toBe("Installed by Tether.");
  });
  it("dates: only a plausible unix time", () => {
    expect([agentDate(0), agentDate(-1), agentDate(1e15), agentDate("1795000000"), agentDate(NaN)]).toEqual(["", "", "", "", ""]);
    expect(agentDate(T)).not.toBe("");
  });
});

describe("session dialog: confirmations", () => {
  it("install says it stays and how the practice removes it", () => {
    const c = agentConfirm("install", ans("not-installed"));
    expect([c.title, c.button, c.action, c.warn]).toEqual(["Install the health monitoring agent?", "Install", "install", undefined]);
    expect(c.text).toContain("It stays after this session ends");
    expect(c.text).toContain("can remove it under Settings › Apps (Tether Monitor)");
  });
  it("install again after the practice's removal names the date and warns first", () => {
    const c = agentConfirm("reinstall", ans("removed-locally", { at: T }));
    expect([c.title, c.button, c.action]).toEqual(["Install the agent again?", "Install again", "install"]);
    expect(c.warn).toBe(`Someone at the remote computer removed the health monitoring agent on ${agentDate(T)}. Install it again only if they agree; they see this session.`);
  });
  it("remove says what goes, also for an agent installed by hand", () => {
    const c = agentConfirm("remove", ans("installed"));
    expect([c.title, c.button, c.action]).toEqual(["Remove the health monitoring agent?", "Remove", "remove"]);
    expect(c.text).toContain("its service, its folders and its enrolment on every server");
    expect(agentConfirm("remove", ans("installed-by-hand")).text).toContain("It was installed by hand, outside Tether.");
  });
});

describe("session menu item", () => {
  it("disabled with a reason without control", () => {
    expect(agentMenu(false, false)).toEqual({ disabled: false, hint: "Install or remove it on the remote computer" });
    expect(agentMenu(true, false).disabled).toBe(true);
    expect(agentMenu(true, false).hint).toContain("view-only");
    expect(agentMenu(false, true)).toEqual({ disabled: true, hint: "Needs control: the person at the remote computer has turned off mouse and keyboard." });
  });
});

describe("Settings: the practice's line", () => {
  it("only for an agent Tether installed: when, at whose request, how to remove it", () => {
    expect(practiceLine(ans("installed", { installed_by: "policy", at: T }))).toBe(
      `Health monitoring agent: installed by Tether on ${agentDate(T)} at your organisation's request (policy). What it reports: the health of this computer's software and, if your organisation collects it, the list of installed programs (see Tether's privacy policy). Remove it in Settings › Apps (Tether Monitor).`,
    );
    expect(practiceLine(ans("installed", { installed_by: "session" }))).toContain("installed by Tether at your organisation's request (technician in a session).");
    for (const s of ["not-installed", "installed-by-hand", "removed-locally", "removed-by-tether", "failed", "needs-service", "not-supported", "not-allowed"]) expect(practiceLine(ans(s))).toBeNull();
    expect(practiceLine(null)).toBeNull();
  });
});
