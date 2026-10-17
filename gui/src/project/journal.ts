// A journal, read: its description, then its entries in the order they
// were written, each under the stamp the World put on it, in a region that
// scrolls on its own under the page's head. Entries are
// never changed; what later entries said of one is shown beneath it, so
// the meaning that grew around it can be seen without a word of it moved.

import { fileLink } from "../app/places";
import { glyph } from "../conversation/glyphs";
import { escape, render } from "../conversation/prose";
import type { EntryView, JournalView, Writer } from "../world/world";
import type { ProjectWords } from "./words";
import { agentName } from "../world/names";

export interface JournalOptions {
  words: ProjectWords;
  /** The project it belongs to, to go back to. */
  project: { href: string; name: string };
  journal: string;
  head(title: string, actions?: string): string;
  /** Opens the sheet for the user's own entry. */
  write(): void;
  /** Asks for the entries before the first one shown. */
  earlier(before: string): Promise<JournalView>;
}

export class JournalPage {
  readonly element: HTMLElement;
  private body: HTMLElement;
  private shown: EntryView[] = [];
  private view: JournalView | null = null;

  constructor(private readonly options: JournalOptions) {
    this.element = document.createElement("div");
    this.element.className = "journal-page";
    this.element.innerHTML = `
      ${options.head(
        `<a class="crumb" href="${escape(options.project.href)}">${escape(options.project.name)}</a><span class="crumb-sep">/</span><h1 class="page-title journal-name-title">${escape(options.journal)}</h1>`,
        `<button class="text-button write-entry" type="button">${glyph("edit", 16)}<span>${escape(options.words.write)}</span></button>`,
      )}
      <div class="journal-reading scroller"></div>`;
    this.body = this.element.querySelector<HTMLElement>(".journal-reading")!;
    this.element.querySelector(".write-entry")!.addEventListener("click", () => options.write());
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

  /** The journal could not be read: said where it would be. */
  unreachable(why: string): void {
    this.body.innerHTML = `<p class="project-facts unreachable">${escape(why)}</p>`;
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
      <header class="journal-intro">
        ${about ? `<p class="project-lede">${escape(about.text)}</p><p class="project-facts">${escape(words.edited(this.writer(about.by), words.day(new Date(about.at))))}</p>` : ""}
        <p class="project-facts">${escape(words.shown(first, last, view.total))}<span class="sep"></span>${escape(words.writers(view.writers))}</p>
      </header>
      ${first > 1 ? `<button class="earlier" type="button">${escape(words.earlier)}</button>` : ""}
      <ol class="entries">${this.shown.map((entry) => this.entry(entry)).join("")}</ol>`;
  }

  private writer(writer: Writer): string {
    if (writer.kind === "user") return this.options.words.you;
    if (writer.kind === "resident") return `${writer.address.resident} ${writer.address.machine}`;
    if (writer.kind === "page") return this.options.words.aPage(writer.page.page);
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
      .join(`<span class="sep"></span>`);
    const cites = stamp.cites.length
      ? `<p class="entry-note">${escape(words.cites)} ${stamp.cites.map((id) => `<code>${escape(id)}</code>`).join(" ")}</p>`
      : "";
    const attachments = stamp.attachments.length
      ? `<p class="entry-note">${escape(words.attachments)}：${stamp.attachments
          .map(
            (file) =>
              `<span class="attachment">${
                // A text or an image kept where it was written opens there;
                // a text from its end.
                file.kept.kind === "on_machine" && (file.kind === "text" || file.kind === "image")
                  ? `<a href="${escape(fileLink(file.kept.location.machine, file.kept.location.path, file.kind === "text"))}">${escape(file.name)}</a>`
                  : escape(file.name)
              } <span class="faint">${escape(file.kind)} ${size(file.bytes)}${file.kept.kind === "on_machine" ? ` ${escape(words.onMachine(file.kept.location.machine))}` : ""}</span></span>`,
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
