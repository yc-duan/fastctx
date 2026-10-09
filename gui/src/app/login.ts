// Logging in (`world-gui.md`, 账号、机器与授权): the server, the account and
// its password, as MeshCentral Router asks them; or a one-time token another
// device made, scanned on a phone; or, on a hub that lets anyone, a new
// account. A computer registers as one of the account's machines unless
// told not to, and is kept connected by a startup service afterwards; a
// phone only logs in. The first time a computer on its own opens the
// interface it is shown by itself; after that it is how another account is
// added. Once in, the World's tools can be put into this computer's agents.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { LoggedIn, Legacy, Setup } from "../world/bridge";
import { loginsOf } from "../world/accounts";
import type { LoginWords } from "./account-words";
import { loginLabel } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface LoginOptions {
  store: Store;
  words: LoginWords;
  head(title: string, actions?: string): string;
  /** This interface runs on a phone: it logs in as a device, and nothing
   *  is set up on it. */
  device: boolean;
  /** Where this computer stands, where the shell can say. */
  setup?: (() => Promise<Setup>) | undefined;
  place?: (() => Promise<string>) | undefined;
  /** What was done with what an old fastctx left, in a sentence. */
  legacy(legacy: Legacy | null): string;
  login(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn>;
  loginToken(token: string, name: string | null): Promise<LoggedIn>;
  /** Opening an account, where this interface can: a computer can, a
   *  phone logs in only. */
  openAccount?: ((hub: string, account: string, password: string, register: boolean, name: string | null) => Promise<LoggedIn>) | undefined;
  /** Reads a token's QR code with the camera, where the shell can; null
   *  when the person turned back. */
  scan?: (() => Promise<string | null>) | undefined;
  applyHosts?: ((codex: boolean, claude: boolean) => Promise<string[]>) | undefined;
  /** Done here: on to the World. */
  enter(): void;
}

type Step = "password" | "open" | "token" | "done";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";
const machineRule = "[a-z][a-z0-9\\-]{0,31}";

export class LoginPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly here: HTMLElement;
  private readonly body: HTMLElement;
  private step: Step = "password";
  private setup: Setup | null = null;
  private done: LoggedIn | null = null;
  private busy = false;
  /** What was typed, kept when moving between the ways in. */
  private typed = { hub: "", account: "", register: true };
  private readonly stop: () => void;

  constructor(private readonly options: LoginOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "welcome-page login-page";
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
    this.element.addEventListener("change", (event) => {
      if ((event.target as HTMLInputElement).name === "register") this.drawName();
    });
    this.element.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.draw();
    void this.refreshSetup();
    // Who this computer is, and the logins it has, as the World answers.
    let shown = "";
    this.stop = effect(() => {
      const world = options.store.world.get();
      const said = JSON.stringify([world?.here, loginsOf(world?.accounts).map(loginLabel)]);
      if (said === shown) return;
      shown = said;
      this.drawHere();
      if (this.step === "password" || this.step === "open") this.drawName();
    });
  }

  leave(): void {
    this.stop();
  }

  private async refreshSetup(): Promise<void> {
    if (!this.options.setup) return;
    try {
      this.setup = await this.options.setup();
    } catch {
      this.setup = null;
    }
    this.drawHere();
    if (this.step === "done") this.draw();
  }

  private drawHere(): void {
    const { words, store } = this.options;
    const world = store.world.peek();
    const name = world?.here ?? "";
    const setup = this.setup;
    const facts = [
      name ? words.facts(name) : "",
      ...(setup
        ? [setup.installed ? words.placed : setup.carried ? words.notPlaced : words.noCarried, setup.service ? words.serviceOn : words.serviceOff]
        : []),
    ].filter(Boolean);
    const logins = loginsOf(world?.accounts).map(loginLabel);
    const legacy = setup ? this.options.legacy(setup.legacy) : "";
    this.here.innerHTML = `
      <h2 class="section-title">${escape(this.options.device ? words.thisPhone : words.thisComputer)}</h2>
      <p class="welcome-facts">${facts.map((fact) => `<span class="fact">${escape(fact)}</span>`).join(`<span class="sep">·</span>`)}
        ${setup && !setup.installed && setup.carried ? `<button class="text-button place-now" type="button">${escape(words.place)}</button>` : ""}</p>
      ${logins.length && this.step !== "done" ? `<p class="welcome-legacy login-already">${escape(words.already(logins.join(words.separator)))}</p>` : ""}
      ${legacy ? `<p class="welcome-legacy">${escape(legacy)}</p>` : ""}`;
  }

  /** Moves to another step; its first field, or its first button, takes
   *  focus, as a new page would. */
  private go(step: Step): void {
    this.keep();
    this.step = step;
    this.draw();
    this.drawHere();
    this.body.querySelector<HTMLElement>("input:not([type=hidden]):not([type=checkbox]), textarea, .sheet-go")?.focus({ preventScroll: true });
  }

  /** Keeps what was typed of the server and account across the ways in. */
  private keep(): void {
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    this.typed.hub = value("hub")?.value ?? this.typed.hub;
    this.typed.account = value("account")?.value ?? this.typed.account;
    const register = value("register");
    if (register) this.typed.register = register.checked;
  }

  private loggedIn(): boolean {
    return loginsOf(this.options.store.world.peek()?.accounts).length > 0;
  }

  private draw(): void {
    const { words } = this.options;
    const foot = (ways: string, go: string) =>
      `<p class="sheet-note" role="alert" hidden></p>
      <footer class="welcome-foot"><span class="login-ways">${ways}</span><button class="sheet-go" type="submit">${escape(go)}</button></footer>`;
    const way = (to: Step, label: string) => `<button class="text-button login-way" type="button" data-way="${to}">${escape(label)}</button>`;
    switch (this.step) {
      case "password":
      case "open": {
        const opening = this.step === "open";
        const ways = opening
          ? way("password", words.withPassword)
          : [way("token", words.withToken), this.options.openAccount ? way("open", words.openOne) : ""].join("");
        this.body.innerHTML = `
          <form class="login-form welcome-form" autocomplete="on">
            <h2 class="welcome-form-title">${escape(opening ? words.openTitle : this.loggedIn() ? words.another : words.title)}</h2>
            ${opening ? `<p class="field-hint login-what">${escape(words.openWhat)}</p>` : ""}
            <label class="field"><span class="field-label">${escape(words.hub)}</span>
              <input name="hub" class="mono" required autocomplete="url" spellcheck="false" autocapitalize="off" placeholder="hub.example.com:7443" value="${escape(this.typed.hub)}">
              <span class="field-hint">${escape(words.hubHint)}</span></label>
            <label class="field"><span class="field-label">${escape(words.account)}</span>
              <input name="account" required autocomplete="username" spellcheck="false" autocapitalize="off" pattern="${accountRule}" value="${escape(this.typed.account)}"></label>
            <label class="field"><span class="field-label">${escape(words.password)}</span>
              <input name="password" type="password" required autocomplete="${opening ? "new-password" : "current-password"}"></label>
            ${
              opening
                ? `<label class="field"><span class="field-label">${escape(words.passwordAgain)}</span>
              <input name="again" type="password" required autocomplete="new-password"></label>`
                : ""
            }
            ${
              this.options.device
                ? ""
                : `<label class="check-row"><input type="checkbox" name="register"${this.typed.register ? " checked" : ""}><span><span class="check-title">${escape(words.register)}</span><span class="field-hint">${escape(words.registerWhat)}</span></span></label>`
            }
            <div class="login-name"></div>
            ${foot(ways, opening ? words.openGo : words.go)}
          </form>
          ${this.aloneWay()}`;
        this.drawName();
        break;
      }
      case "token":
        this.body.innerHTML = `
          <form class="token-form welcome-form">
            <h2 class="welcome-form-title">${escape(words.withToken)}</h2>
            ${this.options.scan ? `<button class="scan-code" type="button">${glyph("scan", 20)}<span>${escape(words.scan)}</span></button>` : ""}
            <label class="field"><span class="field-label">${escape(this.options.scan ? words.tokenOr : words.token)}</span>
              <textarea name="token" rows="3" class="mono" required spellcheck="false" autocapitalize="off" placeholder="FX5…"></textarea>
              <span class="field-hint">${escape(words.tokenHint)}</span></label>
            <div class="login-name"></div>
            ${foot(way("password", words.withPassword), words.go)}
          </form>
          ${this.aloneWay()}`;
        this.drawName();
        break;
      case "done":
        this.drawDone();
        break;
    }
  }

  /** The way past logging in, for a computer on its own the first time. */
  private aloneWay(): string {
    const { words } = this.options;
    if (this.options.device || this.loggedIn()) return "";
    return `<p class="login-alone"><button class="text-button alone" type="button">${escape(words.alone)}</button><span class="field-hint">${escape(words.aloneWhat)}</span></p>`;
  }

  /** A name is asked where one is chosen now: a phone's first login, or a
   *  computer registering on its first hub. A device keeps one name on
   *  every hub after that. */
  private drawName(): void {
    const slot = this.body.querySelector<HTMLElement>(".login-name");
    if (!slot) return;
    const { words, store } = this.options;
    const world = store.world.peek();
    const first = !loginsOf(world?.accounts).length;
    const registering = this.step === "token" || (this.body.querySelector<HTMLInputElement>("[name=register]")?.checked ?? false);
    const typed = slot.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
    const asked = first && (this.options.device || registering);
    if (!asked) {
      slot.innerHTML = "";
      return;
    }
    // A phone has no name until it first logs in; a computer has its own.
    const phone = this.options.device;
    slot.innerHTML = `<label class="field"><span class="field-label">${escape(phone ? words.phoneName : words.name)}</span>
      <input name="name" class="mono" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${machineRule}"${phone ? " required" : ""} placeholder="${escape(phone ? "phone" : (world?.here ?? ""))}" value="${escape(typed)}">
      <span class="field-hint">${escape(words.nameHint)}</span></label>`;
  }

  private drawDone(): void {
    const { words } = this.options;
    const done = this.done!;
    const login = `${done.hub} · ${done.account}`;
    const kept = !done.kept
      ? ""
      : done.kept.kind === "command"
        ? `<p>${escape(words.keptCommand)}</p><pre class="welcome-command mono">${escape(done.kept.command)}</pre>`
        : done.kept.kind in words.kept
          ? `<p>${escape(words.kept[done.kept.kind as keyof LoginWords["kept"]])}</p>`
          : "";
    const setup = this.setup;
    const world = this.options.store.world.peek();
    const registered = world?.accounts.hubs.some((hub) => hub.name === done.hub && hub.home === done.account) ?? false;
    const host = (key: "codex" | "claude", label: string) => {
      const state = setup ? setup[key] : "absent";
      return `<label class="host-row"><input type="checkbox" name="${key}" ${state === "out" || state === "old" ? "checked" : ""} ${state === "absent" || state === "in" ? "disabled" : ""}><span class="host-name">${escape(label)}</span><span class="host-state">${escape(words.hostState[state])}</span></label>`;
    };
    this.body.innerHTML = `
      <div class="welcome-form">
        <h2 class="welcome-form-title">${escape(words.done(login, done.name))}</h2>
        <div class="welcome-kept">
          <p>${escape(this.options.device ? words.phoneDone : registered ? words.registered : words.onlyLoggedIn)}</p>
          ${kept}
        </div>
        ${
          this.options.device || !this.options.applyHosts
            ? ""
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
  }

  private note(text: string): void {
    const note = this.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.textContent = text;
    note.hidden = !text;
  }

  private async click(event: Event): Promise<void> {
    const target = event.target as HTMLElement;
    const way = target.closest<HTMLElement>("[data-way]")?.dataset.way as Step | undefined;
    if (way) return this.go(way);
    if (target.closest(".alone") || target.closest(".enter")) return this.options.enter();
    if (target.closest(".scan-code") && this.options.scan) {
      // What the camera read goes where a pasted token would.
      try {
        const token = await this.options.scan();
        if (!token) return;
        const field = this.body.querySelector<HTMLTextAreaElement>("[name=token]");
        if (field) field.value = token;
        this.body.querySelector<HTMLElement>(".token-form .sheet-go")?.focus();
      } catch (error) {
        this.note(this.options.words.scanFailed(error instanceof Error ? error.message : String(error)));
      }
      return;
    }
    if (target.closest(".place-now") && this.options.place) {
      await this.options.place().catch((error: unknown) => this.note(this.options.words.failed(String(error))));
      return this.refreshSetup();
    }
    if (target.closest(".apply-now") && !this.busy && this.options.applyHosts) {
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

  private async submit(): Promise<void> {
    if (this.busy || this.step === "done") return;
    const { words } = this.options;
    const field = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    const name = field("name")?.value.trim() || null;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const label = go.textContent ?? "";
    let act: () => Promise<LoggedIn>;
    if (this.step === "token") {
      const token = this.body.querySelector<HTMLTextAreaElement>("[name=token]")?.value.trim() ?? "";
      if (!token) return;
      act = () => this.options.loginToken(token, name);
    } else {
      const hub = field("hub")?.value.trim() ?? "";
      const account = field("account")?.value.trim() ?? "";
      const password = field("password")?.value ?? "";
      const register = !this.options.device && (field("register")?.checked ?? false);
      if (!hub || !account || !password) return;
      if (this.step === "open") {
        if (password !== field("again")?.value) return this.note(words.passwordsDiffer);
        const open = this.options.openAccount;
        if (!open) return;
        act = () => open(hub, account, password, register, name);
      } else act = () => this.options.login(hub, account, password, register, name);
    }
    this.keep();
    this.busy = true;
    go.disabled = true;
    go.textContent = words.going;
    this.note("");
    try {
      this.done = await act();
      this.busy = false;
      await this.options.store.refresh();
      this.step = "done";
      this.draw();
      this.drawHere();
      void this.refreshSetup();
      this.body.querySelector<HTMLElement>(".sheet-go")?.focus({ preventScroll: true });
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      go.textContent = label;
      this.note(words.failed(error instanceof Error ? error.message : String(error)));
    }
  }
}

/** The World's mark, large, drawn with the marker: rings round a knot. */
function heroMark(): string {
  return `<svg class="welcome-mark" viewBox="0 0 120 96" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke="currentColor" stroke-width="3.2" stroke-linecap="round"><circle cx="60" cy="50" r="7"/><circle cx="22" cy="24" r="14"/><circle cx="98" cy="27" r="14"/><circle cx="58" cy="82" r="11"/><path d="M34 31 L53 45M86 34 L67 46M59 57 L58 70"/></g></svg>`;
}
