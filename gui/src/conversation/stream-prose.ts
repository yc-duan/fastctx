// An agent's words in the page as they grow, cut into their top-level
// blocks; a block whose source is unchanged keeps its nodes, so words
// streaming in at any rate redraw only the paragraph, list or block of code
// they are adding to, never what was said before it. While the words grow,
// only the last block and what follows it is read again (from the list or
// quote before it, which can still take it in): every block before is
// closed, and Markdown read whole would read it the same way. A reference
// definition (`[x]: url`) is the exception, read whole each time it is in
// the text, and the words done are read whole once more.

import { proseBlocks, type ProseBlock } from "./prose";

interface Part {
  key: string;
  nodes: ChildNode[];
  /** Where in the text it begins, and whether what follows can join it. */
  start: number | null;
  container: boolean;
}

/** A line that defines a link reference anywhere in a text: links before
 *  it read differently once it comes. */
const REFERENCE = /^ {0,3}\[[^\]\n]+\]:/m;

export class StreamedProse {
  readonly element: HTMLElement;
  private parts: Part[] = [];
  private text: string | null = null;
  private writing = false;
  /** How many blocks at the start are closed: a growing text is read again
   *  from where the block after them begins. */
  private closed = 0;
  private references = false;

  constructor(className = "prose") {
    this.element = document.createElement("div");
    this.element.className = className;
  }

  /** Brings the words up to `text`; `writing` while more may come. Says
   *  whether anything was drawn. */
  update(text: string, writing: boolean): boolean {
    if (text === this.text && writing === this.writing) return false;
    const prior = this.text;
    const grows = writing && prior !== null && this.closed > 0 && text.length > prior.length && text.startsWith(prior);
    // A reference definition can come in the words added or join a line
    // that began before them.
    const added = grows ? text.slice(Math.max(0, prior.lastIndexOf("\n"))) : text;
    this.references = (grows && this.references) || REFERENCE.test(added);
    this.text = text;
    this.writing = writing;
    this.element.classList.toggle("writing", writing);
    let from = 0;
    let kept = 0;
    let blocks: ProseBlock[];
    if (grows && !this.references) {
      // The closed blocks stand as drawn; only the rest is read again.
      kept = this.closed;
      from = this.parts[kept]!.start!;
      blocks = proseBlocks(text.slice(from), writing);
    } else {
      blocks = proseBlocks(text, writing);
    }
    const tail = text.slice(from);
    const total = kept + blocks.length;
    const next: Part[] = this.parts.slice(0, kept);
    blocks.forEach((block, at) => {
      const index = kept + at;
      const old = this.parts[index];
      const start = block.line === null ? null : from + offsetOfLine(tail, block.line);
      // The block being written is told apart from the same block done:
      // done, it is drawn once more, its code coloured.
      const key = writing && index === total - 1 ? `${block.source}\u0000w` : block.source;
      if (old?.key === key) {
        next.push({ ...old, start, container: block.container });
        return;
      }
      const holder = document.createElement("template");
      holder.innerHTML = block.html();
      const nodes = [...holder.content.childNodes];
      // The new nodes stand where the part replaced stood: before the first
      // node of any part after it.
      const anchor = this.parts.slice(index + 1).find((part) => part.nodes.length)?.nodes[0] ?? null;
      for (const node of old?.nodes ?? []) node.remove();
      this.element.insertBefore(holder.content, anchor);
      next.push({ key, nodes, start, container: block.container });
    });
    for (const gone of this.parts.slice(total)) for (const node of gone.nodes) node.remove();
    this.parts = next;
    // What the next growth reads from: the last block, or the list or quote
    // before it, which can still take it in.
    let open = next.length - 1;
    if (open > 0 && next[open - 1]!.container) open -= 1;
    // A display formula begun and not closed yet reads as paragraphs until
    // its end comes, blank lines and all: the blocks from its first line are
    // read again until then, so its end finds its beginning.
    const formula = writing && (text.includes("$$") || text.includes("\\[")) ? openFormula(text) : null;
    if (formula !== null) {
      let at = -1;
      next.forEach((part, index) => {
        if (part.start !== null && part.start <= formula) at = index;
      });
      open = Math.min(open, Math.max(at, 0));
    }
    this.closed = open > 0 && next[open]!.start !== null ? open : 0;
    return true;
  }
}

/** Where the first line of a display formula (`$$` or `\[`) not yet closed
 *  begins in `text`, outside code fences; null when every one is closed. */
function openFormula(text: string): number | null {
  let fence: string | null = null;
  let opened: number | null = null;
  let closer = "";
  let at = 0;
  for (const line of text.split("\n")) {
    const trimmed = line.trimStart();
    if (opened !== null) {
      const end = trimmed.indexOf(closer);
      if (end !== -1 && trimmed.slice(end + closer.length).trim() === "") opened = null;
    } else {
      const marker = /^(`{3,}|~{3,})/.exec(trimmed)?.[1];
      if (fence !== null) {
        if (marker !== undefined && marker[0] === fence[0] && marker.length >= fence.length) fence = null;
      } else if (marker !== undefined) {
        fence = marker;
      } else {
        const begins = trimmed.startsWith("$$") ? "$$" : trimmed.startsWith("\\[") ? "\\[" : null;
        if (begins !== null) {
          closer = begins === "$$" ? "$$" : "\\]";
          const rest = trimmed.slice(begins.length);
          const end = rest.indexOf(closer);
          if (end === -1 || rest.slice(end + closer.length).trim() !== "") opened = at;
        }
      }
    }
    at += line.length + 1;
  }
  return opened;
}

/** Where `line` begins in `text`, counting lines from 0. */
function offsetOfLine(text: string, line: number): number {
  let at = 0;
  for (let seen = 0; seen < line; seen += 1) {
    const end = text.indexOf("\n", at);
    if (end < 0) return text.length;
    at = end + 1;
  }
  return at;
}
