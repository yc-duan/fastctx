// A conversation drawn in the page. Each block keeps its element for as long
// as what it shows does not change, so a stream of words redraws one
// paragraph, not the conversation. Folding and opening are the reader's:
// they change the element in place, so they can move, and a later redraw
// keeps them.

import type { Content, MachineName, Speaker, Tool } from "../world/events";
import type { Words } from "../words";
import { glyph } from "./glyphs";
import type { Activity, Block, Conversation, Entry, SendState } from "./model";
import { escape, plain, render } from "./prose";
import { numbered, parse, renderRows } from "./diff";
import { groupHeading, headline, tenseOf } from "./tool";
import { drawDiagrams } from "./diagrams";
import { readTold } from "./told";

export interface Context {
  words: Words;
  /** The machine this interface runs on. */
  here: MachineName;
  /** The machine the resident lives on: a tool that acted there needs no
   *  word of where; one that acted elsewhere says so. */
  residentMachine: MachineName;
  /** Where another resident's conversation opens, where it can. */
  link?: ((address: { resident: string; machine: MachineName }) => string) | undefined;
}

type ToolEntry = Extract<Entry, { kind: "tool" }>;

export class ConversationView {
  private open = new Set<string>();
  private folded = new Set<string>();
  /** Outputs shown whole after the reader asked for the rest. */
  private whole = new Set<string>();
  private drawn = new Map<string, { signature: string; element: HTMLElement }>();
  /** Rows already on the page once; only rows new to a live conversation
   *  make an entrance. */
  private seen = new Set<string>();
  private settled = false;

  constructor(
    private readonly root: HTMLElement,
    private readonly conversation: Conversation,
    private readonly context: Context,
  ) {
    root.classList.add("transcript");
    root.addEventListener("click", (event) => this.click(event));
  }

  /** Brings the page up to the conversation as it now stands. */
  update(): void {
    const blocks = this.conversation.blocks();
    const present = new Set<string>();
    let previous: HTMLElement | null = null;
    for (const block of blocks) {
      present.add(block.key);
      const signature = this.signature(block);
      let drawn = this.drawn.get(block.key);
      if (!drawn || drawn.signature !== signature) {
        const element = this.draw(block);
        if (drawn) drawn.element.replaceWith(element);
        drawn = { signature, element };
        this.drawn.set(block.key, drawn);
      }
      const want: Element | null = previous ? previous.nextElementSibling : this.root.firstElementChild;
      if (want !== drawn.element) {
        if (previous) previous.after(drawn.element);
        else this.root.prepend(drawn.element);
      }
      previous = drawn.element;
    }
    for (const [key, drawn] of this.drawn) {
      if (!present.has(key)) {
        drawn.element.remove();
        this.drawn.delete(key);
      }
    }
    this.settled = true;
    void drawDiagrams(this.root);
  }

  private signature(block: Block): string {
    if (block.kind === "activity-group") return block.members.map((member) => this.memberSignature(member)).join("|");
    return this.memberSignature(block);
  }

  private memberSignature(entry: Entry): string {
    switch (entry.kind) {
      case "prose":
        return `${entry.key}p${entry.version}${entry.writing ? "w" : ""}`;
      case "tool":
        return `${entry.key}t${entry.tool.status}${entry.ended?.use ?? "-"}${entry.tool.output.length}`;
      case "heard":
        return `${entry.key}h${JSON.stringify(entry.sending ?? null)}`;
      case "steps":
        return `${entry.key}s${JSON.stringify(entry.steps)}${entry.note ?? ""}`;
      default:
        return entry.key;
    }
  }

  private draw(block: Block): HTMLElement {
    const element = document.createElement("div");
    element.dataset.key = block.key;
    if (block.kind === "activity-group") {
      element.className = "block block-activity";
      element.innerHTML = this.group(block);
      return element;
    }
    element.className = `block block-${block.kind}`;
    if (block.kind === "prose") element.classList.add(`prose-${block.prose}`);
    if (block.kind === "heard") element.classList.add(!block.from || block.from.kind === "user" ? "from-user" : "from-other");
    element.innerHTML = this.entry(block);
    return element;
  }

