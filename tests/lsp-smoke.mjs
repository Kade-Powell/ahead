import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { constants } from "node:fs";
import { access, cp, mkdir, mkdtemp, readFile, realpath, rename, stat, symlink, unlink, writeFile } from "node:fs/promises";
import { homedir, tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";
import { fileURLToPath, pathToFileURL } from "node:url";

// By default this is an offline smoke using already-installed language servers.
// Set AHEAD_LSP_INSTALL_MISSING=1 to explicitly exercise AHEAD's managed
// acquisition path; its cache stays under this disposable fixture's
// AHEAD_DATA_HOME. AHEAD_LSP_CACHE_SEED can copy an existing language-server
// cache into that disposable home. AHEAD_LSP_LANGUAGES selects a subset.
const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const fixture = fileURLToPath(new URL("./fixtures/editor-smoke/", import.meta.url));
const selectedLanguages = new Set((process.env.AHEAD_LSP_LANGUAGES ?? "typescript,python,rust")
  .split(",").map((language) => language.trim()).filter(Boolean));
const supportedLanguages = new Set(["typescript", "python", "rust"]);
assert(selectedLanguages.size > 0, "AHEAD_LSP_LANGUAGES must select at least one language");
for (const language of selectedLanguages) {
  assert(supportedLanguages.has(language), `Unsupported LSP smoke language: ${language}`);
}
const rustAnalyzerOverride = process.env.AHEAD_LSP_RUST_ANALYZER_PATH
  ? path.resolve(process.env.AHEAD_LSP_RUST_ANALYZER_PATH)
  : undefined;
const inheritedServerEnvironment = Object.fromEntries(
  [
    "PATH",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "RUSTUP_HOME",
    "RUSTUP_TOOLCHAIN",
    "SystemRoot",
    "WINDIR",
    "PATHEXT",
    "ComSpec",
  ]
    .filter((name) => process.env[name] !== undefined)
    .map((name) => [name, process.env[name]]),
);
assert(
  !Object.keys(inheritedServerEnvironment).some((name) =>
    /(?:api[_-]?key|token|secret|password|credential)/i.test(name),
  ),
  "LSP smoke must not forward credentials to language servers",
);
const isolatedServerEnvironment = {
  ...inheritedServerEnvironment,
  HOME: tmpdir(),
  USERPROFILE: tmpdir(),
  TMPDIR: tmpdir(),
  TMP: tmpdir(),
  TEMP: tmpdir(),
  RUSTUP_HOME:
    inheritedServerEnvironment.RUSTUP_HOME ?? path.join(homedir(), ".rustup"),
};
const serverCommands = [
  ...(selectedLanguages.has("typescript") ? ["vtsls"] : []),
  ...(selectedLanguages.has("python") ? ["basedpyright-langserver", "ruff"] : []),
  ...(selectedLanguages.has("rust") ? ["rust-analyzer"] : []),
  ...(process.env.AHEAD_LSP_INSTALL_ZED_HTML === "1" ? ["vscode-html-language-server"] : []),
];
const installMissingServers = process.env.AHEAD_LSP_INSTALL_MISSING === "1";
const installZedHtmlExtension = process.env.AHEAD_LSP_INSTALL_ZED_HTML === "1";
assert(
  !installZedHtmlExtension || installMissingServers,
  "AHEAD_LSP_INSTALL_ZED_HTML requires AHEAD_LSP_INSTALL_MISSING=1",
);
const serverTimeoutMs = installMissingServers ? 150_000 : 30_000;
async function fetchZedHtmlExtension() {
  const catalogUrl = new URL("https://api.zed.dev/extensions");
  catalogUrl.searchParams.set("max_schema_version", "1");
  catalogUrl.searchParams.set("provides", "language-servers");
  const response = await fetch(catalogUrl, { signal: AbortSignal.timeout(15_000) });
  assert.equal(response.status, 200, `Zed extension catalog returned HTTP ${response.status}`);
  const bytes = new Uint8Array(await response.arrayBuffer());
  assert(bytes.length <= 4 * 1024 * 1024, "Zed extension catalog exceeded AHEAD's response limit");
  const catalog = JSON.parse(new TextDecoder().decode(bytes));
  const entry = catalog.data?.find((candidate) => candidate.id === "html"
    && candidate.schema_version <= 1
    && candidate.provides?.includes("language-servers")
    && ["0.6.0", "0.7.0", "0.8.0"].includes(candidate.wasm_api_version));
  assert(entry, "Zed catalog has no HTML extension supported by AHEAD's current Wasm host");
  assert.match(entry.version, /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/);
  const expectedSha256 = entry.archive_sha256 ?? null;
  if (expectedSha256 !== null) assert.match(expectedSha256, /^[a-fA-F0-9]{64}$/);
  return {
    url: new URL(`/extensions/html/${entry.version}/download`, catalogUrl).href,
    version: entry.version,
    expectedSha256,
  };
}
const zedHtmlExtension = installZedHtmlExtension ? await fetchZedHtmlExtension() : undefined;
const executableExtensions = process.platform === "win32"
  ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
  : [""];
async function commandIsAvailable(command) {
  for (const directory of (process.env.PATH ?? "").split(path.delimiter)) {
    for (const extension of executableExtensions) {
      try {
        const candidate = path.join(directory, `${command}${extension}`);
        const metadata = await stat(candidate);
        await access(candidate, constants.X_OK);
        if (metadata.isFile()) return true;
      } catch {}
    }
  }
  return false;
}
const missingServers = [];
if (!installMissingServers) {
  for (const command of serverCommands) {
    if (command === "rust-analyzer" && rustAnalyzerOverride) {
      const metadata = await stat(rustAnalyzerOverride).catch(() => undefined);
      if (!metadata?.isFile()) missingServers.push(rustAnalyzerOverride);
      else await access(rustAnalyzerOverride, constants.X_OK);
      continue;
    }
    if (!await commandIsAvailable(command)) missingServers.push(command);
  }
  if (serverCommands.includes("rust-analyzer")) {
    const probe = spawnSync(rustAnalyzerOverride ?? "rust-analyzer", ["--version"], {
      encoding: "utf8",
      timeout: 5000,
      env: isolatedServerEnvironment,
    });
    if (probe.error || probe.status !== 0) {
      const detail = (probe.stderr || probe.stdout || probe.error?.message || "unknown error")
        .trim().split(/\r?\n/, 1)[0].replace(/\.$/, "");
      missingServers.push(`rust-analyzer could not start: ${detail}`);
    }
  }
  assert.equal(
    missingServers.length,
    0,
    `Missing installed language servers on PATH: ${missingServers.join(", ")}. Set AHEAD_LSP_INSTALL_MISSING=1 to test managed acquisition.`,
  );
}
const workspace = await mkdtemp(path.join(tmpdir(), "ahead-lsp-smoke-"));
await cp(fixture, workspace, {
  recursive: true,
  filter: (source) => !["target", "node_modules", "__pycache__", ".ahead"].includes(path.basename(source)),
});
const dataHome = path.join(workspace, "ahead-data");
const runtimeHome = path.join(workspace, "ahead-runtime");
const userHome = path.join(workspace, "ahead-user");
const cargoHome = path.join(workspace, "cargo-home");
const cacheHome = path.join(workspace, "cache-home");
const configHome = path.join(workspace, "config-home");
await Promise.all([
  mkdir(dataHome),
  mkdir(runtimeHome),
  mkdir(userHome),
  mkdir(cargoHome),
  mkdir(cacheHome),
  mkdir(configHome),
]);
if (process.env.AHEAD_LSP_CACHE_SEED) {
  const cacheSeed = path.resolve(process.env.AHEAD_LSP_CACHE_SEED);
  const cacheMetadata = await stat(cacheSeed);
  assert(cacheMetadata.isDirectory(), "AHEAD_LSP_CACHE_SEED must name a language-server cache directory");
  const languageServerCache = path.join(dataHome, "cache", "language-servers");
  await mkdir(path.dirname(languageServerCache), { recursive: true });
  await cp(cacheSeed, languageServerCache, { recursive: true });
}
await writeFile(path.join(dataHome, "trusted-workspaces.toml"),
  `trusted = [${JSON.stringify(await realpath(workspace))}]\nblocked = []\n`);
const serverEnvironment = {
  ...isolatedServerEnvironment,
  HOME: userHome,
  USERPROFILE: userHome,
  CARGO_HOME: cargoHome,
  XDG_CACHE_HOME: cacheHome,
  XDG_CONFIG_HOME: configHome,
  AHEAD_DATA_HOME: dataHome,
  AHEAD_HOME: runtimeHome,
  AHEAD_USER_HOME: userHome,
};
if (rustAnalyzerOverride && selectedLanguages.has("rust")) {
  assert.notEqual(process.platform, "win32", "The explicit Rust Analyzer alias is currently Unix-only");
  const serverBin = path.join(workspace, "server-bin");
  await mkdir(serverBin);
  await symlink(rustAnalyzerOverride, path.join(serverBin, "rust-analyzer"));
  serverEnvironment.PATH = `${serverBin}${path.delimiter}${serverEnvironment.PATH ?? ""}`;
}
const proxy = spawn(binary, ["--proxy"], { cwd: workspace, detached: true, stdio: ["pipe", "pipe", "pipe"], env: serverEnvironment });
const messages = [];
const waiters = new Set();
let requestId = 0;
let stderr = "";
let failure;
proxy.stderr.on("data", (chunk) => {
  const text = String(chunk);
  stderr = (stderr + text).slice(-4000);
});
proxy.on("error", (error) => { failure = error; });
proxy.on("exit", (code, signal) => {
  failure = new Error(`Proxy exited: ${code ?? signal}\n${stderr}`);
  for (const waiter of waiters) waiter();
});
const lines = createInterface({ input: proxy.stdout });
lines.on("line", (line) => {
  try { messages.push(JSON.parse(line)); }
  catch (error) { failure = new Error(`Invalid proxy JSON: ${error.message}`); }
  for (const waiter of waiters) waiter();
});
function waitFor(predicate, label, after = messages.length) {
  return new Promise((resolve, reject) => {
    const finish = (error, value) => {
      clearTimeout(timer);
      waiters.delete(check);
      if (error) reject(error); else resolve(value);
    };
    const check = () => {
      if (failure) return finish(failure);
      const recentMessages = messages.slice(after);
      const message = recentMessages.find(predicate);
      if (message) return finish(null, message);
      const failedServer = recentMessages.find((message) => message.method === "server_status"
        && message.params?.params?.health === "error");
      if (failedServer) {
        const params = failedServer.params.params;
        finish(new Error(`${label}: ${params.server_name ?? "language server"} failed: ${params.message ?? "unknown error"}\n${stderr}`));
      }
    };
    const timer = setTimeout(() => {
      const relevant = messages.filter((message) => message.error || ["publish_diagnostics", "server_status", "show_message", "log"].includes(message.method)
        && !message.params?.target?.endsWith("$/logTrace"));
      finish(new Error(`Timed out: ${label}\n${JSON.stringify(relevant.slice(-8))}\n${stderr}`));
    }, serverTimeoutMs);
    waiters.add(check);
    check();
  });
}
async function waitForLanguageServer(serverName, label, after = messages.length) {
  const status = await waitFor((message) => message.method === "server_status"
    && message.params.params.server_name === serverName
    && ["ok", "error"].includes(message.params.params.health), label, after);
  assert.equal(status.params.params.health, "ok",
    `${label}: ${status.params.params.message ?? "server failed"}`);
  console.log(`PASS ${serverName}: initialized`);
}
function notify(method, params) { proxy.stdin.write(JSON.stringify({ method, params }) + "\n"); }
async function request(method, params, expectError = false) {
  const id = ++requestId;
  const reply = waitFor((message) => message.id === id && !message.method, method);
  proxy.stdin.write(JSON.stringify({ id, method, params }) + "\n");
  const message = await reply;
  if (expectError) {
    assert(message.error, `${method}: expected an error`);
    return message.error;
  }
  assert.equal(message.error, undefined, JSON.stringify(message.error));
  return message.result;
}
const editorContents = new Map();
function singleSpliceDelta(previous, current) {
  const oldCharacters = [...previous];
  const newCharacters = [...current];
  let prefix = 0;
  while (prefix < oldCharacters.length && prefix < newCharacters.length
    && oldCharacters[prefix] === newCharacters[prefix]) prefix++;
  let suffix = 0;
  while (suffix < oldCharacters.length - prefix
    && suffix < newCharacters.length - prefix
    && oldCharacters[oldCharacters.length - suffix - 1]
      === newCharacters[newCharacters.length - suffix - 1]) suffix++;
  const prefixText = oldCharacters.slice(0, prefix).join("");
  const oldMiddle = oldCharacters.slice(prefix, oldCharacters.length - suffix).join("");
  const newMiddle = newCharacters.slice(prefix, newCharacters.length - suffix).join("");
  const ops = [];
  const prefixBytes = Buffer.byteLength(prefixText, "utf8");
  const suffixBytes = Buffer.byteLength(oldCharacters.slice(oldCharacters.length - suffix).join(""), "utf8");
  if (prefixBytes > 0) ops.push({ Retain: prefixBytes });
  const deletedBytes = Buffer.byteLength(oldMiddle, "utf8");
  if (deletedBytes > 0) ops.push({ Delete: deletedBytes });
  if (newMiddle.length > 0) ops.push({ Insert: newMiddle });
  if (suffixBytes > 0) ops.push({ Retain: suffixBytes });
  return { base_len: Buffer.byteLength(previous, "utf8"), ops };
}
async function snapshot(relative, content, acceptDiagnostics) {
  const file = path.join(workspace, relative);
  const uri = pathToFileURL(file).href;
  const previous = editorContents.get(file);
  const sendSnapshot = () => {
    if (previous === undefined) notify("editor_snapshot", { path: file, content });
    else if (previous !== content) {
      notify("editor_edit", { path: file, delta: singleSpliceDelta(previous, content) });
    }
    editorContents.set(file, content);
  };
  if (!acceptDiagnostics) {
    sendSnapshot();
    return;
  }
  const diagnostics = waitFor((message) => message.method === "publish_diagnostics"
    && message.params.diagnostics.uri === uri
    && acceptDiagnostics(message.params.diagnostics.diagnostics), `${relative}: diagnostics for ${content.length}-character snapshot`);
  sendSnapshot();
  return (await diagnostics).params.diagnostics.diagnostics;
}
async function completion(relative, position, expected) {
  const id = ++requestId;
  const response = waitFor((message) => message.method === "completion_response" && message.params.request_id === id, `${relative}: completion`);
  notify("completion", { request_id: id, path: path.join(workspace, relative), input: "", position });
  const params = (await response).params;
  const reply = params.resp;
  const items = Array.isArray(reply) ? reply : reply.items;
  const item = items.find((item) => expected instanceof RegExp ? expected.test(item.label) : item.label === expected);
  assert(item, `${relative}: missing ${expected}`);
  return { plugin_id: params.plugin_id, completion_item: item };
}
async function closeBuffer(relative) {
  const file = path.join(workspace, relative);
  const uri = pathToFileURL(file).href;
  const cleared = waitFor((message) => message.method === "publish_diagnostics"
    && message.params.diagnostics.uri === uri && message.params.diagnostics.diagnostics.length === 0, `${relative}: diagnostics cleared on close`);
  notify("close_editor_buffer", { path: file });
  await cleared;
  editorContents.delete(file);
}
async function definition(relative, position, suffix) {
  const response = await request("get_definition", { request_id: ++requestId, path: path.join(workspace, relative), position });
  assert.equal(response.method, "get_definition_response");
  const definition = response.params.definition;
  const locations = Array.isArray(definition) ? definition : [definition];
  assert(locations.some((location) => (location.uri ?? location.targetUri)?.endsWith(suffix)), `${relative}: wrong definition ${JSON.stringify(locations)}`);
}
async function hover(relative, position) {
  const response = await request("get_hover", { request_id: ++requestId, path: path.join(workspace, relative), position });
  assert.equal(response.method, "hover_response");
  return response.params.hover;
}
async function signatureHelp(relative, position, context = { triggerKind: 1, isRetrigger: false }) {
  const id = ++requestId;
  const response = waitFor((message) => message.method === "signature_help_response"
    && message.params.request_id === id, `${relative}: signature help`);
  notify("signature_help", { request_id: id, path: path.join(workspace, relative), position, context });
  return (await response).params.resp;
}
async function references(relative, position) {
  const response = await request("get_references", { path: path.join(workspace, relative), position });
  assert.equal(response.method, "get_references_response");
  return response.params.references;
}
async function formatDocument(relative) {
  const response = await request("get_document_formatting", { path: path.join(workspace, relative) });
  assert.equal(response.method, "get_document_formatting");
  return response.params.edits;
}
function applyTextEdits(text, edits) {
  const lines = text.split("\n");
  const offset = (position) => {
    assert(position.line >= 0 && position.line < lines.length, "edit line outside buffer");
    assert(position.character >= 0 && position.character <= lines[position.line].length,
      "edit UTF-16 column outside line");
    return lines.slice(0, position.line).reduce((length, line) => length + line.length + 1, 0)
      + position.character;
  };
  const ordered = edits.map((edit) => ({
    start: offset(edit.range.start), end: offset(edit.range.end), text: edit.newText,
  })).sort((left, right) => right.start - left.start || right.end - left.end);
  let boundary = text.length;
  for (const edit of ordered) {
    assert(edit.start <= edit.end && edit.end <= boundary, "overlapping or reversed edits");
    text = text.slice(0, edit.start) + edit.text + text.slice(edit.end);
    boundary = edit.start;
  }
  return text;
}
async function crossFileRename(relative, declaration, oldName, newName, position) {
  const declarationText = await readFile(path.join(workspace, declaration), "utf8");
  await snapshot(declaration, declarationText);
  const prepared = await request("prepare_rename", { path: path.join(workspace, relative), position });
  assert.equal(prepared.method, "prepare_rename");
  const preparation = prepared.params.resp;
  assert(preparation.defaultBehavior === true || preparation.range || preparation.start,
    `${relative}: rename preparation has no usable range`);
  const initialReferences = await references(relative, position);
  for (const expected of [relative, declaration]) {
    assert(initialReferences.some((location) => location.uri.endsWith(`/${expected}`)),
      `${relative}: references must include ${expected}`);
  }
  // Renaming an imported binding may create a local alias; rename its declaration instead.
  const declarationOffset = declarationText.indexOf(`${oldName}(`);
  assert(declarationOffset >= 0, "fixture declaration is missing");
  const declarationLines = declarationText.slice(0, declarationOffset + 2).split("\n");
  const declarationPosition = { line: declarationLines.length - 1, character: declarationLines.at(-1).length };
  const renamed = await request("rename", {
    path: path.join(workspace, declaration), position: declarationPosition, new_name: newName,
  });
  assert.equal(renamed.method, "rename");
  const edit = renamed.params.edit;
  assert(!(edit.changes && edit.documentChanges), "conflicting workspace edit forms");
  const documents = edit.documentChanges ?? Object.entries(edit.changes ?? {})
    .map(([uri, edits]) => ({ textDocument: { uri }, edits }));
  const originals = new Map();
  const diskContents = new Map();
  const replacements = new Map();
  for (const document of documents) {
    assert(document.textDocument && Array.isArray(document.edits), "unexpected resource operation");
    const file = fileURLToPath(document.textDocument.uri);
    const relativePath = path.relative(workspace, file);
    assert(relativePath && !relativePath.startsWith("..") && !path.isAbsolute(relativePath),
      "rename edit escaped the disposable workspace");
    assert(!originals.has(relativePath), "duplicate workspace document edit");
    const diskContent = await readFile(file, "utf8");
    const original = editorContents.get(file) ?? diskContent;
    const replacement = applyTextEdits(original, document.edits);
    assert.notEqual(replacement, original, `${relativePath}: rename must change the buffer`);
    assert(document.edits.some((edit) => edit.newText.includes(newName)), `${relativePath}: missing new name`);
    originals.set(relativePath, original);
    diskContents.set(relativePath, diskContent);
    replacements.set(relativePath, replacement);
  }
  for (const expected of [relative, declaration]) assert(replacements.has(expected), `missing rename edits for ${expected}: ${JSON.stringify(edit)}`);
  for (const [file, content] of replacements) await snapshot(file, content);
  const renamedMain = replacements.get(relative);
  assert(!renamedMain.includes(`${oldName}(`), "old call site survived rename");
  const callOffset = renamedMain.lastIndexOf(`${newName}(`);
  assert(callOffset >= 0, "renamed call is missing");
  const prefixLines = renamedMain.slice(0, callOffset + 2).split("\n");
  const renamedPosition = { line: prefixLines.length - 1, character: prefixLines.at(-1).length };
  await definition(relative, renamedPosition, `/${declaration}`);
  assert.match(JSON.stringify(await hover(relative, renamedPosition)), new RegExp(newName));
  const invalidCall = renamedMain.replace(new RegExp(`${newName}\\(\\d+\\)`), `${newName}("bad")`);
  assert.notEqual(invalidCall, renamedMain, "fixture must have a numeric call argument");
  await snapshot(relative, invalidCall, (items) => items.some((item) => item.severity === 1));
  await snapshot(relative, renamedMain, (items) => items.length === 0);
  for (const [file, content] of originals) {
    assert.equal(await readFile(path.join(workspace, file), "utf8"), diskContents.get(file),
      "rename snapshots must not save source files");
    await snapshot(file, content);
  }
  console.log(`PASS ${relative}: cross-file rename edits work in unsaved buffers, navigation and typed diagnostics`);
}
async function semanticTokens(relative) {
  const file = path.join(workspace, relative);
  const response = await request("get_semantic_tokens", { path: file });
  assert.equal(response.method, "get_semantic_tokens");
  const styles = response.params.styles;
  assert.equal(styles.path, file);
  assert.equal(styles.len, Buffer.byteLength(editorContents.get(file), "utf8"));
  assert(styles.styles.length > 0, `${relative}: expected semantic token styles`);
  return styles.styles;
}
async function codeActions(relative, position, diagnostics) {
  const response = await request("get_code_actions", {
    path: path.join(workspace, relative),
    position,
    diagnostics,
  });
  assert.equal(response.method, "get_code_actions_response");
  assert(Array.isArray(response.params.actions));
  return response.params.actions;
}

async function pythonCodeActions(relative, text, diagnostics) {
  const analyzer = await completion(relative, {
    line: 3, character: text.split("\n")[3].indexOf("double") + 3,
  }, "double");
  const actions = await codeActions(relative, { line: 1, character: 8 }, diagnostics);
  assert(actions.some(({ plugin_id }) => plugin_id === analyzer.plugin_id),
    `BasedPyright's code actions are missing: ${JSON.stringify(actions)}`);
  const ruffFix = actions.find(({ plugin_id, action }) => plugin_id !== analyzer.plugin_id
    && action.diagnostics?.some((item) => String(item.code) === "F401"));
  assert(ruffFix, `Ruff's unused-import fix is missing: ${JSON.stringify(actions)}`);
  let action = ruffFix.action;
  const needsResolve = !action.edit && Boolean(action.data);
  if (needsResolve) {
    const resolved = await request("code_action_resolve", {
      plugin_id: ruffFix.plugin_id, action_item: action,
    });
    assert.equal(resolved.method, "code_action_resolve_response");
    action = resolved.params.item;
  }
  assert(action.edit, "Ruff fix must contain a workspace edit after resolve");
  assert(!(action.edit.changes && action.edit.documentChanges), "conflicting edit forms");
  const documents = action.edit.documentChanges ?? Object.entries(action.edit.changes ?? {})
    .map(([uri, edits]) => ({ textDocument: { uri }, edits }));
  assert.equal(documents.length, 1, "unused-import fix must affect only this document");
  assert.equal(documents[0].textDocument?.uri, pathToFileURL(path.join(workspace, relative)).href);
  const fixed = applyTextEdits(text, documents[0].edits);
  assert(!fixed.includes("import os"), "Ruff fix must remove the unused import");
  await snapshot(relative, fixed, (items) => items.length === 0);
  console.log(`PASS Python: analyzer and Ruff code actions coexist; Ruff ${needsResolve ? "resolve uses its owning server" : "inline fix retains its server owner"} and clears diagnostics in the unsaved buffer`);
}

async function pythonFormatting(relative, restoredText) {
  const file = path.join(workspace, relative);
  const onDisk = await readFile(file, "utf8");
  const unformatted = 'from arithmetic import double\nimport os\n\nvalue = { "answer":double(21) }\nprint(value)\n';
  const isUnusedImport = (item) => item.source === "Ruff" && String(item.code) === "F401";
  const hasUnusedImports = (items) => items.some(isUnusedImport)
    && items.some((item) => item.code === "reportUnusedImport");
  await snapshot(relative, unformatted.replace("double(21)", 'double("wrong")'),
    (items) => items.some(isUnusedImport) && items.some((item) => item.message.includes("cannot be assigned")));
  const diagnostics = await snapshot(relative, unformatted,
    (items) => hasUnusedImports(items) && !items.some((item) => item.message.includes("cannot be assigned")));
  await pythonCodeActions(relative, unformatted, diagnostics);
  await snapshot(relative, unformatted, hasUnusedImports);
  const text = applyTextEdits(unformatted, await formatDocument(relative));
  assert.notEqual(text, unformatted, "Ruff must format the unsaved Python buffer");
  assert.match(text, /value = \{"answer": double\(21\)\}/);
  await snapshot(relative, text);
  assert.deepEqual(await formatDocument(relative), [], "Python formatting must be idempotent");
  const typedHover = await hover(relative, {
    line: 3, character: text.split("\n")[3].indexOf("double") + 1,
  });
  assert.match(JSON.stringify(typedHover), /value.*int/,
    "BasedPyright's typed hover must remain available with Ruff attached");
  assert.equal(await readFile(file, "utf8"), onDisk, "formatting must not save the editor buffer");
  await snapshot(relative, restoredText, (items) => items.length === 0);
  console.log("PASS Python: Ruff formatting is idempotent, analyzer/linter diagnostics coexist, typed hover remains available and disk stays unchanged");
}

async function pythonConfigurationReload(relative, restoredText) {
  const directory = path.dirname(path.join(workspace, relative));
  const pyproject = path.join(directory, "pyproject.toml");
  const ruff = path.join(directory, "ruff.toml");
  const retired = path.join(directory, "disabled-ruff.toml");
  for (const file of [pyproject, ruff, retired]) {
    await assert.rejects(access(file), { code: "ENOENT" });
  }
  const text = 'print("configuration reload")\n';
  await snapshot(relative, text);
  const statusStart = messages.length;
  async function expectQuotes(quote, label) {
    const expected = `print(${quote}configuration reload${quote})\n`;
    const deadline = Date.now() + 12_000;
    let formatted;
    do {
      formatted = applyTextEdits(text, await formatDocument(relative));
      if (formatted === expected) return;
      await new Promise((resolve) => setTimeout(resolve, 100));
    } while (Date.now() < deadline);
    assert.equal(
      formatted,
      expected,
      `Ruff must reload ${label} without restarting\n${JSON.stringify(messages.slice(statusStart).filter((message) => message.method === "log" && message.params.target?.includes("watch")), null, 2)}\n${stderr}`,
    );
  }
  try {
    await expectQuotes('"', "default quote style");
    await writeFile(pyproject, "[tool.ruff.format]\nquote-style = 'single'\n");
    await expectQuotes("'", "created pyproject.toml");
    await writeFile(pyproject, "[tool.ruff.format]\nquote-style = 'double'\n");
    await expectQuotes('"', "edited pyproject.toml");
    await writeFile(ruff, "[format]\nquote-style = 'single'\n");
    await expectQuotes("'", "created ruff.toml");
    await writeFile(ruff, "[format]\nquote-style = 'double'\n");
    await expectQuotes('"', "edited ruff.toml");
    await writeFile(ruff, "[format]\nquote-style = 'single'\n");
    await expectQuotes("'", "edited ruff.toml again");
    await rename(ruff, retired);
    await expectQuotes('"', "renamed ruff.toml (fallback to pyproject.toml)");
    await writeFile(pyproject, "[tool.ruff.format]\nquote-style = 'single'\n");
    await expectQuotes("'", "pyproject.toml after rename");
    await unlink(pyproject);
    await expectQuotes('"', "deleted pyproject.toml (default restored)");
    assert(!messages.slice(statusStart).some((message) => message.method === "server_status"
      && message.params.params.server_name === "ruff"), "configuration reload must keep the existing Ruff process");
  } finally {
    for (const file of [pyproject, ruff, retired]) {
      try { await unlink(file); } catch (error) { if (error.code !== "ENOENT") throw error; }
    }
    await snapshot(relative, restoredText, (items) => items.length === 0);
  }
  console.log("PASS Python: creating, editing, renaming and deleting Ruff/pyproject configuration changes live formatting without a server restart");
}

async function pythonServerRouting(relative, restoredText) {
  const config = path.join(workspace, ".ahead", "config.toml");
  await assert.rejects(access(config), { code: "ENOENT" });
  const file = path.join(workspace, relative);
  const diskText = await readFile(file, "utf8");
  const unsaved = restoredText.replace("double(21)", "double(22)");
  await snapshot(relative, unsaved, (items) => items.length === 0);
  let previous = (await completion(relative, { line: 2, character: 9 }, "double")).plugin_id;
  async function changeRouting(text, ruffEnabled) {
    const after = messages.length;
    if (text === undefined) await unlink(config);
    else await writeFile(config, text);
    for (const name of serverCommands.filter((name) => ruffEnabled || name !== "ruff")) {
      await waitFor((message) => message.method === "server_status"
        && message.params.params.server_name === name && message.params.params.health === "ok",
      `${name}: ready after routing change`, after);
    }
    const current = (await completion(relative, { line: 2, character: 9 }, "double")).plugin_id;
    assert.notEqual(current, previous, "routing changes must replace the old server");
    previous = current;
    assert.equal(await readFile(file, "utf8"), diskText, "routing changes must not save unsaved text");
    assert.equal(editorContents.get(file), unsaved);
    if (!ruffEnabled) {
      assert(!messages.slice(after).some((message) => message.method === "server_status"
        && message.params.params.server_name === "ruff"), "disabled Ruff must not start");
    }
  }
  try {
    await changeRouting('[languages.Python]\nlanguage_servers = ["ruff", "basedpyright-langserver"]\n[languages.Python.requests]\n"textDocument/hover" = "basedpyright-langserver"\n"textDocument/formatting" = "ruff"\n', true);
    assert.match(JSON.stringify(await hover(relative, { line: 2, character: 9 })), /double/);
    await formatDocument(relative);
    await changeRouting('[languages.Python]\nlanguage_servers = ["!ruff", "..."]\n', false);
    let error = await request("get_document_formatting", { path: file }, true);
    assert.equal(error.code, -32601, "disabled formatter must not receive requests");
    await changeRouting('[languages.Python]\nlanguage_servers = ["basedpyright-langserver"]\n[languages.Python.requests]\n"textDocument/formatting" = "ruff"\n', false);
    error = await request("get_document_formatting", { path: file }, true);
    assert.equal(error.code, -32602);
    assert.match(error.message, /ruff.*not enabled/, "invalid feature choices must not silently fall back");
  } finally {
    await changeRouting(undefined, true);
    await snapshot(relative, restoredText, (items) => items.length === 0);
  }
  console.log("PASS Python: live server ordering, per-feature routes, disabling/re-enabling Ruff and unsaved-buffer replay");
}
try {
  assert.equal(applyTextEdits("😀 calculate(21)\n", [{
    range: { start: { line: 0, character: 3 }, end: { line: 0, character: 12 } },
    newText: "compute",
  }]), "😀 compute(21)\n", "rename edits use UTF-16 columns, not UTF-8 bytes");
  assert.throws(() => applyTextEdits("value", [{
    range: { start: { line: 0, character: 2 }, end: { line: 0, character: 1 } },
    newText: "invalid",
  }]), /overlapping or reversed/);
  notify("initialize", { workspace, window_id: 1, tab_id: 1 });
  if (zedHtmlExtension) {
    await request("install_language_extension", {
      url: zedHtmlExtension.url,
      extension_id: "html",
      version: zedHtmlExtension.version,
      expected_sha256: zedHtmlExtension.expectedSha256,
    });
    notify("restart_language_servers", {});
    const html = "zed-extension-smoke.html";
    await snapshot(html, "<di");
    await waitForLanguageServer("vscode-html-language-server", "live Zed HTML extension");
    await completion(html, { line: 0, character: 3 }, /^div\b/);
    console.log(`PASS live Zed gallery install and HTML completion (${zedHtmlExtension.version})`);
  }
  const ts = "typescript/src/main.ts";
  const originalTs = await readFile(path.join(workspace, ts), "utf8");
  const tsPrefix = 'import { calculate } from "./math.ts";\nconst result = calculate(21);\nresult.dou';
  const js = "javascript/src/main.mjs";
  const jsText = 'import { triple } from "./math.mjs";\nconsole.log(triple(14));\n';
  let tsCompletion;
  let candidate;
  if (selectedLanguages.has("typescript")) {
    await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("21")'), (items) => items.some((item) => item.code === 2345));
    await snapshot(ts, originalTs, (items) => items.length === 0);
    await snapshot(ts, tsPrefix, (items) => items.some((item) => item.message.includes("Property 'dou'")));
    tsCompletion = await completion(ts, { line: 2, character: 10 }, "doubled");
    await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
    await definition(ts, { line: 1, character: 19 }, "/typescript/src/math.ts");
    await snapshot(ts, tsPrefix.replace("calculate(21)", 'calculate("21")') + "bled", (items) => items.some((item) => item.code === 2345));
    await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
    await semanticTokens(ts);
    console.log("PASS TypeScript: completion, definition, diagnostics, repair and semantic tokens");

    const unformattedTs = "const value={input:1,doubled:2};console.log(value.input);export {};\n";
    await snapshot(ts, unformattedTs.replace("value.input", "value.unknown"),
      (items) => items.some((item) => item.code === 2339));
    await snapshot(ts, unformattedTs, (items) => items.length === 0);
    const formattedTs = applyTextEdits(unformattedTs, await formatDocument(ts));
    assert.notEqual(formattedTs, unformattedTs, "TypeScript formatting must change the buffer");
    await snapshot(ts, formattedTs);
    assert.equal((await formatDocument(ts)).length, 0, "TypeScript formatting should be stable");
    await snapshot(ts, tsPrefix + "bled");
    const typescriptSignature = await signatureHelp(ts, { line: 1, character: 25 });
    assert(typescriptSignature.signatures.some((signature) => /input:\s*number/.test(signature.label)),
      "TypeScript signature help must include the typed argument");
    await crossFileRename(ts, "typescript/src/math.ts", "calculate", "compute", { line: 1, character: 19 });
    console.log("PASS TypeScript: formatting and typed signature help");

    const unresolvedImport = "export {};\ncalculate\n";
    const importDiagnostics = await snapshot(ts, unresolvedImport, (items) => items.some((item) => item.code === 2304));
    const importActions = await codeActions(ts, { line: 1, character: 2 }, importDiagnostics);
    const importFix = importActions.find(({ action }) => action.title.startsWith("Add import"));
    assert(importFix, `Vtsls should offer an import quick fix: ${JSON.stringify(importActions)}`);
    const { plugin_id: importPluginId, action: importAction } = importFix;
    assert.equal(importPluginId, tsCompletion.plugin_id, "import fix must retain its server owner");
    const importFile = pathToFileURL(path.join(workspace, ts)).href;
    assert.deepEqual(Object.keys(importAction.edit?.changes ?? {}), [importFile]);
    assert(importAction.edit.changes[importFile].some((edit) => edit.newText.includes("import { calculate }")));
    assert(importAction.command, "Vtsls import fix should retain its executeCommand contract");
    const commandResult = await request("execute_lsp_command", {
      plugin_id: importPluginId,
      command: importAction.command.command,
      arguments: importAction.command.arguments ?? [],
    });
    assert.equal(commandResult.method, "execute_lsp_command_response");
    const rejectedCommand = await request("execute_lsp_command", {
      plugin_id: importPluginId,
      command: "ahead.notAdvertised",
      arguments: [],
    }, true);
    assert.match(rejectedCommand.message, /server not capable/);
    console.log("PASS TypeScript: real import quick fix edit and advertised command cross the proxy");
    const unimported = "export {};\ncalc\n";
    await snapshot(ts, unimported, (items) => items.some((item) => item.code === 2304));
    candidate = await completion(ts, { line: 1, character: 4 }, "calculate");
    const resolved = await request("completion_resolve", candidate);
    assert.equal(resolved.method, "completion_resolve_response");
    const item = resolved.params.item;
    assert(item.additionalTextEdits?.some((edit) => edit.newText.includes("import") && edit.newText.includes("math")), "Missing resolved auto-import");
    const primary = item.textEdit ?? { range: { start: { line: 1, character: 0 }, end: { line: 1, character: 4 } }, newText: item.insertText ?? item.label };
    const imported = applyTextEdits(unimported, [
      ...item.additionalTextEdits, { range: primary.range ?? primary.replace, newText: primary.newText },
    ]);
    await snapshot(ts, imported, (items) => items.length === 0);
    console.log("PASS TypeScript: server-specific completion resolve supplies a working auto-import");
    await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("21")'), (items) => items.some((item) => item.code === 2345));
    await closeBuffer(ts);
    await snapshot(ts, originalTs.replace("calculate(21)", 'calculate("reopened")'), (items) => items.some((item) => item.code === 2345));
    await snapshot(ts, originalTs, (items) => items.length === 0);
    console.log("PASS TypeScript: close clears unsaved diagnostics; reopen and repair work");

    await snapshot(js, jsText);
    const jsCompletion = await completion(js, { line: 1, character: 15 }, "triple");
    assert.equal(jsCompletion.plugin_id, tsCompletion.plugin_id, "TypeScript and JavaScript must share one server instance");
    await definition(js, { line: 1, character: 15 }, "/javascript/src/math.mjs");
    await semanticTokens(js);
    await snapshot(js, jsText.replace("triple(14)", 'triple("14")'), (items) => items.some((item) => item.code === 2345));
    await snapshot(js, jsText, (items) => items.length === 0);
    console.log("PASS JavaScript: shared adapter, completion, definition, diagnostics, repair and semantic tokens");
    const javascriptSignature = await signatureHelp(js, { line: 1, character: 19 });
    assert(javascriptSignature.signatures.some((signature) => /value:\s*number/.test(signature.label)),
      "JavaScript signature help must include its JSDoc argument type");
    await crossFileRename(js, "javascript/src/math.mjs", "triple", "treble", { line: 1, character: 15 });
  }

  const py = "python/main.py";
  const pyText = "from arithmetic import double\n\nprint(double(21))\n";
  if (selectedLanguages.has("python")) {
    const pythonStart = messages.length;
    await snapshot(py, pyText);
    await waitForLanguageServer(
      "basedpyright-langserver",
      "Python server readiness",
      pythonStart,
    );
    await waitForLanguageServer("ruff", "Python formatter readiness", pythonStart);
    await semanticTokens(py);
    const pythonCompletion = await completion(py, { line: 2, character: 9 }, "double");
    const resolvedPythonCompletion = await request("completion_resolve", pythonCompletion);
    assert.equal(resolvedPythonCompletion.method, "completion_resolve_response");
    const pythonCompletionItem = resolvedPythonCompletion.params.item;
    const pythonCompletionDetail = [
      pythonCompletionItem.detail,
      pythonCompletionItem.documentation?.value,
      pythonCompletionItem.labelDetails?.detail,
      pythonCompletionItem.labelDetails?.description,
    ].filter(Boolean).join(" ");
    assert.match(pythonCompletionDetail, /value:\s*int/,
      `Python completion should expose its typed parameter: ${JSON.stringify(pythonCompletionItem)}`);
    assert.match(pythonCompletionDetail, /->\s*int/,
      `Python completion should expose its return type: ${JSON.stringify(pythonCompletionItem)}`);
    const pythonSignature = await signatureHelp(py, { line: 2, character: 14 });
    assert(pythonSignature.signatures.some((signature) =>
      /\(value:\s*int\)\s*->\s*int/.test(signature.label)),
    `Python signature help should describe the active typed call: ${JSON.stringify(pythonSignature)}\n${stderr}`);
    await definition(py, { line: 2, character: 9 }, "/python/arithmetic.py");
    await snapshot(py, pyText.replace("double(21)", 'double("21")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
    await snapshot(py, pyText, (items) => items.length === 0);
    console.log("PASS Python: completion, definition, diagnostics, repair and semantic tokens");
    await pythonFormatting(py, pyText);
    await pythonConfigurationReload(py, pyText);
    await crossFileRename(py, "python/arithmetic.py", "double", "twice", { line: 2, character: 9 });
    await snapshot(py, pyText.replace("double(21)", 'double("close")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
    await closeBuffer(py);
    await snapshot(py, pyText.replace("double(21)", 'double("reopened")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
    await snapshot(py, pyText, (items) => items.length === 0);
    console.log("PASS Python: close clears unsaved diagnostics; reopen and repair work");
  }

  const rust = "src/main.rs";
  const originalRust = await readFile(path.join(workspace, rust), "utf8");
  const rustLines = originalRust.split("\n");
  const rustLine = rustLines.findIndex((line) => line.includes("math::double(21)"));
  let rustPosition;
  let rustCompletion;
  if (selectedLanguages.has("rust")) {
    const unformattedRust = 'fn formatting_probe(){let value=1; println!("{}",value);}\n';
    await snapshot(rust, unformattedRust, (items) => items.length === 0);
    const rustFormatting = await formatDocument(rust);
    assert(rustFormatting.length > 0, "Rust Analyzer should return formatting edits");
    assert(rustFormatting.some((edit) => edit.newText.includes("\n")), "Rust formatting should insert line breaks");
    await snapshot(rust, originalRust, (items) => items.length === 0);
    console.log("PASS Rust: document formatting returns edits for an unformatted buffer");
    assert(rustLine >= 0, "Rust fixture must call math::double(21)");
    rustPosition = { line: rustLine, character: rustLines[rustLine].indexOf("double") + 3 };
    const rustError = originalRust.replace("double(21)", 'double("21")');
    await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
    await snapshot(rust, originalRust, (items) => items.length === 0);
    await semanticTokens(rust);
    rustCompletion = await completion(rust, rustPosition, /^double(?:\(|$)/);
    await definition(rust, rustPosition, "/src/math.rs");
    assert.match(JSON.stringify(await hover(rust, rustPosition)), /double/);
    const rustSignature = await signatureHelp(rust, {
      line: rustLine,
      character: rustLines[rustLine].indexOf("21") + 1,
    });
    assert(rustSignature.signatures.some((signature) => signature.label.includes("double")), `Rust signature help should describe the active call: ${JSON.stringify(rustSignature)}\n${stderr}`);
    const rustReferences = await references(rust, rustPosition);
    assert(rustReferences.some((location) => location.uri.endsWith("/src/main.rs")), "Rust references must include the call site");
    assert(rustReferences.some((location) => location.uri.endsWith("/src/math.rs")), "Rust references must include the declaration");
    const renamePosition = { line: rustLine, character: rustLines[rustLine].indexOf("double") + 2 };
    const preparedRename = await request("prepare_rename", { path: path.join(workspace, rust), position: renamePosition });
    assert.equal(preparedRename.method, "prepare_rename");
    assert.equal(preparedRename.params.resp.start.character, rustLines[rustLine].indexOf("double"));
    assert.equal(preparedRename.params.resp.end.character, rustLines[rustLine].indexOf("double") + "double".length);
    const rename = await request("rename", { path: path.join(workspace, rust), position: renamePosition, new_name: "twice" });
    assert.equal(rename.method, "rename");
    assert.match(JSON.stringify(rename.params.edit), /twice/);
    assert.match(JSON.stringify(rename.params.edit), /src\/main\.rs/);
    assert.match(JSON.stringify(rename.params.edit), /src\/math\.rs/);
    await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
    await closeBuffer(rust);
    await snapshot(rust, rustError, (items) => items.some((item) => item.code === "E0308"));
    console.log("PASS Rust: completion, definition, semantic tokens, hover, signature help, references, cross-file rename, unsaved diagnostics, repair, close and reopen");
  }

  if (selectedLanguages.has("typescript")) {
    await snapshot(ts, tsPrefix, (items) => items.some((item) => item.message.includes("Property 'dou'")));
  }
  if (selectedLanguages.has("python")) {
    await snapshot(py, pyText.replace("double(21)", 'double("restart")'), (items) => items.some((item) => item.message.includes("cannot be assigned")));
  }
  const beforeRestart = messages.length;
  notify("restart_language_servers", {});
  for (const name of serverCommands) {
    await waitFor((message) => message.method === "server_status"
      && message.params.params.server_name === name && message.params.params.health === "ok", `${name}: ready after restart`, beforeRestart);
  }
  const replayChecks = [
    ...(selectedLanguages.has("typescript") ? [[ts, "Property 'dou'"]] : []),
    ...(selectedLanguages.has("python") ? [[py, "cannot be assigned"]] : []),
    ...(selectedLanguages.has("rust") ? [[rust, "expected i32"]] : []),
  ];
  for (const [relative, errorText] of replayChecks) {
    const uri = pathToFileURL(path.join(workspace, relative)).href;
    await waitFor((message) => message.method === "publish_diagnostics"
      && message.params.diagnostics.uri === uri
      && message.params.diagnostics.diagnostics.some((item) => item.message.includes(errorText)), `${relative}: unsaved diagnostics replayed`, beforeRestart);
    assert(messages.slice(beforeRestart).some((message) => message.method === "publish_diagnostics"
      && message.params.diagnostics.uri === uri && message.params.diagnostics.diagnostics.length === 0), `${relative}: retired diagnostics must clear`);
  }
  if (selectedLanguages.has("typescript")) {
    const restartedTs = await completion(ts, { line: 2, character: 10 }, "doubled");
    assert.notEqual(restartedTs.plugin_id, tsCompletion.plugin_id, "restart must create a new server instance");
    const restartedJs = await completion(js, { line: 1, character: 15 }, "triple");
    assert.equal(restartedTs.plugin_id, restartedJs.plugin_id, "TS/JS must still share the replacement server");
    await snapshot(ts, tsPrefix + "bled", (items) => items.length === 0);
    assert.equal(await readFile(path.join(workspace, ts), "utf8"), originalTs, "restart must not save the unsaved buffer");
    const obsolete = await request("completion_resolve", candidate, true);
    assert.match(obsolete.message, /plugin doesn't exist/);
  }
  if (selectedLanguages.has("python")) {
    await completion(py, { line: 2, character: 9 }, "double");
    await definition(py, { line: 2, character: 9 }, "/python/arithmetic.py");
    await snapshot(py, pyText, (items) => items.length === 0);
    await pythonFormatting(py, pyText);
    await pythonConfigurationReload(py, pyText);
  }
  if (selectedLanguages.has("rust")) {
    const restartedRust = await completion(rust, rustPosition, /^double(?:\(|$)/);
    assert.notEqual(restartedRust.plugin_id, rustCompletion.plugin_id, "Rust restart must create a new instance");
    await definition(rust, rustPosition, "/src/math.rs");
    await snapshot(rust, originalRust, (items) => items.length === 0);
    assert.equal(await readFile(path.join(workspace, rust), "utf8"), originalRust, "Rust restart must not save the unsaved buffer");
  }
  console.log(`PASS restart: fresh processes and selected-language recovery (${[...selectedLanguages].join(", ")})`);
  if (selectedLanguages.has("python")) await pythonServerRouting(py, pyText);
  for (const name of serverCommands) {
    assert(messages.some((message) => message.method === "server_status"
      && message.params.params.server_name === name && message.params.params.health === "ok"), `${name}: missing initialized status`);
    const latestStatus = messages.filter((message) => message.method === "server_status" && message.params.params.server_name === name).at(-1);
    assert.equal(latestStatus.params.params.health, "ok", `${name}: retired process replaced current status`);
  }
  const shutdown = new Promise((resolve) => proxy.once("exit", (code, signal) => resolve({ code, signal })));
  notify("shutdown", {});
  const deadline = setTimeout(() => proxy.kill("SIGKILL"), 10000);
  const stopped = await shutdown.finally(() => clearTimeout(deadline));
  assert.equal(stopped.code, 0, `proxy shutdown failed: ${stopped.signal ?? stderr}`);
  assert.throws(() => process.kill(-proxy.pid, 0), { code: "ESRCH" }, "language-server process group survived graceful shutdown");
  console.log(`PASS standard LSP readiness and graceful proxy shutdown (${[...selectedLanguages].join(", ")}). Disposable workspace retained: ${workspace}`);
} catch (error) {
  console.error(error);
  console.error(JSON.stringify(messages.filter((message) => /log|show_message/.test(message.method)).slice(-20), null, 2));
  console.error(`Disposable workspace retained: ${workspace}`);
  process.exitCode = 1;
} finally {
  lines.close();
  if (proxy.pid) {
    try { process.kill(-proxy.pid, "SIGTERM"); }
    catch (error) { if (error.code !== "ESRCH") throw error; }
  }
}
