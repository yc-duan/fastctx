// The World's tools as the interface carries them out for an open page
// (`../page/tools.ts` is the contract a page is written against). Reading
// tools answer from what the user can read anyway: the World the interface
// holds, the project's map and journals, a file on a machine. Doing tools
// go to the core as acts of this page's version, where the declaration and
// the user's consent are checked; the interface never decides them itself.

import { machinesOf } from "../app/reach";
import type { Tools } from "../page/tools";
import { residentsOf, type Store } from "../app/store";
import { look, project, readText } from "../world/bridge";
import type { Address } from "../world/events";
import type { PageInvocation, PageResult, RunReply } from "../world/pages";
import type { JobRecord, ResidentNow } from "../world/world";
import { agentName, titleOf } from "../world/names";
import type { HeldReads } from "../world/reads";

export interface ToolsOptions {
  town: string;
  store: Store;
  /** Opens a conversation in the interface, leaving the page. */
  open(address: Address): void;
  /** Asks the core to do one of the page's acts. */
  act(action: PageInvocation): Promise<PageResult>;
  /** The files the page reads, let go of when it closes. */
  reads: HeldReads;
}

type Answer<K extends keyof Tools> = Tools[K]["answer"];

function state(resident: ResidentNow): Answer<"world.now">["agents"][number]["state"] {
  if (resident.now?.waiting_for_user) return "waiting";
  if (resident.now?.activity === "busy") return "working";
  if (resident.now?.activity === "asleep") return "asleep";
  return "idle";
}

/** How a job stands, from its record: the shell exiting is not the end
 *  while processes it started still run. */
function jobState(record: JobRecord): Answer<"job.read">["state"] {
  if (!record.end) return "running";
  const how = record.end.how.kind;
  return how === "finished" ? "exited" : how;
}

function text(value: unknown, name: string): string {
  if (typeof value !== "string" || !value) throw new Error(`${name} is missing`);
  return value;
}

/** What a run printed, as the World windowed it. */
function printed(reply: RunReply): string {
  return reply.output.pieces
    .map((piece) => {
      switch (piece.kind) {
        case "text":
        case "part_of_line":
          return piece.text;
        case "omitted":
          return `… lines ${piece.first_line}–${piece.last_line} …\n`;
        case "image":
          return `[image ${piece.width}×${piece.height}]\n`;
        case "unreadable":
          return `[line ${piece.line}: ${piece.reason}]\n`;
      }
    })
    .join("");
}

function exit(reply: RunReply): number | null {
  const status = reply.state.kind === "ended" || reply.state.kind === "exited" ? reply.state.status : null;
  if (!status) return null;
  return "code" in status ? status.code : 128 + status.signal;
}

export function toolsFor(options: ToolsOptions): (tool: string, args: Record<string, unknown>) => Promise<unknown> {
  const { town, store } = options;
  const now = async () => {
    if (!store.world.get()) await store.refresh();
    const world = store.world.get();
    if (!world) throw new Error(store.trouble.get() ?? "Fastctx has not answered");
    return world;
  };

  const tools: { [K in keyof Tools]: (args: Record<string, unknown>) => Promise<Answer<K>> } = {
    async "world.now"() {
      const world = await now();
      return {
        // A page reads the machines this device reaches; a phone is not one.
        machines: machinesOf(world).map((machine) => ({
          name: machine.name,
          online: machine.here || machine.online === true,
          system: "Ok" in machine.facts ? machine.facts.Ok.os : null,
          device: false,
        })),
        agents: residentsOf(world).map((resident) => {
          const address = { resident: resident.about.id, machine: resident.about.machine };
          return {
            resident: address.resident,
            machine: address.machine,
            agent: agentName(resident.about.host.agent),
            project: resident.town,
            title: titleOf(resident),
            state: state(resident),
            since: resident.now?.since ?? null,
            last: resident.last_action?.what ?? null,
          };
        }),
      };
    },

    async "project.map"() {
      const { map } = await project(town);
      return {
        project: map.town,
        name: map.record.name,
        what: map.record.what,
        machines: map.record.machines,
        pages: map.pages.filter((summary) => !summary.current.withdrawn).map((summary) => ({ page: summary.page, title: summary.current.title, description: summary.current.description || null })),
      };
    },

    async "file.read"(args) {
      const tail = typeof args.tail === "number" && args.tail > 0 ? args.tail : null;
      const machine = text(args.machine, "machine");
      const path = text(args.path, "path");
      const read = await options.reads.hold((held) => readText(machine, path, tail ? { kind: "tail", bytes: tail } : { kind: "all" }, held));
      return { text: read.text, size: read.info.file.size, modified: read.info.file.modified, offset: read.info.offset };
    },

    async "job.read"(args) {
      const machine = text(args.machine, "machine");
      const id = text(args.job, "job");
      const [looked] = await look([id]);
      const record = looked?.found.flatMap((found) => (found.kind === "job" && found.record.machine === machine ? [found.record] : []))[0];
      if (!record) {
        throw new Error(looked?.unasked.includes(machine) ? `${machine} could not be asked about job ${id}` : `there is no job ${id} on ${machine}`);
      }
      const output = await options.reads.hold((held) => readText(machine, record.output, { kind: "tail", bytes: 16 * 1024 }, held));
      const status = record.exit?.status;
      return {
        job: id,
        command: record.script,
        state: jobState(record),
        exit: status && "code" in status ? status.code : null,
        signal: status && "signal" in status ? status.signal : null,
        started: record.started_at,
        output: output.text ?? "",
      };
    },

    async open(args) {
      options.open({ resident: text(args.resident, "resident"), machine: text(args.machine, "machine") });
      return {};
    },

    async run(args) {
      const result = await options.act({ kind: "run", machine: text(args.machine, "machine"), script: text(args.command, "command") });
      if (result.kind !== "run") throw new Error("Fastctx answered something other than a run");
      return { exit: exit(result), output: printed(result) };
    },

    async say(args) {
      const to = { resident: text(args.resident, "resident"), machine: text(args.machine, "machine") };
      const result = await options.act({ kind: "say", to, text: text(args.text, "text") });
      if (result.kind !== "said") throw new Error("Fastctx answered something other than words said");
      return { delivered: true };
    },

  };

  return (tool, args) => {
    const known = Object.prototype.hasOwnProperty.call(tools, tool) ? tools[tool as keyof Tools] : undefined;
    if (!known) return Promise.reject(new Error(`Fastctx has no tool ${tool} for a page`));
    return known(args);
  };
}