  private entry(entry: Entry): string {
    const { words } = this.context;
    switch (entry.kind) {
      case "heard":
        return this.heard(entry.from, entry.text, entry.sending ?? null);
      case "prose":
        if (entry.prose === "plan") {
          return `<div class="plan"><div class="plan-label">${escape(words.plan)}</div><div class="prose">${render(entry.text)}</div></div>`;
        }
        return `<div class="prose${entry.writing ? " writing" : ""}">${render(entry.text)}</div>`;
      case "steps":
        return `<ol class="steps">${entry.steps
          .map(
            (step) =>
              `<li class="step step-${step.status}"><span class="step-mark">${step.status === "done" ? glyph("check", 14) : ""}</span><span class="step-text">${escape(step.text)}</span></li>`,
          )
          .join("")}</ol>${entry.note ? `<p class="steps-note">${escape(entry.note)}</p>` : ""}`;
      case "compaction":
        return `<div class="divider"><span>${escape(words.compacted)}</span></div>`;
      case "other":
        return `<div class="note faint">${escape(entry.hostKind)}</div>`;
      case "ended": {
        const said = words.turn(entry.end.how, entry.end.reason);
        if (!said) return "";
        return `<div class="note ${entry.end.how === "failed" ? "wrong" : "caution"}">${escape(said)}</div>`;
      }
      case "health":
        return `<div class="note caution">${escape(entry.health.text)}</div>`;
      case "uncertain":
        return `<div class="note caution">${escape(words.uncertain(entry.why))}</div>`;
      case "tool":
      case "activity":
        return "";
    }
  }

  private heard(from: Speaker | null, text: string, sending: SendState | null): string {
    const { words } = this.context;
    if (!from || from.kind === "user") {
      // The user's own words; which machine they came from is said only
      // when it is not this one. Words not yet heard say how they stand.
      const who =
        from && from.machine !== this.context.here ? `<div class="bubble-from">${escape(words.you(from.machine))}</div>` : "";
      const state = sending ? ` ${sending.state}` : "";
      let note = "";
      if (sending?.state === "failed") note = `<div class="bubble-note wrong">${escape(words.notSent(sending.why))}</div>`;
      if (sending?.state === "landed") {
        note = `<div class="bubble-note">${escape(words.landed(sending.how, sending.machine, sending.why ?? null))}</div>`;
      }
      return `${who}<div class="bubble${state}">${plain(text)}</div>${note}`;
    }
    // Words from another resident, or the World's own telling: never in the
    // user's bubble. The head notes written for the model are said again
    // in the interface's words.
    const world = from.kind === "world";
    const told = readTold(text);
    let label = world ? words.world : `${from.address.resident} · ${from.address.machine}`;
    let how = "";
    let body: string;
    switch (told.kind) {
      case "job":
        body = `<p class="told-line">${escape(words.jobEnded(told.job, told.machine))}<span class="told-how">${escape(words.ended(told.how))}</span></p><p class="told-path">${escape(told.output)}</p>`;
        break;
      case "colleague":
        label = `${told.colleague} · ${told.machine}`;
        how = words.ended(told.how);
        body = `<div class="prose">${render(told.body)}</div>`;
        break;
      default:
        // The World's own words are plain text; another agent's are its
        // Markdown.
        body = world ? `<div class="told-plain">${plain(told.body)}</div>` : `<div class="prose">${render(told.body)}</div>`;
    }
    const caption = how ? `<span class="relayed-how">${escape(how)}</span>` : "";
    return `<div class="relayed${world ? " relayed-world" : ""}"><div class="relayed-from">${glyph(world ? "web" : "colleague", 16)}<span>${escape(label)}</span>${caption}</div><div class="relayed-text">${body}</div></div>`;
  }

  private group(group: Activity): string {
    const { words } = this.context;
    const tools = group.members.filter((member): member is ToolEntry => member.kind === "tool");
    const thinking = group.members.filter((member) => member.kind === "prose").length;
    const running =
      tools.some((tool) => tool.ended === null) || group.members.some((member) => member.kind === "prose" && member.writing);
    const folded = this.folded.has(group.key);
    const rows = group.members.map((member) => this.member(member)).join("");
    return `<button class="group-head${running ? " running" : ""}" data-fold="${group.key}" aria-expanded="${!folded}"><span class="group-title">${escape(groupHeading(tools, thinking, words))}</span>${glyph("chevron", 16)}</button><div class="fold${folded ? "" : " open"}"><div class="fold-inner"><div class="rows">${rows}</div></div></div>`;
  }

