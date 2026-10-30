// What was last read of each machine's logins and of each login's usage: a
// page opened again shows it at once and reads anew behind it, so going from
// the agents to a login's usage and back never waits on the network to show
// something. Each machine's logins are kept across restarts as well, for the
// accounts that read them, so the first look after one shows the machine as
// it last was while its node is asked. A read already under way is shared,
// not asked twice.

import type { AccountsReply } from "../../world/accounts";
import type { AgentLogins, Login, MachineLogins } from "../../world/logins";
import type { UsageReport } from "../../world/usage";

const records = new Map<string, MachineLogins>();
const reports = new Map<string, { report: UsageReport; at: number }>();
const asking = new Map<string, Promise<UsageReport>>();

/** The machines whose logins outlast a restart, the latest read first. */
const KEPT = "kept-logins";
const KEEP = 40;

/** Whom records are read for: the accounts this computer is in, hub by hub;
 *  none known yet keeps them for this run only. */
let reader: () => string = () => "";

/** Tells the records whose they are, from the accounts the World says this
 *  computer is in. */
export function keepLoginsFor(accounts: () => AccountsReply | null | undefined): void {
  reader = () =>
    (accounts()?.hubs ?? [])
      .filter((hub) => hub.logins.length)
      .map((hub) => `${hub.hub}:${[...hub.logins].sort().join(",")}`)
      .sort()
      .join(" ");
}

/** Whom a read asked now is for, to be handed back with what it brings
 *  (`keepRecord`). */
export function readerNow(): string {
  return reader();
}

export function keptRecord(machine: string): MachineLogins | null {
  const who = reader();
  const key = `${who}\n${machine}`;
  const held = records.get(key);
  if (held || !who) return held ?? null;
  const kept = restored(key);
  if (kept) records.set(key, kept);
  return kept;
}

/** Keeps what a read brought for `asked`, whom it was asked for: not when
 *  the accounts have changed since, whose record it would not be. */
export function keepRecord(logins: MachineLogins, asked: string): void {
  const who = reader();
  if (who !== asked) return;
  const key = `${who}\n${logins.machine}`;
  records.set(key, logins);
  if (!who) return;
  try {
    localStorage.setItem(`${KEPT}:${key}`, JSON.stringify(logins));
    const order = (JSON.parse(localStorage.getItem(KEPT) ?? "[]") as string[]).filter((one) => one !== key);
    order.unshift(key);
    for (const gone of order.splice(KEEP)) localStorage.removeItem(`${KEPT}:${gone}`);
    localStorage.setItem(KEPT, JSON.stringify(order));
  } catch {
    // Not kept: the first look after a restart reads it anew.
  }
}

/** A machine's logins as an earlier run left them, without what was passing
 *  then: a scan or a folder's search running, a change under way. */
function restored(key: string): MachineLogins | null {
  try {
    const kept = localStorage.getItem(`${KEPT}:${key}`);
    return kept ? settled(JSON.parse(kept) as MachineLogins) : null;
  } catch {
    return null;
  }
}

function settled(logins: MachineLogins): MachineLogins {
  const still = (login: Login): Login => {
    const kept = { ...login };
    delete kept.change;
    return kept;
  };
  const side = (agent: AgentLogins): AgentLogins => ({
    ...agent,
    listed: agent.listed.map(still),
    candidates: agent.candidates.map(still),
  });
  const kept: MachineLogins = { ...logins, codex: side(logins.codex), claude: side(logins.claude) };
  if (kept.scan?.usual.state === "running") delete kept.scan;
  if (kept.folders) kept.folders = kept.folders.map((folder) => (folder.search?.stage.state === "running" ? { dir: folder.dir } : folder));
  return kept;
}

export function reportKey(machine: string, agent: string, dir: string): string {
  return `${machine}|${agent}|${dir}`;
}

/** The report last read for `key`, and when. */
export function keptReport(key: string): { report: UsageReport; at: number } | null {
  return reports.get(key) ?? null;
}

/** Reads the report for `key` by `read`, or joins the read under way. */
export function readReport(key: string, read: () => Promise<UsageReport>): Promise<UsageReport> {
  const running = asking.get(key);
  if (running) return running;
  const reading = read()
    .then((report) => {
      reports.set(key, { report, at: Date.now() });
      return report;
    })
    .finally(() => asking.delete(key));
  asking.set(key, reading);
  return reading;
}
