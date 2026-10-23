// What a run did, read from its script and what it printed: each command
// of the script, in order, with what the readers found it printed, the
// command named alone where nothing of its own was recognised. Commands
// print in the order they run, so each takes the first of what it can
// print that comes after what the commands before it took. Nothing
// recognised at all is null: the run is shown in its own words.

import { rustChecks } from "./diagnostics";
import { printedFile, printsFile, viewedFiles } from "./files";
import { gitCommit, gitLog, gitPush, gitStatus } from "./git";
import { scriptCommands, wordsOf, type ScriptCommand } from "./script";
import { searchHits, searchOf } from "./search";
import { pytestTests, rustTests } from "./tests";
import { goTests, jestTests, vitestTests } from "./runners";
import { eslintChecks, tscChecks } from "./linters";
import { copies, jobEnds, pages, pictures, projects, stops, worldAct } from "./world";
import type { Deed, Found, OutputLines } from "./types";

type Kind = Deed["kind"];

/** What one command can print that is read here. */
function wants(command: ScriptCommand): Kind[] {
  let words = wordsOf(command.text);
  // A program sent to another machine prints there what it prints here.
  if (words[0] === "on" && words.length > 2) words = words.slice(2);
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
  if (command.world) {
    const [own, sub] = first === "fastctx" ? [second, words[2]] : [first, second];
    if (own === "view") return ["file", "picture"];
    if (own === "copy") return ["copy"];
    if (own === "wait") return ["job"];
    if (own === "stop") return ["stop"];
    if (own === "page" && sub === "publish") return ["page"];
    if (own === "project" && sub === "enter") return ["project"];
  }
  return [];
}

/** What a run of `script` on `machine` did, from `lines` it printed. */
export function readRun(script: string, machine: string | null, lines: OutputLines): Deed[] | null {
  const commands = scriptCommands(script, machine);
  const work = commands.filter((command) => !command.world || wants(command).length);
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
  if (kinds.has("picture")) found.push(...pictures(lines, machine));
  if (kinds.has("copy")) found.push(...copies(lines));
  if (kinds.has("job")) found.push(...jobEnds(lines));
  if (kinds.has("stop")) found.push(...stops(lines));
  if (kinds.has("page")) found.push(...pages(lines));
  if (kinds.has("project")) found.push(...projects(lines));
  // A run that only printed one file is read as that file.
  if (work.length === 1 && !found.length) {
    const printed = printsFile(work[0]!.text);
    if (printed) add(printedFile(lines, printed, machine));
  }
  const settled = commands.some((command) => worldAct(command.text));
  if (!found.length && !settled) return null;
  found.sort((a, b) => a.at - b.at);
  const taken = new Set<Found>();
  const deeds: Deed[] = [];
  let after = -1;
  const free = (kind: Kind) => (one: Found) => one.deed.kind === kind && !taken.has(one) && one.at >= after;
  let wrote = false;
  for (const command of commands) {
    // The World's own writes are rows of their own, drawn where the first
    // of them was given.
    if (command.world && WRITES.test(command.text)) {
      if (!wrote) deeds.push({ kind: "writes" });
      wrote = true;
      continue;
    }
    // The plan written, a goal settled, a sentinel set: the command says.
    const act = worldAct(command.text);
    if (act) {
      deeds.push(act);
      continue;
    }
    // The World's other commands (an `on` elsewhere, a wait for nothing
    // read) are named as they were given.
    if (!work.includes(command)) {
      deeds.push({ kind: "plain", command: command.text });
      continue;
    }
    const kinds = wants(command);
    const mine: Found[] = [];
    // A `view` shows each of its files; a shell command that printed one
    // is that file where it is the run's only command.
    if (kinds.includes("file") || (!kinds.length && work.length === 1)) mine.push(...found.filter(free("file")));
    // What a command prints of these kinds, it prints all of.
    for (const kind of ["picture", "copy", "job", "stop"] as const) if (kinds.includes(kind)) mine.push(...found.filter(free(kind)));
    for (const kind of kinds) {
      if (kind === "file" || kind === "picture" || kind === "copy" || kind === "job" || kind === "stop") continue;
      const one = found.find(free(kind));
      if (one) mine.push(one);
    }
    for (const one of mine) taken.add(one);
    if (!mine.length) {
      deeds.push({ kind: "plain", command: command.text });
      continue;
    }
    mine.sort((a, b) => a.at - b.at);
    after = Math.max(after, mine[mine.length - 1]!.at);
    for (const one of mine) deeds.push({ ...one.deed, command: command.text, cwd: command.cwd } as Deed);
  }
  for (const one of found) if (!taken.has(one)) deeds.push(one.deed);
  return deeds.some((deed) => deed.kind !== "plain" && deed.kind !== "writes") ? deeds : null;
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
