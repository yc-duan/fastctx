// A computer meeting the World: what it is now, and the three ways on from
// there: join a World someone already has, found a new one, or stay a
// World of one. Joining asks an administrator once for the service that
// keeps the computer connected; afterwards the World's tools can be put
// into the agents on this computer. Shown on its own the first time a
// computer on its own opens the interface, and from the sidebar after.

import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { JoinKept, Legacy, Setup } from "../world/bridge";
import type { Page } from "./shell";
import type { Store } from "./store";
import type { WelcomeWords } from "./words";

export interface WelcomeOptions {
  store: Store;
  words: WelcomeWords;
  head(title: string, actions?: string): string;
  /** This interface runs on a phone: it joins as a device, and nothing
   *  is set up on it. */
  device: boolean;
  setup(): Promise<Setup>;
  place(): Promise<string>;
  join(code: string, name: string | null): Promise<{ world: string; name: string; kept: JoinKept | null }>;
  hubHere(addresses: string[]): Promise<string>;
  applyHosts(codex: boolean, claude: boolean): Promise<string[]>;
  /** What was done with what an old fastctx left, in a sentence. */
  legacy(legacy: Legacy | null): string;
  /** Done here: on to the World. */
  enter(): void;
}

type Step = "choose" | "join" | "found" | "joined";

