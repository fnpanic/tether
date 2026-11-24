import { describe, expect, it } from "vitest";
import { checksOf, clip, groupChecks, healthOf, healthText, healthRank, linkHolder, linkState, mergeStatus, notSince, pick, statusNote } from "./health";
import { buildHosts, emptyStore, indexHost, type ReportCheck, type StatusHostReply, type StatusSummary } from "./model";

const NOW = 1_800_000_000;
const sum = (p: Partial<StatusSummary>): StatusSummary => ({
  uuid: "u1",
  tether_id: null,
  host: "PC",
  agent: "0.1.0",
  overall: "ok",
  counts: { ok: 3, warn: 0, crit: 0, unknown: 0, setup: 0 },
  products: [],
  faults: [],
  last_seen: NOW - 60,
  next: 300,
  stale: false,
  relay: "r",
  ...p,
});

describe("health", () => {
  it("joins by Tether ID, agent-only rows get m:<uuid> and no connect", () => {
    const s = [sum({ uuid: "u1", tether_id: "222222222", overall: "crit" }), sum({ uuid: "u9", tether_id: null, host: "LAB-PC", overall: "ok" })];
    const h = buildHosts(emptyStore(), [], [{ uuid: "x", id: "222222222", profile: "customer", version: "0.5.21", os: "", last_seen: NOW }], null, NOW, s);
    expect(h.find((x) => x.id === "222222222")!.health).toBe("crit");
    const m = h.find((x) => x.id === "m:u9")!;
    expect([m.agentOnly, m.hostname, m.health]).toEqual([true, "LAB-PC", "ok"]);
  });
  it("rows without a summary are unchanged; a summary with an unknown Tether ID makes a connectable row", () => {
    const h = buildHosts(emptyStore(), [{ id: "111111111", hostname: "A", username: "", platform: "", last: NOW }], [], null, NOW, [sum({ uuid: "u2", tether_id: "333333333", host: "SRV", products: [{ name: "Records Server", version: "26.3.2", role: "server" }] })]);
    const plain = h.find((x) => x.id === "111111111")!;
    expect([plain.health, plain.agentOnly, "status" in plain]).toEqual(["none", false, false]);
    const r = h.find((x) => x.id === "333333333")!;
    expect([r.agentOnly, r.hostname, r.health]).toEqual([false, "SRV", "ok"]);
    indexHost(r);
    expect(r.hay).toContain("records server");
  });
  it("merges relays: newest last_seen wins", () => {
    expect(mergeStatus([{ relay: "a", hosts: [sum({ uuid: "u1", last_seen: 10, overall: "ok" })] }, { relay: "b", hosts: [sum({ uuid: "u1", last_seen: 20, overall: "warn" })] }, { relay: "c", error: "x" }])).toEqual([
      expect.objectContaining({ uuid: "u1", relay: "b", overall: "warn" }),
    ]);
  });
  it("health states", () => {
    expect(healthOf(undefined)).toBe("none");
    expect(healthOf(sum({ overall: "crit", stale: true }))).toBe("stale");
    expect(healthOf(sum({ overall: "ok", counts: { ok: 3, warn: 0, crit: 0, unknown: 1, setup: 1 } }))).toBe("setup");
    expect(healthOf(sum({ overall: "unknown" }))).toBe("ok");
    expect(["crit", "stale", "warn", "setup", "ok", "none"].map((x) => healthRank(x as never))).toEqual([5, 4, 3, 2, 1, 0]);
    expect(pick({ de: "Dienst" }, "en")).toBe("Dienst");
    expect(pick({ de: "Dienst", en: "Service" }, "de")).toBe("Dienst");
    expect(pick({ de: "Dienst", en: "Service" }, "en")).toBe("Service");
    expect(pick(undefined, "en")).toBe("");
  });
  it("not OK since the earliest warn or crit fault", () => {
    const s = sum({
      faults: [
        { id: "a", state: "warn", since: "2026-11-20T10:00:00Z" },
        { id: "b", state: "unknown", since: "2026-11-01T00:00:00Z" },
        { id: "c", state: "crit", since: "2026-11-21T00:00:00Z" },
        { id: "d", state: "crit" },
      ],
    });
    expect(notSince(s)).toBe(Date.parse("2026-11-20T10:00:00Z") / 1000);
    expect(notSince(sum({}))).toBeNull();
  });
});

