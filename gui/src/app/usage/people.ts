// Who a login's usage is of, as this computer or device knows them: its own
// accounts are 「我」, the machine's own sessions are its owner's, and an
// account the login assists carries that assistance with it.

import type { AssistSeen, HubAccount, HubId } from "../../world/accounts";
import { loginsOf } from "../../world/accounts";
import type { WorldNow } from "../../world/bridge";
import { assistantsOn, machinesOf, sameHome } from "../reach";
import type { Named } from "./model";
import type { UsageWords } from "./words";

/** Names the accounts of a report from `machine`. */
export function namer(world: WorldNow | null, machine: string, words: UsageWords): (account: HubAccount | null) => Named {
  const logins = loginsOf(world?.accounts);
  const named = (hub: HubId, account: string): Named => {
    const me = logins.some((login) => login.hub === hub && login.account === account);
    return { key: `${hub}:${account}`, name: me ? words.me : account, me };
  };
  const found = machinesOf(world).find((one) => one.name === machine || one.written === machine);
  const on = found?.on.find((one) => one.owner);
  const assisting = assistantsOn(world, machine).find((one) => one.owner);
  const owner = on?.owner ? named(on.hub, on.owner) : assisting?.owner ? named(assisting.login.hub, assisting.owner) : null;
  return (account) => (account ? named(account.hub, account.account) : (owner ?? { key: "machine", name: words.machineItself, me: false }));
}

/** The assistances of `agent`'s login `dir` on `machine`, each with its
 *  hub; `isDefault` is whether it is the agent's default login there. */
export function assistsOf(world: WorldNow | null, machine: string, agent: string, dir: string, isDefault: boolean): { hub: HubId; assist: AssistSeen }[] {
  const home = isDefault ? null : dir;
  return (world?.accounts.hubs ?? []).flatMap((hub) =>
    hub.assists.filter((assist) => assist.machine === machine && assist.agent === agent && sameHome(assist.home, home)).map((assist) => ({ hub: hub.hub, assist })),
  );
}

/** The assistance that lets this computer's user use the login, where that
 *  is how they may: the machine is not one of theirs to work on. */
export function assistingMe(world: WorldNow | null, machine: string, agent: string, dir: string, isDefault: boolean): { assist: AssistSeen; owner: string | null } | null {
  const assistant = assistantsOn(world, machine).find((one) => one.agent === agent && sameHome(one.home, isDefault ? null : dir));
  if (!assistant) return null;
  const hub = world?.accounts.hubs.find((one) => one.hub === assistant.login.hub);
  const assist = hub?.assists.find((one) => one.machine === assistant.machine && one.account === assistant.login.account && one.agent === agent && sameHome(one.home, assistant.home));
  return assist ? { assist, owner: assistant.owner } : null;
}
