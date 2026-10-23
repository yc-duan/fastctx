// A run's script as the commands it gives, one after another (`world-gui.md`,
// 对话): lines joined where they go on, `&&`, `||` and `;` parting the
// commands of one line, a pipe kept whole. The words of a here-doc are its
// command's input, not commands, except an `on` to the machine the script
// already runs on, which only wraps the script it carries: what it carries
// is read as the script. Commands that only set the scene (`cd`, `set -e`)
// and the shell's own words (`if`, `done`) give none.

/** One command of a script, as written. `world` marks the World's own
 *  commands (`on`, `edit`, `view` and their kin), which say what they did as
 *  facts of their own. */
export interface ScriptCommand {
  text: string;
  world: boolean;
  /** The folder the script had gone into when it gave it, where it said. */
  cwd: string | null;
}

const WORLD = /^(?:on|fastctx|edit|write|wait|view|copy)\b/;
const SCENE = /^(?:cd|pushd|popd|set|export|unset|shopt|source|\.|trap|local|declare|true|false|:|exit|return|echo|printf|sleep)(?:\s|$)/;
const SHELL = /^(?:for|while|until|do|done|if|then|elif|else|fi|case|esac|in|function|\{|\}|\(|\))(?:\s|$)/;
const HEREDOC = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/;

/** The commands `script` gives, run on `machine`. */
export function scriptCommands(script: string, machine: string | null): ScriptCommand[] {
  const commands: ScriptCommand[] = [];
  read(script.split("\n"), machine, commands, 0, { cwd: null });
  return commands;
}

/** The command a run is named by: its first that does work, the World's
 *  own after the others; null where it gives none. */
export function scriptSaid(script: string, machine: string | null): string | null {
  const commands = scriptCommands(script, machine);
  return (commands.find((command) => !command.world) ?? commands[0])?.text ?? null;
}

function read(lines: readonly string[], machine: string | null, into: ScriptCommand[], depth: number, at: { cwd: string | null }): void {
  for (let index = 0; index < lines.length; index += 1) {
    let line = lines[index]!;
    // A line that goes on: the next is part of it.
    while (/(^|[^\\])\\$/.test(line) && index + 1 < lines.length) {
      line = `${line.slice(0, -1)} ${lines[index + 1]!.trim()}`;
      index += 1;
    }
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const heredoc = HEREDOC.exec(trimmed);
    let body: string[] = [];
    if (heredoc) {
      const tag = heredoc[2]!;
      const start = index + 1;
      let end = start;
      while (end < lines.length && lines[end]!.trim() !== tag) end += 1;
      body = lines.slice(start, end);
      index = end;
    }
    for (const part of partsOf(trimmed)) {
      const command = bare(part);
      const went = /^cd\s+(?:'([^']*)'|"([^"]*)"|(\S+))\s*$/.exec(command);
      if (went) at.cwd = went[1] ?? went[2] ?? went[3] ?? null;
      if (!command || SCENE.test(command) || SHELL.test(command)) continue;
      const carried = onHere(command, machine);
      if (carried !== null) {
        // An `on` to where the script already is carries its script, or
        // its one program, and nothing more.
        if (heredoc && depth < 4) read(body, machine, into, depth + 1, at);
        else if (carried) into.push({ text: carried, world: WORLD.test(carried), cwd: at.cwd });
        continue;
      }
      into.push({ text: command, world: WORLD.test(command), cwd: at.cwd });
    }
  }
}

/** What an `on` to `machine` itself runs: its program and words, "" where
 *  it reads a script from its input; null where it is not one. */
function onHere(command: string, machine: string | null): string | null {
  if (!machine) return null;
  const match = /^on\s+(?:'([^']*)'|"([^"]*)"|(\S+))(.*)$/.exec(command);
  if (!match) return null;
  const named = match[1] ?? match[2] ?? match[3] ?? "";
  if (named !== machine) return null;
  const rest = match[4]!.trim();
  return HEREDOC.test(rest) || rest.startsWith("<") ? "" : rest;
}

/** A command without what only surrounds it: leading assignments, `time`,
 *  `timeout N`, `env`, `sudo`, `exec`, `nice`. */
export function bare(command: string): string {
  let text = command.trim().replace(/^!\s*/, "");
  for (;;) {
    const before = text;
    text = text
      .replace(/^[A-Za-z_][A-Za-z0-9_]*=(?:'[^']*'|"[^"]*"|\S*)\s+/, "")
      .replace(/^(?:time|env|sudo|exec|nice|nohup|command)\s+/, "")
      .replace(/^timeout\s+(?:-\S+\s+)*\d+[smhd]?\s+/, "");
    if (text === before) return text;
  }
}

/** The commands of one line, as `&&`, `||` and `;` part them outside
 *  quotes; a pipe stays inside its command. */
function partsOf(line: string): string[] {
  const parts: string[] = [];
  let quote: string | null = null;
  let current = "";
  for (let at = 0; at < line.length; at += 1) {
    const char = line[at]!;
    if (quote) {
      current += char;
      if (char === quote && line[at - 1] !== "\\") quote = null;
      continue;
    }
    if (char === "'" || char === '"') {
      quote = char;
      current += char;
      continue;
    }
    if (char === "#" && /\s/.test(line[at - 1] ?? " ")) break;
    const two = line.slice(at, at + 2);
    if (two === "&&" || two === "||") {
      parts.push(current);
      current = "";
      at += 1;
      continue;
    }
    if (char === ";") {
      parts.push(current);
      current = "";
      continue;
    }
    current += char;
  }
  parts.push(current);
  return parts.map((part) => part.trim()).filter(Boolean);
}

/** The words of a command, quotes taken off, as far as it says them
 *  plainly; null where a word is not plain (a substitution, a glob). */
export function wordsOf(command: string): string[] {
  const words: string[] = [];
  const pattern = /'([^']*)'|"((?:[^"\\]|\\.)*)"|(\S+)/g;
  for (const match of command.matchAll(pattern)) {
    const word = match[1] ?? match[2]?.replace(/\\(.)/g, "$1") ?? match[3] ?? "";
    if (match[3] && /^(?:\||&|>|<|2>|2>&1|>>|&>)/.test(match[3])) break;
    words.push(word);
  }
  return words;
}
