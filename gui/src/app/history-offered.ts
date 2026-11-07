// Whether the interface shows history (`world-history.md`): the build offers
// it (`preview.ts`), and the account a conversation or project works for
// turned it on, or, for a computer in no network, the computer did. Documents
// already written stay readable wherever they are linked from.

import type { Acting } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import { specimenHistory } from "../world/history-switch";
import { HISTORY } from "./preview";
import { inNoNetwork } from "./reach";

/** For a conversation working as `acting`. */
export function historyForTalk(world: WorldNow | null, acting: Acting | null, live: boolean): boolean {
  if (!HISTORY) return false;
  if (!live) return specimenHistory();
  if (!world) return false;
  if (inNoNetwork(world)) return world.accounts.history === true;
  const named = acting?.kind === "accounts" ? acting.accounts : null;
  return world.accounts.hubs.some((hub) =>
    (hub.history ?? []).some((account) => !named || named.some((one) => one.hub === hub.hub && one.account === account)),
  );
}

/** For the project `town`: one of the accounts here that may use it. */
export function historyForProject(world: WorldNow | null, town: string, live: boolean): boolean {
  if (!HISTORY) return false;
  if (!live) return specimenHistory();
  if (!world) return false;
  if (inNoNetwork(world)) return world.accounts.history === true;
  return world.accounts.hubs.some((hub) => {
    const project = hub.projects.find((one) => one.name === town);
    return !!project && (hub.history ?? []).some((account) => account === project.owner || project.granted.includes(account));
  });
}
