// nebula's managed OpenCode plugin — written by nebula before every OpenCode
// session it starts; edits here are overwritten. Inert outside nebula: without
// the NEBULA_* session environment it registers no hooks, so an `opencode` you
// run yourself in the same checkout never phones home.
//
// It maps OpenCode's server events onto the hook events nebula installs for
// Claude Code and POSTs each one to the daemon's loopback hook receiver
// (`/api/hooks/opencode`), fail-soft: an unreachable daemon costs a short
// timeout, never the turn. The daemon answers `UserPromptSubmit` with an empty
// body or the session auto-title instruction, in the same JSON envelope Claude
// Code and Codex read; here it rides that turn's system prompt through
// OpenCode's `experimental.chat.system.transform` hook.

const AGENT_ID = process.env.NEBULA_AGENT_ID;
const API_URL = process.env.NEBULA_API_URL;
const API_TOKEN = process.env.NEBULA_API_TOKEN ?? "";
const TIMEOUT_MS = 3000;
// OpenCode's question tool: the one that stops the turn to ask you something.
const ASK_TOOL = "question";

type PluginInput = { directory: string };
type ServerEvent = { type: string; properties?: Record<string, any> };
type MessagePart = { type: string; text?: string; synthetic?: boolean };
type Hooks = Record<string, unknown>;

export const NebulaPlugin = async ({ directory }: PluginInput): Promise<Hooks> => {
  if (!AGENT_ID || !API_URL) return {};

  // Subagent sessions (a task tool's child, created with a parent) post
  // under the root session's id — the one the daemon's status machine
  // adopted; any other id reads as a foreign session and is dropped —
  // with their own id as the origin, so a child's permission prompt is
  // answered by its own next tool event. Only a root session's turns drive
  // the row's busy / idle.
  const parents = new Map<string, string>();
  const rootOf = (sessionID: string): string => {
    let id = sessionID;
    for (let hops = 0; hops < 16; hops++) {
      const parent = parents.get(id);
      if (!parent) return id;
      id = parent;
    }
    return id;
  };
  const isChild = (sessionID: string): boolean => parents.has(sessionID);
  // Sessions with a turn in flight: one UserPromptSubmit when it starts and
  // one Stop when it ends, whichever of `session.status` (idle) and
  // `session.idle` lands first.
  const busy = new Set<string>();
  // The daemon's UserPromptSubmit reply per session, appended to that
  // turn's system prompt until the turn ends.
  const injected = new Map<string, string>();
  // Open permission requests: request id → the gated tool, so the reply
  // reads as that tool's PostToolUse (the one hook an approval fires).
  const permissions = new Map<string, string>();

  const post = async (
    event: string,
    sessionID: string | undefined,
    extra: Record<string, unknown> = {},
  ): Promise<string> => {
    const url =
      `${API_URL}/api/hooks/opencode?agentId=${encodeURIComponent(AGENT_ID)}` +
      `&hookEvent=${encodeURIComponent(event)}`;
    const body = JSON.stringify({ session_id: sessionID, cwd: directory, ...extra });
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

  // A dialog's events: posted under the root session, with a child's own
  // id as the subagent origin.
  const dialog = async (event: string, sessionID: string, extra: Record<string, unknown> = {}) => {
    const origin = isChild(sessionID) ? { agent_id: sessionID } : {};
    await post(event, rootOf(sessionID), { ...extra, ...origin });
  };

  const startTurn = async (sessionID: string, prompt?: string) => {
    if (isChild(sessionID)) return;
    const fresh = !busy.has(sessionID);
    busy.add(sessionID);
    // A prompt typed while a turn runs is queued: it is reported (for the
    // row's RECENT PROMPTS) when typed, and its own turn starts when the
    // running one ends — `session.status` says so, and lands here again.
    if (!fresh && prompt === undefined) return;
    const context = injectedContext(
      await post("UserPromptSubmit", sessionID, prompt === undefined ? {} : { prompt }),
    );
    if (context) injected.set(sessionID, context);
    else injected.delete(sessionID);
  };

  const endTurn = async (sessionID: string) => {
    if (!busy.delete(sessionID)) return;
    injected.delete(sessionID);
    if (!isChild(sessionID)) await post("Stop", sessionID);
  };

  return {
    event: async ({ event }: { event: ServerEvent }) => {
      const p = event.properties ?? {};
      switch (event.type) {
        case "session.created":
          if (p.info?.parentID && p.info?.id) parents.set(p.info.id, p.info.parentID);
          break;
        // `session.status` is the run state itself (busy while a turn's
        // loop runs, idle after, an abort included); `session.idle` follows
        // every idle. Either ends the turn once.
        case "session.status":
          if (p.status?.type === "busy") await startTurn(p.sessionID);
          else if (p.status?.type === "idle") await endTurn(p.sessionID);
          break;
        case "session.idle":
          await endTurn(p.sessionID);
          break;
        // The gated tool: `permission` on current OpenCode (1.18), `type`
        // on the older SDK shape.
        case "permission.asked":
          permissions.set(p.id, p.permission ?? p.type ?? "permission");
          await dialog("PermissionRequest", p.sessionID);
          break;
        case "permission.replied": {
          const tool = permissions.get(p.requestID) ?? "permission";
          permissions.delete(p.requestID);
          await dialog("PostToolUse", p.sessionID, { tool_name: tool });
          break;
        }
        case "question.asked":
          await dialog("PreToolUse", p.sessionID, { tool_name: ASK_TOOL });
          break;
        case "question.replied":
        case "question.rejected":
          await dialog("PostToolUse", p.sessionID, { tool_name: ASK_TOOL });
          break;
      }
    },

    // One user message is one turn (or one queued prompt). The typed text
    // goes along for the row's RECENT PROMPTS; synthetic parts (a
    // compaction's "continue") are not something you typed.
    "chat.message": async (
      input: { sessionID: string },
      output: { parts: MessagePart[] },
    ) => {
      const prompt = output.parts
        .filter((part) => part.type === "text" && !part.synthetic && typeof part.text === "string")
        .map((part) => part.text)
        .join("\n");
      await startTurn(input.sessionID, prompt);
    },

    // Every LLM call of the turn builds its system prompt through here; the
    // daemon's reply (the auto-title instruction) rides it until the turn
    // ends. Calls with no session (a title generation) get nothing.
    "experimental.chat.system.transform": async (
      input: { sessionID?: string },
      output: { system: string[] },
    ) => {
      const context = input.sessionID ? injected.get(input.sessionID) : undefined;
      if (context) output.system.push(context);
    },
  };
};

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
