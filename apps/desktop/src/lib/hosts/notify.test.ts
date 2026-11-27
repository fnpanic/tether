import { describe, expect, it } from "vitest";
import type { HealthState, Host } from "./model";
import { healthAlerts } from "./notify";

const host = (id: string, health: HealthState): Host =>
  ({ id, name: "", hostname: `PC-${id}`, user: "", health, status: { counts: { ok: 1, warn: 2, crit: 1, unknown: 0, setup: 0 }, last_seen: -1800 } }) as unknown as Host;

describe("health notifications", () => {
  it("one alert per host and change, at most hourly", () => {
    let r = healthAlerts(null, [host("a", "crit")], 1000);
    expect(r.alerts).toEqual([]); // first refresh: baseline only
    r = healthAlerts(r.next, [host("a", "ok"), host("b", "stale")], 1300);
    expect(r.alerts.map((a) => a.key)).toEqual(["b"]);
    r = healthAlerts(r.next, [host("a", "crit"), host("b", "ok")], 1600);
    expect(r.alerts.map((a) => a.key)).toEqual(["a"]);
    r = healthAlerts(r.next, [host("a", "ok"), host("b", "crit")], 2000);
    expect(r.alerts).toEqual([]); // b alerted at 1300, < 3600 s ago
    r = healthAlerts(r.next, [host("b", "ok")], 5000);
    r = healthAlerts(r.next, [host("b", "crit")], 5100);
    expect(r.alerts.map((a) => a.key)).toEqual(["b"]);
  });
  it("hosts without health data never alert and the switch-off is the caller's", () => {
    let r = healthAlerts(null, [host("a", "none")], 0);
    r = healthAlerts(r.next, [host("a", "none"), host("n", "none")], 10);
    expect(r.alerts).toEqual([]);
  });
  it("a host back after a gap in the same state does not alert again", () => {
    let r = healthAlerts(null, [host("a", "crit"), host("b", "ok")], 0);
    r = healthAlerts(r.next, [host("a", "none"), host("b", "ok")], 4000); // summary missing (relay error)
    r = healthAlerts(r.next, [host("b", "ok")], 8000); // row gone altogether
    r = healthAlerts(r.next, [host("a", "crit"), host("b", "ok")], 12000);
    expect(r.alerts).toEqual([]);
    r = healthAlerts(r.next, [host("a", "stale")], 16000);
    expect(r.alerts.map((a) => a.key)).toEqual(["a"]); // a real change still alerts
  });
  it("more than 3 alerts in one refresh become one summary", () => {
    const ids = ["a", "b", "c", "d", "e"];
    let r = healthAlerts(null, ids.map((i) => host(i, "ok")), 0);
    r = healthAlerts(r.next, [host("a", "crit")], 10);
    expect(r.alerts.map((a) => a.key)).toEqual(["a"]);
    r = healthAlerts(r.next, ids.map((i) => host(i, i === "a" ? "crit" : "stale")), 20);
    expect(r.alerts).toEqual([{ key: "many", title: "4 hosts need attention", body: "PC-b, PC-c, PC-d, PC-e" }]);
    // Each of them counts as notified (hourly limit).
    r = healthAlerts(r.next, ids.map((i) => host(i, "ok")), 30);
    r = healthAlerts(r.next, ids.map((i) => host(i, "crit")), 40);
    expect(r.alerts).toEqual([]);
  });
  it("no repeat while the state stays, crit to stale is a change, warn never alerts", () => {
    let r = healthAlerts(null, [host("a", "ok")], 0);
    r = healthAlerts(r.next, [host("a", "warn")], 10);
    expect(r.alerts).toEqual([]);
    r = healthAlerts(r.next, [host("a", "crit")], 20);
    expect(r.alerts).toEqual([{ key: "a", title: "PC-a is critical", body: "1 critical, 2 warnings" }]);
    r = healthAlerts(r.next, [host("a", "crit")], 8000);
    expect(r.alerts).toEqual([]);
    r = healthAlerts(r.next, [host("a", "stale")], 9000);
    expect(r.alerts.map((a) => [a.title, a.body])).toEqual([["PC-a stopped reporting", "No health report for 3 h"]]);
  });
});
