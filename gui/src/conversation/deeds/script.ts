// A run's script as the commands it gives, one after another (`world-gui.md`,
// 对话): lines joined where they go on, `&&`, `||` and `;` parting the
// commands of one line, a pipe kept whole unless an `on` stands in it. An
// `on` is a command of its own that carries what it sends, which follow it
// as commands run on that machine: the command it was given, or the
// here-doc it reads. Commands in the body of a loop may run many times.
// Commands that only set the scene (`cd`, `set -e`) and the shell's own
// words (`if`, `done`) give none.

/** One command of a script, as written. `world` marks the World's own
 *  commands (`on`, `edit`, `view` and their kin), which say what they did as
 *  facts of their own. */
export interface ScriptCommand {
  text: string;
  world: boolean;
  /** The folder the script had gone into when it gave it, where it said. */
  cwd: string | null;
  /** Where it runs: the machine an `on` sent it to, or the script's own. */
  machine: string | null;
  /** Given in the body of a loop. */
  loop: boolean;
  /** The `on` that carried it, by its place among the commands. */
  carrier: number | null;
  /** For an `on`, the machine it sends to; what it carries follows it. */
  on: string | null;
  /** Its input is the output of the stage before it in a pipe. */
  piped: boolean;
}

const WORLD = /^(?:on|fastctx|edit|write|wait|view|copy)\b/;
const SCENE = /^(?:cd|pushd|popd|set|export|unset|shopt|source|\.|trap|local|declare|true|false|:|exit|return|echo|printf|sleep|read|break|continue|shift)(?:\s|$)/;
/** A name given a value and nothing run after it. */
const ASSIGNED = /^[A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]*\])?\+?=(?:'[^']*'|"(?:[^"\\]|\\.)*"|\S)*$/;
const SHELL = /^(?:if|elif|then|else|while|until|do|done|fi|case|esac|in|function|\{|\}|\(|\)|;;)(?:\s|$)/;
/** Words that open a loop's head; its body runs until `done`. */
const LOOP = /^(?:for|while|until|select)\s/;
/** Words of the shell before a command that still runs: `if cmd`, `do cmd`. */
const LEADING = /^(?:if|elif|while|until|then|else|do|!)\s+/;
const HEREDOC = /<<-?\s*(['"]?)([A-Za-z_][A-Za-z0-9_]*)\1/;
/** What a pipe's later stage does to the output only. */
const PLUMBING = /^(?:head|tail|wc|sort|uniq|grep|egrep|rg|less|more|cat|tee|cut|tr|column|sed|awk)(?:\s|$)/;

/** The commands `script` gives, run on `machine`. */
export function scriptCommands(script: string, machine: string | null): ScriptCommand[] {
  const commands: ScriptCommand[] = [];
  read(script.split("\n"), { machine, carrier: null, depth: 0 }, commands, { cwd: null, loops: 0 });
  return commands;
}

/** The commands a person would name: an `on` is named by what it carried,
 *  when it carried anything. */
export function ownCommands(commands: readonly ScriptCommand[]): ScriptCommand[] {
  return commands.filter((command, index) => command.on === null || !commands.some((other) => other.carrier === index));
}

/** The command a run is named by: its first that does work, the World's
 *  own after the others; null where it gives none. */
export function scriptSaid(script: string, machine: string | null): string | null {
  const commands = ownCommands(scriptCommands(script, machine));
  return (commands.find((command) => !command.world) ?? commands[0])?.text ?? null;
}

interface Where {
  machine: string | null;
  carrier: number | null;
  /** How deep in here-docs carried by `on`. */
  depth: number;
}

function read(lines: readonly string[], where: Where, into: ScriptCommand[], at: { cwd: string | null; loops: number }): void {
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
    let body: string[] | null = null;
    if (heredoc) {
      const tag = heredoc[2]!;
      const start = index + 1;
      let end = start;
      while (end < lines.length && lines[end]!.trim() !== tag) end += 1;
      body = lines.slice(start, end);
      index = end;
    }
    for (const part of partsOf(trimmed)) {
      let command = bare(part);
      if (LOOP.test(command)) {
        at.loops += 1;
        // `while cmd` runs cmd each time round; `for x in …` runs nothing.
        if (!/^(?:while|until)\s/.test(command)) continue;
      }
      command = bare(command.replace(LEADING, ""));
      while (LEADING.test(command)) command = bare(command.replace(LEADING, ""));
      if (/^done(?:\s|$)/.test(part.trim())) {
        at.loops = Math.max(0, at.loops - 1);
        continue;
      }
      const went = /^cd\s+(?:'([^']*)'|"([^"]*)"|(\S+))\s*$/.exec(command);
      if (went) at.cwd = went[1] ?? went[2] ?? went[3] ?? null;
      // The shell's own `wait` for its background commands.
      if (!command || SCENE.test(command) || SHELL.test(command) || ASSIGNED.test(command) || /^wait\s*$/.test(command)) continue;
      const stages = stagesOf(command);
      const piping = stages.length > 1 && stages.some((stage) => onOf(stage) !== null);
      (piping ? stages : [command]).forEach((stage, at_) => {
        const piped = piping && at_ > 0;
        // A later stage that only shapes the output is no command of its own.
        if (piped && onOf(stage) === null && PLUMBING.test(stage)) return;
        give(stage, piped, heredoc && at_ === stages.length - 1 ? body : piping ? null : body, where, into, at);
      });
    }
  }
}

