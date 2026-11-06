import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import path from "node:path";
import { createInterface } from "node:readline";

const binary = path.resolve(process.argv[2] ?? "target/debug/ahead");
const root = await mkdtemp(path.join(tmpdir(), "ahead-git-source-control-"));
const workspace = path.join(root, "project");
const profile = path.join(root, "profile");
const home = path.join(profile, "home");
const environment = {
  PATH: process.env.PATH ?? "/usr/bin:/bin:/usr/sbin:/sbin",
  HOME: home,
  TMPDIR: tmpdir(),
  LANG: process.env.LANG,
  LC_ALL: process.env.LC_ALL,
  RUST_LOG: "error",
  AHEAD_DATA_HOME: path.join(profile, "data"),
  AHEAD_HOME: path.join(profile, "runtime"),
  AHEAD_USER_HOME: path.join(profile, "user"),
  GIT_CONFIG_GLOBAL: path.join(profile, "gitconfig"),
  GIT_CONFIG_NOSYSTEM: "1",
  GIT_AUTHOR_NAME: "Human Developer",
  GIT_AUTHOR_EMAIL: "human@example.invalid",
  GIT_COMMITTER_NAME: "Human Developer",
  GIT_COMMITTER_EMAIL: "human@example.invalid",
};
for (const directory of [
  workspace,
  home,
  environment.AHEAD_DATA_HOME,
  environment.AHEAD_HOME,
  environment.AHEAD_USER_HOME,
]) {
  await mkdir(directory, { recursive: true, mode: 0o700 });
}
await writeFile(environment.GIT_CONFIG_GLOBAL, "");

function gitAt(directory, arguments_) {
  const result = spawnSync("git", arguments_, {
    cwd: directory,
    env: environment,
    encoding: "utf8",
  });
  assert.equal(
    result.status,
    0,
    `git ${arguments_.join(" ")} failed in ${directory}: ${result.stderr}`,
  );
  return result.stdout.trim();
}

const git = (arguments_) => gitAt(workspace, arguments_);
const source = path.join(workspace, "src", "main.ts");
await mkdir(path.dirname(source), { recursive: true });
await writeFile(source, "export const value = 1;\n");
git(["init", "--quiet", "--initial-branch=main"]);
git(["config", "user.name", "Human Developer"]);
git(["config", "user.email", "human@example.invalid"]);
git(["config", "commit.gpgsign", "false"]);

const origin = path.join(root, "origin.git");
const backup = path.join(root, "backup.git");
gitAt(root, ["init", "--quiet", "--bare", "--initial-branch=main", origin]);
gitAt(root, ["init", "--quiet", "--bare", "--initial-branch=main", backup]);
git(["remote", "add", "origin", origin]);
git(["remote", "add", "backup", backup]);

let child;
let lines;
let stderr = "";
let exited = false;
let exitResult;
let nextId = 0;
const pending = new Map();
const notifications = [];
function request(method, params = {}) {
  const id = ++nextId;
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      pending.delete(id);
      reject(new Error(`Timed out waiting for ${method}\n${stderr}`));
    }, 15000);
    pending.set(id, { resolve, reject, timer });
    child.stdin.write(`${JSON.stringify({ id, method, params })}\n`);
  });
}

function startProxy() {
  child = spawn(binary, ["--proxy"], {
    cwd: workspace,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: environment,
  });
  exited = false;
  exitResult = undefined;
  child.stderr.on("data", (chunk) => {
    stderr = (stderr + chunk).slice(-5000);
  });
  child.once("exit", (code, signal) => {
    exited = true;
    exitResult = { code, signal };
    for (const waiter of pending.values()) {
      clearTimeout(waiter.timer);
      waiter.reject(new Error(`Proxy exited before replying\n${stderr}`));
    }
    pending.clear();
  });
  lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const message = JSON.parse(line);
    if (message.method) {
      notifications.push(message);
      return;
    }
    const waiter = pending.get(message.id);
    if (!waiter) return;
    pending.delete(message.id);
    clearTimeout(waiter.timer);
    if (message.error) waiter.reject(new Error(JSON.stringify(message.error)));
    else waiter.resolve(message.result);
  });
  child.stdin.write(`${JSON.stringify({
    method: "initialize",
    params: { workspace, window_id: 1, tab_id: 1 },
  })}\n`);
}

async function response(method, params = {}) {
  const result = await request(method, params);
  assert.equal(result.method, method, `Unexpected response for ${method}: ${JSON.stringify(result)}`);
  return result.params;
}

async function operation(method, params = {}) {
  const result = await request(method, params);
  assert.equal(result.method, "success", `Unexpected response for ${method}: ${JSON.stringify(result)}`);
}

async function status() {
  return (await response("git_status")).status;
}

async function stopProxy() {
  child.stdin.write(`${JSON.stringify({ method: "shutdown", params: {} })}\n`);
  while (!exited) await new Promise((resolve) => setTimeout(resolve, 10));
  assert.deepEqual(exitResult, { code: 0, signal: null }, stderr);
  lines.close();
}

