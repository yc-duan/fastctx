// Updating a machine of 1.0.1 beta 1 or 2 from here (`world-upgrade.md`,
// 别的机器的更新). Such a machine takes the newest release on the line its
// network names, and a hub of those releases keeps the line its super
// administrator last set, the stable releases' unless set: there it finds no
// test release to take. A later hub names the test releases' line to such
// machines whatever it keeps; under an earlier one, the super administrator
// asking from here names that line first, as the later hub would.

import type { AccountChange, Login } from "../world/accounts";
import type { UpdateNetwork, UpdateState } from "../world/updates";

/** How such a machine says it found nothing on its line. */
const NOTHING_ON_LINE = "no release is known here yet";
/** How often the machine is asked once the line is named. */
const TRIES = 3;

export interface OlderUpdating {
  /** What this device last heard of its networks' updates. */
  network(): UpdateNetwork | undefined;
  /** The login here that is the super administrator of a hub `machine` is
   *  on, if any. */
  superOver(machine: string): Login | undefined;
  change(login: Login, change: AccountChange): Promise<void>;
  /** Has `name` update itself to the newest release on its line. */
  machine(name: string): Promise<UpdateState>;
}

/** Has the machine `name` update itself, naming the test releases' line
 *  first where an earlier hub keeps another and this device may set it. */
export async function updateOlder(name: string, seam: OlderUpdating): Promise<UpdateState> {
  const network = seam.network();
  const login = network && !network.hub && network.policy.channel !== "beta" ? seam.superOver(name) : undefined;
  if (!network || !login) return seam.machine(name);
  await seam.change(login, { change: "updates", policy: { ...network.policy, channel: "beta" } });
  // The hub tells every machine the line once it keeps it; a machine asked
  // at once may not have heard it yet.
  for (let tries = 1; ; tries += 1) {
    await new Promise((settle) => window.setTimeout(settle, tries * 1000));
    try {
      return await seam.machine(name);
    } catch (error) {
      const said = error instanceof Error ? error.message : String(error);
      if (tries === TRIES || !said.includes(NOTHING_ON_LINE)) throw error;
    }
  }
}
