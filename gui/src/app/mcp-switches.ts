// The account's MCP servers as the switches in 「机器与文件夹」 list them
// (`world-user-config.md`): read from its node when first asked, kept a
// minute, and read again in the background once older, the panel drawn
// again as each list comes.

import type { Login } from "../world/accounts";
import type { Listed } from "../world/profile";
import { pickables, type Pickable } from "./agent-machines";
import type { FocusServer, FocusServers } from "./focus-panel";
import { loginKey, type ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import type { Store } from "./store";

/** How long a list read stands before it is read again. */
const FRESH_MS = 60_000;

export class ServerLists {
  private readonly kept = new Map<string, { at: number; list: FocusServer[] | null }>();
  private readonly redraws = new Set<() => void>();

  constructor(
    private readonly calls: ProfileCalls,
    private readonly store: Store,
    private readonly words: ProfileWords,
  ) {}

  /** The switches' list and its words for the account `login` names. */
  switches(login: () => Login | null): Omit<FocusServers, "off" | "changed"> {
    return {
      title: this.words.focus.title,
      when: this.words.focus.when,
      list: () => this.list(login()),
      watch: (redraw) => {
        this.redraws.add(redraw);
      },
    };
  }

  private list(login: Login | null): FocusServer[] | null {
    if (!login) return null;
    const key = loginKey(login);
    const held = this.kept.get(key);
    if (!held || Date.now() - held.at > FRESH_MS) {
      this.kept.set(key, { at: Date.now(), list: held?.list ?? null });
      void this.read(login, key);
    }
    return held?.list ?? null;
  }

  private async read(login: Login, key: string): Promise<void> {
    try {
      const reply = await this.calls.mcp(login.hubName, login.account, { do: "list" });
      const machines = pickables(this.store.world.peek());
      const list = reply.reply === "listed" ? reply.servers.map((one) => switchOf(one, machines)) : [];
      this.kept.set(key, { at: Date.now(), list });
    } catch {
      // An older node or hub keeps none: the switches are not shown.
      this.kept.set(key, { at: Date.now(), list: [] });
    }
    for (const redraw of this.redraws) redraw();
  }
}

function switchOf(listed: Listed, machines: Pickable[]): FocusServer {
  const server = listed.server;
  if (server.kind !== "command") return { name: listed.name, kind: server.kind, machine: null, away: false };
  const machine = machines.find((one) => one.name === server.machine || one.written === server.machine);
  return { name: listed.name, kind: "command", machine: machine?.written ?? server.machine, away: !machine?.here && machine?.online !== true };
}
