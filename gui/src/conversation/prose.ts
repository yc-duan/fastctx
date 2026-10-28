// What an agent writes, as it reads: Markdown with code, tables, formulas
// and diagrams. Raw HTML in the text is shown as text, never run; a
// formula or a diagram that cannot be drawn stays as its source.

import MarkdownIt from "markdown-it";
import type StateBlock from "markdown-it/lib/rules_block/state_block.mjs";
import type StateInline from "markdown-it/lib/rules_inline/state_inline.mjs";
import hljs from "highlight.js/lib/common";
import katex from "katex";

const markdown: MarkdownIt = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
  highlight: (code, language) => highlight(code, language),
});

markdown.use(math);

// Links leave the conversation in the system's browser, never inside it; a
// link to a file carries its path for the page to open (`file-links.ts`).
// `file:` addresses are such links, not a protocol to refuse.
const validLink = markdown.validateLink.bind(markdown);
markdown.validateLink = (url) => /^file:/i.test(url.trim()) || validLink(url);
const defaultLink =
  markdown.renderer.rules.link_open ?? ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));
markdown.renderer.rules.link_open = (tokens, index, options, env, self) => {
  const token = tokens[index];
  const path = filePathOf(token.attrGet("href") ?? "");
  if (path) {
    token.attrSet("href", "#");
    token.attrSet("data-file", path);
  } else {
    token.attrSet("target", "_blank");
    token.attrSet("rel", "noreferrer noopener");
  }
  return defaultLink(tokens, index, options, env, self);
};

// A picture in an answer that names a file is drawn from the machine holding
// it, when it comes into view (`pictures.ts`).
markdown.renderer.rules.image = (tokens, index) => {
  const token = tokens[index];
  const alt = token.children?.map((child) => child.content).join("") ?? token.content;
  const path = filePathOf(token.attrGet("src") ?? "");
  if (path) return `<span class="prose-picture"><img data-picture data-path="${escape(path)}" alt="${escape(alt)}" tabindex="0"></span>`;
  return `<img src="${escape(token.attrGet("src") ?? "")}" alt="${escape(alt)}">`;
};

// A diagram is drawn later, when the turn is done; until then its source.
const defaultFence =
  markdown.renderer.rules.fence ?? ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));
markdown.renderer.rules.fence = (tokens, index, options, env, self) => {
  const token = tokens[index];
  const language = token.info.trim().split(/\s+/)[0] ?? "";
  if (language === "mermaid") {
    return `<figure class="diagram" data-mermaid>${escape(token.content)}</figure>`;
  }
  const label = language ? `<span class="code-language">${escape(language)}</span>` : "";
  const body = defaultFence(tokens, index, options, env, self);
  return `<div class="code-block">${label}<button class="code-copy" type="button" aria-label="${escape(codeCopy)}" title="${escape(codeCopy)}"></button>${body}</div>`;
};

/** What a block of code's copy button is called, in the interface's
 *  language; prose is drawn without words of its own. */
let codeCopy = "Copy code";

export function nameCodeCopy(label: string): void {
  codeCopy = label;
}

/** The path a link names, when it names a file: a Windows path, an absolute
 *  POSIX one, or a `file:` address, without the line or column a reader
 *  of code points at. Anything else is a link to the web. */
