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
}

/** What a run did through the World's own commands, step by step
 *  (`fastctx_types::run_facts::RunStep`): told from inside the job whose
 *  script gave each command, by the node carrying an `on` or by the
 *  command itself. A step told again under its id replaces the earlier
 *  telling; one that ended says more than one going on. */
export type RunStep = StepBase & StepDid;

interface StepBase {
  id: string;
  /** The job whose script gave the command: the run's own, or that of an
   *  `on` it sent elsewhere. */
  job: string;
  /** The machine that job runs on. */
  machine: MachineName;
  began: string;
  /** Unset while it goes on. */
  ended?: string;
}

export type StepDid =
  | {
      kind: "on";
      to: MachineName;
      /** The command line as it was given, or the script it carried. */
      command: string;
      /** The job it ran as there. */
      sent?: string;
      end?: StepEnd;
      /** The machine of the `on` its output is piped into. */
      pipes_to?: MachineName;
      /** The job whose output came straight in as its input. */
      fed_by?: string;
      /** Its last lines, at most twenty. */
      tail?: string;
    }
  | {
      kind: "copy";
      from: StepPlace[];
      to: StepPlace;
      files: number;
      bytes: number;
      /** Files there already with the same contents. */
      unchanged: number;
      /** What was not copied, and why, as the copy said it. */
      problems?: string[];
    }
  /** Files looked at on the step's machine; its pictures in the order of
   *  the images in the call's output. */
  | { kind: "view"; paths: string[]; pictures?: StepPicture[] }
  | { kind: "wait"; jobs: WaitedJob[] }
  /** A job, a colleague or a sentinel stopped. */
  | { kind: "stop"; what: string; on?: MachineName; how: StopHow }
  | { kind: "page"; name: string; town: string; title: string; version: number }
  | { kind: "project"; town: string }
  | { kind: "plan"; version: number }
  | { kind: "goal"; mark: "complete" | "blocked" | "pause" }
  | { kind: "sense"; sentinel: string; condition: string };

export type OnStep = Extract<RunStep, { kind: "on" }>;

export interface StepPlace {
  machine: MachineName;
  path: string;
}

export interface StepPicture {
  path: string;
  /** The page of a document drawn as a picture. */
  page?: number;
  width: number;
  height: number;
  mime: string;
}

export interface WaitedJob {
  job: string;
  machine: MachineName;
  /** When it ended. */
  at: string;
  end: StepEnd;
  tail?: string;
  /** The script it ran, its beginning where it is long. */
  command?: string;
}

/** How what a step started or waited for ended: an exit code, a signal as
 *  128 and its number. */
export type StepEnd = { kind: "exited"; code: number } | { kind: "stopped" } | { kind: "lost"; why: string } | { kind: "not_run"; why: string };

export type StopHow = "stopped" | "already_stopped" | "had_ended" | "not_cleanly" | "still_running";

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
  steps?: RunStep[];
}

/** `incoming` joined to `steps` by id: a step that ended is kept over the
 *  same step going on, a new one goes after those known. */
export function mergeSteps(steps: readonly RunStep[], incoming: readonly RunStep[]): RunStep[] {
  const merged = [...steps];
  for (const step of incoming) {
    const at = merged.findIndex((known) => known.id === step.id);
    if (at < 0) merged.push(step);
    else if (step.ended !== undefined || merged[at]!.ended === undefined) merged[at] = step;
  }
  return merged;
}
