// A page, open: its title under the project it belongs to, who wrote this
// version and when, and the page itself filling the rest of the screen,
// scrolling in its own frame. The first time a version wants to do things
// the user has not agreed to, it asks, once; what was agreed is in the
// project's ledger, so another device does not ask again. An older version
// can be looked at and brought back; the page can be taken down. Every one
// of these is a version in the ledger, and nothing is lost.

import { Confirm, Sheet } from "../app/confirm";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Address } from "../world/events";
import { holds, sameAct, unconsented, type PageAct, type PageConsent, type PageVersion, type PageView, type PageWriter } from "../world/pages";
import { compose } from "./compose";
import { mountFrame } from "./frame";
import { PageHost, type HostContext } from "./host";
import type { PageWords } from "./words";
import { agentName } from "../world/names";
import { HeldReads } from "../world/reads";

export interface PageWorld {
  read(version: number | null): Promise<PageView>;
  history(): Promise<PageVersion[]>;
  consent(version: number, base: number | null, acts: PageAct[]): Promise<PageConsent>;
  withdraw(base: number): Promise<unknown>;
  restore(base: number, version: number): Promise<unknown>;
}

export interface PageScreenOptions {
  words: PageWords;
  page: string;
  /** The version asked for; `null` is the current one. */
  version: number | null;
  project: { href: string; name: string };
  head(title: string, actions?: string): string;
  /** Where this page is at a version, or its current one. */
  link(version: number | null): string;
  ago(at: Date): string;
  /** "Codex lab", for the resident at an address. */
  someone(address: Address): string;
  world: PageWorld;
  /** The tools an open version calls, carried out for it; the files they
   *  read are held by `reads`. */
  tools(view: PageView, reads: HeldReads): (tool: string, args: Record<string, unknown>) => Promise<unknown>;
  context(): HostContext;
  /** The interface's version, told to the page. */
  version_: string;
  confirm: Confirm;
  /** The title changed, for the window and the sidebar. */
  retitle(title: string): void;
  /** The current version changed: the project's list may differ. */
  changed(): void;
}

export class PageScreen {
  readonly element: HTMLElement;
  private readonly title: HTMLElement;
  private readonly about: HTMLElement;
  private readonly notice: HTMLElement;
  private readonly stage: HTMLElement;
  private readonly sheet = new Sheet("form-sheet page-sheet");
  /** The files the open version's tools read, let go of when it goes. */
  private readonly reads = new HeldReads();
  private view: PageView | null = null;
  private host: PageHost | null = null;
  private asked = false;
  private left = false;