export function filePathOf(href: string): string | null {
  let path: string;
  try {
    path = decodeURI(href);
  } catch {
    path = href;
  }
  if (/^file:/i.test(path)) {
    path = path.replace(/^file:\/\/(?:localhost)?/i, "");
    if (/^\/[A-Za-z]:[\\/]/.test(path)) path = path.slice(1);
  } else if (!/^[A-Za-z]:[\\/]/.test(path) && !/^\/(?!\/)/.test(path)) {
    return null;
  }
  return path.replace(/#L\d+(?:-L?\d+)?$/, "").replace(/(\.[^\\/:.]+):\d+(?::\d+)?$/, "$1");
}

/** A path as its own system writes it: backslashes for Windows. */
export const naturalPath = (path: string): string => (/^[A-Za-z]:[\\/]/.test(path) ? path.replace(/\//g, "\\") : path);

/** Markdown as HTML. Safe to place in the page: raw HTML is escaped. */
export function render(text: string): string {
  return markdown.render(text);
}

/** One block at the top of a text's Markdown (a paragraph, a list, a block
 *  of code, a table), as the whole text reads it: its source, which names
 *  it, and its HTML. */
export interface ProseBlock {
  source: string;
  html(): string;
}

/** Markdown read whole and cut into its top-level blocks, each drawn on its
 *  own: a text that grows draws again only the blocks that changed. A
 *  block of code still being written is coloured only where its language
 *  is named; guessing one is left until the words are done. */
export function proseBlocks(text: string, writing: boolean): ProseBlock[] {
  const env = {};
  const tokens = markdown.parse(text, env);
  const lines = text.split("\n");
  const blocks: ProseBlock[] = [];
  let start = 0;
  let depth = 0;
  for (let index = 0; index < tokens.length; index += 1) {
    depth += tokens[index]!.nesting;
    if (depth !== 0) continue;
    const slice = tokens.slice(start, index + 1);
    const map = slice[0]!.map ?? slice.find((token) => token.map)?.map ?? null;
    const source = map ? lines.slice(map[0], map[1]).join("\n") : slice.map((token) => token.content).join("");
    blocks.push({
      source,
      html: () => {
        guessing = !writing;
        growing = writing;
        try {
          return markdown.renderer.render(slice, markdown.options, env);
        } finally {
          guessing = true;
          growing = false;
        }
      },
    });
    start = index + 1;
  }
  return blocks;
}

/** Whether unlabelled code is coloured by guessing its language. */
let guessing = true;
/** Whether the words are still being written. */
let growing = false;
/** Code longer than this is coloured once it is written, not again at
 *  every word that arrives while it grows. */
const COLOURED_WHILE_WRITING = 6000;

/** Text with no Markdown, as one escaped paragraph per blank-line block. */
export function plain(text: string): string {
  return text
    .split(/\n{2,}/)
    .map((paragraph) => `<p>${escape(paragraph).replace(/\n/g, "<br>")}</p>`)
    .join("");
}

function highlight(code: string, language: string): string {
  const name = language.trim().toLowerCase();
  if (name && hljs.getLanguage(name)) {
    if (growing && code.length > COLOURED_WHILE_WRITING) return escape(code);
    return hljs.highlight(code, { language: name, ignoreIllegals: true }).value;
  }
  // Unlabelled code is guessed only while it is small enough to guess fast.
  if (!name && guessing && code.length <= 8000) return hljs.highlightAuto(code).value;
  return escape(code);
}

export function escape(text: string): string {
  return text.replace(/[&<>"']/g, (char) => `&#${char.charCodeAt(0)};`);
}

// Formulas: `$…$` and `\(…\)` inline, `$$…$$` and `\[…\]` on their own
// lines. A dollar sign is a formula only where TeX writers mean one: not
// before a digit-led amount, not with a space inside its edges.
function math(md: MarkdownIt): void {
  md.inline.ruler.after("escape", "math_inline", inlineMath);
  md.block.ruler.after("blockquote", "math_block", blockMath, { alt: ["paragraph", "reference", "blockquote", "list"] });
  md.renderer.rules.math_inline = (tokens, index) => tex(tokens[index].content, false);
  md.renderer.rules.math_block = (tokens, index) => `<div class="math-display">${tex(tokens[index].content, true)}</div>`;
}

function inlineMath(state: StateInline, silent: boolean): boolean {
  const source = state.src;
  const start = state.pos;
  let open: string;
  let close: string;
  if (source.startsWith("\\(", start)) {
    open = "\\(";
    close = "\\)";
  } else if (source[start] === "$" && source[start + 1] !== "$") {
    open = "$";
    close = "$";
    const next = source[start + 1];
    if (next === undefined || /\s/.test(next)) return false;
  } else {
    return false;
  }
  let end = start + open.length;
  for (;;) {
    end = source.indexOf(close, end);
    if (end === -1) return false;
    if (source[end - 1] === "\\") {
      end += 1;
      continue;
    }
    break;
  }
  const content = source.slice(start + open.length, end);
  if (!content.trim()) return false;
  if (open === "$") {
    if (/\s$/.test(content)) return false;
    // `$5 and $6`: a closing dollar right before a digit is a price.
    if (/\d/.test(source[end + 1] ?? "")) return false;
  }
  if (!silent) {
    const token = state.push("math_inline", "math", 0);
    token.content = content;
    token.markup = open;
  }
  state.pos = end + close.length;
  return true;
}

function blockMath(state: StateBlock, startLine: number, endLine: number, silent: boolean): boolean {
  const lineStart = state.bMarks[startLine] + state.tShift[startLine];
  const lineEnd = state.eMarks[startLine];
  const first = state.src.slice(lineStart, lineEnd);
  let open: string;
  let close: string;
  if (first.startsWith("$$")) {
    open = "$$";
    close = "$$";
  } else if (first.startsWith("\\[")) {
    open = "\\[";
    close = "\\]";
  } else {
    return false;
  }
  let content = "";
  let line = startLine;
  const rest = first.slice(open.length);
  const sameLine = rest.indexOf(close);
  if (sameLine !== -1 && rest.slice(sameLine + close.length).trim() === "") {
    content = rest.slice(0, sameLine);
  } else {
    const parts = [rest];
    let found = false;
    while (++line < endLine) {
      const text = state.src.slice(state.bMarks[line] + state.tShift[line], state.eMarks[line]);
      const at = text.indexOf(close);
      if (at !== -1 && text.slice(at + close.length).trim() === "") {
        parts.push(text.slice(0, at));
        found = true;
        break;
      }
      parts.push(text);
    }
    if (!found) return false;
    content = parts.join("\n");
  }
  if (silent) return true;
  state.line = line + 1;
  const token = state.push("math_block", "math", 0);
  token.block = true;
  token.content = content;
  token.map = [startLine, state.line];
  return true;
}

function tex(source: string, display: boolean): string {
  try {
    return katex.renderToString(source, { displayMode: display, throwOnError: true, trust: false, strict: "ignore" });
  } catch {
    const fence = display ? "$$" : "$";
    return `<span class="math-source">${escape(fence + source + fence)}</span>`;
  }
}
