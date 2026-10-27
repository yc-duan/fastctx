// What a run did, read from its script, what it printed and the steps the
// World told of it: each command of the script, in order. A command of the
// World's own (a copy, a wait, an `on`) is what its step says it did, taken
// from the steps of the job that ran it; another is what the readers found
// it printed, or named alone where nothing of its own was recognised.
// Commands print in the order they run, so each takes the first of what it
// can print that comes after what the commands before it took. One command
// sent to many machines in a loop is one deed. Nothing recognised at all is
// null: the run is shown in its own words.

import type { OnStep, RunStep } from "../../world/run-facts";
import { rustChecks } from "./diagnostics";
import { printedFile, printsFile, viewedFiles } from "./files";
import { gitCommit, gitLog, gitPush, gitStatus } from "./git";
import { scriptCommands, wordsOf, type ScriptCommand } from "./script";
import { searchHits, searchOf } from "./search";
import { pytestTests, rustTests } from "./tests";
import { goTests, jestTests, vitestTests } from "./runners";
import { eslintChecks, tscChecks } from "./linters";
import { deedsOfStep, onSaid, RunSteps, stepKind, stepsGiven, type StepScope } from "./steps";
import type { Deed, Found, OutputLines, PlainDeed } from "./types";

type Kind = Deed["kind"];

/** How many machines one command goes to before it is one deed. */
export const FAN = 4;

/** What one command can print that is read here. */
function wants(command: ScriptCommand): Kind[] {
  if (command.on !== null) return [];
  let words = wordsOf(command.text);
  const script = packageScript(words);
  if (script !== null) {
    if (/^test(?::|$)/.test(script)) return ["tests"];
    if (/^(?:lint|typecheck|type-check|tsc|check|build)(?::|$)/.test(script)) return ["checks"];
    return [];
  }
  words = runnerWords(words);
  const [first, second] = words;
  if (first === "jest" || first === "vitest") return ["tests"];
  if (first === "go" && second === "test") return ["tests"];
  if (first === "tsc" || first === "vue-tsc" || first === "eslint") return ["checks"];
  if (first === "cargo") {
    if (second === "test" || second === "nextest") return ["checks", "tests"];
    if (second === "build" || second === "check" || second === "clippy" || second === "rustc" || second === "doc") return ["checks"];
    return [];
  }
  if (first === "pytest" || (first === "python" || first === "python3" || first === "py") && words.includes("pytest") || (first === "uv" || first === "poetry") && words.includes("pytest")) return ["tests"];
  if (first === "git") {
    if (second === "commit") return ["commit"];
    if (second === "status") return ["status"];
    if (second === "log") return ["log"];
    if (second === "push") return ["push"];
    if (second === "grep") return ["search"];
    return [];
  }
  if (first === "rg" || first === "grep" || first === "egrep") return ["search"];
  // A `view` shows its text files in the run's output.
  if (command.world && stepKind(command.text) === "view") return ["file"];
  return [];
}

/** What a run of `script` on `machine` did, from `lines` it printed and
 *  the `steps` told of it. */
