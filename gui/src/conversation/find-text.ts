/** Search offsets refer to displayed UTF-16 text, even when lowercasing
 * expands a character (for example, İ). */
export interface TextMatch {
  start: number;
  end: number;
}

/** Stream offsets rather than allocating one object for every match. */
export function* matchingText(text: string, query: string): Generator<TextMatch> {
  if (!query) return;
  const lower = text.toLowerCase().replace(/ς/g, "σ");
  query = query.toLowerCase().replace(/ς/g, "σ");
  let original = 0; let folded = 0; let previous = -1;
  const position = (offset: number): number => {
    while (original < text.length) {
      const char = String.fromCodePoint(text.codePointAt(original)!);
      const next = folded + char.toLowerCase().length;
      if (next > offset) break;
      folded = next; original += char.length;
    }
    return original;
  };
  for (let at = lower.indexOf(query); at >= 0; at = lower.indexOf(query, at + query.length)) {
    const start = lower.length === text.length ? at : position(at);
    const last = lower.length === text.length ? at + query.length - 1 : position(at + query.length - 1);
    const end = lower.length === text.length ? at + query.length : last + (text.codePointAt(last)! > 0xffff ? 2 : 1);
    if (start === previous) continue;
    previous = start;
    yield { start, end };
  }
}
export function textMatches(text: string, query: string): TextMatch[] { return [...matchingText(text, query)]; }
export function countTextMatches(text: string, query: string): number {
  let count = 0;
  for (const _match of matchingText(text, query)) count += 1;
  return count;
}
export function textMatchAt(text: string, query: string, ordinal: number): TextMatch | null {
  for (const match of matchingText(text, query)) if (ordinal-- === 0) return match;
  return null;
}

const OMIT = "script, style, svg title, svg desc, .katex-mathml, .turn-acts, .code-copy, .io-more, .long-text-full, .unnumbered .diff-num, [hidden]";
const SEPARATE = "p, pre, li, h1, h2, h3, h4, h5, h6, tr, .lv-line, .row-line, .row-sub, .group-title, .group-time, .io-end, .plan-label, .code-language";
const INLINE = "td, th, .diff-num, .diff-sign, .diff-code, .row-action, .row-object, .row-outcome, .row-machine, .row-stats .add, .row-stats .del";

/** Text shared by the search projection and its DOM ranges. Folds remain
 * searchable; controls, hidden accessibility copies and metadata do not. */
export function displayedText(root: Node): { text: string; range(match: TextMatch): Range | null } {
  const nodes: { node: Text; from: number; to: number }[] = [];
  let text = "";
  let previous: Element | null | undefined;
  let inline: Element | null = null;
  const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
    acceptNode: (node) => {
      const parent = node instanceof Element ? node : node.parentElement;
      if (parent?.closest(OMIT)) return NodeFilter.FILTER_REJECT;
      if (parent?.closest("[aria-hidden=\"true\"]") && !parent.closest(".katex-html")) return NodeFilter.FILTER_REJECT;
      if (node instanceof Element && !(node instanceof HTMLBRElement)) return NodeFilter.FILTER_SKIP;
      return NodeFilter.FILTER_ACCEPT;
    },
  });
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (node instanceof HTMLBRElement) {
      text += "\n";
      continue;
    }
    const raw = node.nodeValue ?? "";
    if (!raw) continue;
    const boundary = node.parentElement?.closest(SEPARATE) ?? null;
    const field = node.parentElement?.closest(INLINE) ?? null;
    if (previous !== undefined && boundary !== previous && text && !text.endsWith("\n") && !raw.startsWith("\n")) text += "\n";
    else if (previous !== undefined && field !== inline && text && !/\s$/.test(text) && !/^\s/.test(raw)) text += " ";
    previous = boundary;
    inline = field;
    nodes.push({ node: node as Text, from: text.length, to: text.length + raw.length });
    text += raw;
  }
  return {
    text,
    range: ({ start, end }) => {
      const first = nodes.find((one) => one.to > start);
      const last = nodes.find((one) => one.to >= end);
      if (!first || !last || start < first.from || end > last.to) return null;
      const range = document.createRange();
      range.setStart(first.node, start - first.from);
      range.setEnd(last.node, end - last.from);
      return range;
    },
  };
}

export function htmlText(html: string): string {
  if (!html) return "";
  const holder = document.createElement("template");
  holder.innerHTML = html;
  return displayedText(holder.content).text;
}
