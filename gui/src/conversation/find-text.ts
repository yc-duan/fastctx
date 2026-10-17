/** Search offsets refer to displayed UTF-16 text, even when lowercasing
 * expands a character (for example, İ). */
export interface TextMatch {
  start: number;
  end: number;
}

export function textMatches(text: string, query: string): TextMatch[] {
  if (!query) return [];
  // Final sigma must fold the same in a substring and in a whole word.
  const lower = text.toLowerCase().replace(/ς/g, "σ");
  query = query.toLowerCase().replace(/ς/g, "σ");
  const offsets: number[] | null = lower.length === text.length ? null : [];
  if (offsets) {
    let at = 0;
    for (const char of text) {
      for (let index = 0; index < char.toLowerCase().length; index += 1) offsets.push(at);
      at += char.length;
    }
    offsets.push(text.length);
  }
  const matches: TextMatch[] = [];
  for (let at = lower.indexOf(query); at >= 0; at = lower.indexOf(query, at + query.length)) {
    const start = offsets?.[at] ?? at;
    const last = offsets?.[at + query.length - 1] ?? at + query.length - 1;
    const end = offsets ? last + (text.codePointAt(last)! > 0xffff ? 2 : 1) : at + query.length;
    if (matches[matches.length - 1]?.start !== start) matches.push({ start, end });
  }
  return matches;
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
