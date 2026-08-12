import { execFileSync } from "node:child_process";
import { readFile } from "node:fs/promises";

const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
const tag = process.argv[2] || process.env.GITHUB_REF_NAME || "";
const expected = `pi-v${packageJson.version}`;

requireVersion("Node", process.versions.node, "22.14.0");
const npmVersion = execFileSync("npm", ["--version"], { encoding: "utf8" }).trim();
requireVersion("npm", npmVersion, "11.5.1");

if (tag !== expected) {
  console.error(
    `Release tag ${JSON.stringify(tag)} does not match package version ${packageJson.version}. Expected ${expected}.`,
  );
  process.exit(1);
}

const prerelease = packageJson.version.includes("-");
const distTag = prerelease ? "next" : "latest";
console.log(
  `Validated ${packageJson.name}@${packageJson.version} from ${tag}; npm dist-tag=${distTag}`,
);

if (process.env.GITHUB_OUTPUT) {
  const { appendFile } = await import("node:fs/promises");
  await appendFile(
    process.env.GITHUB_OUTPUT,
    `package_name=${packageJson.name}\nversion=${packageJson.version}\ndist_tag=${distTag}\n`,
  );
}

function requireVersion(name, actual, minimum) {
  const actualParts = actual.split(".").map(Number);
  const minimumParts = minimum.split(".").map(Number);
  for (let index = 0; index < 3; index += 1) {
    if (actualParts[index] > minimumParts[index]) {
      return;
    }
    if (actualParts[index] < minimumParts[index]) {
      console.error(
        `${name} ${actual} is too old for npm trusted publishing; require ${minimum} or newer.`,
      );
      process.exit(1);
    }
  }
}
