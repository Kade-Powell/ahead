import { execFileSync, spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { lstat, readFile, readlink } from "node:fs/promises";
import { isAbsolute, resolve } from "node:path";

const excludedAheadPathspec = ":(exclude).ahead/**";
const findingHeadingPattern = /^##\s+(AR-\d{3,})\b/gim;
const fingerprintPattern = /^AHEAD-Review-Snapshot:\s*([a-f0-9]{64})\s*$/im;

export interface SourceLocation {
  path: string;
  line?: number;
  column?: number;
}

export interface ReviewSnapshot {
  api_version: "ahead.review-snapshot/v0.1";
  base_ref: string;
  base_oid: string;
  merge_base_oid: string;
  head_oid: string;
  captured_at: string;
  fingerprint: string;
  status: string;
  changed_files: string[];
  untracked_files: Array<{ path: string; sha256: string }>;
  diff: string;
}

export interface ReviewHost {
  showDiff(snapshot: ReviewSnapshot): Promise<void>;
  openLocation(location: SourceLocation): Promise<boolean>;
}

export async function collectReviewSnapshot(root: string): Promise<ReviewSnapshot> {
  const headOid = gitValue(root, ["rev-parse", "HEAD^{commit}"]);
  const base = headOid ? resolveReviewBase(root) : "empty-tree";
  const baseOid = headOid
    ? git(root, ["rev-parse", `${base}^{commit}`])
    : git(root, ["hash-object", "-t", "tree", "--stdin"], "");
  const mergeBaseOid = headOid ? git(root, ["merge-base", baseOid, headOid]) : baseOid;
  const pathspec = ["--", ".", excludedAheadPathspec];
  const status = git(root, ["status", "--porcelain=v1", "--untracked-files=all", ...pathspec]);
  const diff = git(root, [
    "diff",
    "--no-ext-diff",
    "--binary",
    "--unified=3",
    mergeBaseOid,
    ...pathspec,
  ]);
  const trackedFiles = gitPaths(root, [
    "diff",
    "--name-only",
    "--diff-filter=ACDMRTUXB",
    "-z",
    mergeBaseOid,
    ...pathspec,
  ]);
  const untrackedPaths = gitPaths(root, [
    "ls-files",
    "--others",
    "--exclude-standard",
    "-z",
    "--",
    ".",
    excludedAheadPathspec,
  ]);
  const untrackedFiles = [];
  for (const path of untrackedPaths) {
    const absolute = safeRepositoryPath(root, path);
    const metadata = await lstat(absolute);
    const content = metadata.isSymbolicLink()
      ? Buffer.from(`symlink:${await readlink(absolute)}`)
      : metadata.isFile()
        ? await readFile(absolute)
        : Buffer.from(`special:${metadata.mode}`);
    untrackedFiles.push({ path, sha256: createHash("sha256").update(content).digest("hex") });
  }
  const changedFiles = [...new Set([...trackedFiles, ...untrackedPaths])].toSorted();
  const fingerprint = createHash("sha256")
    .update(baseOid)
    .update("\0")
    .update(mergeBaseOid)
    .update("\0")
    .update(headOid ?? "UNBORN")
    .update("\0")
    .update(status)
    .update("\0")
    .update(diff)
    .update("\0")
    .update(JSON.stringify(untrackedFiles))
    .digest("hex");
  return {
    api_version: "ahead.review-snapshot/v0.1",
    base_ref: base,
    base_oid: baseOid,
    merge_base_oid: mergeBaseOid,
    head_oid: headOid ?? "UNBORN",
    captured_at: new Date().toISOString(),
    fingerprint,
    status,
    changed_files: changedFiles,
    untracked_files: untrackedFiles,
    diff,
  };
}

export function reviewSnapshotMarkdown(snapshot: ReviewSnapshot): string {
  return `# Review snapshot

AHEAD-Review-Snapshot: ${snapshot.fingerprint}

- Base ref: \`${snapshot.base_ref}\`
- Base commit: \`${snapshot.base_oid}\`
- Merge base: \`${snapshot.merge_base_oid}\`
- HEAD: \`${snapshot.head_oid}\`
- Captured: ${snapshot.captured_at}
- Changed files: ${snapshot.changed_files.length}
- Untracked files: ${snapshot.untracked_files.length}

## Changed files

${snapshot.changed_files.length ? snapshot.changed_files.map((path) => `- \`${path}\``).join("\n") : "No engineering changes detected."}

