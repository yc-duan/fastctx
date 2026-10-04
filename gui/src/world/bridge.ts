// The way to the World from inside the shell: each function is one of the
// shell's commands (`src-tauri/src/lib.rs`), which is one call of the
// core's client. Outside the shell there is no World to reach, and
// `inShell` says so.

import { Channel, invoke } from "@tauri-apps/api/core";
import type { Address, MachineName, SessionEvent } from "./events";
import type { JournalSummary, JournalView, LookedUp, MapView, NodesReply, ProjectRecord, Stamp, TownSummary, WorldWho } from "./world";

export function inShell(): boolean {
  return "__TAURI_INTERNALS__" in window;
}

export interface WorldNow {
  here: MachineName;
  who: WorldWho;
  nodes: NodesReply;
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
  // A handler that throws stops the channel for good: Tauri delivers
  // pieces in order and never moves past one that failed. A piece the page
  // cannot show is reported and passed over, so the conversation goes on.
  pieces.onmessage = (piece) => {
    try {
      onPiece(piece);
    } catch (error) {
      console.error("a piece of the conversation could not be shown", piece, error);
    }
  };
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

/** A project's overview: its map, and who is in its town now. */
export function project(town: string): Promise<{ map: MapView; who: WorldWho }> {
  return invoke("project", { town });
}

/** A journal's latest entries that fit, or those before an entry. */
export function journal(town: string, name: string, before: string | null): Promise<JournalView> {
  return invoke("journal", { town, journal: name, before });
}

/** Residents or jobs in detail, from every machine that has them. */
export function look(ids: string[]): Promise<LookedUp[]> {
  return invoke("look", { ids });
}

/** What a version of a project's record says, before the World stamps it;
 *  what is left out stays as it was. */
export interface RecordFields {
  name?: string | null;
  what?: string | null;
  machines?: MachineName[] | null;
  places?: { what: string; machine: MachineName; path: string }[] | null;
  canon?: { machine: MachineName; path: string } | null;
}

export interface TownRecord {
  town: string;
  record: ProjectRecord;
  opened: Stamp;
}

export function projectNew(name: string, fields: RecordFields): Promise<TownRecord> {
  return invoke("project_new", { name, fields });
}

export function projectEdit(name: string, fields: RecordFields): Promise<TownRecord> {
  return invoke("project_edit", { name, fields });
}

export function journals(name: string): Promise<JournalSummary[]> {
  return invoke("journals", { name });
}

/** An entry the user writes; returns its id. */
export function journalAdd(name: string, journal: string, text: string, description: string | null): Promise<string> {
  return invoke("journal_add", { name, journal, text, description });
}

/** A one-use code for one more machine, valid for `hours`. */
export function invite(name: string | null, hours: number): Promise<string> {
  return invoke("invite", { name, hours });
}

export function revoke(name: MachineName, reason: string): Promise<void> {
  return invoke("revoke", { name, reason });
}

export function leave(): Promise<void> {
  return invoke("leave");
}
