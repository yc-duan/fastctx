// Account opt-ins shared by settings, conversations and their plans.
import { loginsOf, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { sentinels, type SentinelSeam, type SentinelSettings } from "../world/sentinels";
import type { Address } from "../world/wire";
import { signal } from "./signals";
import { residentAt } from "./store";

const OFF: SentinelSettings = { home: null, model: null, search: null };

/** The single account a conversation works for; ambiguous ownership stays off. */
export function featureAccount(world: WorldNow | null, address: Address): Login | null {
  const acting = residentAt(world, address)?.about.acting;
  if (!acting) return null;
  const accounts = acting.kind === "accounts" ? acting.accounts : loginsOf(world?.accounts);
  if (accounts.length !== 1) return null;
  const one = accounts[0]!;
  const hub = world?.accounts.hubs.find((hub) => hub.hub === one.hub);
  return hub?.logins.includes(one.account) ? { ...one, hubName: hub.name } : null;
}

class AccountFeatures {
  private seam: SentinelSeam = sentinels;
  private readonly revision = signal(0);
  private readonly entries = new Map<string, { at: number; settings: SentinelSettings }>();
  private readonly pending = new Set<string>();
  private readonly generations = new Map<string, number>();

  connect(seam: SentinelSeam): void { this.seam = seam; }

  get(login: Login | null): SentinelSettings {
    this.revision.get();
    if (!login) return OFF;
    const key = JSON.stringify([login.hub, login.account]);
    const held = this.entries.get(key);
    if ((!held || Date.now() - held.at > 30_000) && !this.pending.has(key)) {
      this.pending.add(key);
      const generation = this.generations.get(key) ?? 0;
      void this.seam.read(login.hubName, login.account).then(
        (view) => {
          if ((this.generations.get(key) ?? 0) === generation) this.saved(login, view.settings);
        },
        () => {
          if ((this.generations.get(key) ?? 0) === generation) this.entries.set(key, { at: Date.now(), settings: held?.settings ?? OFF });
        },
      ).finally(() => this.pending.delete(key));
    }
    return held?.settings ?? OFF;
  }

  talk(world: WorldNow | null, address: Address): SentinelSettings {
    return this.get(featureAccount(world, address));
  }

  saved(login: Login, settings: SentinelSettings): void {
    const key = JSON.stringify([login.hub, login.account]);
    this.generations.set(key, (this.generations.get(key) ?? 0) + 1);
    const changed = JSON.stringify(this.entries.get(key)?.settings) !== JSON.stringify(settings);
    this.entries.set(key, { at: Date.now(), settings });
    if (changed) this.revision.update((value) => value + 1);
  }
}

export const accountFeatures = new AccountFeatures();
