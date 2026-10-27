// Taking machines off their networks, one or several, on the one sheet of
// three ways (`world-network.md`, 移出网络与卸载): each machine is taken off
// as the login that may, and Fastctx uninstalled on those it may be on;
// the rest only leave, and the sheet says how many and why. What a machine
// told to uninstall could not do reaches its hub's log, which the sheet
// reads for a while after.

import type { AccessWords } from "./account-words";
import type { AccountChange, Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Running } from "../world/updates";
import { cannotUninstall, removerLoginOf } from "./machine-logins";
import type { ReadLog } from "./machine-log";
import type { ManageWords } from "./machine-manage-words";
import type { Following, RemoveMachineSheet } from "./machine-remove";
import { owns, type Machine } from "./reach";

export interface RemovingOptions {
  manage: ManageWords;
  access: AccessWords;
  sheet: RemoveMachineSheet;
  /** What the network last heard each machine runs. */
  running(name: string): Running | undefined;
  change(login: Login, change: AccountChange): Promise<void>;
  /** A hub's log, to follow those told to uninstall; none, not followed. */
  log?: ReadLog;
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
    others: each.some((one) => !owns(one.machine, one.login)) ? manage.othersLose(each.length) : null,
    remove: async (way) => {
      const uninstall = way === "leave" ? null : way;
      const told = uninstall ? each.filter((one) => one.cannot === null) : [];
      // Where each hub's log stood before, so what comes after is theirs;
      // a log not read leaves them unfollowed, never kept on the network.
      const since =
        options.log && told.length
          ? await newest(options.log, told.map((one) => one.login)).catch((error: unknown) => {
              console.error("reading the machine log before taking machines off", error);
              return null;
            })
          : null;
      const failed: string[] = [];
      const gone: typeof told = [];
      for (const one of each) {
        const then = uninstall && told.includes(one) ? { then: uninstall } : {};
        try {
          await options.change(one.login, { change: "remove", name: one.machine.name, reason: options.access.reasonDefault, ...then });
          if (told.includes(one)) gone.push(one);
        } catch (error) {
          failed.push(`${one.machine.written}: ${error instanceof Error ? error.message : String(error)}`);
        }
      }
      options.done();
      const log = options.log;
      // Those taken off to uninstall are followed even when others could
      // not be taken off, which the sheet says beside them.
      if (!log || !since || !gone.length) {
        if (failed.length) throw new Error(failed.join("; "));
        return null;
      }
      const following: Following = {
        ...(failed.length ? { failed: options.manage.removeFailed(failed.join("; ")) } : {}),
        machines: gone.map((one) => ({ written: one.machine.written, away: one.machine.online === false })),
        left: async () => {
          const left = new Map<string, string>();
          for (const [key, { login, after }] of since) {
            // Newest first, page after page, back to where the log stood
            // when they were taken off.
            let before: number | null = null;
            for (let pages = 0; pages < LOG_PAGES; pages += 1) {
              const page = await log(login.hubName, login.account, before);
              for (const record of page.records) {
                if (record.id <= after || record.action.change !== "uninstall_left") continue;
                const { name, left: said } = record.action;
                const one = gone.find((one) => one.machine.name === name && hubKey(one.login) === key);
                if (one && !left.has(one.machine.written)) left.set(one.machine.written, said);
              }
              if (page.next_before === null || page.records.some((record) => record.id <= after)) break;
              before = page.next_before;
            }
          }
          return left;
        },
      };
      return following;
    },
  });
}

/** How far back a hub's log is read for what machines said as they left. */
const LOG_PAGES = 10;

function hubKey(login: Login): string {
  return `${login.hub}\u0000${login.account}`;
}

/** The newest record of each hub's log as `logins` read it. */
async function newest(log: ReadLog, logins: Login[]): Promise<Map<string, { login: Login; after: number }>> {
  const since = new Map<string, { login: Login; after: number }>();
  for (const login of logins) {
    const key = hubKey(login);
    if (since.has(key)) continue;
    const page = await log(login.hubName, login.account, null);
    since.set(key, { login, after: Math.max(0, ...page.records.map((record) => record.id)) });
  }
  return since;
}