describe("P5 review fixes", () => {
  const peer = (id: string) => ({ id, hostname: "PEER", username: "", platform: "", last: NOW });
  it("a device-chosen tether_id never takes an m: key or a prototype name", () => {
    const h = buildHosts(emptyStore(), [], [], null, NOW, [
      sum({ uuid: "u1", tether_id: null, host: "REAL" }),
      sum({ uuid: "u2", tether_id: "m:u1", host: "FAKE" }),
      sum({ uuid: "u3", tether_id: "constructor", host: "C" }),
      sum({ uuid: "u4", tether_id: "__proto__", host: "P" }),
      sum({ uuid: "u5", tether_id: "toString", host: "T" }),
    ]);
    expect(h.map((x) => [x.id, x.hostname, x.agentOnly]).sort()).toEqual([
      ["m:u1", "REAL", true],
      ["m:u2", "FAKE", true],
      ["m:u3", "C", true],
      ["m:u4", "P", true],
      ["m:u5", "T", true],
    ]);
  });
  it("store entries are read as own keys only", () => {
    const h = buildHosts(emptyStore(), [peer("constructor"), peer("__proto__")], [], null, NOW);
    expect(h.map((x) => [x.id, x.tags, x.notes, x.pinned])).toEqual([
      ["constructor", [], "", false],
      ["__proto__", [], "", false],
    ]);
  });
  it("two devices linked to one Tether ID (on different relays): a conflict, nothing joins; a stale summary joins its peer row", () => {
    const h = buildHosts(emptyStore(), [peer("222222222"), peer("111111111")], [], null, NOW, [
      sum({ uuid: "a", tether_id: "111111111", last_seen: NOW - 50, overall: "warn", relay: "r1" }),
      sum({ uuid: "b", tether_id: "111111111", last_seen: NOW - 10, overall: "crit", relay: "r2" }),
      sum({ uuid: "d", tether_id: "222222222", stale: true }),
    ]);
    const host = h.find((x) => x.id === "111111111")!;
    expect([host.status, host.health, host.conflict, host.agentOnly]).toEqual([undefined, "conflict", true, false]);
    for (const u of ["a", "b"]) {
      const m = h.find((x) => x.id === `m:${u}`)!;
      expect([m.agentOnly, m.conflict, m.status?.uuid]).toEqual([true, true, u]);
    }
    expect(h.find((x) => x.id === "m:b")!.health).toBe("crit");
    expect([healthRank("conflict") > healthRank("crit"), healthText(host, NOW)]).toEqual([true, "Link conflict"]);
    // The conflicting agents can be unlinked and removed.
    const st = linkState(h.find((x) => x.id === "m:a")!, null, true);
    expect([st.unlink, st.remove, st.conflict]).toEqual([true, true, true]);
    const p = h.find((x) => x.id === "222222222")!;
    expect([p.hostname, p.health, p.recent, p.agentOnly]).toEqual(["PEER", "stale", true, false]);
  });
  it("relays with the same last_seen: the smaller relay URL wins, whatever the order", () => {
    const a = { relay: "b.example", hosts: [sum({ uuid: "u1", last_seen: 10, overall: "warn" })] };
    const b = { relay: "a.example", hosts: [sum({ uuid: "u1", last_seen: 10, overall: "crit" })] };
    expect(mergeStatus([a, b])[0].relay).toBe("a.example");
    expect(mergeStatus([b, a])[0].relay).toBe("a.example");
  });
  it("a relay that was not reached keeps its last hosts, and the footer says so", () => {
    const r = [
      { relay: "a.example", hosts: [sum({ uuid: "u1" })] },
      { relay: "b.example", hosts: [sum({ uuid: "u2" })], error: "offline: timed out" },
    ];
    expect(mergeStatus(r).map((x) => x.uuid)).toEqual(["u1", "u2"]);
    expect(statusNote(r)).toBe("b.example: timed out");
    expect(statusNote([{ relay: "a.example", hosts: [] }])).toBeUndefined();
  });
  it("an older relay (404) gets a short note instead of nothing", () => {
    expect(statusNote([{ relay: "a.example", error: "unsupported: The server does not offer team access yet." }])).toBe("this relay does not report health");
    expect(statusNote([{ relay: "a.example", error: "off: Health reporting is switched off on this server." }])).toBe("Health reporting is switched off on this server.");
  });
});

