// How a tool reads in one line: what was done, in the tense of where it
// stands, to what, and how much changed. The verbs follow claude.ai's
// activity list as incipit settled them: done is past, running is
// present, failed is its own word. Only what the tool says is shown; a
// count that cannot be read from it is not guessed.

import type { Change, Tool } from "../world/events";
import type { Act, Tally, Tense, Words } from "../words";
import type { Use } from "./model";
import { counts } from "./diff";

export type Glyph = "read" | "edit" | "command" | "search" | "image" | "colleague" | "web" | "tasks" | "tool";

export interface Headline {
  glyph: Glyph;
  action: string;
  /** What it was done to: a file's base name, a query, a command's words. */
  object: string | null;
  /** The whole of `object` where it was shortened: a full path, a command. */
  title: string | null;
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
function viewing(command: string): string[] | null {
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

export function headline(tool: Tool, ended: { use: Use } | null, words: Words, named: Named = () => null): Headline {
  const tense = tenseOf(ended);
  const say = (act: Act) => words.act(act, tense);
  const plain = { title: null, added: null, removed: null };
  switch (tool.does.as) {
    case "command": {
      const viewed = viewing(tool.does.command);
      if (viewed) {
        return {
          glyph: "read",
          action: say("read"),
          object: viewed.map((path) => named(path) ?? basename(path)).join(words.listSeparator),
          title: tool.does.command,
          added: null,
          removed: null,
        };
      }
      return {
        glyph: "command",
        action: say("command"),
        object: input(tool, "description") ?? firstLine(tool.does.command),
        title: tool.does.command,
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
        title: tool.does.path,
      };
    case "colleague":
      return {
        glyph: "colleague",
        action: say(tool.name === "ask_colleague" ? "ask" : "message"),
        object: tool.does.task ?? tool.does.to,
        ...plain,
      };
    case "call":
      return call(tool, tense, say, words);
  }
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
    return { glyph: "edit", action: say(act), object, title: change.path, added, removed };
  }
  return {
    glyph: "edit",
    action: say("edit"),
    object: changes.length ? `${changes.length} files` : null,
    title: changes.map((change) => change.path).join("\n") || null,
    added: changes.length ? added : null,
    removed: changes.length ? removed : null,
  };
}

function call(tool: Tool, tense: Tense, say: (act: Act) => string, words: Words): Headline {
  const plain = { title: null, added: null, removed: null };
  switch (tool.name) {
    case "Read": {
      const path = input(tool, "file_path");
      return { glyph: "read", action: say("read"), object: path ? basename(path) : null, ...plain, title: path };
    }
    case "WebFetch":
      return { glyph: "web", action: say("fetch"), object: input(tool, "url"), ...plain };
    case "WebSearch":
      return { glyph: "web", action: say("websearch"), object: input(tool, "query"), ...plain };
    case "TodoWrite":
      return { glyph: "tasks", action: say("tasks"), object: null, ...plain };
    default: {
      // Another server's tool keeps the name it acts by.
      const name = tool.server ? `${tool.server} · ${tool.name}` : tool.name;
      return { glyph: "tool", action: tense === "failed" ? words.failedTool(name) : name, object: null, ...plain };
    }
  }
}

/** A group's heading: while something runs, what it is doing; once done,
 *  what the group did, by kind. */
export function groupHeading(
  tools: { tool: Tool; ended: { use: Use } | null }[],
  thinking: number,
  words: Words,
): string {
  const running = tools.find((entry) => entry.ended === null);
  if (running) return headline(running.tool, null, words).action;
  const tally: Tally = { read: 0, edited: 0, ran: 0, searched: 0, asked: 0, other: 0, failed: 0, thinking };
  for (const { tool, ended } of tools) {
    if (ended && (ended.use === "failed" || ended.use === "declined")) tally.failed += 1;
    switch (tool.does.as) {
      case "command": {
        const viewed = viewing(tool.does.command);
        if (viewed) tally.read += viewed.length;
        else tally.ran += 1;
        break;
      }
      case "edit":
        tally.edited += Math.max(1, tool.does.changes.length);
        break;
      case "search":
        tally.searched += 1;
        break;
      case "colleague":
        tally.asked += 1;
        break;
      default:
        if (tool.name === "Read") tally.read += 1;
        else tally.other += 1;
    }
  }
  return words.tally(tally);
}
