import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

type OyoMode = "on" | "off";

const ENTRY_TYPE = "oyo-mode";

const OYO_PROMPT = [
  "OYO CODE REVIEW.",
  "To review code changes, read Oyo comments, or work on comments in Git or jj, use the oyo-code-review skill.",
  "Run `oy skill path`, read the file it prints, then use `oy` and `oy review` for the task.",
].join("\n");

let mode: OyoMode = "on";

type UiContext = {
  ui: {
    notify: (message: string, type?: "info" | "warning" | "error") => void;
  };
};

function parseMode(input: string): OyoMode | "status" | undefined {
  const token = input.trim().toLowerCase();
  if (!token || token === "on") return "on";
  if (token === "off" || token === "stop" || token === "disable") return "off";
  if (token === "status") return "status";
  return undefined;
}

function setMode(next: OyoMode, ctx: UiContext, pi: ExtensionAPI): void {
  mode = next;
  pi.appendEntry(ENTRY_TYPE, { mode: next });
  ctx.ui.notify(`Oyo review: ${next}`, "info");
}

export default function oyoExtension(pi: ExtensionAPI): void {
  pi.on("session_start", async (_event, ctx) => {
    mode = "on";

    for (const entry of ctx.sessionManager.getBranch()) {
      if (entry.type === "custom" && entry.customType === ENTRY_TYPE) {
        const candidate = (entry as { data?: { mode?: OyoMode } }).data?.mode;
        if (candidate === "on" || candidate === "off") mode = candidate;
      }
    }
  });

  pi.on("before_agent_start", async (event) => {
    if (mode === "off") return;
    return { systemPrompt: `${event.systemPrompt}\n\n${OYO_PROMPT}` };
  });

  pi.registerCommand("oyo", {
    description: "Set Oyo review instruction: on|off|status",
    handler: async (args, ctx) => {
      const next = parseMode(args);

      if (next === "status") {
        ctx.ui.notify(`Oyo review: ${mode}`, "info");
        return;
      }

      if (!next) {
        ctx.ui.notify("Usage: /oyo [on|off|status]", "warning");
        return;
      }

      setMode(next, ctx, pi);
    },
  });
}
