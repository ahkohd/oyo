import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";
import { spawnSync } from "node:child_process";
import { randomBytes } from "node:crypto";
import { readFileSync, unlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

function parseArgs(args?: string): string[] {
  if (!args) return [];
  return args.split(/\s+/).filter(Boolean);
}

function stripManagedReviewFlags(args: string[]): string[] {
  const out: string[] = [];

  for (let i = 0; i < args.length; i++) {
    const token = args[i]!;

    if (token === "--no-print-review") continue;

    if (token === "--review-output-file") {
      i += 1;
      continue;
    }

    if (token.startsWith("--review-output-file=")) continue;

    out.push(token);
  }

  return out;
}

function runOyAndReadReview(cwd: string, args: string[] = []): { comments: string; error?: Error } {
  const outputFile = join(
    tmpdir(),
    `oy-review-${Date.now()}-${randomBytes(6).toString("hex")}.txt`,
  );

  const userArgs = stripManagedReviewFlags(args);
  const oyArgs = [
    ...userArgs,
    "--review-output-file",
    outputFile,
    "--no-print-review",
  ];

  const result = spawnSync("oy", oyArgs, { cwd, stdio: "inherit" });
  if (result.error) {
    return { comments: "", error: result.error };
  }

  let comments = "";
  try {
    comments = readFileSync(outputFile, "utf8");
  } catch {
    comments = "";
  }

  try {
    unlinkSync(outputFile);
  } catch {
    // ignore
  }

  return { comments: comments.trimEnd() };
}

function delay(ms: number): Promise<void> {
  return new Promise((resolve) => {
    setTimeout(resolve, ms);
  });
}

function buildReviewOyArgs(args?: string): string[] {
  const inputArgs = parseArgs(args);

  let tempSession = false;
  let clearSession = false;
  let i = 0;
  while (i < inputArgs.length) {
    const token = inputArgs[i]!;
    if (token === "temp") {
      tempSession = true;
      i += 1;
      continue;
    }
    if (token === "new") {
      clearSession = true;
      i += 1;
      continue;
    }
    break;
  }

  const oyArgs = inputArgs.slice(i);
  if (tempSession && !oyArgs.includes("--no-review-persist")) {
    oyArgs.push("--no-review-persist");
  }
  if (clearSession && !oyArgs.includes("--clear-review-session")) {
    oyArgs.push("--clear-review-session");
  }

  return oyArgs;
}

export default function oyoExtension(pi: ExtensionAPI): void {
  const cwd = process.cwd();

  pi.registerCommand("diff", {
    description: "Diff changes",
    handler: async (args, ctx) => {
      const { error } = runOyAndReadReview(cwd, parseArgs(args));

      if (error) {
        ctx.ui.notify(`Failed to run oy: ${error.message}`, "error");
      }
    },
  });

  pi.registerCommand("review", {
    description: "Review changes",
    handler: async (args, ctx) => {
      const oyArgs = buildReviewOyArgs(args);
      const { comments, error } = runOyAndReadReview(cwd, oyArgs);

      if (error) {
        ctx.ui.notify(`Failed to run oy: ${error.message}`, "error");
        return;
      }

      if (!comments.trim()) {
        ctx.ui.notify("No review comments", "info");
        return;
      }

      await delay(50);
      ctx.ui.pasteToEditor(`${comments}\n`);
    },
  });
}
