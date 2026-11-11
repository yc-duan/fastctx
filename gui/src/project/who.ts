// Project residents and jobs from the existing World watch. A one-time
// town listing supplies the last action of residents that moved away;
// the watch still decides whether their current identity is visible.
import type { MachineWho, ProjectRecord, WorldWho } from "../world/world";

function machineKey(machine: MachineWho): string {
  const source = machine.observation?.source;
  return source ? `${source.hub}/${source.key}` : machine.kind === "answered" ? machine.reply.machine : machine.machine;
}

/** Keep historical actions across incomplete replies; live state is never merged in. */
export function mergeFormer(previous: WorldWho | null, incoming: WorldWho): WorldWho {
  const machines = new Map(previous?.machines.map((machine) => [machineKey(machine), machine]) ?? []);
  for (const machine of incoming.machines) {
    const key = machineKey(machine);
    if (machine.kind !== "answered") continue;
    const before = machines.get(key);
    const residents = new Map(before?.kind === "answered" ? before.reply.residents.filter((resident) => resident.moved).map((resident) => [resident.about.id, resident]) : []);
    for (const resident of machine.reply.residents) {
      if (!resident.moved) continue;
      const prior = residents.get(resident.about.id);
      if (!prior?.moved || resident.moved.at >= prior.moved.at) residents.set(resident.about.id, resident);
    }
    machines.set(key, { ...machine, reply: { ...machine.reply, residents: [...residents.values()], jobs: [] } });
  }
  return { ...incoming, machines: [...machines.values()] };
}

export function projectWho(world: WorldWho, town: string, record: ProjectRecord, former: WorldWho | null): WorldWho {
  return {
    ...world,
    scope: { kind: "town", town, record },
    machines: world.machines.map((machine) => {
      if (machine.kind !== "answered") return machine;
      const source = machine.observation?.source;
      const prior = former?.machines.find((one) => {
        if (one.kind !== "answered") return false;
        const previous = one.observation?.source;
        return source && previous ? source.hub === previous.hub && source.key === previous.key : one.reply.machine === machine.reply.machine;
      });
      const current = machine.reply.residents.filter((resident) => resident.town === town);
      const visible = new Map(machine.reply.residents.map((resident) => [resident.about.id, resident]));
      const past = prior?.kind === "answered" ? prior.reply.residents.filter((resident) => resident.moved && resident.town === town && visible.has(resident.about.id) && !current.some((one) => one.about.id === resident.about.id)).map((resident) => ({ ...resident, about: visible.get(resident.about.id)!.about, removed: visible.get(resident.about.id)!.removed ?? null })) : [];
      return { ...machine, reply: { ...machine.reply, residents: [...current, ...past], jobs: machine.reply.jobs.filter((job) => job.town === town) } };
    }),
  };
}