let passed = false;
try {
  startProxy();
  const initial = await status();
  assert.equal(initial.branch, "main");
  assert.deepEqual(
    initial.files.map((file) => file.path),
    [".ahead/.gitignore", "src/main.ts"],
  );
  const privateStateRules = await readFile(
    path.join(workspace, ".ahead", ".gitignore"),
    "utf8",
  );
  assert(privateStateRules.startsWith("# Private and runtime AHEAD state"));
  git(["add", ".ahead/.gitignore", "src/main.ts"]);
  git(["commit", "--quiet", "-m", "Initial commit"]);
  const committedInitial = await status();
  assert(committedInitial.branches.includes("main"));
  assert.deepEqual(committedInitial.files, []);

  await writeFile(source, "export const value = 2;\n");
  await writeFile(path.join(workspace, "new file 'one'.ts"), "export {};\n");
  const changed = await status();
  assert(changed.files.some((file) => file.path === "src/main.ts" && !file.staged));
  assert(changed.files.some((file) => file.path === "new file 'one'.ts" && !file.staged));

  const patch = await response("git_worktree_patch");
  assert(patch.patch.patch.includes("src/main.ts"));
  assert(patch.patch.patch.includes("new file 'one'.ts"));
  await operation("git_stage_paths", { paths: ["new file 'one'.ts"], staged: true });
  let staged = await status();
  assert(staged.files.some((file) => file.path === "new file 'one'.ts" && file.staged));
  assert(staged.files.some((file) => file.path === "src/main.ts" && !file.staged));
  await operation("git_stage_paths", { paths: ["new file 'one'.ts"], staged: false });
  staged = await status();
  assert(staged.files.every((file) => !file.staged));
  await operation("git_stage_all");
  staged = await status();
  assert(staged.files.length === 2 && staged.files.every((file) => file.staged));

  await operation("git_commit", { message: "Committed through AHEAD", diffs: [] });
  assert.deepEqual((await status()).files, []);
  const history = await response("git_history", { limit: 10 });
  const commit = history.commits.find((entry) => entry.subject === "Committed through AHEAD");
  assert(commit, JSON.stringify(history.commits));
  const commitPatch = await response("git_commit_patch", { id: commit.id });
  assert(commitPatch.patch.patch.includes("export const value = 2;"));

  await operation("git_push", { remote: "origin" });
  let published = await status();
  assert(published.branch_has_upstream);
  assert.equal(gitAt(origin, ["rev-parse", "refs/heads/main"]), commit.id);
  await operation("git_push", { remote: "backup" });
  assert.equal(gitAt(backup, ["rev-parse", "refs/heads/main"]), commit.id);
  assert.equal(git(["config", "branch.main.remote"]), "origin");
  await assert.rejects(operation("git_push", { remote: "missing" }), /no longer exists/);

  await operation("git_create_branch", { reference: "merged-branch" });
  await operation("git_checkout", { reference: "main" });
  await operation("git_delete_branch", { reference: "merged-branch" });
  await assert.rejects(operation("git_create_branch", { reference: "bad..branch" }));
  await assert.rejects(operation("git_delete_branch", { reference: "main" }), /current branch/);

  await operation("git_create_branch", { reference: "side" });
  await writeFile(source, "export const value = 'side';\n");
  git(["add", "src/main.ts"]);
  git(["commit", "--quiet", "-m", "Side change"]);
  await operation("git_checkout", { reference: "main" });
  await writeFile(source, "export const value = 'main';\n");
  git(["add", "src/main.ts"]);
  git(["commit", "--quiet", "-m", "Main change"]);
  const merge = spawnSync("git", ["merge", "side"], {
    cwd: workspace,
    env: environment,
    encoding: "utf8",
  });
  assert.notEqual(merge.status, 0, "the fixture should create a merge conflict");
  const conflicts = await status();
  assert(conflicts.repository_has_conflicts);
  assert(conflicts.files.some((file) => file.path === "src/main.ts" && file.state === "UU"));

  await writeFile(path.join(workspace, "safe.txt"), "stage this without the conflict\n");
  await operation("git_stage_all");
  staged = await status();
  assert(staged.files.some((file) => file.path === "safe.txt" && file.staged));
  assert(staged.files.some((file) => file.path === "src/main.ts" && !file.staged));
  await assert.rejects(operation("git_commit", { message: "Blocked conflict", diffs: [] }), /conflicted files/);

  await writeFile(source, "export const value = 'resolved';\n");
  await operation("git_stage_paths", { paths: ["src/main.ts"], staged: true });
  await operation("git_commit", { message: "Resolve merge in AHEAD", diffs: [] });
  assert.deepEqual((await status()).files, []);
  await operation("git_push", { remote: "origin" });
  const resolvedCommit = git(["rev-parse", "HEAD"]);
  assert.equal(gitAt(origin, ["rev-parse", "refs/heads/main"]), resolvedCommit);
  assert.equal(await readFile(source, "utf8"), "export const value = 'resolved';\n");

  await stopProxy();
  passed = true;
  console.log("PASS disposable Git workflow: status, diff, per-file stage/unstage, stage all, commit/history, publish/push, branch lifecycle, conflict protection and resolution");
} finally {
  if (child && !exited && child.pid) {
    process.kill(-child.pid, "SIGKILL");
    while (!exited) await new Promise((resolve) => setTimeout(resolve, 10));
    lines?.close();
  }
  if (passed) await rm(root, { recursive: true, force: true });
  else console.error(`Disposable Git fixture retained at ${root}`);
}