## Working-tree status

\`\`\`text
${snapshot.status || "clean"}
\`\`\``;
}

export function reviewRequest(snapshot: ReviewSnapshot): string {
  return [
    "AHEAD mode: independently review the exact current changeset before human review.",
    `AHEAD-Review-Snapshot: ${snapshot.fingerprint}`,
    `Review the diff from merge base ${snapshot.merge_base_oid} through the working tree, including the listed untracked files.`,
    `Changed files: ${snapshot.changed_files.length ? snapshot.changed_files.join(", ") : "none"}.`,
    "First call ahead_get_review_snapshot and confirm its fingerprint still matches. If it differs, stop and tell the human the snapshot changed.",
    "Review correctness, security, tests, architecture, plan compliance, operations, and maintainability without modifying files.",
    "Use this exact Markdown shape for each finding: `## AR-001`, then nonempty `- Severity:`, `- Category:`, `- Location:`, `- Evidence:`, `- Impact:`, and `- Explanation:` fields. Use `## No material findings` with supporting evidence only when there are none.",
    "End with `## Areas not assessed` and a nonempty statement. Keep questions separate from findings.",
    "Include the exact AHEAD-Review-Snapshot line in the artifact. Record only AI findings as ai-review with ahead_record_artifact. Do not propose or record the human disposition, accept the gate, or transition the run.",
  ].join("\n");
}

export function extractFindingIds(content: string): string[] {
  return [
    ...new Set([...content.matchAll(findingHeadingPattern)].map((match) => match[1])),
  ].toSorted();
}

export function extractReviewFingerprint(content: string): string | undefined {
  return content.match(fingerprintPattern)?.[1];
}

export function validateAiReviewArtifact(content: string, expectedFingerprint: string): string[] {
  const errors = [];
  if (extractReviewFingerprint(content) !== expectedFingerprint) {
    errors.push("the AI review is not bound to the current changeset snapshot");
  }
  const headingIds = [...content.matchAll(findingHeadingPattern)].map((match) => match[1]);
  const findingIds = [...new Set(headingIds)];
  if (findingIds.length !== headingIds.length) {
    errors.push("AI review finding identifiers must be unique");
  }
  if (findingIds.length === 0) {
    const noFindings = markdownSection(content, "No material findings");
    if (!hasSubstantiveContent(noFindings)) {
      errors.push(
        "AI review must contain structured AR findings or a supported no-findings statement",
      );
    }
  }
  for (const id of findingIds) {
    const section = markdownSection(content, id);
    for (const field of ["Severity", "Category", "Location", "Evidence", "Impact", "Explanation"]) {
      if (!section?.match(new RegExp(`^- ${field}:[ \\t]*(\\S.*)$`, "im"))?.[1]?.trim()) {
        errors.push(`${id} needs a nonempty ${field.toLowerCase()} field`);
      }
    }
  }
  if (!hasSubstantiveContent(markdownSection(content, "Areas not assessed"))) {
    errors.push("AI review must state which material areas were not assessed, including none");
  }
  return errors;
}

export function reviewDispositionTemplate(snapshot: ReviewSnapshot, findingIds: string[]): string {
  const dispositions = findingIds.length
    ? findingIds
        .map(
          (id) => `## ${id}

- Disposition: <!-- fixed | invalid | accepted-risk | follow-up -->
- Rationale and evidence:
- Resulting change or follow-up:
`,
        )
        .join("\n")
    : `## No material findings

- Confirmation: <!-- Confirm that the AI review reported no material AR findings. -->
`;
  return `# Human disposition of AI review findings

AHEAD-Review-Snapshot: ${snapshot.fingerprint}

<!-- This is a human-owned record. Validate the AI findings; do not copy an AI decision. Every material AR finding must have one allowed disposition and rationale. -->

${dispositions}`;
}

