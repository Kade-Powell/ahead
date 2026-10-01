import assert from "node:assert/strict";
import test from "node:test";
import { triple } from "./src/math.mjs";

test("JavaScript calculations work across files", () => {
  assert.equal(triple(14), 42);
  assert.equal(triple(-3), -9);
});