  private member(entry: Entry): string {
    const enter = this.settled && !this.seen.has(entry.key) ? " entering" : "";
    this.seen.add(entry.key);
    if (entry.kind === "prose") return this.thinking(entry, enter);
    if (entry.kind === "activity") {
      return `<div class="row${enter}"><span class="rail">${glyph("colleague")}</span><div class="row-line static"><span class="row-object">${escape(entry.text)}</span></div></div>`;
    }
    if (entry.kind !== "tool") return "";
    const { words } = this.context;
    const line = headline(entry.tool, entry.ended, words);
    const tense = tenseOf(entry.ended);
    const open = this.open.has(entry.key);
    const expandable = this.hasDetails(entry.tool);
    const elsewhere =
      entry.tool.machine && entry.tool.machine !== this.context.residentMachine
        ? `<span class="row-machine">${escape(words.on(entry.tool.machine))}</span>`
        : "";
    const stats =
      line.added !== null && line.removed !== null
        ? `<span class="row-stats"><span class="add">+${line.added}</span><span class="del">−${line.removed}</span></span>`
        : "";
    const mark = tense === "running" ? `<span class="pulse" aria-hidden="true"></span>` : glyph(line.glyph);
    const object = line.object
      ? `<span class="row-object"${line.title ? ` title="${escape(line.title)}"` : ""}>${escape(line.object)}</span>`
      : "";
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable
      ? `<div class="fold${open ? " open" : ""}"><div class="fold-inner">${open ? this.details(entry) : ""}</div></div>`
      : "";
    // A colleague it asked lives on in the World: its own conversation is a
    // step away.
    const colleague =
      entry.tool.does.as === "colleague" && entry.tool.does.to && this.context.link
        ? `<a class="row-link" href="${escape(this.context.link({ resident: entry.tool.does.to, machine: entry.tool.machine ?? this.context.residentMachine }))}">${escape(words.openColleague)}${glyph("chevron", 14)}</a>`
        : "";
    return `<div class="row row-tool row-${tense}${enter}" data-row="${entry.key}"><span class="rail">${mark}</span><button class="row-line" ${expandable ? `data-open="${entry.key}"` : "disabled"} aria-expanded="${open}"><span class="row-action">${escape(line.action)}</span>${object}${stats}${this.outcome(entry)}${elsewhere}${chevron}</button>${colleague}${fold}</div>`;
  }

  private thinking(entry: Extract<Entry, { kind: "prose" }>, enter: string): string {
    const { words } = this.context;
    // Its words are shown only where the host gave them; none are made up.
    const empty = !entry.text.trim();
    const expandable = !empty;
    const open = this.open.has(entry.key) || (entry.writing && !empty);
    // A summary of thinking comes in sections, each under a bold title of
    // the host's: the row says the latest while it thinks, the first after.
    const titles = [...entry.text.matchAll(/^\*\*(.+?)\*\*\s*$/gm)].map((match) => match[1]);
    const title = entry.writing ? titles[titles.length - 1] : titles[0];
    const label = entry.writing ? words.thinking : empty ? words.thoughtWithheld : words.thought;
    const said = title
      ? `<span class="row-action">${escape(label)}</span><span class="row-object">${escape(title)}</span>`
      : `<span class="row-action">${escape(label)}</span>`;
    const chevron = expandable ? `<span class="row-chevron">${glyph("chevron", 16)}</span>` : "";
    const fold = expandable
      ? `<div class="fold${open ? " open" : ""}"><div class="fold-inner"><div class="thinking-text prose">${render(entry.text)}</div></div></div>`
      : "";
    return `<div class="row row-thinking${entry.writing ? " row-running" : ""}${empty ? " withheld" : ""}${enter}" data-row="${entry.key}"><span class="rail">${glyph("thinking")}</span><button class="row-line" ${expandable ? `data-open="${entry.key}"` : "disabled"} aria-expanded="${open}">${said}${chevron}</button>${fold}</div>`;
  }

  private outcome(entry: ToolEntry): string {
    const { words } = this.context;
    const { tool } = entry;
    if (tool.does.as === "command") {
      if (tool.does.job) return `<span class="row-outcome went-on">${escape(words.wentOnAs(tool.does.job))}</span>`;
      if (tool.does.exit !== null && tool.does.exit !== 0) {
        return `<span class="row-outcome wrong">${escape(words.exited(tool.does.exit))}</span>`;
      }
    }
    switch (entry.ended?.use) {
      case "cut":
        return `<span class="row-outcome">${escape(words.cut)}</span>`;
      case "unknown":
        return `<span class="row-outcome">${escape(words.unknown)}</span>`;
      case "declined":
        return `<span class="row-outcome wrong">${escape(words.declined)}</span>`;
      default:
        return "";
    }
  }