export function readRun(script: string, machine: string | null, lines: OutputLines, steps: readonly RunStep[] = []): Deed[] | null {
  const commands = scriptCommands(script, machine);
  const told = new RunSteps(steps);
  const carried = (index: number) => commands.some((command) => command.carrier === index);
  // The commands that do work of their own: what an `on` carried, not the
  // `on`; the World's own that tell nothing else.
  const work = commands.filter((command, index) => (command.on !== null ? !carried(index) : !command.world || wants(command).length > 0 || stepKind(command.text) !== null));
  const kinds = new Set(work.flatMap(wants));
  const found: Found[] = [];
  const add = (one: Found | null) => one && found.push(one);
  if (kinds.has("tests")) {
    add(rustTests(lines));
    add(pytestTests(lines));
    add(jestTests(lines));
    add(vitestTests(lines));
    add(goTests(lines));
  }
  if (kinds.has("checks")) {
    const builds = work.some((command) => wants(command).includes("checks") && !wants(command).includes("tests"));
    add(rustChecks(lines, builds));
    add(tscChecks(lines));
    add(eslintChecks(lines));
  }
  if (kinds.has("commit")) add(gitCommit(lines));
  if (kinds.has("status")) add(gitStatus(lines));
  if (kinds.has("log")) add(gitLog(lines));
  if (kinds.has("push")) add(gitPush(lines));
  if (kinds.has("search")) {
    const command = work.find((one) => wants(one).includes("search"));
    const of = command ? searchOf(command.text) : null;
    if (of) add(searchHits(lines, of));
  }
  if (kinds.has("file")) for (const one of viewedFiles(lines, machine) ?? []) found.push(one);
  // A run that only printed one file is read as that file.
  if (work.length === 1 && !found.length && !steps.length) {
    const printed = printsFile(work[0]!.text);
    if (printed) add(printedFile(lines, printed, machine));
  }
  found.sort((a, b) => a.at - b.at);
  const taken = new Set<Found>();
  const deeds: Deed[] = [];
  let after = -1;
  const free = (kind: Kind) => (one: Found) => one.deed.kind === kind && !taken.has(one) && one.at >= after;
  // Where each command's steps are: an `on`'s carried commands in the job
  // it sent, the rest in the run's own.
  const top = told.top();
  const scopes = new Map<number, StepScope>();
  const sent = new Map<number, OnStep | null>();
  const skip = new Set<number>();
  const scopeOf = (command: ScriptCommand): StepScope => (command.carrier === null ? top : (scopes.get(command.carrier) ?? top));
  /** What an `on` told, for a deed of what it carried. */
  const carriedBy = (command: ScriptCommand): Pick<PlainDeed, "machine" | "on"> => {
    if (command.carrier === null) return command.piped ? { on: onSaid(null, true) } : {};
    const carrier = commands[command.carrier]!;
    return { machine: command.machine, on: onSaid(sent.get(command.carrier) ?? null, carrier.piped) };
  };
  let wrote = false;
  commands.forEach((command, index) => {
    if (skip.has(index) || (command.carrier !== null && skip.has(command.carrier))) {
      skip.add(index);
      return;
    }
    const scope = scopeOf(command);
    // An `on`: the step of its job there, and what it carried after it.
    if (command.on !== null) {
      const to = (step: OnStep) => command.on!.startsWith("$") || step.to === command.on;
      const steps = command.loop ? scope.takeRun("on", to) : [scope.take("on", to)].filter((step): step is OnStep => step !== null);
      const said = carriedText(commands, index) ?? command.text;
      if (steps.length >= FAN) {
        deeds.push({ kind: "fan", command: said, steps });
        skip.add(index);
        return;
      }
      if (steps.length > 1) {
        for (const step of steps) deeds.push({ kind: "plain", command: said, machine: step.to, on: onSaid(step, command.piped) });
        skip.add(index);
        return;
      }
      const step = steps[0] ?? null;
      sent.set(index, step);
      if (step?.sent) scopes.set(index, told.within(step.sent));
      if (!carried(index)) deeds.push({ kind: "plain", command: command.text, machine: command.on, on: onSaid(step, command.piped) });
      return;
    }
    // The World's own writes are rows of their own, drawn where the first
    // of them was given.
    if (command.world && WRITES.test(command.text)) {
      if (!wrote) deeds.push({ kind: "writes" });
      wrote = true;
      return;
    }
    const placed = <T extends Deed>(deed: T): T => ({ ...deed, command: command.text, cwd: command.cwd, where: command.machine });
    // A World command: what its steps say it did.
    const kind = command.world ? stepKind(command.text) : null;
    if (kind !== null) {
      const given = command.loop ? scope.takeRun(kind) : Array.from({ length: stepsGiven(command.text) }, () => scope.take(kind)).filter((step): step is RunStep => step !== null);
      const mine: Deed[] = given.flatMap((step) => deedsOfStep(step, command.text, told));
      // A `view` of text files shows them in the run's output.
      if (kind === "view") {
        const files = found.filter(free("file"));
        for (const one of files) taken.add(one);
        if (files.length) after = Math.max(after, files[files.length - 1]!.at);
        mine.push(...files.map((one) => one.deed));
        // A file it looked at that the output does not show is still read.
        for (const step of given) {
          if (step.kind !== "view") continue;
          for (const path of step.paths) {
            if (mine.some((deed) => (deed.kind === "file" || deed.kind === "picture") && samePath(deed.path, path))) continue;
            mine.push({ kind: "file", command: command.text, machine: step.machine, path, rows: [], of: null });
          }
        }
      }
      if (mine.length) deeds.push(...mine.map(placed));
      else deeds.push({ kind: "plain", command: command.text, ...carriedBy(command) });
      return;
    }
    // The World's other commands are named as they were given.
    if (!work.includes(command)) {
      deeds.push({ kind: "plain", command: command.text, ...carriedBy(command) });
      return;
    }
    const kinds = wants(command);
    const mine: Found[] = [];
    // A shell command that printed one file is that file where it is the
    // run's only command.
    if (!kinds.length && work.length === 1) mine.push(...found.filter(free("file")));
    for (const kind of kinds) {
      if (kind === "file") continue;
      const one = found.find(free(kind));
      if (one) mine.push(one);
    }
    for (const one of mine) taken.add(one);
    if (!mine.length) {
      deeds.push({ kind: "plain", command: command.text, ...carriedBy(command) });
      return;
    }
    mine.sort((a, b) => a.at - b.at);
    after = Math.max(after, mine[mine.length - 1]!.at);
    for (const one of mine) deeds.push(placed(one.deed));
  });
  for (const one of found) if (!taken.has(one)) deeds.push(one.deed);
  // What the World told that no command took is still something done.
  for (const step of told.rest()) deeds.push(...deedsOfStep(step, null, told));
  const drawn = deeds.some((deed) => (deed.kind !== "plain" && deed.kind !== "writes") || (deed.kind === "plain" && (deed.machine != null || deed.on?.end)));
  return drawn ? deeds : null;
}

