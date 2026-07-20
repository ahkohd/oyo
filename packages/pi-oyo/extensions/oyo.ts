import type { ExtensionAPI } from "@mariozechner/pi-coding-agent";

type OyoMode = "on" | "off";

const ENTRY_TYPE = "oyo-mode";

const OYO_PROMPT = [
  "For code reviews or comments in Git or jj, use oyo-code-review: run `oy skill path` and read the file it prints.",
  "For a guided walkthrough of a live diff, use oyo-code-walkthrough: run `oy skill path walkthrough` and read the file it prints.",
  "To steer a running Oyo TUI, use oyo-tui-control: run `oy skill path control` and read the file it prints.",
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
  ctx.ui.notify(`Oyo: ${next}`, "info");
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
    description: "Set Oyo skill instructions: on|off|status",
    handler: async (args, ctx) => {
      const next = parseMode(args);

      if (next === "status") {
        ctx.ui.notify(`Oyo: ${mode}`, "info");
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
