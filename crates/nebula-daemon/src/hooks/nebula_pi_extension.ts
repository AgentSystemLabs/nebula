// nebula's managed pi extension — written by nebula before every pi session
// it starts; edits here are overwritten. Inert outside nebula: without the
// NEBULA_* session environment it registers nothing, so a bare `pi` in the
// same checkout never phones home.
//
// It mirrors the hook set nebula installs for Claude Code onto pi's extension
// events and POSTs each one to the daemon's loopback hook receiver
// (`/api/hooks/pi`), fail-soft: an unreachable daemon costs a short timeout,
// never the turn. The daemon answers `UserPromptSubmit` with an empty body or
// the session auto-title instruction, in the same JSON envelope Claude Code
// and Codex read; here it is appended to the system prompt for that run.

type SessionContext = {
  cwd: string;
  sessionManager: { getSessionId(): string };
};

const AGENT_ID = process.env.NEBULA_AGENT_ID;
const API_URL = process.env.NEBULA_API_URL;
const API_TOKEN = process.env.NEBULA_API_TOKEN ?? "";
const TIMEOUT_MS = 3000;
// pi's AskUserQuestion: the tool that stops the turn to ask you something.
const ASK_TOOL = "ask_question";

export default function (pi: any) {
  if (!AGENT_ID || !API_URL) return;

  // Whether an agent run is in flight: UI prompts raised between runs (an
  // extension asking something at startup) are not the turn waiting on you.
  let running = false;

  const post = async (
    event: string,
    ctx: SessionContext,
    extra: Record<string, unknown> = {},
  ): Promise<string> => {
    const url =
      `${API_URL}/api/hooks/pi?agentId=${encodeURIComponent(AGENT_ID)}` +
      `&hookEvent=${encodeURIComponent(event)}`;
    const body = JSON.stringify({
      session_id: ctx.sessionManager.getSessionId(),
      cwd: ctx.cwd,
      ...extra,
    });
    const controller = new AbortController();
    const timer = setTimeout(() => controller.abort(), TIMEOUT_MS);
    try {
      const res = await fetch(url, {
        method: "POST",
        headers: {
          Authorization: `Bearer ${API_TOKEN}`,
          "Content-Type": "application/json",
        },
        body,
        signal: controller.signal,
      });
      return res.ok ? await res.text() : "";
    } catch {
      return "";
    } finally {
      clearTimeout(timer);
    }
  };

  pi.on("session_start", async (event: { reason?: string }, ctx: SessionContext) => {
    await post("SessionStart", ctx, { source: event.reason });
  });

  // One user prompt starts one agent run. The response body is empty or the
  // auto-title instruction; a non-empty one rides this run's system prompt.
  pi.on(
    "before_agent_start",
    async (event: { systemPrompt: string }, ctx: SessionContext) => {
      running = true;
      const context = injectedContext(await post("UserPromptSubmit", ctx));
      if (context) {
        return { systemPrompt: `${event.systemPrompt}\n\n${context}` };
      }
    },
  );

  // The run is over however it ended — a finished turn, an error, or an
  // abort — which is the one end-of-turn signal Claude's hooks never send.
  pi.on("agent_end", async (_event: unknown, ctx: SessionContext) => {
    running = false;
    await post("Stop", ctx);
  });

  pi.on(
    "tool_execution_start",
    async (event: { toolName: string }, ctx: SessionContext) => {
      if (event.toolName === ASK_TOOL) {
        await post("PreToolUse", ctx, { tool_name: event.toolName });
      }
    },
  );
  pi.on(
    "tool_execution_end",
    async (event: { toolName: string }, ctx: SessionContext) => {
      if (event.toolName === ASK_TOOL) {
        await post("PostToolUse", ctx, { tool_name: event.toolName });
      }
    },
  );

  // A blocking prompt an extension raises mid-run (confirm / select /
  // input) is the turn waiting on you, like a permission prompt; its end
  // reads as the question tool finishing so the row goes back to running.
  pi.on("ui_prompt_start", async (_event: unknown, ctx: SessionContext) => {
    if (running) await post("PermissionRequest", ctx);
  });
  pi.on("ui_prompt_end", async (_event: unknown, ctx: SessionContext) => {
    if (running) await post("PostToolUse", ctx, { tool_name: ASK_TOOL });
  });
}

// The daemon's UserPromptSubmit reply: `hookSpecificOutput.additionalContext`
// out of the envelope, bare text as-is, nothing for an empty body.
function injectedContext(body: string): string | undefined {
  const text = body.trim();
  if (!text) return undefined;
  try {
    const context = JSON.parse(text)?.hookSpecificOutput?.additionalContext;
    return typeof context === "string" && context.trim() ? context : undefined;
  } catch {
    return text;
  }
}
