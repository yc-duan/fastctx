// Lookups into one World, made once for each World the store holds. The
// pages ask the same few questions of every machine, member and person
// (who is this machine on that hub, who holds it, what groups does its
// owner keep), and asking each of them of every member in turn is where a
// network of thousands spends its time. A new World is a new object, and
// nothing here changes what it was made from.

import type { AccountName, AccountSeen, HubAccounts, MemberSeen } from "../world/accounts";
import type { WorldNow } from "../world/bridge";

/** One hub as the World has it, looked up by name. */
export interface HubIndex {
  hub: HubAccounts;
  /** Its members by the name each goes by now, every kind, in the hub's
   *  order. */
  named: Map<string, MemberSeen[]>;
  /** Its first computer by each name it goes by or had, in the hub's
   *  order (`names`). */
  computer: Map<string, MemberSeen>;
  accounts: Map<AccountName, AccountSeen>;
  /** `machine\naccount` for each machine shared with an account. */
  shares: Set<string>;
  /** `machine\naccount` for each machine that acted for an account. */
  acting: Set<string>;
  /** Each owner's groups, by name, in the order `groupsOf` gives. */
  groups: Map<AccountName, string[]>;
}

export interface WorldIndex {
  hubs: Map<string, HubIndex>;
  /** The first computer by the name it goes by now, on any hub, in the
   *  hubs' order. */
  computer: Map<string, MemberSeen>;
  /** The names of the managed machines, on any hub. */
  managed: Set<string>;
  /** The group of the first computer in a group by each name it goes by
   *  or had, on any hub, in the hubs' order. */
  grouped: Map<string, string>;
}

const made = new WeakMap<WorldNow, WorldIndex>();

export function indexOf(world: WorldNow): WorldIndex {
  const kept = made.get(world);
  if (kept) return kept;
  const index = indexMade(world);
  made.set(world, index);
  return index;
}

/** `machine` and `account` as one key of `HubIndex.shares` and `acting`. */
export const pair = (machine: string, account: string): string => `${machine}\n${account}`;

function indexMade(world: WorldNow): WorldIndex {
  const hubs = new Map<string, HubIndex>();
  const computer = new Map<string, MemberSeen>();
  const managed = new Set<string>();
  const grouped = new Map<string, string>();
  for (const hub of world.accounts.hubs) {
    const named = new Map<string, MemberSeen[]>();
    const computers = new Map<string, MemberSeen>();
    const groups = new Map<AccountName, Set<string>>();
    for (const member of hub.members) {
      const same = named.get(member.name);
      if (same) same.push(member);
      else named.set(member.name, [member]);
      if (member.managed) managed.add(member.name);
      if (member.home && member.group) {
        const owned = groups.get(member.home);
        if (owned) owned.add(member.group);
        else groups.set(member.home, new Set([member.group]));
      }
      if (member.kind !== "computer") continue;
      if (!computer.has(member.name)) computer.set(member.name, member);
      for (const name of [member.name, ...(member.was ?? [])]) {
        if (!computers.has(name)) computers.set(name, member);
        if (member.group && !grouped.has(name)) grouped.set(name, member.group);
      }
    }
    hubs.set(hub.hub, {
      hub,
      named,
      computer: computers,
      accounts: new Map(hub.accounts.map((account) => [account.name, account])),
      shares: new Set(hub.shares.map((share) => pair(share.machine, share.account))),
      acting: new Set(hub.acting.map((acting) => pair(acting.machine, acting.account))),
      groups: new Map([...groups].map(([owner, names]) => [owner, [...names].sort((a, b) => a.localeCompare(b))])),
    });
  }
  return { hubs, computer, managed, grouped };
}
