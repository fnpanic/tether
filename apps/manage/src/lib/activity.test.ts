import { test } from "node:test";
import assert from "node:assert/strict";
import { activityCsv, refused, targetText, who, type ActivityRecord } from "./activity.ts";

const rec = (over: Partial<ActivityRecord>): ActivityRecord => ({ ts: 1_795_000_000, event: "report", relay: "https://r1", ...over });

test("people, targets and refusals as the page shows them", () => {
  const r = rec({ actor: { key_fingerprint: "ab", name: "Anna", oid: "o1", upn: "anna@example.com" }, target: { uuid: "dev-1", tether_id: "123456789", customer: "4711" }, result: 200 });
  assert.equal(who(r), "Anna (anna@example.com)");
  assert.equal(who(rec({})), "–");
  assert.equal(targetText(r), "123456789 · dev-1 · customer 4711");
  assert.ok(!refused(r) && refused(rec({ result: 403 })) && refused(rec({ event: "sign-in-refused", result: 401 })));
});

test("the CSV quotes what a spreadsheet would run or split", () => {
  const csv = activityCsv([rec({ actor: { key_fingerprint: "ab", name: "=cmd|x" }, detail: { q: "a,b" }, self_reported: true, result: 204 })]);
  const [head, line] = csv.trimEnd().split("\r\n");
  assert.equal(head, "time (UTC),relay,person,oid,event,target,detail,result,self-reported,ip");
  assert.equal(line, `2026-11-18T11:06:40.000Z,https://r1,"'=cmd|x",,report,,"{""q"":""a,b""}",204,yes,`);
});
