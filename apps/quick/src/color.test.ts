// node --test src/color.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { onAccent } from "./color.ts";

test("text on a brand colour is the one with the higher contrast", () => {
  assert.equal(onAccent("#003366"), "#ffffff"); // navy: black would be 1.67:1
  assert.equal(onAccent("#ffcc00"), "#000000"); // yellow: white would be 1.5:1
  assert.equal(onAccent("#0071e3"), "#ffffff"); // default: white 4.70:1, black 4.47:1
  assert.equal(onAccent("#2997FF"), "#000000");
  assert.equal(onAccent(""), null);
  assert.equal(onAccent("red"), null);
});
