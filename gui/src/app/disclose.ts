// The window before a change to an agent (`world-layers.md`, 接入之前讲清楚做了什么;
// `world-gui.md`, 接入与工作台): a paper card each kind of change, in plain
// words, the lines it adds and takes out folded under it a file at a time,
// and a quieter card for what connecting never does. The cards are drawn
// from the change the login's node worked out with nothing written, so
// that what the user agrees to is what is done.

import { diffPaint, memoryDiff, rowOf } from "../conversation/diff-lines";
import { glyph, type GlyphName } from "../conversation/glyphs";
import { LongText, type LongTextWords } from "../conversation/long-text";
import { escape } from "../conversation/prose";
import { AnchorLayout } from "../longview";
import { Sketch } from "../pen/sketch";
import type { Change, ChangeCard, ChangePreview, FileDiff } from "../world/logins";
import { shortPath } from "./change-diff";
import type { DiscloseWords } from "./disclose-words";

/** The order a change is told in: what the agent gains, what it gives up,
 *  what it is taught, then the rest. */
const ORDER: Change[] = ["tools", "shell", "guidance", "limits", "plugin", "consent", "legacy"];

/** Each kind's mark, as the interface draws its own symbols. */
const MARK: Record<Change, GlyphName> = {
  tools: "plug",
  shell: "terminal",
  guidance: "page",
  limits: "expand",
  plugin: "tool",
  consent: "unlock",
  legacy: "versions",
};

/** The preview's cards in the order they are told. */
export function toldCards(preview: ChangePreview): ChangeCard[] {
  return [...(preview.cards ?? [])].sort((a, b) => ORDER.indexOf(a.change) - ORDER.indexOf(b.change));
}

/** A file of a card, as `open` names it. */
const key = (card: number, file: number) => `${card}:${file}`;

/** The cards, and what connecting never does after them. `open` holds the
 *  files unfolded; `also` a line more for a kind, said of this login. */
/** `bench`: the workbench opened (`true`) or closed in the login, told on
 *  a card of its own in place of what connecting never does. */
export function discloseCards(preview: ChangePreview, words: DiscloseWords, open: ReadonlySet<string>, also: Partial<Record<Change, string>> = {}, bench?: boolean): string {
  const told = preview.into ? words.into : words.out;
  const cards = toldCards(preview).map((card, index) => {
    const said = bench !== undefined && card.change === "tools" ? words.benchTools(preview.agent, bench) : told[card.change];
    const more = also[card.change];
    const files = card.files.map((file, at) => fileLine(file, index, at, words, open.has(key(index, at)))).join("");
    const commands = card.commands?.length
      ? `<ul class="dc-commands">${card.commands.map((line) => `<li><code>${escape(line)}</code></li>`).join("")}</ul>`
      : "";
    const folded = files || commands ? `<div class="dc-folds">${files}${commands}</div>` : "";
    return `<li class="dc-card" data-kind="${card.change}">
      <span class="dc-mark" aria-hidden="true">${glyph(MARK[card.change], 18)}</span>
      <div class="dc-text"><h3 class="dc-title">${escape(said.title)}</h3><p class="dc-line">${escape(said.line)}</p>${more ? `<p class="dc-line">${escape(more)}</p>` : ""}${folded}</div>
    </li>`;
  });
  const same = cards.length || bench !== undefined ? "" : `<li class="dc-same">${escape(words.same)}</li>`;
  const last = bench === undefined ? (preview.into ? { said: words.never, mark: "shield" as const } : null) : { said: bench ? words.bench : words.benchClosed, mark: bench ? ("home" as const) : ("pause" as const) };
  const never = last
    ? `<li class="dc-card dc-never">
        <span class="dc-mark" aria-hidden="true">${glyph(last.mark, 18)}</span>
        <div class="dc-text"><h3 class="dc-title">${escape(last.said.title)}</h3><p class="dc-line">${escape(last.said.line)}</p></div>
      </li>`
    : "";
  return `<ol class="dc-cards">${cards.join("")}${same}${never}</ol>`;
}

/** A file's line: its short name and how many lines change, which unfolds
 *  into the lines themselves. */
