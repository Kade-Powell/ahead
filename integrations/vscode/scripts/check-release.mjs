import { appendFile, readFile } from "node:fs/promises";

const packageJson = JSON.parse(await readFile(new URL("../package.json", import.meta.url), "utf8"));
const tag = process.argv[2] || process.env.GITHUB_REF_NAME || "";
const expected = `vscode-v${packageJson.version}`;

if (tag !== expected) {
  console.error(
    `Release tag ${JSON.stringify(tag)} does not match extension version ${packageJson.version}. Expected ${expected}.`,
  );
  process.exit(1);
}

if (!packageJson.publisher) {
  console.error("The VS Code extension manifest needs a Marketplace publisher.");
  process.exit(1);
}

const extensionId = `${packageJson.publisher}.${packageJson.name}`;
const vsixName = `${packageJson.name}-${packageJson.version}.vsix`;
console.log(`Validated ${extensionId}@${packageJson.version} from ${tag}`);

if (process.env.GITHUB_OUTPUT) {
  await appendFile(
    process.env.GITHUB_OUTPUT,
    `extension_id=${extensionId}\nversion=${packageJson.version}\nvsix_name=${vsixName}\n`,
  );
}
