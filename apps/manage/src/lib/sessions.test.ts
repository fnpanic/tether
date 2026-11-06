// node --test src/lib/sessions.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { dedupe, duration, matches, sessions, toCsv, type AuditEvent } from "./sessions.ts";

const ev = (ts: number, action: string, extra: Partial<AuditEvent> = {}): AuditEvent => ({
  ts, action, kind: action === "send" || action === "receive" ? "file" : "conn", id: "111 222 333", uuid: "QUJD", conn_id: 7, ...extra,
});

test("the same record from two relays or a retry is listed once", () => {
  const a = ev(100, "connect", { nonce: "n1", relay: "https://r1" });
  const b = ev(101, "connect", { nonce: "n1", relay: "https://r2" }); // same post, other relay time
  const c = ev(100, "disconnect");
  const d = ev(100, "disconnect", { relay: "https://r2" }); // no nonce: device+conn_id+action+ts
  assert.deepEqual(dedupe([a, b, c, d]), [a, c]);
});

test("connections: technician on every row, duration on the disconnect, newest first", () => {
  const rows = sessions([
    ev(100, "connect", { ip: "198.51.100.2" }),
    ev(101, "login", { peer_id: "987", peer_name: "Anna", conn_type: "remote" }),
    ev(160, "receive", { peer_id: "987", path: "C:\\x", files: [["a.txt", 3]], num: 1 }),
    ev(3825, "disconnect"),
    // Host restarted: conn_id 7 again is a different connection, no login seen.
    ev(5000, "connect"),
    ev(5030, "disconnect"),
    // Another host, still open.
    ev(4000, "connect", { uuid: "OTHER" }),
    { ts: 4100, kind: "alarm", action: "alarm", id: "1", uuid: "OTHER", conn_id: 9, typ: 0, info: "{}" },
  ]);
  assert.deepEqual(
    rows.map((r) => [r.ts, r.action, r.peer_id ?? null, r.duration ?? null]),
    [
      [5030, "disconnect", null, 30],
      [5000, "connect", null, null],
      [4100, "alarm", null, null],
      [4000, "connect", null, null],
      [3825, "disconnect", "987", 3725],
      [160, "receive", "987", null],
      [101, "login", "987", null],
      [100, "connect", "987", null],
    ],
  );
  assert.equal(rows[4].peer_name, "Anna");
  assert.equal(rows[4].ip, "198.51.100.2"); // from the connect record
  assert.equal(rows[0].ip, undefined);
  assert.equal(duration(3725), "1 h 2 min");
  assert.equal(duration(30), "30 s");
  assert.equal(duration(125), "2 min 5 s");
});

test("search ignores spaces in IDs", () => {
  const r = sessions([ev(1, "login", { peer_id: "987 654 321", peer_name: "Anna" })])[0];
  assert.ok(matches(r, "111222"));
  assert.ok(matches(r, "987654321"));
  assert.ok(matches(r, "anna"));
  assert.ok(!matches(r, "bob"));
});

test("CSV: quoted, formulas defused, one line per row", () => {
  const csv = toCsv(sessions([ev(0, "receive", { peer_name: '=HYPERLINK("x")', path: 'C:\\a "b"', files: [["f.txt", 1]], num: 4 })]));
  const [head, line, end] = csv.split("\r\n");
  assert.ok(head.startsWith("time_utc,host_id,"));
  assert.ok(line.startsWith('"1970-01-01T00:00:00.000Z","111 222 333","QUJD",'));
  assert.ok(line.includes(`"'=HYPERLINK(""x"")"`));
  assert.ok(line.includes('"C:\\a ""b"""'));
  assert.ok(line.includes('"4: f.txt"'));
  assert.equal(end, "");
});
