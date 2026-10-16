// A hub's record of changes for a specimen, as `machine_log` answers it:
// newest first, a page at a time.

import type { MachineLogPage, MachineLogRecord } from "../world/machine-log";

const records: MachineLogRecord[] = [
  { id: 12, at: "2026-10-04T07:15:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "rename", machine: "izbp1abc0x2y3z", to: "server" }, outcome: { kind: "done" } },
  { id: 11, at: "2026-10-04T06:30:00Z", actor: { account: null, authenticated: true, member: null, peer: null }, action: { change: "lend_ended", machine: "laptop", to: "bo", agent: "claude", home: null }, outcome: { kind: "done" } },
  { id: 10, at: "2026-10-04T06:02:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "lend", machine: "laptop", to: "bo", agent: "claude", home: null, lent: true, until: "2026-10-04T06:30:00Z" }, outcome: { kind: "done" } },
  { id: 9, at: "2026-10-04T05:58:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "relays", machine: "server", relays: [{ label: "lab", addresses: ["203.0.113.20:7444"] }] }, outcome: { kind: "done" } },
  { id: 8, at: "2026-10-04T05:57:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "relay_at", machine: "lab", addresses: ["203.0.113.20:7444"] }, outcome: { kind: "done" } },
  { id: 7, at: "2026-10-04T05:41:00Z", actor: { account: "me", authenticated: true, member: "pixel", peer: null }, action: { change: "lend", machine: "laptop", to: "ada", agent: "codex", home: null, lent: true }, outcome: { kind: "done" } },
  { id: 6, at: "2026-10-04T05:30:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "set_account", name: "bo", locked: false }, outcome: { kind: "done" } },
  { id: 5, at: "2026-10-04T05:12:00Z", actor: { account: "ada", authenticated: false, member: "studio", peer: null }, action: { change: "remove", name: "laptop", reason: "" }, outcome: { kind: "refused", reason: "ada may not remove a machine of me's" } },
  { id: 4, at: "2026-10-03T22:20:00Z", actor: { account: "me", authenticated: true, member: null, peer: null }, action: { change: "joined", name: "pixel", kind: "phone", registered: false, via: "token" }, outcome: { kind: "done" } },
  { id: 3, at: "2026-10-03T22:19:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "token", register: false, name: null, kind: "phone", expires: "2026-10-03T23:19:00Z" }, outcome: { kind: "done" } },
  { id: 2, at: "2026-10-03T17:10:00Z", actor: { account: "me", authenticated: true, member: "desktop", peer: null }, action: { change: "share", machine: "laptop", to: "ada", shared: true }, outcome: { kind: "done" } },
  { id: 1, at: "2026-10-03T17:00:00Z", actor: { account: "me", authenticated: true, member: null, peer: null }, action: { change: "joined", name: "desktop", kind: "computer", registered: true, via: "password" }, outcome: { kind: "done" } },
];

/** A machine the specimen took off, told to uninstall Fastctx as `then`:
 *  logged at once; `lab`, whose node is not an administrator, then says a
 *  moment later what it could not do. */
export function specimenRemoved(name: string, then: "uninstall" | "everything" | undefined): void {
  const next = () => Math.max(0, ...records.map((record) => record.id)) + 1;
  const actor = { account: "me", authenticated: true, member: "desktop", peer: null };
  records.unshift({ id: next(), at: new Date().toISOString(), actor, action: { change: "remove", name, reason: "", ...(then ? { then } : {}) }, outcome: { kind: "done" } });
  if (!then || name !== "lab") return;
  const everything = then === "everything" ? " --everything" : "";
  window.setTimeout(() => {
    records.unshift({
      id: next(),
      at: new Date().toISOString(),
      actor: { account: null, authenticated: true, member: null, peer: null },
      action: { change: "uninstall_left", name, left: `the boot service needs an administrator: run \`fastctx uninstall run${everything}\` on this machine as one`, by: "me", home: "me" },
      outcome: { kind: "done" },
    });
  }, 1500);
}

export async function specimenMachineLog(_hub: string, _account: string, before: number | null): Promise<MachineLogPage> {
  const older = records.filter((record) => before === null || record.id < before);
  const page = older.slice(0, 6);
  return { records: page, next_before: older.length > page.length ? page[page.length - 1]!.id : null };
}
