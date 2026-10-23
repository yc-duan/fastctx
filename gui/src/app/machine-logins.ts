// Who may do what to a machine from this device: whose it is, who updates
// it (`world-upgrade.md`, 谁来更新), who takes it off the network and
// whether Fastctx on it can be uninstalled as it goes (`world-network.md`,
// 移出网络与卸载). One machine's page, the drawing and the list ask the same.

import { loginsOf, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Running } from "../world/updates";
import { isManaged } from "./machine-list";
import type { ManageWords } from "./machine-manage-words";
import { holds, isSuper, isVisitor, ownerOf, owns, type Machine } from "./reach";

/** The login a machine is the account's own under: this computer's on a
 *  hub it is registered on, or the one that registered another. */
export function ownerLoginOf(machine: Machine, world: WorldNow): Login | undefined {
  const logins = loginsOf(world.accounts);
  if (machine.here) return logins.find((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account));
  return logins.find((login) => owns(machine, login));
}

/** The login `machine` is in the hands of: its owner's first, or one it is
 *  shared with whole, who manages it as its owner does. */
export function holderLoginOf(machine: Machine, world: WorldNow): Login | undefined {
  const owner = ownerLoginOf(machine, world);
  if (owner || machine.here) return owner;
  return loginsOf(world.accounts).find((login) => holds(machine, login, world));
}

/** The login that may update `machine` from here and set its choices: one
 *  that holds it, or the super administrator's but for a visitor's machine;
 *  none for this device, which updates itself. */
export function updaterLoginOf(machine: Machine, world: WorldNow): Login | null {
  if (machine.here) return null;
  const holder = holderLoginOf(machine, world);
  if (holder) return holder;
  for (const login of loginsOf(world.accounts)) {
    if (!isSuper(world, login) || !machine.on.some((on) => on.hub === login.hub)) continue;
    const of = ownerOf(machine, login);
    if (of && !isVisitor(world, { ...login, account: of })) return login;
  }
  return null;
}

/** The login that may take `machine` off its network: one that holds it,
 *  or the super administrator's there; none for this device, which leaves
 *  it from its own settings. */
export function removerLoginOf(machine: Machine, world: WorldNow): Login | null {
  if (machine.here) return null;
  const holder = holderLoginOf(machine, world);
  if (holder) return holder;
  return superLoginOver(machine, world) ?? null;
}

/** The login here that is the super administrator of a hub `machine` is
 *  on, if any. */
export function superLoginOver(machine: Machine, world: WorldNow): Login | undefined {
  return loginsOf(world.accounts).find((login) => isSuper(world, login) && machine.on.some((on) => on.hub === login.hub));
}

/** Why Fastctx on `machine` is not uninstalled as `login` takes it off, or
 *  null where it may be: never on the machine that runs the hub or on a
 *  computer an application carries it on, managed or not; a member's
 *  machine only by those who hold it; and only through a hub that tells it
 *  to. */
export function cannotUninstall(machine: Machine, world: WorldNow, login: Login, running: Running | undefined, words: ManageWords): string | null {
  const managed = isManaged(world, machine.name);
  if (running?.hub_here) return words.notHub;
  if (running?.app) return managed ? words.managedApp : words.notApp;
  const holder = holds(machine, login, world) ? login : null;
  if (!holder && !managed) return words.notMember;
  if (!world.accounts.hubs.find((hub) => hub.hub === login.hub)?.uninstalls) return words.hubOlder;
  return null;
}
