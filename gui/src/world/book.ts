// The activity book of each machine (`world-layers.md`, 活动账本): one deed
// for each script `run` ran there, kept 90 days, and counts by the hour
// kept a year. Each machine answers for itself; the activity page asks each
// and puts them together.

import { invoke } from "@tauri-apps/api/core";

export type BookQuery =
  | { ask: "deeds"; from: string; to: string; limit: number }
  | { ask: "hours"; from: string; to: string };

export type Outcome = "running" | "done" | "failed" | "stopped" | "lost";
export type TouchHow = "added" | "changed" | "removed" | "copied";

export interface Touched {
  path: string;
  how: TouchHow;
  added?: number;
  removed?: number;
  /** Seen changed in its repository while the script ran, rather than
   *  written by the World's own commands. */
  seen?: boolean;
}

export interface Deed {
  job: string;
  began: string;
  ended?: string;
  /** The machine of the calling session, when not the deed's own. */
  from?: string;
  /** The calling session, `resident@machine`. */
  session?: string;
  agent?: "claude" | "codex";
  /** One of Fastctx's conversations, not one the user opened on their own. */
  held?: boolean;
  project?: string;
  script: string;
  outcome: Outcome;
  code?: number;
  output: number;
  went?: string[];
  looked: number;
  copied: number;
  touched?: Touched[];
}

export interface Hour {
  at: string;
  session?: string;
  agent?: "claude" | "codex";
  held?: boolean;
  runs: number;
  failed: number;
  busy: number;
  files: number;
}

export type BookAnswer =
  | { kind: "deeds"; machine: string; deeds: Deed[]; more: boolean }
  | { kind: "hours"; machine: string; hours: Hour[] };

/** The book of `machine`, or of this computer when none. */
export function readBook(machine: string | null, query: BookQuery): Promise<BookAnswer> {
  return invoke("book", { machine, query });
}

/** Brings a Codex session the user opened on their own into Fastctx. */
export function bringIn(resident: string): Promise<void> {
  return invoke("bring_in", { resident });
}