describe("report view", () => {
  const chk = (p: Partial<ReportCheck>): ReportCheck => ({ id: "x", state: "ok", title: { en: "X" }, ...p });
  const reply = (report: unknown): StatusHostReply => ({ uuid: "u1", tether_id: null, last_seen: NOW, next: 300, stale: false, report: report as StatusHostReply["report"], history: [] });
  it("a report without products, changes or body shows no checks", () => {
    expect(checksOf(reply({ v: 1, uuid: "u1", host: "PC", seq: 1, time: "", next: 300, agent: "0.1.0", packs: {}, overall: "ok", counts: { ok: 0, warn: 0, crit: 0, unknown: 0, setup: 0 }, faults: [] }))).toEqual([]);
    expect(checksOf(reply({ body: {} }))).toEqual([]);
    expect(checksOf(reply(null))).toEqual([]);
    expect(checksOf(null)).toEqual([]);
    // Malformed entries from the device are dropped, not rendered.
    expect(checksOf(reply({ body: { checks: [null, 5, { id: 3 }, { id: "a", state: "bad" }, { id: "b", state: "warn", title: "plain" }, chk({ id: "ok" })] } })).map((c) => c.id)).toEqual(["b", "ok"]);
    // Duplicate ids: the first wins.
    expect(checksOf(reply({ body: { checks: [chk({ id: "d", state: "crit" }), chk({ id: "d" })] } })).map((c) => c.state)).toEqual(["crit"]);
  });
  it("groups: worst first, crit before warn, then setup and unknown apart", () => {
    const g = groupChecks([
      chk({ id: "s/1", group: "System" }),
      chk({ id: "b/1", group: "Backup", state: "warn", title: { en: "B" } }),
      chk({ id: "a/1", group: "App", state: "warn", title: { en: "W" } }),
      chk({ id: "a/2", group: "App", state: "crit", title: { en: "C" } }),
      chk({ id: "a/3", group: "App" }),
      chk({ id: "c/1", group: "Security", state: "unknown", setup: true }),
      chk({ id: "u/1", state: "unknown" }),
    ]);
    // Shown in this order: all faults (across groups), setup, unknown, then passed per group.
    expect(Object.keys(g)).toEqual(["faults", "setup", "unknown", "passed"]);
    expect(g.faults.map((x) => [x.name, x.checks.map((c) => c.id)])).toEqual([
      ["App", ["a/2", "a/1"]],
      ["Backup", ["b/1"]],
    ]);
    expect(g.setup.map((c) => c.id)).toEqual(["c/1"]);
    expect(g.unknown.map((c) => c.id)).toEqual(["u/1"]);
    expect(g.passed.map((x) => [x.name, x.checks.map((c) => c.id)])).toEqual([
      ["App", ["a/3"]],
      ["System", ["s/1"]],
    ]);
  });
  it("clips device text for display", () => {
    expect(clip("abc", 5)).toBe("abc");
    expect(clip("abcdef", 5)).toBe("abcd…");
    expect(clip(undefined, 5)).toBe("");
    expect(pick({ en: 5 as never, de: "Dienst" }, "en")).toBe("Dienst");
  });
});

describe("health words", () => {
  it("say the state in text", () => {
    const c = (crit: number, warn: number) => sum({ counts: { ok: 1, warn, crit, unknown: 0, setup: 0 }, last_seen: NOW - 3 * 3600 });
    expect(healthText({ health: "crit", status: c(1, 2) }, NOW)).toBe("1 critical, 2 warnings");
    expect(healthText({ health: "warn", status: c(0, 1) }, NOW)).toBe("1 warning");
    expect(healthText({ health: "crit", status: c(0, 0) }, NOW)).toBe("Critical");
    expect(healthText({ health: "stale", status: c(0, 0) }, NOW)).toBe("No health report for 3 h");
    expect(healthText({ health: "stale", status: sum({ last_seen: NOW - 600 }) }, NOW)).toBe("No health report for 10 min");
    expect(healthText({ health: "stale", status: sum({ last_seen: NOW - 3 * 86400 }) }, NOW)).toBe("No health report for 3 d");
    expect([healthText({ health: "ok" }, NOW), healthText({ health: "setup" }, NOW), healthText({ health: "none" }, NOW)]).toEqual(["OK", "Setup needed", "No agent"]);
  });
});