export function validateReviewDisposition(
  content: string,
  expectedFingerprint: string,
  findingIds: string[],
): string[] {
  const errors = [];
  if (extractReviewFingerprint(content) !== expectedFingerprint) {
    errors.push("the disposition is not bound to the current AI review snapshot");
  }
  for (const id of findingIds) {
    const escaped = id.replaceAll("-", "\\-");
    const section = content.match(
      new RegExp(`##\\s+${escaped}([\\s\\S]*?)(?=\\n##\\s+AR-|$)`, "i"),
    )?.[1];
    const disposition = section?.match(
      /Disposition:\s*(fixed|invalid|accepted-risk|follow-up)\s*$/im,
    );
    const rationale = section?.match(/Rationale and evidence:[ \t]*(.+)[ \t]*$/im)?.[1]?.trim();
    if (!disposition || !rationale) {
      errors.push(`${id} needs fixed, invalid, accepted-risk, or follow-up with rationale`);
    }
  }
  if (
    findingIds.length === 0 &&
    !content.match(/Confirmation:[ \t]*(?!<!--)(\S.*)[ \t]*$/im)?.[1]?.trim()
  ) {
    errors.push("confirm that the AI review reported no material findings");
  }
  return errors;
}

export function openInConfiguredEditor(root: string, location: SourceLocation): boolean {
  const command = configuredEditorCommand();
  if (!command) {
    return false;
  }
  const absolute = safeRepositoryPath(root, location.path);
  const line = location.line ?? 1;
  const column = location.column ?? 1;
  const target = `${absolute}:${line}:${column}`;
  const result = spawnSync(command, ["--goto", target], { stdio: "ignore" });
  return !result.error && result.status === 0;
}

function resolveReviewBase(root: string): string {
  const explicit = process.env.AHEAD_REVIEW_BASE?.trim();
  if (explicit) {
    if (gitOptional(root, ["rev-parse", "--verify", `${explicit}^{commit}`])) {
      return explicit;
    }
    throw new Error(`AHEAD_REVIEW_BASE does not identify a commit: ${explicit}`);
  }
  const candidates = [
    "origin/HEAD",
    "origin/main",
    "origin/master",
    "main",
    "master",
    "@{upstream}",
    "HEAD^",
  ];
  for (const candidate of candidates) {
    if (gitOptional(root, ["rev-parse", "--verify", `${candidate}^{commit}`])) {
      return candidate;
    }
  }
  return "HEAD";
}

function configuredEditorCommand(): string | undefined {
  const explicit = process.env.AHEAD_EDITOR?.trim();
  if (explicit === "vscode" || explicit === "code") {
    return "code";
  }
  if (explicit === "none") {
    return undefined;
  }
  return process.env.VSCODE_IPC_HOOK_CLI || process.env.TERM_PROGRAM === "vscode"
    ? "code"
    : undefined;
}

function safeRepositoryPath(root: string, path: string): string {
  if (isAbsolute(path)) {
    throw new Error("review path must be relative to the repository");
  }
  const repositoryRoot = resolve(root);
  const absolute = resolve(repositoryRoot, path);
  if (absolute !== repositoryRoot && !absolute.startsWith(`${repositoryRoot}/`)) {
    throw new Error("review path escaped the repository");
  }
  return absolute;
}

function git(root: string, args: string[], input?: string): string {
  return execFileSync("git", args, {
    cwd: root,
    encoding: "utf8",
    input,
    maxBuffer: 32 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  }).trimEnd();
}

function gitPaths(root: string, args: string[]): string[] {
  const output = execFileSync("git", args, {
    cwd: root,
    maxBuffer: 32 * 1024 * 1024,
    stdio: ["ignore", "pipe", "pipe"],
  });
  return output.toString("utf8").split("\0").filter(Boolean);
}

function gitValue(root: string, args: string[]): string | undefined {
  try {
    return git(root, args);
  } catch {
    return undefined;
  }
}

function gitOptional(root: string, args: string[]): boolean {
  try {
    git(root, args);
    return true;
  } catch {
    return false;
  }
}

function markdownSection(content: string, heading: string): string | undefined {
  const escaped = heading.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  const match = new RegExp(`^##\\s+${escaped}\\s*$`, "im").exec(content);
  if (!match) {
    return undefined;
  }
  const remaining = content.slice(match.index + match[0].length).replace(/^\r?\n/, "");
  const nextHeading = remaining.search(/^##\s+/m);
  return nextHeading < 0 ? remaining : remaining.slice(0, nextHeading);
}

function hasSubstantiveContent(content: string | undefined): boolean {
  return !!content
    ?.split("\n")
    .map((line) => line.trim())
    .find((line) => line && !line.startsWith("<!--"));
}
