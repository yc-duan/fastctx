// Taking machines off their networks, one or several, on the one sheet of
// three ways (`world-network.md`, 移出网络与卸载): each machine is taken off
// as the login that may, and Fastctx uninstalled on those it may be on;
// the rest only leave, and the sheet says how many and why.

import type { AccessWords } from "./account-words";
import type { AccountChange, Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Running } from "../world/updates";
import { cannotUninstall, removerLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import type { RemoveMachineSheet } from "./machine-remove";
import type { Machine } from "./reach";

export interface RemovingOptions {
  manage: ManageWords;
  access: AccessWords;
  sheet: RemoveMachineSheet;
  /** What the network last heard each machine runs. */
  running(name: string): Running | undefined;
  change(login: Login, change: AccountChange): Promise<void>;
  /** Once every machine was asked to go, whether or not each went. */
  done(): void;
}

/** Opens the sheet that takes `machines` off, as `title` names them; those
 *  no login here may take off are left out. */
export function removeMachines(machines: Machine[], world: WorldNow, title: string, options: RemovingOptions): void {
  const { manage } = options;
  const each = machines.flatMap((machine) => {
    const login = removerLoginOf(machine, world);
    return login ? [{ machine, login, cannot: cannotUninstall(machine, world, login, options.running(machine.name), manage) }] : [];
  });
  if (!each.length) return;
  const unable = each.filter((one) => one.cannot !== null);
  const reasons = [...new Set(unable.map((one) => one.cannot!))];
  const why = reasons.length === 1 ? reasons[0]! : null;
  const cannot = unable.length === each.length ? (why ?? manage.noneUninstall) : null;
  options.sheet.open({
    title,
    cannot,
    some: unable.length && unable.length < each.length ? manage.onlyLeave(unable.length, why) : null,
    offline: each.some((one) => one.machine.online === false),
    remove: async (way) => {
      const failed: string[] = [];
      for (const one of each) {
        const then = way !== "leave" && one.cannot === null ? { then: way } : {};
        try {
          await options.change(one.login, { change: "remove", name: one.machine.name, reason: options.access.reasonDefault, ...then });
        } catch (error) {
          failed.push(`${one.machine.written}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
      options.done();
      if (failed.length) throw new Error(failed.join("; "));
    },
  });
}
