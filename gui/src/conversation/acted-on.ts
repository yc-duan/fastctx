// Which machine a file an agent names in its words is on. Its words carry
// only the path; the machine is read off what the same turn did before it
// spoke, so a turn read later still names the machine it worked on then,
// whatever its conversation works from now (`world-placement.md`).

import type { MachineName } from "../world/events";
import type { JsonValue } from "../world/wire";
import { worldServer, type Entry } from "./model";

type ToolEntry = Extract<Entry, { kind: "tool" }>;

/** Where a tool acted: a machine of the World's, `null` for the
 *  conversation's own host, `undefined` where it acted on no machine (the
 *  World's words to the user, its plan). */
function actedOn(entry: ToolEntry): MachineName | null | undefined {
  if (!worldServer(entry.tool.server)) return null;
  return entry.tool.machine ?? undefined;
}

/** A path as compared: its separators alike, a drive letter in one case. */
const comparable = (path: string) => path.replace(/\\/g, "/").replace(/^([A-Za-z]):/, (_, drive: string) => `${drive.toLowerCase()}:`);

function names(value: JsonValue | undefined, path: string): boolean {
  if (typeof value === "string") return comparable(value).includes(path);
  if (Array.isArray(value)) return value.some((one) => names(one, path));
  if (value && typeof value === "object") return Object.values(value).some((one) => names(one, path));
  return false;
}

/** The machine `message_user` sent the file `path` from, where it did. */
function sentFrom(entry: ToolEntry, path: string): MachineName | undefined {
  if (entry.tool.name !== "message_user" || !worldServer(entry.tool.server)) return undefined;
  const input = entry.tool.input;
  if (!input || typeof input !== "object" || Array.isArray(input) || !Array.isArray(input.attachments)) return undefined;
  for (const one of input.attachments) {
    if (one && typeof one === "object" && !Array.isArray(one) && typeof one.machine === "string" && typeof one.path === "string" && comparable(one.path) === path) return one.machine;
  }
  return undefined;
}

/** The machine holding the file `path` that `words`, an entry of `turn`,
 *  names: the one where the last tool before it that named the path acted,
 *  else the one the turn last worked on; `null` for the conversation's own
 *  host, or where nothing the turn did says. */
export function machineNamed(turn: readonly Entry[], words: Entry, path: string): MachineName | null {
  const before = turn.indexOf(words);
  const tools = (before < 0 ? turn : turn.slice(0, before + 1)).filter((entry): entry is ToolEntry => entry.kind === "tool");
  const wanted = comparable(path);
  for (let at = tools.length - 1; at >= 0; at -= 1) {
    const tool = tools[at]!;
    const sent = sentFrom(tool, wanted);
    if (sent) return sent;
    const machine = actedOn(tool);
    if (machine === undefined) continue;
    if (names(tool.tool.input, wanted) || tool.tool.output.some((part) => part.type === "text" && comparable(part.text).includes(wanted))) return machine;
  }
  for (let at = tools.length - 1; at >= 0; at -= 1) {
    const machine = actedOn(tools[at]!);
    if (machine !== undefined) return machine;
  }
  return null;
}
