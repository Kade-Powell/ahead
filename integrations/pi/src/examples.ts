import { completeSimple } from "@earendil-works/pi-ai/compat";
import type { ExtensionCommandContext } from "@earendil-works/pi-coding-agent";

export interface FieldExamplesInput {
  workflowTitle: string;
  phaseTitle: string;
  runTitle: string;
  fields: string[];
}

const EXAMPLES_TIMEOUT_MS = 25_000;

/**
 * Draft two inspiration-only example lines per artifact field.
 *
 * The model is called without repository context on purpose: the examples are
 * creative sparks to react to, not guidance the human should trust. They are
 * rendered as HTML comments inside the field so form validation still treats
 * an untouched field as empty.
 */
export async function draftFieldExamples(
  ctx: ExtensionCommandContext,
  input: FieldExamplesInput,
): Promise<string[][] | undefined> {
  const model = ctx.model;
  if (!model) {
    return undefined;
  }

  const prompt = [
    "You are helping a human begin a short written artifact in a software workflow.",
    `Run title: ${input.runTitle}`,
    `Workflow: ${input.workflowTitle} · Phase: ${input.phaseTitle}`,
    "",
    "You have NO access to the codebase or real project details. For each numbered field, write exactly 2 short example answers that are creative sparks only — plausibly generic and concrete, never claimed as fact about this project. Keep each line under 15 words.",
    "",
    "Output format, nothing else:",
    "1: <example line>",
    "1: <example line>",
    "2: <example line>",
    "2: <example line>",
    "...one pair per field, in order.",
    "",
    "Fields:",
    ...input.fields.map((field, index) => `${index + 1}. ${field}`),
  ].join("\n");

  try {
    const auth = await ctx.modelRegistry.getProviderAuth(model.provider);
    if (!auth) {
      return undefined;
    }
    const message = await Promise.race([
      completeSimple(
        model,
        {
          messages: [
            {
              role: "user",
              content: [{ type: "text", text: prompt }],
              timestamp: Date.now(),
            },
          ],
        },
        {
          apiKey: auth.auth.apiKey,
          headers: auth.auth.headers,
          env: auth.env,
          reasoning: "minimal",
        },
      ),
      new Promise<undefined>((resolve) => {
        setTimeout(() => resolve(undefined), EXAMPLES_TIMEOUT_MS);
      }),
    ]);
    if (!message) {
      return undefined;
    }
    return parseExampleLines(message, input.fields.length);
  } catch {
    return undefined;
  }
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null;
}

function textBlocks(content: unknown): string[] {
  if (!Array.isArray(content)) {
    return [];
  }
  const blocks: string[] = [];
  for (const block of content) {
    if (isRecord(block) && block.type === "text" && typeof block.text === "string") {
      blocks.push(block.text);
    }
  }
  return blocks;
}

function parseExampleLines(message: unknown, fieldCount: number): string[][] | undefined {
  const text = textBlocks(isRecord(message) ? message.content : undefined).join("\n");

  const perField: string[][] = Array.from({ length: fieldCount }, () => []);
  for (const line of text.split("\n")) {
    const match = /^\s*(\d+)\s*:\s*(.+)$/.exec(line);
    if (!match) {
      continue;
    }
    const fieldIndex = Number.parseInt(match[1] ?? "", 10) - 1;
    const example = match[2]?.trim();
    if (
      Number.isInteger(fieldIndex) &&
      fieldIndex >= 0 &&
      fieldIndex < fieldCount &&
      example &&
      perField[fieldIndex].length < 2
    ) {
      perField[fieldIndex].push(example);
    }
  }
  if (perField.every((examples) => examples.length === 0)) {
    return undefined;
  }
  return perField;
}
