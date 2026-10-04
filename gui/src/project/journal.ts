// A journal, read: its description, then its entries in the order they
// were written, each under the stamp the World put on it. Entries are
// never changed; what later entries said of one is shown beneath it, so
// the meaning that grew around it can be seen without a word of it moved.

import { glyph } from "../conversation/glyphs";
import { escape, render } from "../conversation/prose";
import type { EntryView, JournalView, Writer } from "../world/world";
import type { ProjectWords } from "./words";

export interface JournalOptions {
  words: ProjectWords;
  back: { href: string; label: string };
  /** Asks for the entries before the first one shown. */
  earlier(before: string): Promise<JournalView>;
}

const agentName = (agent: string) => (agent === "claude" ? "Claude" : agent === "codex" ? "Codex" : agent);

export class JournalPage {
  readonly element: HTMLElement;
  private body: HTMLElement;
  private shown: EntryView[] = [];
  private view: JournalView | null = null;

  constructor(private readonly options: JournalOptions) {
    this.element = document.createElement("div");
    this.element.className = "page journal-page";
    this.element.innerHTML = `
      <header class="bar">
        <div class="bar-inner">
          <a class="bar-back" href="${escape(options.back.href)}" aria-label="${escape(options.back.label)}">${glyph("chevron", 20)}</a>
          <div class="who"><div class="who-state">${escape(options.words.journals)}</div></div>
        </div>
      </header>
      <main class="home journal-reading"></main>`;
    this.body = this.element.querySelector<HTMLElement>(".journal-reading")!;
    this.body.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".earlier");
      if (!button || !this.view || !this.shown[0]) return;
      button.disabled = true;
      void this.options
        .earlier(this.shown[0].stamp.id)
        .then((view) => {
          this.view = { ...view, first: view.first };
          this.shown = [...view.entries, ...this.shown];
          this.draw();
        })
        .catch((error: unknown) => {
          button.disabled = false;
          button.textContent = String(error);
        });
    });
  }

  /** The journal could not be read: said in the bar. */
  unreachable(why: string): void {
    const state = this.element.querySelector<HTMLElement>(".who-state")!;
    state.textContent = why;
    state.classList.add("gone");
  }

  show(view: JournalView): void {
    this.view = view;
    this.shown = view.entries;
    this.draw();
  }

  private draw(): void {
    const view = this.view;
    if (!view) return;
    const { words } = this.options;
    const first = view.first;
    const last = first + this.shown.length - 1;
    const about = view.description;
    this.body.innerHTML = `
      <header class="project-head">
        <h1 class="project-title journal-title">${escape(view.journal)}</h1>
        ${about ? `<p class="project-lede">${escape(about.text)}</p><p class="project-facts">${escape(words.edited(this.writer(about.by), words.day(new Date(about.at))))}</p>` : ""}
        <p class="project-facts">${escape(words.shown(first, last, view.total))}<span class="sep">·</span>${escape(words.writers(view.writers))}</p>
      </header>
      ${first > 1 ? `<button class="earlier" type="button">${escape(words.earlier)}</button>` : ""}
      <ol class="entries">${this.shown.map((entry) => this.entry(entry)).join("")}</ol>`;
  }

  private writer(writer: Writer): string {
    if (writer.kind === "user") return this.options.words.you;
    if (writer.kind === "resident") return `${writer.address.resident} · ${writer.address.machine}`;
    return agentName(writer.agent);
  }

  private entry(entry: EntryView): string {
    const { words } = this.options;
    const stamp = entry.stamp;
    const at = new Date(stamp.at);
    const who = stamp.agent ? `${agentName(stamp.agent.agent)} ${stamp.agent.version}` : this.writer(stamp.by);
    const model = stamp.model
      ? `${stamp.model.name}${stamp.model.self_reported ? ` (${words.selfReported})` : ""}`
      : "";
    const head = [`${words.day(at)} ${words.clock(at)}`, `${who} ${words.onMachine(stamp.machine)}`, model]
      .filter(Boolean)
      .map(escape)
      .join(`<span class="sep">·</span>`);
    const cites = stamp.cites.length
      ? `<p class="entry-note">${escape(words.cites)} ${stamp.cites.map((id) => `<code>${escape(id)}</code>`).join(" ")}</p>`
      : "";
    const attachments = stamp.attachments.length
      ? `<p class="entry-note">${escape(words.attachments)}：${stamp.attachments
          .map(
            (file) =>
              `<span class="attachment">${escape(file.name)} <span class="faint">${escape(file.kind)} · ${size(file.bytes)}${file.kept.kind === "on_machine" ? ` · ${escape(words.onMachine(file.kept.location.machine))}` : ""}</span></span>`,
          )
          .join("、")}</p>`
      : "";
    const mentioned = entry.mentioned_by.length
      ? `<p class="entry-note">${escape(words.mentionedBy)} ${entry.mentioned_by.map((id) => `<code>${escape(id)}</code>`).join(" ")}</p>`
      : "";
    return `<li class="entry" id="${escape(stamp.id)}">
      <p class="entry-stamp">${head}</p>
      ${cites}
      <div class="entry-text prose">${render(entry.text)}</div>
      ${attachments}${mentioned}
      <p class="entry-id"><code>${escape(stamp.id)}</code></p>
    </li>`;
  }
}

function size(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}
