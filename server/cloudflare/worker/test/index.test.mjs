// node --test server/cloudflare/worker/test  (Node 18+: global Request/Response)
import { test } from "node:test";
import assert from "node:assert/strict";
import worker from "../src/index.js";

test("a miss is a plain 404, never cached", async () => {
  const r = await worker.fetch(new Request("https://tether-dl.example.workers.dev/stable/customer/windows-x86_64/TetherQS-9.exe.gz"));
  assert.equal(r.status, 404);
  assert.equal(r.headers.get("cache-control"), "no-store");
  assert.equal(await r.text(), "not found\n");
});

test("HEAD has no body", async () => {
  const r = await worker.fetch(new Request("https://x/y", { method: "HEAD" }));
  assert.equal(r.status, 404);
  assert.equal(await r.text(), "");
});

test("writes are refused", async () => {
  for (const method of ["PUT", "POST", "DELETE", "PATCH"]) {
    const r = await worker.fetch(new Request("https://x/stable/a", { method, body: method === "DELETE" ? undefined : "x" }));
    assert.equal(r.status, 405, method);
    assert.equal(r.headers.get("allow"), "GET, HEAD");
  }
});
