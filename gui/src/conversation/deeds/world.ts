// What the World's own commands said they did, read from the lines they
// print (`world-run.md`, `world-view.md`): a copy between machines and how
// much came across, a job that ended and how, a job stopped, a page
// published, a project entered, a picture looked at (an image, a page of a
// PDF drawn). The plan written, a goal settled and a sentinel set are known
// from the command itself.

import { wordsOf } from "./script";
import type { CopyDeed, Deed, Found, JobDeed, OutputLines, PageDeed, PictureDeed, ProjectDeed, StopDeed, WorldDeed } from "./types";

const COPY = /^=== copy (\S+?):(.+?) -> (\S+?):(.+?) \((.*)\) ===$/;
const NOT_COPIED = /^not copied: (.*)$/;
const ENDED = /^=== (j-[0-9a-z]+) on (\S+) ended \((.+?)\)(.*?); its output is in (\S+?)(?:;.*)? ===$/;
const LAST_LINES = /^=== its last lines ===$/;
const STOP = /^=== stop (j-[0-9a-z]+) on (\S+) \((.+)\) ===$/;
const PAGE = /^=== page (\S+) · (.+?) on (\S+), version (\d+), HTML written to (.+) ===$/;
const PROJECT = /^=== you work in the project (.+) from now on ===$/;
const PICTURE = /^=== (.+) \(image (\d+)x(\d+), ([^)]+)\) ===$/;
const PDF_PAGE = /^page (\d+)$/;

/** Copies, each as its heading and what it could not copy said them. */
export function copies(lines: OutputLines): Found<CopyDeed>[] {
  const found: Found<CopyDeed>[] = [];
  let open: CopyDeed | null = null;
  lines.forEach((line, index) => {
    if (line === null || line === undefined) return;
    const head = COPY.exec(line);
    if (head) {
      const said = head[5]!;
      const number = (pattern: RegExp) => Number(pattern.exec(said)?.[1] ?? 0);
      open = {
        kind: "copy",
        command: null,
        from: { machine: head[1]!, path: head[2]! },
        to: { machine: head[3]!, path: head[4]! },
        files: /(\d+) files?/.test(said) ? number(/(\d+) files?/) : null,
        size: /files?, ([^;]+)/.exec(said)?.[1]?.trim() ?? null,
        same: number(/(\d+) already the same/),
        notCopied: [],
      };
      found.push({ at: index, deed: open });
      return;
    }
    const missed = NOT_COPIED.exec(line);
    if (missed && open) open.notCopied.push(missed[1]!);
  });
  return found;
}

/** Jobs that ended while they were waited for, with the last lines they
 *  wrote where those followed. */
export function jobEnds(lines: OutputLines): Found<JobDeed>[] {
  const found: Found<JobDeed>[] = [];
  let open: JobDeed | null = null;
  let tail = false;
  lines.forEach((line, index) => {
    if (line === null || line === undefined) return;
    const ended = ENDED.exec(line);
    if (ended) {
      const how = ended[3]!;
      open = { kind: "job", command: null, job: ended[1]!, machine: ended[2]!, how, failed: !/^exit 0$/.test(how), ago: ended[4]!.trim(), log: ended[5]!, tail: [] };
      found.push({ at: index, deed: open });
      tail = false;
      return;
    }
    if (open && LAST_LINES.test(line)) {
      tail = true;
      return;
    }
    if (line.startsWith("=== ")) {
      tail = false;
      return;
    }
    if (open && tail) open.tail.push(line);
  });
  return found;
}

export function stops(lines: OutputLines): Found<StopDeed>[] {
  return each(lines, STOP, (match) => ({ kind: "stop", command: null, job: match[1]!, machine: match[2]!, how: match[3]!, stopped: /^(?:stopped|already stopped)$/.test(match[3]!) }));
}

export function pages(lines: OutputLines): Found<PageDeed>[] {
  return each(lines, PAGE, (match) => ({ kind: "page", command: null, name: match[1]!, title: match[2]!, machine: match[3]!, version: Number(match[4]) }));
}

export function projects(lines: OutputLines): Found<ProjectDeed>[] {
  return each(lines, PROJECT, (match) => ({ kind: "project", command: null, town: match[1]! }));
}

/** Pictures a `view` gave, in the order they came: an image, or a page of
 *  a PDF drawn as one. */
export function pictures(lines: OutputLines, machine: string | null): Found<PictureDeed>[] {
  let index = 0;
  let file: string | null = null;
  const found: Found<PictureDeed>[] = [];
  lines.forEach((line, at) => {
    if (line === null || line === undefined) return;
    const picture = PICTURE.exec(line);
    if (!picture) {
      // A PDF's own heading, before the pages drawn under it.
      const heading = /^=== (.+?) \((?!image).*\) ===$/.exec(line);
      if (heading && !/^(?:view|page \d+|lines )/.test(heading[1]!)) file = heading[1]!;
      return;
    }
    const page = PDF_PAGE.exec(picture[1]!);
    const { at: where, path } = placeOf(page ? (file ?? "") : picture[1]!);
    found.push({ at, deed: { kind: "picture", command: null, path, machine: where ?? machine, width: Number(picture[2]), height: Number(picture[3]), mime: picture[4]!.split(",")[0]!.trim(), page: page ? Number(page[1]) : null, index: index++ } });
  });
  return found;
}

/** What the World's commands that settle things did, known from the
 *  command itself: the plan written, a goal settled, a sentinel set. */
export function worldAct(command: string): WorldDeed | null {
  let words = wordsOf(command);
  if (words[0] === "on" && words.length > 2) words = words.slice(2);
  if (words[0] === "fastctx") words = words.slice(1);
  const [first, second] = words;
  if (first === "plan" && words.includes("--file")) return { kind: "world", command, act: "plan", said: null };
  if (first === "goal" && (second === "complete" || second === "blocked" || second === "pause")) return { kind: "world", command, act: second === "complete" ? "goal-complete" : second === "blocked" ? "goal-blocked" : "goal-pause", said: null };
  if (first === "sense") return { kind: "world", command, act: "sense", said: words.slice(1).find((word) => !word.startsWith("-")) ?? null };
  return null;
}

function each<T extends Deed>(lines: OutputLines, pattern: RegExp, make: (match: RegExpExecArray) => T): Found<T>[] {
  const found: Found<T>[] = [];
  lines.forEach((line, index) => {
    const match = line ? pattern.exec(line) : null;
    if (match) found.push({ at: index, deed: make(match) });
  });
  return found;
}

/** A heading's machine and path: `lab:/srv/x.png`, `E:/x.png`, `/srv/x.png`. */
function placeOf(said: string): { at: string | null; path: string } {
  const match = /^([^:/\\]+):((?:[A-Za-z]:)?[\\/].*)$/.exec(said);
  return match ? { at: match[1]!, path: match[2]! } : { at: null, path: said };
}
