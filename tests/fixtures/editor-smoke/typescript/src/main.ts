import { calculate } from "./math.ts";

// AHEAD_TYPESCRIPT_NEEDLE — café 日本語
const result = calculate(21);
console.log(`${result.input} doubled is ${result.doubled}`);
