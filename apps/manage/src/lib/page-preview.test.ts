// node --test src/lib/page-preview.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { previewDoc } from "./page-preview.ts";

test("links in the preview cannot navigate the preview frame", () => {
  const doc = previewDoc('<!doctype html><html lang="en"><head><meta charset="utf-8"></head><body><a href="#macos">macOS</a></body></html>');
  assert.match(doc, /<head><base target="_blank"><meta charset="utf-8">/);
  assert.equal(doc.match(/<base /g)?.length, 1);
});
