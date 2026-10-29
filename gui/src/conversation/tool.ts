// How a tool reads in one line: what was done, in the tense of where it
// stands, to what, and how much changed. The verbs follow claude.ai's
// activity list: done is past, running is
// present, failed is its own word. Only what the tool says is shown; a
// count that cannot be read from it is not guessed.

import { scriptCommands, scriptSaid } from "./deeds/script";
import type { Change, Tool } from "../world/events";
import type { WriteFact } from "../world/run-facts";
import type { Act, Tally, TallyPart, Tense, Words } from "../words";
import type { Use } from "./model";
import { counts } from "./diff";
import { writeNote } from "./run-output";
import type { Card } from "./cards/cards";

export type Glyph = "read" | "edit" | "command" | "search" | "image" | "colleague" | "delegate" | "web" | "tasks" | "tool" | "wake";

export interface Headline {
  glyph: Glyph;
  action: string;
  /** What it was done to: a file's base name, a query, a command's words. */
  object: string | null;
  added: number | null;
  removed: number | null;
}

export function tenseOf(ended: { use: Use } | null): Tense {
  if (!ended) return "running";
  return ended.use === "failed" || ended.use === "declined" ? "failed" : "done";
}

export function basename(path: string): string {
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut === -1 ? trimmed : trimmed.slice(cut + 1);
}

function input(tool: Tool, key: string): string | null {
  const value = (tool.input as Record<string, unknown> | null)?.[key];
  return typeof value === "string" && value.trim() ? value : null;
}

function firstLine(text: string): string {
  return text.trim().split("\n")[0] ?? "";
}

/** The files a command only looks at, when it is nothing but one
 *  `fastctx view` of named files: the World's own way to read, so it reads
 *  as reading. Anything more (a pipe, a substitution, a second command, a
 *  range, a pattern) is a command and shown as one. */