  constructor(private readonly options: PageScreenOptions) {
    const { words } = options;
    this.element = document.createElement("div");
    this.element.className = "page-screen";
    this.element.innerHTML = `
      ${options.head(
        `<a class="crumb" href="${escape(options.project.href)}">${escape(options.project.name)}</a><span class="crumb-sep">/</span><h1 class="page-title">${escape(options.page)}</h1>`,
        `<button class="text-button page-acts" type="button" hidden>${glyph("tool", 16)}<span>${escape(words.acts)}</span></button>
         <button class="text-button page-versions" type="button">${glyph("versions", 16)}<span>${escape(words.versions)}</span></button>
         <button class="icon-button page-reload" type="button" aria-label="${escape(words.reload)}" title="${escape(words.reload)}">${glyph("reread", 18)}</button>`,
      )}
      <p class="page-about"></p>
      <div class="page-notice" hidden></div>
      <div class="page-stage"></div>`;
    this.title = this.element.querySelector<HTMLElement>(".page-title")!;
    this.about = this.element.querySelector<HTMLElement>(".page-about")!;
    this.notice = this.element.querySelector<HTMLElement>(".page-notice")!;
    this.stage = this.element.querySelector<HTMLElement>(".page-stage")!;
    this.element.querySelector(".page-acts")!.addEventListener("click", () => this.showActs());
    this.element.querySelector(".page-versions")!.addEventListener("click", () => void this.showVersions());
    this.element.querySelector(".page-reload")!.addEventListener("click", () => void this.load());
    this.notice.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const button = target.closest<HTMLButtonElement>("[data-bring]");
      if (button) this.bringBack(Number(button.dataset.bring));
      if (target.closest("[data-reload]")) void this.load();
    });
    void this.load();
  }

  /** The theme or the language changed: the page hears it. */
  contextChanged(): void {
    this.host?.contextChanged();
  }

  leave(): void {
    this.left = true;
    this.host?.close();
    this.reads.release();
    this.sheet.close();
  }

  private async load(): Promise<void> {
    try {
      const view = await this.options.world.read(this.options.version);
      if (!this.left) this.show(view);
    } catch (error) {
      if (this.left) return;
      this.host?.close();
      this.host = null;
      this.stage.replaceChildren();
      this.notice.hidden = false;
      const why = error instanceof Error ? error.message : String(error);
      this.notice.innerHTML = `<p class="unreachable" title="${escape(why)}">${escape(this.options.words.failed(why))}</p>`;
    }
  }

  private show(view: PageView): void {
    const { words } = this.options;
    this.view = view;
    const version = view.publication;
    this.title.textContent = version.title || view.page;
    this.options.retitle(version.title || view.page);
    this.about.innerHTML = [
      escape(words.version(version.version)),
      escape(words.written(this.writer(version.by), this.options.ago(new Date(version.at)))),
      version.description ? `<span class="page-description">${escape(version.description)}</span>` : "",
    ]
      .filter(Boolean)
      .join(`<span class="sep"></span>`);
    const acts = this.element.querySelector<HTMLButtonElement>(".page-acts")!;
    acts.hidden = version.acts.length === 0 || version.withdrawn;
    this.mount(view);
    void this.drawNotice(view);
    if (this.current(view) && !version.withdrawn && unconsented(view).length && !this.asked) {
      this.asked = true;
      this.ask(view);
    }
  }

  /** Whether the version shown is the current one. */
  private current(view: PageView): boolean {
    return this.options.version === null || this.options.version === view.publication.version;
  }

  private mount(view: PageView): void {
    this.host?.close();
    this.reads.release();
    this.host = null;
    this.stage.replaceChildren();
    if (view.html === null) return;
    const context = this.options.context();
    const html = compose(view.html, { theme: context.theme, locale: context.locale });
    const frame = mountFrame(this.stage, html, view.publication.title || view.page);
    this.host = new PageHost({
      frame,
      version: this.options.version_,
      context: this.options.context,
      call: this.options.tools(view, this.reads),
      left: (error) => {
        // A page that navigates its own frame away is closed, and said so.
        this.host = null;
        this.stage.replaceChildren();
        this.notice.hidden = false;
        this.notice.innerHTML = `<p>${escape(error ?? this.options.words.wentAway)}</p><button class="text-button" type="button" data-reload>${escape(this.options.words.reload)}</button>`;
      },
    });
  }

  private async drawNotice(view: PageView): Promise<void> {
    const { words } = this.options;
    const version = view.publication;
    let said = "";
    if (version.withdrawn) {
      const last = (await this.options.world.history().catch(() => [] as PageVersion[])).find((v) => !v.withdrawn && v.version < version.version);
      said = `<p>${escape(words.withdrawnNow(version.version))}</p>${last ? `<button class="text-button" type="button" data-bring="${last.version}">${escape(words.bringBackLast(last.version))}</button>` : ""}`;
    } else if (!this.current(view)) {
      const history = await this.options.world.history().catch(() => [] as PageVersion[]);
      const latest = history.reduce((most, v) => Math.max(most, v.version), version.version);
      if (latest !== version.version) {
        said = `<p>${escape(words.older(version.version, latest))}</p><a class="text-button" href="${escape(this.options.link(null))}">${escape(words.seeCurrent)}</a><button class="text-button" type="button" data-bring="${version.version}">${escape(words.bringBack)}</button>`;
      }
    }
    if (this.left || this.view !== view) return;
    this.notice.hidden = !said;
    this.notice.innerHTML = said;
  }

  private writer(by: PageWriter): string {
    switch (by.kind) {
      case "user":
        return this.options.words.you;
      case "resident":
        return this.options.someone(by.address);
      case "session":
        return agentName(by.agent);
      case "page":
        return by.page.page;
      case "elsewhere":
        return this.options.words.elsewhere(by.page);
    }
  }

  /** One act, said as what it is. */
  private act(act: PageAct): string {
    const { words } = this.options;
    switch (act.kind) {
      case "run":
        return `<span class="act-what">${escape(words.run(act.machine))}</span><code class="act-command">${escape(act.script)}</code>`;
      case "say":
        return `<span class="act-what">${escape(words.say(this.options.someone(act.to)))}</span><span class="act-detail">${escape(`${act.to.resident} ${act.to.machine}`)}</span>`;
      case "journal":
        return `<span class="act-what">${escape(words.journal(act.journal))}</span>`;
    }
  }

  private actList(acts: PageAct[], ticked: (act: PageAct) => boolean | null): string {
    return `<ul class="page-act-list">${acts
      .map((act, index) => {
        const tick = ticked(act);
        return tick === null
          ? `<li class="page-act">${this.act(act)}</li>`
          : `<li class="page-act"><label><input type="checkbox" data-act="${index}"${tick ? " checked" : ""}><span class="page-act-body">${this.act(act)}</span></label></li>`;
      })
      .join("")}</ul>`;
  }

  /** The first time a version wants to do what the user has not agreed to. */
  private ask(view: PageView): void {
    const { words } = this.options;
    const asked = unconsented(view);
    const agreed = view.publication.acts.filter((act) => holds(view.consent?.acts ?? [], act));
    const { sheet } = this;
    sheet.body.innerHTML = `
      ${sheet.head(words.askTitle, words.close)}
      <p class="confirm-what">${escape(words.askWhat(asked.length))}</p>
      ${this.actList(asked, () => true)}
      ${agreed.length ? `<h3 class="page-act-heading">${escape(words.agreedBefore)}</h3>${this.actList(agreed, () => null)}` : ""}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><button class="text-button sheet-close" type="button">${escape(words.notNow)}</button><button class="sheet-go" type="submit">${escape(words.agree)}</button></footer>`;
    this.ticking(asked.length, words.agree, words.agreeChosen);
    this.submit(async () => {
      const chosen = asked.filter((_, index) => this.ticked(index));
      return this.consent(view, [...(view.consent?.acts ?? []), ...chosen]);
    });
    sheet.show();
  }

  /** What this version may do, and what the user agreed to, to change. */
  private showActs(): void {
    const view = this.view;
    if (!view) return;
    const { words } = this.options;
    const declared = view.publication.acts;
    const agreed = view.consent?.acts ?? [];
    const { sheet } = this;
    sheet.body.innerHTML = `
      ${sheet.head(words.actsTitle, words.close)}
      ${declared.length ? this.actList(declared, (act) => holds(agreed, act)) : `<p class="confirm-what">${escape(words.noActs)}</p>`}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot">${agreed.length ? `<button class="text-button page-revoke" type="button">${escape(words.revoke)}</button>` : "<span></span>"}<button class="sheet-go" type="submit">${escape(words.change)}</button></footer>`;
    sheet.body.querySelector(".page-revoke")?.addEventListener("click", () => void this.run(() => this.consent(view, [])));
    this.submit(async () => {
      // What this version does not declare stays as the user left it: a
      // later version that wants it back is not asked again.
      const kept = agreed.filter((act) => !declared.some((own) => sameAct(own, act)));
      return this.consent(view, [...kept, ...declared.filter((_, index) => this.ticked(index))]);
    });
    sheet.show();
  }

  private async consent(view: PageView, acts: PageAct[]): Promise<void> {
    const given = await this.options.world.consent(view.publication.version, view.consent?.version ?? null, acts);
    if (this.view === view) this.view = { ...view, consent: given };
  }

  private async showVersions(): Promise<void> {
    const view = this.view;
    if (!view) return;
    const { words } = this.options;
    const { sheet } = this;
    const history = [...(await this.options.world.history().catch(() => [] as PageVersion[]))].sort((a, b) => b.version - a.version);
    const latest = history[0];
    sheet.body.innerHTML = `
      ${sheet.head(words.versionsTitle, words.close)}
      <ol class="page-versions-list">${history
        .map((version) => {
          const marks = [
            version === latest ? words.current : "",
            version.withdrawn ? words.withdrawnMark : "",
            version.restored_from !== null ? words.restoredFrom(version.restored_from) : "",
          ].filter(Boolean);
          const shown = version.version === view.publication.version;
          return `<li class="page-version${shown ? " shown" : ""}">
            <div class="page-version-line"><span class="page-version-n">${escape(words.version(version.version))}</span>${marks.map((mark) => `<span class="tag">${escape(mark)}</span>`).join("")}</div>
            <div class="page-version-by">${escape(words.written(this.writer(version.by), this.options.ago(new Date(version.at))))}${version.title ? ` ${escape(version.title)}` : ""}</div>
            <div class="page-version-go">${
              version.withdrawn || shown ? "" : `<a class="text-button" href="${escape(this.options.link(version === latest ? null : version.version))}">${escape(words.look)}</a>`
            }${version === latest || version.withdrawn ? "" : `<button class="text-button" type="button" data-bring="${version.version}">${escape(words.bringBack)}</button>`}</div>
          </li>`;
        })
        .join("")}</ol>
      <footer class="sheet-foot">${latest && !latest.withdrawn ? `<button class="text-button danger page-withdraw" type="button">${escape(words.withdraw)}</button>` : "<span></span>"}<button class="sheet-go sheet-close" type="button">${escape(words.close)}</button></footer>`;
    sheet.body.onsubmit = (event) => event.preventDefault();
    sheet.body.querySelectorAll<HTMLButtonElement>("[data-bring]").forEach((button) =>
      button.addEventListener("click", () => {
        sheet.close();
        this.bringBack(Number(button.dataset.bring));
      }),
    );
    sheet.body.querySelector(".page-withdraw")?.addEventListener("click", () => {
      sheet.close();
      if (latest) this.takeDown(latest.version);
    });
    sheet.body.querySelectorAll("a[href]").forEach((a) => a.addEventListener("click", () => sheet.close()));
    sheet.show();
  }

  private bringBack(version: number): void {
    const { words } = this.options;
    void this.options.world.history().then((history) => {
      const latest = history.reduce((most, v) => Math.max(most, v.version), 0);
      this.options.confirm.ask({
        title: words.bringBack,
        what: words.bringBackWhat(version),
        go: words.bringBack,
        act: async () => {
          await this.options.world.restore(latest, version);
        },
        done: () => this.toCurrent(),
      });
    });
  }

  private takeDown(base: number): void {
    const { words } = this.options;
    this.options.confirm.ask({
      title: words.withdrawTitle,
      what: words.withdrawWhat,
      go: words.withdraw,
      act: async () => {
        await this.options.world.withdraw(base);
      },
      done: () => this.toCurrent(),
    });
  }

  /** After a new version: the current one, read again. */
  private toCurrent(): void {
    this.options.changed();
    const current = this.options.link(null);
    if (location.hash !== current) location.hash = current;
    else void this.load();
  }

  private ticking(count: number, all: string, some: string): void {
    const go = this.sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.sheet.body.addEventListener("change", () => {
      const chosen = Array.from({ length: count }, (_, index) => this.ticked(index)).filter(Boolean).length;
      go.textContent = chosen === count ? all : some;
      go.disabled = chosen === 0;
    });
  }

  private ticked(index: number): boolean {
    return this.sheet.body.querySelector<HTMLInputElement>(`input[data-act="${index}"]`)?.checked ?? false;
  }

  private submit(work: () => Promise<void>): void {
    this.sheet.body.onsubmit = (event) => {
      event.preventDefault();
      void this.run(work);
    };
  }

  /** Does what the sheet asked, saying it there if the World refuses. */
  private async run(work: () => Promise<void>): Promise<void> {
    const { sheet } = this;
    if (sheet.busy) return;
    const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note");
    const label = go?.textContent ?? "";
    sheet.busy = true;
    if (go) {
      go.disabled = true;
      go.textContent = this.options.words.working;
    }
    try {
      await work();
      sheet.busy = false;
      sheet.close();
    } catch (error) {
      sheet.busy = false;
      if (go) {
        go.disabled = false;
        go.textContent = label;
      }
      if (note) {
        note.textContent = this.options.words.failed(error instanceof Error ? error.message : String(error));
        note.title = error instanceof Error ? error.message : String(error);
        note.hidden = false;
      }
    }
  }
}
