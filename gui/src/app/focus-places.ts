// A machine as the machines and folders panel sets it out (`focus-panel.ts`):
// whose it is, the group its owner put it in and what kind of computer it
// is, as the machine list knows them (`map-of.ts`).

import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { FocusPlace } from "./focus-panel";
import { ownerOf, owns, type Machine } from "./reach";
import { indexOf } from "./world-index";

export function focusPlace(world: WorldNow | null, machine: Machine): FocusPlace {
  const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
  const logins = world ? loginsOf(world.accounts) : [];
  const mine = machine.here || logins.some((login) => owns(machine, login));
  const owner = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
  const member = world ? indexOf(world).computer.get(machine.name) : undefined;
  return {
    written: machine.written,
    name: machine.name,
    here: machine.here,
    online: machine.here ? true : machine.online,
    group: member?.group ?? null,
    owner,
    ...(facts?.form ? { form: facts.form } : {}),
  };
}