  private hasDetails(tool: Tool): boolean {
    if (tool.does.as === "edit") return tool.does.changes.length > 0;
    if (tool.does.as === "command") return true;
    const given = tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}";
    return given || tool.output.length > 0;
  }

  /** What opens under a tool: a change as a diff, a command's words and
   *  what it printed, anything else's input and output. */
  private details(entry: ToolEntry): string {
    const { words } = this.context;
    const { tool } = entry;
    if (tool.does.as === "edit") {
      return tool.does.changes
        .map((change) => {
          const rows = parse(change.diff);
          const place = numbered(rows) ? "" : " unnumbered";
          return `<div class="surface diff"><div class="surface-head"><span class="surface-title" title="${escape(change.path)}">${escape(change.path)}</span></div><div class="diff-rows${place}">${renderRows(rows, change.path, words.unchangedLines)}</div></div>`;
        })
        .join("");
    }
    const whole = this.whole.has(entry.key);
    const output = textOf(tool.output);
    const images = imagesOf(tool.output);
    const given =
      tool.does.as === "command"
        ? tool.does.command
        : tool.input !== null && tool.input !== undefined && JSON.stringify(tool.input) !== "{}"
          ? JSON.stringify(tool.input, null, 2)
          : "";
    const parts: string[] = [];
    if (given) parts.push(`<div class="io-part"><span class="io-label">IN</span>${clip(given, 6, whole, entry.key, words)}</div>`);
    if (output || images) {
      parts.push(
        `<div class="io-part"><span class="io-label">OUT</span><div class="io-out">${output ? clip(output, 12, whole, entry.key, words) : ""}${images}</div></div>`,
      );
    }
    return `<div class="surface io">${parts.join("")}</div>`;
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-open], [data-fold], [data-whole], .code-copy");
    if (!target) return;
    if (target.classList.contains("code-copy")) {
      const code = target.parentElement?.querySelector("code")?.textContent ?? "";
      void navigator.clipboard?.writeText(code).then(() => {
        target.classList.add("copied");
        setTimeout(() => target.classList.remove("copied"), 1400);
      });
      return;
    }
    if (target.dataset.whole) {
      this.whole.add(target.dataset.whole);
      this.refill(target.dataset.whole);
      return;
    }
    if (target.dataset.fold) {
      const key = target.dataset.fold;
      const open = this.folded.has(key);
      if (open) this.folded.delete(key);
      else this.folded.add(key);
      target.setAttribute("aria-expanded", String(open));
      target.nextElementSibling?.classList.toggle("open", open);
      return;
    }
    const key = target.dataset.open;
    if (!key) return;
    const open = !this.open.has(key);
    if (open) this.open.add(key);
    else this.open.delete(key);
    target.setAttribute("aria-expanded", String(open));
    if (open) this.refill(key);
    target.nextElementSibling?.classList.toggle("open", open);
  }

  /** Fills a tool's fold with what it shows open, the first time it opens
   *  and whenever the reader asks for more of it. */
  private refill(key: string): void {
    const row = this.root.querySelector<HTMLElement>(`[data-row="${key}"]`);
    const inner = row?.querySelector<HTMLElement>(":scope > .fold > .fold-inner");
    const entry = this.conversation.entries.find((candidate) => candidate.key === key);
    if (!inner || entry?.kind !== "tool") return;
    inner.innerHTML = this.details(entry);
  }
}

function textOf(output: Content[]): string {
  return output
    .filter((content): content is Extract<Content, { type: "text" }> => content.type === "text")
    .map((content) => content.text)
    .join("\n");
}

function imagesOf(output: Content[]): string {
  return output
    .map((content) => {
      if (content.type === "image") return `<img class="io-image" alt="" src="data:${escape(content.mime)};base64,${content.base64}">`;
      if (content.type === "image_url") return `<img class="io-image" alt="" src="${escape(content.url)}">`;
      return "";
    })
    .join("");
}

/** Long text shown to a few lines with the rest a click away; all of it
 *  where the rest would be a line or two. */
function clip(text: string, keep: number, whole: boolean, key: string, words: Words): string {
  const lines = text.replace(/\n$/, "").split("\n");
  if (whole || lines.length <= keep + 2) return `<pre class="io-text">${escape(lines.join("\n"))}</pre>`;
  return `<pre class="io-text">${escape(lines.slice(0, keep).join("\n"))}</pre><button class="io-more" data-whole="${key}">${escape(words.moreLines(lines.length - keep))}</button>`;
}
