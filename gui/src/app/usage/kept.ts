// What was last read of each machine's logins and of each login's usage,
// kept while the interface is open: a page opened again shows it at once
// and reads anew behind it, so going from the agents to a login's usage
// and back never waits on the network to show something. A read already
// under way is shared, not asked twice.

import type { MachineLogins } from "../../world/logins";
import type { UsageReport } from "../../world/usage";

const records = new Map<string, MachineLogins>();
const reports = new Map<string, { report: UsageReport; at: number }>();
const asking = new Map<string, Promise<UsageReport>>();

export function keptRecord(machine: string): MachineLogins | null {
  return records.get(machine) ?? null;
}

export function keepRecord(logins: MachineLogins): void {
  records.set(logins.machine, logins);
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