/** Whether two writings of a path name one file: one ends as the other. */
function samePath(a: string, b: string): boolean {
  const [x, y] = [a, b].map((path) => path.replace(/\\/g, "/"));
  return x === y || x!.endsWith(`/${y}`) || y!.endsWith(`/${x}`);
}

/** What an `on` carried, as one line: its one command, or the first of its
 *  script's. */
function carriedText(commands: readonly ScriptCommand[], index: number): string | null {
  return commands.find((command) => command.carrier === index)?.text ?? null;
}

/** The World's commands that write files. */
const WRITES = /^(?:fastctx\s+)?(?:edit|write|apply_patch)\b/;

/** The words of a command that runs a package's tool, without what only
 *  fetches it (`npx`, `pnpm exec`, `yarn dlx`, `bunx`). */
function runnerWords(words: string[]): string[] {
  if (words[0] === "npx" || words[0] === "bunx") return words.slice(1).filter((word, index) => index > 0 || !word.startsWith("-"));
  if ((words[0] === "pnpm" || words[0] === "npm" || words[0] === "yarn" || words[0] === "bun") && (words[1] === "exec" || words[1] === "dlx" || words[1] === "x")) return words.slice(2);
  return words;
}

/** The script a package manager is asked to run (`npm test`, `npm run
 *  lint`, `pnpm build`, `yarn test`); null where it is not one. */
function packageScript(words: string[]): string | null {
  const [manager, verb, name] = words;
  if (manager !== "npm" && manager !== "pnpm" && manager !== "yarn" && manager !== "bun") return null;
  if (verb === "test" || verb === "t") return "test";
  if (verb === "run" || verb === "run-script") return name ?? null;
  if (manager !== "npm" && verb && !verb.startsWith("-") && !["install", "add", "remove", "i", "exec", "dlx", "x", "init", "create"].includes(verb)) return verb;
  return null;
}
