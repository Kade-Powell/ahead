import assert from "node:assert/strict";
import test from "node:test";
import { calculate, double } from "./src/math.ts";

test("TypeScript calculations work across files", () => {
  assert.deepEqual(calculate(21), { input: 21, doubled: 42 });
  assert.equal(double(-3), -6);
  assert.equal(double(0), 0);
});