describe("verified links", () => {
  const reply = (p: Partial<StatusHostReply>): StatusHostReply => ({ uuid: "u1", tether_id: null, last_seen: NOW, next: 300, stale: false, report: null, history: [], ...p });
  it("a claimed entry never joins a host row", () => {
    const h = buildHosts(emptyStore(), [{ id: "111111111", hostname: "FRONT", username: "", platform: "", last: NOW }], [], null, NOW, [sum({ uuid: "u7", tether_id: null, claimed_id: "111111111", host: "AGENT", overall: "crit" })]);
    const row = h.find((x) => x.id === "111111111")!;
    expect([row.health, "status" in row]).toEqual(["none", false]);
    const m = h.find((x) => x.id === "m:u7")!;
    expect([m.agentOnly, m.hostname, m.health, m.status?.claimed_id]).toEqual([true, "AGENT", "crit", "111111111"]);
  });
  it("link, unlink and remove are offered only to members with write", () => {
    const [linked, claimed] = buildHosts(emptyStore(), [], [], null, NOW, [sum({ uuid: "u1", claimed_id: "111111111" }), sum({ uuid: "u2", tether_id: "222222222", last_seen: NOW - 5 })]).sort((a, b) => a.id.localeCompare(b.id));
    expect([linked.id, claimed.id]).toEqual(["222222222", "m:u1"]);
    const c = linkState(claimed, null, true);
    expect([c.claim, c.link, c.unlink, c.remove, c.linked]).toEqual(["111111111", "on", false, true, null]);
    expect([linkState(claimed, null, false).link, linkState(claimed, null, false).remove]).toEqual(["disabled", false]);
    const l = linkState(linked, reply({ tether_id: "222222222", linked_by: "Anna", linked_at: 1000 }), true);
    expect([l.claim, l.link, l.unlink, l.remove, l.linked]).toEqual([null, "none", true, true, { by: "Anna", at: 1000 }]);
    const ro = linkState(linked, reply({ tether_id: "222222222" }), false);
    expect([ro.unlink, ro.remove, ro.linked]).toEqual([false, false, { by: null, at: null }]);
    // No summary: nothing to offer.
    expect(linkState({ agentOnly: false, status: undefined }, null, true)).toEqual({ claim: null, linked: null, link: "none", blocked: null, unlink: false, remove: false, dismissed: false, allow: false, conflict: false });
  });
  it("a dismissed link offers Allow a new claim to members with write", () => {
    const [m] = buildHosts(emptyStore(), [], [], null, NOW, [sum({ uuid: "u1" })]);
    const rep = reply({ no_claim: true });
    expect([linkState(m, rep, true).dismissed, linkState(m, rep, true).allow, linkState(m, rep, false).allow]).toEqual([true, true, false]);
    expect(linkState(m, reply({}), true).dismissed).toBe(false);
  });
});

describe("link refusals", () => {
  it("a claim cannot be linked to a host that already has a health agent joined", () => {
    const hosts = buildHosts(emptyStore(), [], [], null, NOW, [sum({ uuid: "u1", claimed_id: "111111111", host: "IMPOSTER" }), sum({ uuid: "u2", tether_id: "111111111", host: "FRONT", linked_at: 500 })]);
    const claimed = hosts.find((x) => x.id === "m:u1")!;
    const holder = hosts.find((x) => x.id === "111111111")!.status;
    const st = linkState(claimed, null, true, holder);
    expect([st.link, st.blocked]).toEqual(["blocked", { host: "FRONT", at: 500 }]);
    expect(linkState(claimed, null, true, undefined).link).toBe("on");
  });
});

describe("final review fixes", () => {
  it("a claim to a host in a link conflict cannot be linked", () => {
    const hosts = buildHosts(emptyStore(), [], [], null, NOW, [
      sum({ uuid: "u1", tether_id: "111111111", relay: "a" }),
      sum({ uuid: "u2", tether_id: "111111111", relay: "b" }),
      sum({ uuid: "u3", claimed_id: "111111111", host: "NEW" }),
    ]);
    const host = hosts.find((x) => x.id === "111111111")!;
    expect(linkHolder(host)).toBe("conflict");
    const st = linkState(hosts.find((x) => x.id === "m:u3")!, null, true, linkHolder(host));
    expect([st.link, st.blocked]).toEqual(["blocked", { host: "", at: null, conflict: true }]);
    // A joined host: its agent; an agent-only row or no row: nothing.
    const one = buildHosts(emptyStore(), [], [], null, NOW, [sum({ uuid: "u1", tether_id: "111111111" })]);
    expect(linkHolder(one[0])?.toString()).toBe(one[0].status?.toString());
    expect([linkHolder(undefined), linkHolder(hosts.find((x) => x.id === "m:u1"))]).toEqual([undefined, undefined]);
  });
  it("a dismissed agent says so from the summary alone", () => {
    const [m] = buildHosts(emptyStore(), [], [], null, NOW, [sum({ uuid: "u1", no_claim: true })]);
    expect(linkState(m, null, true).dismissed).toBe(true);
  });
});
