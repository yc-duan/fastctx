// Where a conversation works (`where.ts`), read from the World as the
// interface holds it: the computer it lives on, then any other it was
// pointed at as it was opened, each with the folders it works in there.

import { assistantOf, machinesOf } from "../app/reach";
import { assistedBy, bareName, residentAt, residentsOf, unreachedOf } from "../app/store";
import type { WorldNow } from "../world/bridge";
import type { Address } from "../world/wire";
import type { WhereMachine, WhereShown } from "./types";

/** Where the conversation at `address` works, as the World last listed it,
 *  its machine away or not; null while it is not listed. */
export function whereOf(world: WorldNow | null, address: Address, link: (machine: string) => string | null): WhereShown | null {
  const known =
    residentAt(world, address) ??
    [...residentsOf(world, true), ...unreachedOf(world, true)].find((r) => r.about.id === address.resident && r.about.machine === address.machine) ??
    null;
  if (!known) return null;
  const machines = machinesOf(world);
  const entry = (written: string, folders: string[]): WhereMachine => {
    const machine = machines.find((one) => one.written === written) ?? machines.find((one) => one.name === bareName(written)) ?? null;
    const facts = machine && "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    return {
      name: written,
      form: facts?.form ?? null,
      online: machine ? (machine.here ? true : machine.online) : null,
      here: machine?.here ?? false,
      link: link(machine?.written ?? written),
      folders,
    };
  };
  const focus = known.about.focus?.machines ?? [];
  const lives = (written: string) => written === address.machine || bareName(written) === bareName(address.machine);
  const host = entry(address.machine, focus.find((one) => lives(one.machine))?.folders ?? []);
  const others = focus.filter((one) => !lives(one.machine)).map((one) => entry(one.machine, one.folders));
  const assistant = assistantOf(world, known);
  return { machines: [host, ...others], from: assistant ? (assistant.owner ?? assistant.machine) : assistedBy(world, known) };
}
