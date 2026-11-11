import { test } from "node:test";
import assert from "node:assert/strict";
import { bytes, fraction, parseBytes } from "./format.ts";

test("bytes", () => {
  assert.equal(bytes(0), "0 B");
  assert.equal(bytes(1023), "1023 B");
  assert.equal(bytes(1536), "1.50 KB");
  assert.equal(bytes(5 * 1024 ** 3), "5.00 GB");
  assert.equal(bytes(700 * 1024 ** 2), "700 MB");
  assert.equal(bytes(null), "—");
});

test("parseBytes", () => {
  assert.equal(parseBytes("2 GB"), 2 * 1024 ** 3);
  assert.equal(parseBytes("500"), 500 * 1024 ** 2, "megabytes by default");
  assert.equal(parseBytes("1.5gb"), 1.5 * 1024 ** 3);
  assert.equal(parseBytes(""), null);
  assert.equal(parseBytes("lots"), undefined);
  assert.equal(parseBytes("-5 MB"), undefined);
});

test("fraction", () => {
  assert.equal(fraction(5, 10), 0.5);
  assert.equal(fraction(5, 0), null);
  assert.equal(fraction(20, 10), 1);
});
