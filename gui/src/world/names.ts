// What the interface calls the agents, conversations and systems of the World.

import type { ResidentNow } from "./world";

/** A system's own name, as its makers write it. */
export const osName = (os: string): string => ({ windows: "Windows", linux: "Linux", macos: "macOS" } as Record<string, string>)[os] ?? os;

/** An agent's own name, as its makers write it. */
export const agentName = (agent: string): string => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

/** What a conversation calls itself, when anything: a colleague by the
 *  task it was asked to do, any other by the name its host gave it, or else
 *  by the start of the user's first words. */
const ownTitle = (r: ResidentNow): string | null =>
  (r.about.origin.kind === "colleague" ? r.about.origin.label : r.now?.title || r.now?.first_words) || null;

/** The titles forks with none of their own go by, by `id@machine`: the
 *  conversation each was forked from's (`lendTitles`). */
const borrowed = new Map<string, string>();
const keyOf = (id: string, machine: string) => `${id}@${machine}`;

/** Lends each fork with no title of its own the title of the conversation
 *  it was forked from, among `residents`, as the World was read last. */
export function lendTitles(residents: readonly ResidentNow[]): void {
  borrowed.clear();
  const byKey = new Map(residents.map((r) => [keyOf(r.about.id, r.about.machine), r]));
  for (const r of residents) {
    const from = r.about.forked_from;
    if (!from || ownTitle(r)) continue;
    const source = byKey.get(keyOf(from.resident, from.machine));
    const title = source ? ownTitle(source) : null;
    if (title) borrowed.set(keyOf(r.about.id, r.about.machine), title);
  }
}

/** What a conversation is called, when it is called anything: its own
 *  title, or a fork's, until it has one, that of the conversation it was
 *  forked from (`world-gui.md`, 用词). */
export const titleOf = (r: ResidentNow): string | null => ownTitle(r) ?? borrowed.get(keyOf(r.about.id, r.about.machine)) ?? null;
