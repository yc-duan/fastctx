// The model and reasoning effort a new conversation with an agent begins
// with: the last chosen on this device, on the new conversation page or in
// any conversation with that agent, as Codex and Claude Code each keep a
// choice made in their own pickers as the default for new sessions. The
// effort is kept for each model, as Claude Code keeps it, and the last one
// chosen besides, for a model that has none kept yet.

export type AgentKind = "codex" | "claude";

/** Which agent a program's name, as the pages write it, belongs to. */
export function agentKind(agent: string): AgentKind {
  return /claude/i.test(agent) ? "claude" : "codex";
}

function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    // Not kept: the next conversation begins with the program's default.
  }
}

/** The model last chosen for this agent. */
export function keptModel(agent: string): string | null {
  return read(`model:${agentKind(agent)}`);
}

/** The effort last chosen with this model, else with this agent. */
export function keptEffort(agent: string, model: string | null): string | null {
  const kind = agentKind(agent);
  return (model ? read(`effort:${kind}:${model}`) : null) ?? read(`effort:${kind}`);
}

/** Keeps a choice as what the next new conversation begins with. */
export function keepChoice(agent: string, model: string | null, effort: string | null): void {
  const kind = agentKind(agent);
  if (model) write(`model:${kind}`, model);
  if (!effort) return;
  write(`effort:${kind}`, effort);
  if (model) write(`effort:${kind}:${model}`, effort);
}
