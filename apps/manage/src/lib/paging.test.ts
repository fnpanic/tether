// node --test src/lib/paging.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { paginate } from "./paging.ts";

const rows = Array.from({ length: 10_000 }, (_, i) => i);

test("10 000 rows: 100 pages of 100, only one page rendered", () => {
  const p = paginate(rows, 0);
  assert.deepEqual([p.pages, p.from, p.to, p.total, p.items.length], [100, 1, 100, 10_000, 100]);
  const last = paginate(rows, 99);
  assert.deepEqual([last.from, last.to, last.items[0], last.items.at(-1)], [9_901, 10_000, 9_900, 9_999]);
});

test("a partial last page and out-of-range pages are clamped", () => {
  const r = rows.slice(0, 250);
  assert.deepEqual([paginate(r, 2).from, paginate(r, 2).to, paginate(r, 2).items.length], [201, 250, 50]);
  assert.equal(paginate(r, 7).page, 2, "past the end: last page (e.g. after a filter shrank the list)");
  assert.equal(paginate(r, -3).page, 0);
  assert.equal(paginate(r, NaN).page, 0);
});

test("empty list: one empty page, 0 of 0", () => {
  const p = paginate([], 5);
  assert.deepEqual([p.page, p.pages, p.from, p.to, p.total, p.items.length], [0, 1, 0, 0, 0, 0]);
});
