// The few blocks a plan draws as pictures, read from the lines an agent
// writes in them. Each reader is forgiving: a line it cannot read is kept
// as plain words rather than refused, since nobody is there to fix it.

export interface FlowNode {
  key: string;
  label: string;
  /** Said about the step on a line of its own: `step: note`. */
  note: string;
  /** Marked with a leading `*`: where the work stands, or what matters. */
  strong: boolean;
}

export interface FlowEdge {
  from: string;
  to: string;
  label: string;
  /** Written `-->`: a way not yet taken, or only maybe. */
  dashed: boolean;
}

export interface Flow {
  nodes: FlowNode[];
  edges: FlowEdge[];
}

/** `A -> B -> C`, `A -> B: label`, `A --> B`, a lone `A`, or `A: note`
 *  about a step. A step written once with a bracketed aside after it and
 *  once without is the same step. */
export function readFlow(text: string): Flow {
  const nodes = new Map<string, FlowNode>();
  const edges: FlowEdge[] = [];
  let previous: string | null = null;
  const node = (raw: string, note = ""): string | null => {
    let label = raw.trim();
    // Progress is a checklist's (`world-plan.md`): an older plan's `*` before
    // a step is read past, and the step drawn as any other.
    if (label.startsWith("*")) label = label.slice(1).trim();
    const strong = false;
    label = label.replace(/^\[(.*)\]$/, "$1").replace(/\s+/g, " ");
    if (!label) return null;
    const key = label.replace(/\s*[（(][^（）()]*[）)]$/, "") || label;
    const known = nodes.get(key);
    if (known) {
      known.strong ||= strong;
      if (label.length > known.label.length) known.label = label;
      if (note) known.note = note;
    } else nodes.set(key, { key, label, strong, note });
    return key;
  };
  for (const line of text.split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("%%") || trimmed.startsWith("//")) continue;
    const parts = trimmed.split(/\s*(-->|->|→)\s*/);
    if (parts.length === 1) {
      const [name, note] = splitLabel(trimmed);
      node(name, note);
      continue;
    }
    const last = parts.length - 1;
    const [tail, label] = splitLabel(parts[last]!);
    parts[last] = tail;
    // A line that goes on from where the line before it ended may name
    // that step shorter or longer: it is the same step.
    const first = parts[0]!.trim().replace(/^\*/, "").trim();
    if (previous && !nodes.has(first) && (previous.includes(first) || first.includes(previous))) {
      parts[0] = parts[0]!.trim().startsWith("*") ? `*${previous}` : previous;
    }
    for (let index = 0; index + 2 < parts.length; index += 2) {
      const from = node(parts[index]!);
      const to = node(parts[index + 2]!);
      if (!from || !to) continue;
      edges.push({ from, to, label: index + 2 === last ? label : "", dashed: parts[index + 1] === "-->" });
      previous = to;
    }
  }
  return { nodes: [...nodes.values()], edges };
}

/** The label after the last node of a line: an ASCII colon outside any
 *  quotes or brackets. A full-width colon is part of a name, as in
 *  「后端：存储与接口」. */
function splitLabel(segment: string): [string, string] {
  const closers: Record<string, string> = { "「": "」", "『": "』", "“": "”", '"': '"', "（": "）", "(": ")", "《": "》" };
  const open: string[] = [];
  for (let index = 0; index < segment.length; index += 1) {
    const char = segment[index]!;
    if (open.length && char === open[open.length - 1]) {
      open.pop();
      continue;
    }
    if (closers[char]) {
      open.push(closers[char]!);
      continue;
    }
    if (!open.length && char === ":" && (index + 1 === segment.length || /\s/.test(segment[index + 1]!))) {
      return [segment.slice(0, index).trim(), segment.slice(index + 1).trim()];
    }
  }
  return [segment.trim(), ""];
}

export interface TimelineRow {
  when: string;
  title: string;
  detail: string;
  strong: boolean;
}

/** `when | title | detail`, one a line; an older plan's `*` first is read
 *  past. */
export function readTimeline(text: string): TimelineRow[] {
  const rows: TimelineRow[] = [];
  for (const line of text.split("\n")) {
    let trimmed = line.trim();
    if (!trimmed) continue;
    if (trimmed.startsWith("*")) trimmed = trimmed.slice(1).trim();
    const strong = false;
    const [when = "", title = "", ...rest] = trimmed.split("|").map((part) => part.trim());
    if (!title && !rest.length) rows.push({ when: "", title: when, detail: "", strong });
    else rows.push({ when, title, detail: rest.join(" | "), strong });
  }
  return rows;
}

export interface TreeRow {
  depth: number;
  label: string;
  note: string;
}

/** A whole and its parts by indentation; `label | note` or `label # note`. */
export function readTree(text: string): TreeRow[] {
  const lines = text.split("\n").filter((line) => line.trim());
  const indents = [...new Set(lines.map((line) => line.length - line.trimStart().length))].sort((a, b) => a - b);
  return lines.map((line) => {
    const indent = line.length - line.trimStart().length;
    let body = line.trim().replace(/^[-*+]\s+/, "");
    let note = "";
    const split = body.match(/^(.*?)\s+(?:\||#)\s+(.*)$/);
    if (split) [body, note] = [split[1]!, split[2]!];
    return { depth: indents.indexOf(indent), label: body, note };
  });
}

export interface AskOption {
  text: string;
  note: string;
  suggested: boolean;
}

export interface Ask {
  question: string;
  options: AskOption[];
}

/** The question on the first line, then `* suggested` and `- other` options. */
export function readAsk(text: string): Ask {
  const lines = text.split("\n").map((line) => line.trim()).filter(Boolean);
  const question: string[] = [];
  const options: AskOption[] = [];
  for (const line of lines) {
    const option = line.match(/^([*-])\s+(.*)$/);
    if (!option) {
      if (!options.length) question.push(line);
      continue;
    }
    const [text = "", ...note] = option[2]!.split("|").map((part) => part.trim());
    options.push({ text, note: note.join(" | "), suggested: option[1] === "*" });
  }
  return { question: question.join(" "), options };
}

export type NoteTone = "warn" | "ok" | "info";

export function noteTone(info: string): NoteTone {
  const tone = info.trim().split(/\s+/)[1];
  return tone === "warn" || tone === "ok" ? tone : "info";
}
