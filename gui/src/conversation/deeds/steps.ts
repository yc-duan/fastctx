// What the World's own commands told of a run, as the things it did
// (`world/run-facts.ts`, `RunStep`): each step is taken by the command of
// the script that gave it, in the order they began, within the job that
// ran it. A step no command takes is still a thing done, drawn after them.

import type { OnStep, RunStep } from "../../world/run-facts";
import { wordsOf } from "./script";
import type { CopyDeed, Deed, JobDeed, OnSaid, PageDeed, PictureDeed, PlainDeed, ProjectDeed, StopDeed, WorldDeed } from "./types";

type Kind = RunStep["kind"];

/** The steps of one job: a run's own, or those of an `on` it sent. */
export class StepScope {
  private at = 0;

  constructor(
    private readonly steps: readonly RunStep[],
    private readonly used: Set<RunStep>,
  ) {}

  /** The next step of `kind` not yet taken, at or after the last one
   *  taken. */
  take<K extends Kind>(kind: K, test: (step: Extract<RunStep, { kind: K }>) => boolean = () => true): Extract<RunStep, { kind: K }> | null {
    for (let index = this.at; index < this.steps.length; index += 1) {
      const step = this.steps[index]!;
      if (this.used.has(step) || step.kind !== kind || !test(step as Extract<RunStep, { kind: K }>)) continue;
      this.used.add(step);
      this.at = index + 1;
      return step as Extract<RunStep, { kind: K }>;
    }
    return null;
  }

  /** The steps of `kind` a command in a loop took: the next, and those
   *  right after it of the same kind. */
  takeRun<K extends Kind>(kind: K, test: (step: Extract<RunStep, { kind: K }>) => boolean = () => true): Extract<RunStep, { kind: K }>[] {
    const first = this.take(kind, test);
    if (!first) return [];
    const run = [first];
    for (;;) {
      const next = this.steps[this.at];
      if (!next || this.used.has(next) || next.kind !== kind || !test(next as Extract<RunStep, { kind: K }>)) return run;
      this.used.add(next);
      this.at += 1;
      run.push(next as Extract<RunStep, { kind: K }>);
    }
  }
}

/** A run's steps: the run's own job's, and each `on`'s. */
export class RunSteps {
  private readonly used = new Set<RunStep>();
  private readonly ordered: RunStep[];
  private readonly sent = new Set<string>();
  /** Where each view's pictures begin among the call's. */
  private readonly offsets = new Map<RunStep, number>();

  constructor(steps: readonly RunStep[]) {
    this.ordered = steps.map((step, index) => ({ step, index })).sort((a, b) => Date.parse(a.step.began) - Date.parse(b.step.began) || a.index - b.index).map((one) => one.step);
    for (const step of this.ordered) if (step.kind === "on" && step.sent) this.sent.add(step.sent);
    let pictures = 0;
    for (const step of this.ordered) {
      if (step.kind !== "view") continue;
      this.offsets.set(step, pictures);
      pictures += step.pictures?.length ?? 0;
    }
  }

  /** The steps of the run's own script. */
  top(): StepScope {
    return new StepScope(
      this.ordered.filter((step) => !this.sent.has(step.job)),
      this.used,
    );
  }

  /** The steps of the script an `on` sent as `job`. */
  within(job: string): StepScope {
    return new StepScope(
      this.ordered.filter((step) => step.job === job),
      this.used,
    );
  }

  /** The steps no command took, in the order they began. */
  rest(): RunStep[] {
    return this.ordered.filter((step) => !this.used.has(step));
  }

  pictureOffset(step: RunStep): number {
    return this.offsets.get(step) ?? 0;
  }
}

/** The kind of step a World command gives; null for one that gives none. */
export function stepKind(text: string): Kind | null {
  let words = wordsOf(text);
  if (words[0] === "fastctx") words = words.slice(1);
  const [first, second] = words;
  switch (first) {
    case "copy":
    case "view":
    case "stop":
      return first;
    // Bash's own `wait` waits for the script's background commands.
    case "wait":
      return words.some((word) => /^j-/.test(word)) ? "wait" : null;
    case "page":
      return second === "publish" ? "page" : null;
    case "project":
      return second === "enter" ? "project" : null;
    case "plan":
      return words.includes("--file") ? "plan" : null;
    case "goal":
      return second === "complete" || second === "blocked" || (second === "pause" && words.length === 2) ? "goal" : null;
    case "sense":
      return words.includes("--help") || words.includes("-h") ? null : "sense";
    default:
      return null;
  }
}

/** How many steps of its kind one command gives: a stop, one for each
 *  thing it names. */
export function stepsGiven(text: string): number {
  let words = wordsOf(text);
  if (words[0] === "fastctx") words = words.slice(1);
  if (words[0] !== "stop") return 1;
  let count = 0;
  for (let index = 1; index < words.length; index += 1) {
    const word = words[index]!;
    if (word === "--as" || word === "--hub" || word === "--machine") index += 1;
    else if (!word.startsWith("-")) count += 1;
  }
  return Math.max(1, count);
}

/** What an `on` told of how it went. */
export function onSaid(step: OnStep | null, piped: boolean): OnSaid {
  return { end: step?.end ?? null, piped: piped || !!step?.fed_by, tail: step?.tail ?? null, going: !!step && step.ended === undefined };
}

/** The things a step did, as deeds; `command` is what gave it. */
export function deedsOfStep(step: RunStep, command: string | null, steps: RunSteps): Deed[] {
  switch (step.kind) {
    case "on": {
      const plain: PlainDeed = { kind: "plain", command: command ?? step.command.split("\n")[0]!, machine: step.to, on: onSaid(step, false) };
      return [plain];
    }
    case "copy": {
      const deed: CopyDeed = { kind: "copy", command, from: step.from, to: step.to, files: step.files, bytes: step.bytes, same: step.unchanged, notCopied: step.problems ?? [] };
      return [deed];
    }
    case "view": {
      const offset = steps.pictureOffset(step);
      return (step.pictures ?? []).map((picture, index): PictureDeed => ({
        kind: "picture",
        command,
        path: picture.path,
        machine: step.machine,
        width: picture.width,
        height: picture.height,
        mime: picture.mime,
        page: picture.page ?? null,
        index: offset + index,
      }));
    }
    case "wait":
      return step.jobs.map((job): JobDeed => ({
        kind: "job",
        command,
        job: job.job,
        machine: job.machine,
        end: job.end,
        failed: !(job.end.kind === "exited" && job.end.code === 0),
        at: job.at,
        waited: step.ended ?? null,
        script: job.command ?? null,
        tail: job.tail ? job.tail.split("\n") : [],
      }));
    case "stop": {
      const deed: StopDeed = { kind: "stop", command, what: step.what, machine: step.on ?? null, how: step.how, stopped: step.how !== "not_cleanly" && step.how !== "still_running" };
      return [deed];
    }
    case "page": {
      const deed: PageDeed = { kind: "page", command, name: step.name, title: step.title, machine: step.machine, version: step.version };
      return [deed];
    }
    case "project": {
      const deed: ProjectDeed = { kind: "project", command, town: step.town };
      return [deed];
    }
    case "plan": {
      const deed: WorldDeed = { kind: "world", command, act: "plan", said: null };
      return [deed];
    }
    case "goal": {
      const deed: WorldDeed = { kind: "world", command, act: step.mark === "complete" ? "goal-complete" : step.mark === "blocked" ? "goal-blocked" : "goal-pause", said: null };
      return [deed];
    }
    case "sense": {
      const deed: WorldDeed = { kind: "world", command, act: "sense", said: step.condition };
      return [deed];
    }
  }
}
