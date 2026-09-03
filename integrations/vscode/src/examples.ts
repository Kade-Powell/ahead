import type * as vscode from "vscode";

export interface FieldExamplesInput {
  workflowTitle: string;
  phaseTitle: string;
  runTitle: string;
  fields: string[];
}

export type FieldExamplesSkipped = "no-model" | "failed";

export interface FieldExamplesResult {
  examples?: string[][];
  skipped?: FieldExamplesSkipped;
}

/**
 * Draft two inspiration-only example lines per artifact field using the first
 * available VS Code chat model.
 *
 * The model is called without repository context on purpose: the examples are
 * creative sparks to react to, not guidance the human should trust. They are
 * rendered as HTML comments inside the field so form validation still treats
 * an untouched field as empty. Any failure falls back to the plain template.
 *
 * The `vscode` module is imported dynamically so this module stays importable
 * under plain node for unit tests of the pure parser.
 */
export async function draftFieldExamples(
  input: FieldExamplesInput,
  token?: vscode.CancellationToken,
): Promise<FieldExamplesResult> {
  const vscode = await import("vscode");
  let models: vscode.LanguageModelChat[];
  try {
    models = await vscode.lm.selectChatModels();
  } catch {
    return { skipped: "failed" };
  }
  const model = models[0];
  if (!model) {
    return { skipped: "no-model" };
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
    const response = await model.sendRequest(
      [vscode.LanguageModelChatMessage.User(prompt)],
      {},
      token,
    );
    let text = "";
    for await (const part of response.text) {
      if (token?.isCancellationRequested) {
        return { skipped: "failed" };
      }
      text += part;
    }
    const examples = parseExampleLines(text, input.fields.length);
    if (!examples) {
      return { skipped: "failed" };
    }
    return { examples };
  } catch {
    return { skipped: "failed" };
  }
}

/** Exported for unit testing. Mirrors the parser in the Pi integration. */
export function parseExampleLines(text: string, fieldCount: number): string[][] | undefined {
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
