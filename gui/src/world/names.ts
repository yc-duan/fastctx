// What the interface calls the agents, conversations and systems of the World.

import type { ResidentNow } from "./world";

/** A system's own name, as its makers write it. */
export const osName = (os: string): string => ({ windows: "Windows", linux: "Linux", macos: "macOS" } as Record<string, string>)[os] ?? os;

/** An agent's own name, as its makers write it. */
export const agentName = (agent: string): string => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

/** What a conversation is called, when it is called anything: a colleague
 *  by the task it was asked to do, any other by the name its host gave it,
 *  or else by the start of the user's first words (`world-gui.md`, 用词). */
export const titleOf = (r: ResidentNow): string | null =>
  (r.about.origin.kind === "colleague" ? r.about.origin.label : r.now?.title || r.now?.first_words) || null;