function fileLine(file: FileDiff, card: number, at: number, words: DiscloseWords, opened: boolean): string {
  const added = file.lines.filter((line) => line.kind === "added").length;
  const removed = file.lines.filter((line) => line.kind === "removed").length;
  // Short of room, the folder gives way and the file's name stays.
  const path = shortPath(file.path);
  const cut = Math.max(path.lastIndexOf("\\"), path.lastIndexOf("/")) + 1;
  return `<div class="dc-file${opened ? " open" : ""}" data-card="${card}" data-file="${at}">
    <button class="dc-file-line" type="button" data-change="dc-file" aria-expanded="${opened}" title="${escape(file.path)}">
      <span class="dc-path"><span class="dc-dir">${escape(path.slice(0, cut))}</span><span class="dc-name">${escape(path.slice(cut))}</span></span><span class="row-stats" aria-label="${escape(words.lines(added, removed))}"><span class="add">+${added}</span><span class="del">−${removed}</span></span><span class="dc-chevron" aria-hidden="true">${glyph("chevron", 14)}</span>
    </button>
    <div class="dc-diff"></div>
  </div>`;
}

/** The unfolded files' lines, drawn as a change reads in a conversation,
 *  each once: a file folded again lets its lines go. */
export function drawDisclosed(
  root: HTMLElement,
  preview: ChangePreview,
  open: ReadonlySet<string>,
  words: DiscloseWords,
  lines: LongTextWords & { unchanged(n: number): string },
): void {
  const scroller = root.querySelector<HTMLElement>(".dc-cards");
  if (!scroller) return;
  const layout = new AnchorLayout(scroller);
  toldCards(preview).forEach((card, index) =>
    card.files.forEach((file, at) => {
      const holder = scroller.querySelector<HTMLElement>(`.dc-file[data-card="${index}"][data-file="${at}"] .dc-diff`);
      if (!holder) return;
      if (!open.has(key(index, at))) {
        holder.replaceChildren();
        return;
      }
      if (holder.childElementCount) return;
      const rows = file.lines.map((line, i, all) => rowOf(line, all[i - 1] ?? null, i === 0));
      const text = new LongText(
        { ...memoryDiff(rows), paint: diffPaint(file.path, lines.unchanged, true), kind: "diff" },
        shortPath(file.path),
        layout,
        lines,
      );
      holder.append(text.element);
      if (file.more) {
        const more = document.createElement("p");
        more.className = "dc-more";
        more.textContent = words.more(file.more);
        holder.append(more);
      }
    }),
  );
}

/** What connecting is: the agent's own window, a few lines written in it,
 *  and a stroke of ink from it to each of the user's machines, a screen on
 *  a stand; a drop of clay where the work lands. */
export function connectDrawing(): string {
  const sketch = new Sketch("dc-connect");
  sketch.wash(14, 42, 92, 64, "box", 8);
  sketch.rule([26, 60], [66, 60], 2, "graphite");
  sketch.rule([26, 72], [90, 72], 2, "graphite");
  sketch.rule([26, 84], [56, 84], 2, "graphite");
  const machines: [number, number][] = [
    [196, 12],
    [214, 60],
    [196, 108],
  ];
  for (const [x, y] of machines) {
    sketch.stroke(
      [
        [110, 74],
        [150, 74 + (y + 12 - 74) * 0.5],
        [x - 6, y + 12],
      ],
      1.4,
      "graphite",
    );
    sketch.wash(x, y, 44, 26, "box", 4);
    sketch.rule([x + 13, y + 33], [x + 31, y + 33], 1.6, "graphite");
  }
  sketch.blot([218, 25], 3, "clay");
  return sketch.svg(270, 150, "dc-sketch");
}

/** Whether a change was refused because the files it was worked out from
 *  moved on since it was shown, as the node says so (`apply::Error::Moved`). */
export const moved = (reason: string): boolean => reason.includes("changed since this change was shown");

/** Folds or unfolds the file a press landed on; whether one was. */
export function toggleDisclosed(target: HTMLElement, open: Set<string>): boolean {
  const file = target.closest<HTMLElement>(".dc-file");
  if (!file || !target.closest("[data-change='dc-file']")) return false;
  const name = key(Number(file.dataset.card), Number(file.dataset.file));
  if (open.has(name)) open.delete(name);
  else open.add(name);
  file.classList.toggle("open", open.has(name));
  file.querySelector(".dc-file-line")?.setAttribute("aria-expanded", String(open.has(name)));
  return true;
}
