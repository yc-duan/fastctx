// The model and effort an account's preferences name for each agent, which a
// new conversation working for that account begins with (`world-user-
// config.md`, 偏好): read from the account's node the first time a page asks,
// kept a minute, and taken at once from whatever this device writes.

import { agentKind } from "./model-memory";
import type { ProfileCalls } from "./profile-account";
import type { Login } from "../world/accounts";
import type { Profile } from "../world/profile";

export interface AccountDefaults {
  model: string | null;
  effort: string | null;
}

const FRESH_MS = 60_000;
const MODEL = { claude: "model", codex: "model" } as const;
const EFFORT = { claude: "effortLevel", codex: "model_reasoning_effort" } as const;

export class ProfileDefaults {
  private readonly kept = new Map<string, { at: number; profile: Profile | null }>();
  private readonly redraws = new Set<() => void>();

  constructor(private readonly calls: ProfileCalls) {}

  /** What `login`'s preferences name for `agent`, or null while they are
   *  read or where its node or hub keeps none. */
  of(login: Login | null, agent: string): AccountDefaults | null {
    if (!login) return null;
    const key = keyOf(login.hubName, login.account);
    const held = this.kept.get(key);
    if (!held || Date.now() - held.at > FRESH_MS) {
      this.kept.set(key, { at: Date.now(), profile: held?.profile ?? null });
      void this.read(login, key);
    }
    const table = held?.profile?.[agentKind(agent)];
    if (!table) return null;
    const kind = agentKind(agent);
    const model = table[MODEL[kind]];
    const effort = table[EFFORT[kind]];
    return { model: typeof model === "string" ? model : null, effort: typeof effort === "string" ? effort : null };
  }

  /** What this device just wrote for an account. */
  saved(hub: string, account: string, profile: Profile): void {
    this.kept.set(keyOf(hub, account), { at: Date.now(), profile });
    for (const redraw of this.redraws) redraw();
  }

  /** Draws `redraw` again once what was asked for arrives. */
  watch(redraw: () => void): () => void {
    this.redraws.add(redraw);
    return () => this.redraws.delete(redraw);
  }

  private async read(login: Login, key: string): Promise<void> {
    try {
      const view = await this.calls.read(login.hubName, login.account);
      this.kept.set(key, { at: Date.now(), profile: view.kept ? view.profile : null });
    } catch {
      // An older node or hub keeps none: the device's own memory stands.
      return;
    }
    for (const redraw of this.redraws) redraw();
  }
}

/** Calls name an account by its hub's name. */
function keyOf(hub: string, account: string): string {
  return `${hub}/${account}`;
}