export class WelcomePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly here: HTMLElement;
  private readonly body: HTMLElement;
  private step: Step = "choose";
  private hub: "here" | "server" = "here";
  private setup: Setup | null = null;
  private joined: { world: string; name: string; kept: JoinKept | null } | null = null;
  private busy = false;

  constructor(private readonly options: WelcomeOptions) {
    const { words } = options;
    this.title = "World";
    this.element = document.createElement("div");
    this.element.className = "welcome-page";
    this.element.innerHTML = `
      ${options.head("")}
      <div class="welcome scroller">
        <header class="welcome-hero">
          ${heroMark()}
          <h1 class="welcome-title">World</h1>
          <p class="welcome-lede">${escape(words.lede)}</p>
        </header>
        <section class="welcome-here"></section>
        <div class="welcome-body"></div>
      </div>`;
    this.here = this.element.querySelector<HTMLElement>(".welcome-here")!;
    this.body = this.element.querySelector<HTMLElement>(".welcome-body")!;
    this.element.addEventListener("click", (event) => void this.click(event));
    this.element.addEventListener("submit", (event) => {
      event.preventDefault();
      const form = event.target as HTMLFormElement;
      if (form.classList.contains("join-form")) void this.joinNow();
      if (form.classList.contains("found-form")) void this.foundNow();
    });
    this.draw();
    void this.refreshSetup();
  }

  private async refreshSetup(): Promise<void> {
    try {
      this.setup = await this.options.setup();
    } catch {
      this.setup = null;
    }
    this.drawHere();
  }

  private drawHere(): void {
    const { words, store } = this.options;
    const name = store.world.peek()?.here ?? "";
    const world = store.world.peek()?.nodes.world ?? null;
    const setup = this.setup;
    const facts = setup
      ? [
          setup.installed ? words.placed : setup.carried ? words.notPlaced : words.noCarried,
          setup.service ? words.serviceOn : words.serviceOff,
        ]
      : [];
    this.here.innerHTML = `
      <h2 class="section-title">${escape(this.options.device ? words.thisPhone : words.thisComputer)}</h2>
      <p class="welcome-facts">${[name ? words.facts(name, world) : "", ...facts]
        .filter(Boolean)
        .map((fact) => `<span class="fact">${escape(fact)}</span>`)
        .join(`<span class="sep">·</span>`)}
        ${setup && !setup.installed && setup.carried ? `<button class="text-button place-now" type="button">${escape(words.place)}</button>` : ""}</p>
      ${setup && this.options.legacy(setup.legacy) ? `<p class="welcome-legacy">${escape(this.options.legacy(setup.legacy))}</p>` : ""}`;
  }

  /** Moves to another step; its first field, or its first button, takes
   *  focus, as a new page would. */
  private go(step: Step): void {
    this.step = step;
    this.draw();
    this.body.querySelector<HTMLElement>("input:not([type=hidden]), textarea, button")?.focus({ preventScroll: true });
  }

  private draw(): void {
    const { words } = this.options;
    switch (this.step) {
      case "choose":
        this.body.innerHTML = `
          <ul class="welcome-paths">
            ${(this.options.device
              ? [["join", words.join, words.joinPhoneWhat]]
              : [
                  ["join", words.join, words.joinWhat],
                  ["found", words.found, words.foundWhat],
                  ["alone", words.alone, words.aloneWhat],
                ]
            )
              .map(
                ([path, title, what]) =>
                  `<li><button class="welcome-path" type="button" data-path="${path}"><span class="welcome-path-title">${escape(title!)}</span><span class="welcome-path-what">${escape(what!)}</span></button></li>`,
              )
              .join("")}
          </ul>`;
        break;
      case "join":
        this.body.innerHTML = `
          <form class="join-form welcome-form">
            <h2 class="welcome-form-title">${escape(words.join)}</h2>
            <label class="field"><span class="field-label">${escape(words.code)}</span>
              <textarea name="code" rows="4" class="mono" required spellcheck="false" placeholder="${escape(words.codeHint)}"></textarea></label>
            <label class="field"><span class="field-label">${escape(words.name)}</span>
              <input name="name" class="mono" autocomplete="off" spellcheck="false" pattern="[a-z][a-z0-9-]{0,31}" placeholder="${escape(this.options.store.world.peek()?.here ?? "")}">
              <span class="field-hint">${escape(words.nameHint)}</span></label>
            <p class="sheet-note" role="alert" hidden></p>
            <footer class="welcome-foot"><button class="text-button back" type="button">${escape(words.back)}</button><button class="sheet-go" type="submit">${escape(words.joinGo)}</button></footer>
          </form>`;
        this.body.querySelector<HTMLTextAreaElement>("[name=code]")?.focus();
        break;
      case "found":
        this.body.innerHTML = `
          <form class="found-form welcome-form">
            <h2 class="welcome-form-title">${escape(words.found)}</h2>
            <div class="field"><span class="field-label">${escape(words.hubWhere)}</span>
              <div class="choice-options" role="radiogroup" aria-label="${escape(words.hubWhere)}">${(
                [
                  ["here", words.hubHere],
                  ["server", words.hubServer],
                ] as const
              )
                .map(
                  ([where, label]) =>
                    `<button type="button" class="choice${this.hub === where ? " chosen" : ""}" role="radio" aria-checked="${this.hub === where}" data-hub="${where}">${escape(label)}</button>`,
                )
                .join("")}</div></div>
            ${
              this.hub === "here"
                ? `<label class="field"><span class="field-label">${escape(words.address)}</span>
              <input name="address" class="mono" required autocomplete="off" spellcheck="false" placeholder="203.0.113.5:7443">
              <span class="field-hint">${escape(words.addressHint)}</span></label>`
                : `<ol class="welcome-steps">${words.serverSteps.map((step) => `<li>${escape(step)}</li>`).join("")}</ol>`
            }
            <p class="sheet-note" role="alert" hidden></p>
            <footer class="welcome-foot"><button class="text-button back" type="button">${escape(words.back)}</button>${
              this.hub === "here"
                ? `<button class="sheet-go" type="submit">${escape(words.hubGo)}</button>`
                : `<button class="sheet-go to-join" type="button">${escape(words.join)}</button>`
            }</footer>
          </form>`;
        break;
      case "joined": {
        const joined = this.joined!;
        const kept = !joined.kept
          ? ""
          : joined.kept.kind === "command"
            ? `<p>${escape(words.keptCommand)}</p><pre class="welcome-command mono">${escape(joined.kept.command)}</pre>`
            : `<p>${escape(words.kept[joined.kept.kind])}</p>`;
        const setup = this.setup;
        const host = (key: "codex" | "claude", label: string) => {
          const state = setup ? setup[key] : "absent";
          return `<label class="host-row"><input type="checkbox" name="${key}" ${state === "out" || state === "old" ? "checked" : ""} ${state === "absent" || state === "in" ? "disabled" : ""}><span class="host-name">${escape(label)}</span><span class="host-state">${escape(words.hostState[state])}</span></label>`;
        };
        this.body.innerHTML = `
          <div class="welcome-form">
            <h2 class="welcome-form-title">${escape(words.joined(joined.world, joined.name))}</h2>
            ${kept ? `<div class="welcome-kept">${kept}</div>` : ""}
            ${
              this.options.device
                ? `<p class="welcome-kept">${escape(words.phoneJoined)}</p>`
                : `<section class="welcome-hosts">
              <h3 class="field-label">${escape(words.hosts)}</h3>
              <p class="field-hint">${escape(words.hostsWhat)}</p>
              ${host("codex", words.codex)}${host("claude", words.claude)}
              <button class="text-button apply-now" type="button">${escape(words.applyGo)}</button>
              <p class="sheet-note" role="alert" hidden></p>
            </section>`
            }
            <footer class="welcome-foot"><span></span><button class="sheet-go enter" type="button">${escape(words.enter)}</button></footer>
          </div>`;
        break;
      }
    }
  }

  private note(text: string): void {
    const note = this.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.textContent = text;
    note.hidden = !text;
  }

  private async click(event: Event): Promise<void> {
    const target = event.target as HTMLElement;
    const path = target.closest<HTMLElement>("[data-path]")?.dataset.path;
    if (path === "alone") return this.options.enter();
    if (path === "join" || path === "found") return this.go(path);
    const hub = target.closest<HTMLElement>("[data-hub]")?.dataset.hub;
    if (hub === "here" || hub === "server") {
      this.hub = hub;
      return redrawKeepingFocus(this.body, () => this.draw());
    }
    if (target.closest(".back")) return this.go("choose");
    if (target.closest(".to-join")) return this.go("join");
    if (target.closest(".enter")) return this.options.enter();
    if (target.closest(".place-now")) {
      await this.options.place().catch((error: unknown) => this.note(this.options.words.failed(String(error))));
      return this.refreshSetup();
    }
    if (target.closest(".apply-now") && !this.busy) {
      const codex = this.body.querySelector<HTMLInputElement>("[name=codex]")?.checked ?? false;
      const claude = this.body.querySelector<HTMLInputElement>("[name=claude]")?.checked ?? false;
      if (!codex && !claude) return;
      const button = target.closest<HTMLButtonElement>(".apply-now")!;
      this.busy = true;
      button.disabled = true;
      button.textContent = this.options.words.applying;
      try {
        const notes = await this.options.applyHosts(codex, claude);
        await this.refreshSetup();
        this.draw();
        if (notes.length) this.note(notes.join("\n"));
      } catch (error) {
        this.note(this.options.words.failed(String(error)));
        button.disabled = false;
        button.textContent = this.options.words.applyGo;
      } finally {
        this.busy = false;
      }
    }
  }

  private async joinNow(code?: string): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    const text = code ?? this.body.querySelector<HTMLTextAreaElement>("[name=code]")?.value.trim() ?? "";
    const name = this.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
    if (!text) return;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go");
    this.busy = true;
    if (go) {
      go.disabled = true;
      go.textContent = words.joining;
    }
    this.note("");
    try {
      this.joined = await this.options.join(text, name);
      this.busy = false;
      await this.refreshSetup();
      void this.options.store.refresh();
      this.step = "joined";
      this.draw();
    } catch (error) {
      this.busy = false;
      if (go) {
        go.disabled = false;
        go.textContent = code ? words.hubGo : words.joinGo;
      }
      this.note(words.failed(String(error)));
    }
  }

  private async foundNow(): Promise<void> {
    if (this.busy) return;
    const { words } = this.options;
    const address = this.body.querySelector<HTMLInputElement>("[name=address]")?.value.trim() ?? "";
    if (!address) return;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.busy = true;
    go.disabled = true;
    go.textContent = words.founding;
    this.note("");
    let code: string;
    try {
      code = await this.options.hubHere(address.split(/[\s,]+/).filter(Boolean));
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      go.textContent = words.hubGo;
      this.note(words.failed(String(error)));
      return;
    }
    this.busy = false;
    // The founding code makes this computer the World's first machine.
    await this.joinNow(code);
  }
}

/** The World's mark, large, drawn with the marker: rings round a knot. */
function heroMark(): string {
  return `<svg class="welcome-mark" viewBox="0 0 120 96" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke="currentColor" stroke-width="3.2" stroke-linecap="round"><circle cx="60" cy="50" r="7"/><circle cx="22" cy="24" r="14"/><circle cx="98" cy="27" r="14"/><circle cx="58" cy="82" r="11"/><path d="M34 31 L53 45M86 34 L67 46M59 57 L58 70"/></g></svg>`;
}
