import { expect, test } from "vitest";
import { DRIVES, join, parent } from "./paths";

const W = String.fromCharCode(92);
const w = (p: string) => p.replaceAll("/", W);

test("going up on Windows: the drive root, then the drive list, and back into a drive", () => {
  expect(parent(w("C:/Users/customer"), W)).toBe(w("C:/Users"));
  expect(parent(w("C:/Users"), W)).toBe(w("C:/"));
  expect(parent(w("C:/Users/"), W)).toBe(w("C:/"));
  expect(parent(w("C:/"), W)).toBe(DRIVES);
  expect(parent(DRIVES, W)).toBe(DRIVES);
  expect(join(DRIVES, "D:", W)).toBe(w("D:/"));
  expect(parent(w("//server/share/a"), W)).toBe(w("//server/share"));
});

test("going up on POSIX stops at /", () => {
  expect(parent("/Users/me", "/")).toBe("/Users");
  expect(parent("/Users", "/")).toBe("/");
  expect(parent("/", "/")).toBe("/");
});

test("join", () => {
  expect(join(w("C:/"), "a", W)).toBe(w("C:/a"));
  expect(join(w("C:/Users"), "a", W)).toBe(w("C:/Users/a"));
  expect(join("", "a", "/")).toBe("a");
});
