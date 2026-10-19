import type { RunState } from "./pages";
import type { MachineName } from "./wire";

export interface OutputRef {
  policy?: string;
  content?: import("./events").Fingerprint;
  machine: MachineName;
  path: string;
  bytes: number;
  lines: number;
  cut: boolean;
}

/** UTF-8 JSON lines, one DiffLine per row, read through the line-source APIs;
 *  `bytes` is what they take. A change whose rows take 32 KiB or less carries
 *  them in `lines`, kept with the conversation and shown with no request. */
export interface DiffRef {
  machine: MachineName;
  path: string;
  content?: import("./events").Fingerprint;
  bytes?: number;
  lines?: DiffLine[];
}

export interface DiffLine {
  kind: "context" | "added" | "removed" | "hunk";
  before: number | null;
  after: number | null;
  text: string;
  ending: string;
}

export interface WriteFact {
  id: string;
  machine: MachineName;
  path: string;
  kind: "added" | "modified" | "deleted";
  added: number | null;
  removed: number | null;
  unavailable?: string;
  diff: DiffRef | null;
  /** Exact UTF-8 stdout-note bytes; line ending is part of text. */
  note?: WriteNote;
  /** The `on` it was written in (`RunStep`), where it was written elsewhere
   *  than where the script ran. */
  step?: string;
}

/** One thing a run did through the World, in the order it began: a command
 *  sent to a machine with `on`, files copied between machines, files looked
 *  at. Its writes are their own facts, each naming the step it came in. */
export type RunStep = OnStep | CopyStep | ViewStep;

interface StepFacts {
  id: string;
  /** The `on` it ran inside, where one `on` sent another; null at the top. */
  within: string | null;
  /** When it began, and ended; null while it goes on. */
  began: string;
  ended: string | null;
}

export interface OnStep extends StepFacts {
  kind: "on";
  machine: MachineName;
  /** What was sent: the command as one line, or the script it was given. */
  command: string;
  job: string;
  /** How it ended, once it has. */
  end: { kind: "exited"; code: number } | { kind: "stopped" } | { kind: "lost"; why: string } | null;
  /** The step whose output flowed straight into this one's input, machine
   *  to machine. */
  fed: string | null;
  /** Its last lines, as a job's end tells them: at most 20 lines, 2000
   *  characters. */
  tail: string | null;
}

export interface CopyStep extends StepFacts {
  kind: "copy";
  from: { machine: MachineName; path: string }[];
  to: { machine: MachineName; path: string };
  files: number;
  bytes: number;
  /** What could not be copied, each as the command said it. */
  problems: string[];
}

export interface ViewStep extends StepFacts {
  kind: "view";
  machine: MachineName;
  paths: string[];
}

/** A multi-file operation keeps its one exact note once, then references that fact. */
export type WriteNote =
  | { kind: "text"; command: OutputNote["command"]; text: string }
  | { kind: "same"; fact: string };

export interface OutputNote {
  command: "write" | "edit" | "apply_patch" | "view" | "nodes" | "accounts" | "who"
    | "wait" | "stop" | "copy" | "project" | "journal" | "page" | "on" | "error";
  /** The writer operation: matching WriteFact ids begin with this plus a hyphen. */
  write?: string;
}

export type DisplayPiece =
  | { kind: "text"; first_line: number; text: string }
  | { kind: "part_of_line"; line: number; from: number; to: number; of: number; text: string }
  | { kind: "omitted"; first_line: number; last_line: number }
  | { kind: "image"; image: number; mime: string; width: number; height: number }
  | { kind: "unreadable"; line: number; reason: string };

export interface OutputSpan { note?: OutputNote; piece: DisplayPiece }

/** All capture matches, including notes outside the model's selected window. */
export interface OutputNoteMatch { note: OutputNote; from: number; to: number }

/** Lazy full-output reader positions use 0-based indexed rows and UTF-16 units. */
export interface LineNote { line: number; from: number; to: number; note: OutputNote }
export interface ProjectedLines { start: number; lines: string[]; notes: LineNote[] }
export interface LinePart { line: number; text: string; ended: boolean; note?: OutputNote; utf16_length?: number }

/** lines_open rejects with this object; other failures have no code. */
export interface LineOpenError {
  message: string;
  code?: "original_gone" | "output_not_retained" | "source_missing";
  not_run?: boolean;
}

export interface RunFacts {
  machine: MachineName;
  state: RunState;
  job: string | null;
  output: OutputRef | null;
  visited: [MachineName, string][];
  writes: WriteFact[];
  /** What it did through the World, step by step. */
  steps?: RunStep[];
  spans: OutputSpan[];
  /** Raw byte offsets, end exclusive; these are not decoded viewer coordinates. */
  notes?: OutputNoteMatch[];
}

export interface JobFacts {
  call_id: string;
  primary: boolean;
  output: OutputRef | null;
  notes?: OutputNoteMatch[];
  visited: [MachineName, string][];
  machine: MachineName;
  job: string;
  sequence: number;
  state: RunState | null;
  writes: WriteFact[];
}
