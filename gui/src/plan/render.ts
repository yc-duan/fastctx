// A plan as the user sees it: the agent's Markdown in the interface's own
// type, its picture blocks drawn by the interface, the world's ids alive.
// Raw HTML in the text is shown as text; nothing the agent writes runs.

import MarkdownIt from "markdown-it";
import hljs from "highlight.js/lib/common";
import { drawDiagrams } from "../conversation/diagrams";
import { escape } from "../conversation/prose";
import { askElement, drawAsk, drawTimeline, drawTree, noteElement, timelineElement, treeElement, type BlockWords } from "./blocks";
import { Sketch } from "../pen/sketch";
import { drawFlow, flowElement } from "./flow";
import { liven, type LiveFacts } from "./live";
import { noteTone, readAsk, readFlow, readTimeline, readTree } from "./source";

const pictures = new Set(["flow", "timeline", "tree", "ask", "note"]);

const markdown: MarkdownIt = new MarkdownIt({
  html: false,
  linkify: true,
  breaks: false,
  highlight: (code, language) => {
    if (!language || !hljs.getLanguage(language)) return "";
    return hljs.highlight(code, { language, ignoreIllegals: true }).value;
  },
});

const defaultFence =
  markdown.renderer.rules.fence ?? ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));
markdown.renderer.rules.fence = (tokens, index, options, env, self) => {
  const token = tokens[index]!;
  const kind = token.info.trim().split(/\s+/)[0] ?? "";
  if (kind === "mermaid") return `<figure class="diagram" data-mermaid>${escape(token.content)}</figure>`;
  if (pictures.has(kind)) {
    return `<div class="pb-picture" data-kind="${kind}" data-info="${escape(token.info.trim())}" data-source="${escape(token.content)}"></div>`;
  }
  return `<div class="code-block">${defaultFence(tokens, index, options, env, self)}</div>`;
};

// Links leave for the system's browser, as in a conversation.
const defaultLink =
  markdown.renderer.rules.link_open ?? ((tokens, index, options, _env, self) => self.renderToken(tokens, index, options));
markdown.renderer.rules.link_open = (tokens, index, options, env, self) => {
  tokens[index]!.attrSet("target", "_blank");
  tokens[index]!.attrSet("rel", "noreferrer noopener");
  return defaultLink(tokens, index, options, env, self);
};

export interface PlanSeams {
  facts: LiveFacts;
  words: BlockWords;
  /** The user picked an option of a decision on the plan; a rejection
   *  leaves it to be picked again. */
  answer: (question: string, option: string) => Promise<void> | void;
  /** Opens what one of the world's ids names. */
  open?: ((id: string) => void) | undefined;
}

/** The plan's page, not yet laid out: call `drawPlan` once it is placed,
 *  and again when its width changes. */
export function planElement(source: string, seams: PlanSeams): HTMLElement {
  const root = document.createElement("div");
  root.className = "pb-plan prose";
  root.innerHTML = markdown.render(source);
  checklists(root);
  let count = 0;
  for (const slot of root.querySelectorAll<HTMLElement>(".pb-picture")) {
    const kind = slot.dataset.kind!;
    const text = slot.dataset.source ?? "";
    const seed = `${count++}-${text.length}`;
    const picture =
      kind === "flow"
        ? flowElement(readFlow(text), seed)
        : kind === "timeline"
          ? timelineElement(readTimeline(text), seed)
          : kind === "tree"
            ? treeElement(readTree(text), seed)
            : kind === "ask"
              ? askElement(readAsk(text), seams.words, seed, seams.answer)
              : noteElement(noteTone(slot.dataset.info ?? ""), markdown.render(text), seed);
    slot.replaceWith(picture);
  }
  liven(root, seams.facts);
  // A chip opens what it names, by the pointer or by Enter.
  const open = (event: Event) => {
    const id = (event.target as HTMLElement).closest<HTMLElement>(".pb-chip[data-id]")?.dataset.id;
    if (id && seams.open) seams.open(id);
  };
  root.addEventListener("click", open);
  root.addEventListener("keydown", (event) => {
    if (event.key === "Enter") open(event);
  });
  return root;
}

/** A checklist's items, `[ ] step` and `[x] step` as Markdown leaves them:
 *  each a small box drawn by hand, ticked where done. */
function checklists(root: HTMLElement): void {
  let count = 0;
  for (const item of root.querySelectorAll<HTMLLIElement>("li")) {
    const walker = document.createTreeWalker(item, NodeFilter.SHOW_TEXT);
    let text = walker.nextNode() as Text | null;
    while (text && !text.data.trim()) text = walker.nextNode() as Text | null;
    const mark = text ? /^\s*\[([ xX])\]\s+/.exec(text.data) : null;
    if (!text || !mark) continue;
    text.data = text.data.slice(mark[0].length);
    const done = mark[1] !== " ";
    item.classList.add("pb-check");
    if (done) item.classList.add("done");
    item.parentElement?.classList.add("pb-checklist");
    const box = document.createElement("span");
    box.className = "pb-box";
    box.setAttribute("aria-hidden", "true");
    box.innerHTML = checkBox(done, `check-${count++}`);
    item.prepend(box);
  }
}

/** A small square as a hand rules it, and a tick through it once done. */
export function checkBox(done: boolean, seed: string): string {
  const sketch = new Sketch(seed);
  sketch.rule([3, 3.4], [14.6, 3], 1.1, "graphite");
  sketch.rule([14.6, 3], [14.8, 14.6], 1.1, "graphite");
  sketch.rule([14.8, 14.6], [3.2, 14.8], 1.1, "graphite");
  sketch.rule([3.2, 14.8], [3, 3.4], 1.1, "graphite");
  if (done) sketch.stroke([[5.4, 9.2], [8, 12.2], [15.6, 3.6]], 1.7, "graphite", 0.18);
  return sketch.svg(18, 18, "pb-box-svg");
}

/** The rule a hand lays under the plan's title, as at the head of a
 *  notebook's page. */
function drawTitle(root: HTMLElement): void {
  const title = root.querySelector<HTMLElement>(":scope > h1");
  if (!title) return;
  let rule = title.nextElementSibling as HTMLElement | null;
  if (!rule?.classList.contains("pb-title-rule")) {
    rule = document.createElement("div");
    rule.className = "pb-title-rule";
    rule.setAttribute("aria-hidden", "true");
    title.after(rule);
  }
  const width = root.clientWidth;
  rule.innerHTML = new Sketch("plan-title").rule([0, 3], [width, 3], 1.1, "graphite").svg(width, 6, "pb-ink-svg");
}

/** Lays out and draws every picture for the width the plan has now. */
export function drawPlan(root: HTMLElement): void {
  for (const flow of root.querySelectorAll<HTMLElement>(".pb-route")) drawFlow(flow);
  for (const ask of root.querySelectorAll<HTMLElement>(".pb-ask")) drawAsk(ask);
  drawTitle(root);
  for (const timeline of root.querySelectorAll<HTMLElement>(".pb-timeline")) drawTimeline(timeline);
  for (const tree of root.querySelectorAll<HTMLElement>(".pb-tree")) drawTree(tree);
  void drawDiagrams(root);
}
