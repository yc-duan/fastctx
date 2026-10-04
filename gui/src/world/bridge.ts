// The way to the World from inside the shell: each function is one of the
// shell's commands (`src-tauri/src/lib.rs`), which is one call of the
// core's client. Outside the shell there is no World to reach, and
// `inShell` says so.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Address, MachineName, SessionEvent } from "./events";
import type { MachineWho, Node, TownSummary } from "./world";

export function inShell(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export interface WorldNow {
  here: MachineName;
  who: { machine: MachineName; scope: unknown; machines: MachineWho[] };
  nodes: { machine: MachineName; world: string | null; link: unknown; machines: Node[] };
  towns: TownSummary[];
}

export type Piece = { kind: "event"; event: SessionEvent } | { kind: "ended"; error: string | null };

export type Landed =
  | { kind: "uncertain"; why: string }
  | { kind: "new_turn"; turn: string }
  | { kind: "joined_turn"; turn: string }
  | { kind: "waiting"; why: string | null; answered: boolean }
  | { kind: "sent" }
  | { kind: "handed"; id: string };

export function worldNow(): Promise<WorldNow> {
  return invoke<WorldNow>("world_now");
}

/** Watches a resident: what came before, then each piece as it comes. */
export async function watch(
  who: Address,
  onPiece: (piece: Piece) => void,
): Promise<{ id: number; history: SessionEvent[] }> {
  const pieces = new Channel<Piece>();
  pieces.onmessage = onPiece;
  return invoke("watch", { residentId: who.resident, machineName: who.machine, pieces });
}

export function unwatch(id: number): Promise<void> {
  return invoke("unwatch", { id });
}

export function talk(who: Address, text: string): Promise<{ to: Address; landed: Landed }> {
  return invoke("talk", { residentId: who.resident, machineName: who.machine, text });
}

/** Asks an agent into the World, with the user's first words. */
export function open(
  agent: string,
  machine: MachineName,
  town: string | null,
  text: string,
): Promise<{ to: Address; landed: Landed }> {
  return invoke("open", { agent, town, machineName: machine, text });
}

export function stop(who: Address): Promise<boolean> {
  return invoke("stop", { residentId: who.resident, machineName: who.machine });
}