export function viewing(command: string): string[] | null {
  const words = simpleWords(command.trim());
  if (!words || words[0] !== "fastctx" || words[1] !== "view") return null;
  const rest = words.slice(2);
  const split = rest.indexOf("--");
  const options = split === -1 ? rest.filter((word) => word.startsWith("-")) : rest.slice(0, split);
  const paths = split === -1 ? rest : rest.slice(split + 1);
  if (options.length || !paths.length || paths.some((path) => /[*?[]/.test(path))) return null;
  return paths;
}

/** A command's words, when it is one simple command: plain and quoted
 *  words, nothing the shell would expand, redirect or join. */
function simpleWords(command: string): string[] | null {
  const words: string[] = [];
  let word: string | null = null;
  for (let index = 0; index < command.length; index += 1) {
    const char = command[index];
    if (char === "'" || char === '"') {
      const end = command.indexOf(char, index + 1);
      if (end === -1) return null;
      const inner = command.slice(index + 1, end);
      if (char === '"' && /[$`\\]/.test(inner)) return null;
      word = (word ?? "") + inner;
      index = end;
    } else if (char === " " || char === "\t") {
      if (word !== null) words.push(word);
      word = null;
    } else if (/[|;&<>()$`\\\n{}~]/.test(char)) {
      return null;
    } else {
      word = (word ?? "") + char;
    }
  }
  if (word !== null) words.push(word);
  return words;
}

/** A file's name as a person knows it: an attachment the World keeps under
 *  a name of its own goes by the name it came with. */
export type Named = (path: string) => string | null;

/** `steady` keeps the verb as it is once done, whatever the tool's state:
 *  a row says how it stands by its mark and its colour, so that its verb
 *  keeps one width. */
export function headline(tool: Tool, ended: { use: Use } | null, words: Words, named: Named = () => null, steady = false): Headline {
  const tense = tenseOf(ended);
  const say = (act: Act) => words.act(act, steady ? "done" : tense);
  const plain = { added: null, removed: null };
  switch (tool.does.as) {
    case "command": {
      const viewed = viewing(tool.does.command);
      if (viewed) {
        return {
          glyph: "read",
          action: say("read"),
          object: viewed.map((path) => named(path) ?? basename(path)).join(words.listSeparator),
          added: null,
          removed: null,
        };
      }
      return {
        glyph: "command",
        action: say("command"),
        // A script is named by its first command that does work, what an
        // `on` to its own machine wraps read as its own.
        object: input(tool, "description") ?? scriptSaid(tool.does.command, tool.machine ?? null) ?? firstLine(tool.does.command),
        added: null,
        removed: null,
      };
    }
    case "edit":
      return edits(tool.does.changes, say);
    case "search":
      return { glyph: "search", action: say("search"), object: tool.does.query, ...plain };
    case "image":
      return {
        glyph: "image",
        action: say("image"),
        object: tool.does.path ? (named(tool.does.path) ?? basename(tool.does.path)) : null,
        ...plain,
      };
    case "colleague":
      // What an older node told of the host's own agents as colleagues.
      if (tool.name === "Agent" || tool.name === "Task" || tool.name === "spawnAgent") {
        return { glyph: "delegate", action: say("delegate"), object: tool.does.task, ...plain };
      }
      return {
        glyph: "colleague",
        action: say(tool.name === "ask_colleague" ? "ask" : "message"),
        object: tool.does.task ?? tool.does.to,
        ...plain,
      };
    case "agent": {
      // Which agents it names is said by the row, which knows them.
      const act: Act = { start: "delegate", message: "message", resume: "resume", wait: "wait", stop: "stop", list: "list" }[tool.does.act] as Act;
      const task = tool.does.task ? firstLine(tool.does.task) : null;
      return { glyph: "delegate", action: say(act), object: tool.does.act === "start" ? (task ?? tool.does.kind) : task, ...plain };
    }
    case "call":
      return call(tool, steady ? "done" : tense, say, words);
  }
}

/** One file a run wrote, as its own row: made, changed or removed, and the
 *  lines it added and took away where they could be counted. */
export function writeHeadline(write: WriteFact, words: Words): Headline {
  const act: Act = write.kind === "added" ? "create" : write.kind === "deleted" ? "delete" : "edit";
  return { glyph: "edit", action: words.act(act, "done"), object: basename(write.path), added: write.added, removed: write.removed };
}

/** Whether a run did nothing but write files, and ended well: its writes'
 *  rows say all it did, so it has no row of its own. */
export function onlyWrites(tool: Tool): boolean {
  const run = tool.run;
  if (tool.does.as !== "command" || !run?.writes.length) return false;
  const state = run.state;
  if (state.kind !== "ended" || !("code" in state.status) || state.status.code !== 0) return false;
  return run.spans.every((span) => writeNote(span.note, run.writes));
}

function edits(changes: Change[], say: (act: Act) => string): Headline {
  let added = 0;
  let removed = 0;
  for (const change of changes) {
    const counted = counts(change.diff);
    added += counted.added;
    removed += counted.removed;
  }
  if (changes.length === 1) {
    const [change] = changes;
    const how = change.how;
    let act: Act = "edit";
    let object = basename(change.path);
    if (typeof how === "object") {
      act = "move";
      object = `${basename(change.path)} → ${basename(how.moved.to)}`;
    } else if (how === "added") act = "create";
    else if (how === "deleted") act = "delete";
    else if (how === "replaced") act = "write";
    return { glyph: "edit", action: say(act), object, added, removed };
  }
  return {
    glyph: "edit",
    action: say("edit"),
    object: changes.length ? `${changes.length} files` : null,
    added: changes.length ? added : null,
    removed: changes.length ? removed : null,
  };
}

function call(tool: Tool, tense: Tense, say: (act: Act) => string, words: Words): Headline {
  const plain = { added: null, removed: null };
  switch (tool.name) {
    case "Read": {
      const path = input(tool, "file_path");
      return { glyph: "read", action: say("read"), object: path ? basename(path) : null, ...plain };
    }
    case "WebFetch":
      return { glyph: "web", action: say("fetch"), object: input(tool, "url"), ...plain };
    case "WebSearch":
      return { glyph: "web", action: say("websearch"), object: input(tool, "query"), ...plain };
    case "TodoWrite":
      return { glyph: "tasks", action: say("tasks"), object: null, ...plain };
    // A watcher's reflex, named by what it looks at.
    case "install_reflex": {
      const probe = tool.input && typeof tool.input === "object" && !Array.isArray(tool.input) ? (tool.input as Record<string, unknown>).probe : null;
      const looks = probe && typeof probe === "object" ? ((probe as Record<string, unknown>).command ?? (probe as Record<string, unknown>).url) : null;
      return { glyph: "wake", action: say("reflex"), object: typeof looks === "string" ? firstLine(looks) : input(tool, "id"), ...plain };
    }
    default: {
      // Another server's tool keeps the name it acts by.
      const name = tool.server ? `${tool.server} ${tool.name}` : tool.name;
      return { glyph: "tool", action: tense === "failed" ? words.failedTool(name) : name, object: null, ...plain };
    }
  }
}

/** A group's heading, in its parts: while something runs, what it is
 *  doing; once done, what the group did, by kind. */
export function groupHeading<T extends { tool: Tool; ended: { use: Use } | null }>(
  tools: T[],
  thinking: number,
  words: Words,
  cards?: (entry: T) => Card[],
): TallyPart[] {
  const running = tools.find((entry) => entry.ended === null);
  if (running) return [{ said: headline(running.tool, null, words).action, kind: "done" }];
  const tally: Tally = { read: 0, edited: 0, ran: 0, copied: 0, searched: 0, asked: 0, delegated: 0, coordinated: 0, other: 0, failed: 0, thinking };
  // A file changed more than once is one file changed.
  const changed = new Set<string>();
  for (const entry of tools) {
    const { tool, ended } = entry;
    if (ended && (ended.use === "failed" || ended.use === "declined")) tally.failed += 1;
    // A run counts as its rows show it, where they are known.
    const rows = tool.does.as === "command" ? cards?.(entry) : undefined;
    if (rows) {
      for (const card of rows) counted(card, tally, changed);
      continue;
    }
    switch (tool.does.as) {
      case "command": {
        // Each file a run wrote counts as a file changed; a run that did
        // nothing else is not also a command run.
        for (const write of tool.run?.writes ?? []) changed.add(`${write.machine}|${write.path}`);
        // A run that did things through the World counts each as its row
        // shows it: each command it sent, each machine one went to, each
        // file copied or looked at, and what it did on its own machine.
        const steps = tool.run?.steps ?? [];
        if (steps.length) {
          // An `on` that only carried the World's own commands counts as
          // what they did.
          const carriers = new Set(steps.map((step) => step.job));
          const own = scriptCommands(tool.does.command, tool.machine ?? null).some((command) => command.carrier === null && command.on === null && !command.world);
          if (own) tally.ran += 1;
          for (const step of steps) {
            if (step.kind === "on" && !(step.sent && carriers.has(step.sent)) && !/^(?:fastctx\s+)?(?:edit|write|apply_patch)\b/.test(step.command)) tally.ran += 1;
            else if (step.kind === "copy") tally.copied += step.files;
            else if (step.kind === "view") tally.read += step.paths.length;
          }
          break;
        }
        if (onlyWrites(tool)) break;
        const viewed = viewing(tool.does.command);
        if (viewed) tally.read += viewed.length;
        else tally.ran += 1;
        break;
      }
      case "edit":
        for (const change of tool.does.changes) changed.add(`${tool.machine ?? ""}|${change.path}`);
        break;
      case "search":
        tally.searched += 1;
        break;
      case "colleague":
        if (tool.name === "Agent" || tool.name === "Task" || tool.name === "spawnAgent") tally.delegated += 1;
        else tally.asked += 1;
        break;
      case "agent":
        if (tool.does.act === "start") tally.delegated += 1;
        else tally.coordinated += 1;
        break;
      default:
        if (tool.name === "Read") tally.read += 1;
        else tally.other += 1;
    }
  }
  tally.edited = changed.size;
  return words.tally(tally);
}

/** What one row of a run adds to a heading's count. */
function counted(card: Card, tally: Tally, changed: Set<string>): void {
  switch (card.kind) {
    case "change":
      changed.add(`${card.machine ?? ""}|${card.path}`);
      break;
    case "read":
      tally.read += 1;
      break;
    case "picture":
      tally.read += card.deeds.length;
      break;
    case "search":
      tally.searched += 1;
      break;
    case "copy":
      tally.copied += card.deed?.files ?? card.from.length;
      break;
    case "command":
      tally.ran += card.commands?.length ?? 1;
      break;
    case "fan":
      tally.ran += card.deed.steps.length;
      break;
    case "web":
      tally.searched += 1;
      break;
    case "tool":
    case "image":
    case "tasks":
      tally.other += 1;
      break;
    default:
      tally.ran += 1;
  }
}