/** Adds one command, and what an `on` carries after it. */
function give(text: string, piped: boolean, body: string[] | null, where: Where, into: ScriptCommand[], at: { cwd: string | null; loops: number }): void {
  const sent = onOf(text);
  const loop = at.loops > 0;
  if (sent === null) {
    into.push({ text, world: WORLD.test(text), cwd: at.cwd, machine: where.machine, loop, carrier: where.carrier, on: null, piped });
    return;
  }
  const carrier = into.length;
  into.push({ text, world: true, cwd: at.cwd, machine: where.machine, loop, carrier: where.carrier, on: sent.machine, piped });
  const inner: Where = { machine: sent.machine, carrier, depth: where.depth + 1 };
  // Its one command, as that machine runs it; a here-doc after it is that
  // command's input.
  if (sent.rest && !sent.reads && !sent.rest.startsWith("<")) {
    give(sent.rest, false, null, inner, into, { cwd: null, loops: at.loops });
    return;
  }
  // The script it reads, as that machine runs it.
  if (body && where.depth < 4) read(body, inner, into, { cwd: null, loops: at.loops });
}

/** An `on` and what it sends: its machine, and the command it was given;
 *  `reads` where that command is a shell reading a script. Null where it
 *  is not one. */
function onOf(command: string): { machine: string; rest: string; reads: boolean } | null {
  const match = /^(?:fastctx\s+)?on\s+(?:'([^']*)'|"([^"]*)"|(\S+))(.*)$/.exec(command.trim());
  if (!match) return null;
  const rest = match[4]!.trim();
  return { machine: match[1] ?? match[2] ?? match[3] ?? "", rest, reads: /^(?:bash|sh|zsh)(?:\s+-\S+)*\s*(?:<<|$)/.test(rest) };
}

/** A command without what only surrounds it: leading assignments, `time`,
 *  `timeout N`, `env`, `sudo`, `exec`, `nice`. */
export function bare(command: string): string {
  // A command sent to the background still runs.
  let text = command.trim().replace(/^!\s*/, "").replace(/\s+&$/, "");
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
  return split(line, (text, at) => {
    const two = text.slice(at, at + 2);
    if (two === "&&" || two === "||") return 2;
    return text[at] === ";" ? 1 : 0;
  });
}

/** The stages of a pipe, outside quotes. */
function stagesOf(command: string): string[] {
  return split(command, (text, at) => (text[at] === "|" && text[at + 1] !== "|" && text[at - 1] !== "|" && text[at - 1] !== ">" ? 1 : 0));
}

/** `line` parted where `cut` says, outside quotes and after a comment's
 *  start nothing. */
function split(line: string, cut: (text: string, at: number) => number): string[] {
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
    const width = cut(line, at);
    if (width) {
      parts.push(current);
      current = "";
      at += width - 1;
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
