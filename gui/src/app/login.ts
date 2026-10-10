// Coming into a World (`world-gui.md`, 账号、机器、共享与借出). A computer
// that is in none yet is offered two ways: join one, with the server, the
// account and its password as MeshCentral Router asks them or with a
// one-time token another device made; or create one, with this computer
// as its hub and the person its super administrator. A phone only joins,
// and reads a token's code with its camera. A computer registers as one of
// the account's machines unless told not to, and is kept connected by a
// startup service afterwards. Once in, the World's tools can be put into
// this computer's agents, and on a World just made another computer can
// be let in at once.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { LoggedIn, Legacy, Setup } from "../world/bridge";
import type { Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { LoginWords } from "./account-words";
import { loginLabel } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

/** An address of this computer another might reach it at. */
export interface HereAddress {
  interface: string;
  address: string;
  physical: boolean;
}

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
  /** Creating a World with this computer its hub, where it can: this
   *  computer's addresses, and the World made and logged in to. */
  addresses?: (() => Promise<HereAddress[]>) | undefined;
  create?: ((world: string, admin: string, password: string, public_: string[], register: boolean, name: string | null) => Promise<LoggedIn>) | undefined;
  /** Opens the sheet that lets another computer into `login`. */
  invite?: ((login: Login) => void) | undefined;
  /** Reads a token's QR code with the camera, where the shell can; null
   *  when the person turned back. */
  scan?: (() => Promise<string | null>) | undefined;
  applyHosts?: ((codex: boolean, claude: boolean) => Promise<string[]>) | undefined;
  /** Done here: on to the World. */
  enter(): void;
}

type Step = "choose" | "password" | "open" | "token" | "create" | "done";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";
const machineRule = "[a-z][a-z0-9\\-]{0,31}";
const hubRule = "[a-z][a-z0-9\\-]{0,31}";
/** The port a hub made here listens on unless told otherwise. */
const hubPort = 7443;

export class LoginPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly here: HTMLElement;
  private readonly body: HTMLElement;
  private step: Step;
  private setup: Setup | null = null;
  private done: LoggedIn | null = null;
  /** The World was made here, not joined. */
  private created = false;
  private busy = false;
  /** What was typed, kept when moving between the ways in. */
  private typed = { hub: "", account: "", register: true, world: "", port: String(hubPort), extra: "" };
  /** This computer's addresses, once asked, and those chosen. */
  private addresses: HereAddress[] | null = null;
  private chosen = new Set<string>();
  private readonly stop: () => void;

  constructor(private readonly options: LoginOptions) {
    const { words } = options;
    this.title = words.title;
    this.step = options.device ? "token" : this.loggedIn() ? "password" : "choose";
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
        <div class="welcome-body"></div>
        <section class="welcome-here"></section>
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
      if (this.step === "password" || this.step === "open" || this.step === "create") this.drawName();
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

  /** This computer, at the foot: its name, whether fastctx is in place and
   *  kept running, and the logins it has. */
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
      <p class="welcome-facts">${glyph(this.options.device ? "phone" : "machine", 14)}${facts.map((fact) => `<span class="fact">${escape(fact)}</span>`).join(`<span class="sep">·</span>`)}
        ${setup && !setup.installed && setup.carried ? `<button class="text-button place-now" type="button">${escape(words.place)}</button>` : ""}</p>
      ${logins.length && this.step !== "done" ? `<p class="welcome-legacy">${escape(words.already(logins.join(words.separator)))}</p>` : ""}
      ${legacy ? `<p class="welcome-legacy">${escape(legacy)}</p>` : ""}`;
  }

  /** Moves to another step; its first field, or its first button, takes
   *  focus, as a new page would. */
  private go(step: Step): void {
    this.keep();
    this.step = step;
    this.draw();
    this.drawHere();
    this.body.querySelector<HTMLElement>("input:not([type=hidden]):not([type=checkbox]), textarea, .sheet-go, .welcome-way")?.focus({ preventScroll: true });
  }

  /** Keeps what was typed across the ways in. */
  private keep(): void {
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    this.typed.hub = value("hub")?.value ?? this.typed.hub;
    this.typed.account = value("account")?.value ?? this.typed.account;
    this.typed.world = value("world")?.value ?? this.typed.world;
    this.typed.port = value("port")?.value ?? this.typed.port;
    this.typed.extra = value("extra")?.value ?? this.typed.extra;
    const register = value("register");
    if (register) this.typed.register = register.checked;
  }

  private loggedIn(): boolean {
    return loginsOf(this.options.store.world.peek()?.accounts).length > 0;
  }

  private draw(): void {
    const { words } = this.options;
    const foot = (ways: string, go: string, note = "") =>
      `<p class="sheet-note" role="alert" hidden></p>
      <footer class="welcome-foot"><span class="login-ways">${ways}</span><span class="welcome-go">${note ? `<span class="welcome-go-note">${escape(note)}</span>` : ""}<button class="sheet-go" type="submit">${escape(go)}</button></span></footer>`;
    const way = (to: Step, label: string) => `<button class="text-button login-way" type="button" data-way="${to}">${escape(label)}</button>`;
    const register = this.options.device
      ? ""
      : `<label class="check-row" title="${escape(words.registerWhat)}"><input type="checkbox" name="register"${this.typed.register ? " checked" : ""}><span class="check-title">${escape(words.register)}</span></label>`;
    switch (this.step) {
      case "choose":
        this.body.innerHTML = `
          <div class="welcome-ways">
            <button class="welcome-way" type="button" data-way="password">${joinMark()}<span class="welcome-way-title">${escape(words.join)}</span><span class="welcome-way-what">${escape(words.joinWhat)}</span></button>
            ${
              this.options.create
                ? `<button class="welcome-way" type="button" data-way="create">${createMark()}<span class="welcome-way-title">${escape(words.create)}</span><span class="welcome-way-what">${escape(words.createWhat)}</span></button>`
                : ""
            }
          </div>
          ${this.aloneWay()}`;
        break;
      case "password":
      case "open": {
        const opening = this.step === "open";
        const ways = opening
          ? way("password", words.withPassword)
          : [way("token", words.withToken), this.options.openAccount ? way("open", words.openOne) : "", this.backWay()].join("");
        this.body.innerHTML = `
          <form class="login-form welcome-form" autocomplete="on">
            <h2 class="welcome-form-title">${escape(opening ? words.openTitle : this.loggedIn() ? words.another : words.join)}</h2>
            ${opening ? `<p class="field-hint login-what">${escape(words.openWhat)}</p>` : ""}
            <label class="field"><span class="field-label">${escape(words.hub)}</span>
              <input name="hub" class="mono" required autocomplete="url" spellcheck="false" autocapitalize="off" placeholder="203.0.113.10:7443" value="${escape(this.typed.hub)}"></label>
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
            ${register}
            <div class="login-name"></div>
            ${foot(ways, opening ? words.openGo : words.go)}
          </form>`;
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
            ${foot([way("password", words.withPassword), this.backWay()].join(""), words.go)}
          </form>`;
        this.drawName();
        break;
      case "create":
        this.body.innerHTML = `
          <form class="create-form welcome-form" autocomplete="off">
            <h2 class="welcome-form-title">${escape(words.create)}</h2>
            <div class="field-pair">
              <label class="field"><span class="field-label">${escape(words.worldName)}</span>
                <input name="world" class="mono" required spellcheck="false" autocapitalize="off" pattern="${hubRule}" placeholder="home" value="${escape(this.typed.world)}"></label>
              <label class="field"><span class="field-label">${escape(words.admin)}</span>
                <input name="account" class="mono" required spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="me" value="${escape(this.typed.account)}"></label>
            </div>
            <div class="field-pair">
              <label class="field"><span class="field-label">${escape(words.password)}</span>
                <input name="password" type="password" required autocomplete="new-password"></label>
              <label class="field"><span class="field-label">${escape(words.passwordAgain)}</span>
                <input name="again" type="password" required autocomplete="new-password"></label>
            </div>
            <div class="field reach-field"><span class="field-label" title="${escape(words.reachWhat)}">${escape(words.reach)}</span>
              <div class="reach-chips"></div>
              <div class="reach-more">
                <input name="extra" class="mono" spellcheck="false" autocapitalize="off" placeholder="${escape(words.extraHint)}" aria-label="${escape(words.extraHint)}" value="${escape(this.typed.extra)}">
                <label class="reach-port"><span>${escape(words.port)}</span><input name="port" class="mono" inputmode="numeric" pattern="[0-9]{2,5}" required value="${escape(this.typed.port)}"></label>
              </div>
            </div>
            ${register}
            <div class="login-name"></div>
            ${foot(this.backWay(), words.createGo, words.createNote)}
          </form>`;
        this.drawName();
        void this.drawAddresses();
        break;
      case "done":
        this.drawDone();
        break;
    }
  }

  /** Back to the two ways, for a computer that has none yet. */
  private backWay(): string {
    if (this.options.device || this.loggedIn()) return this.options.create && this.step !== "create" ? `<button class="text-button login-way" type="button" data-way="create">${escape(this.options.words.create)}</button>` : "";
    return `<button class="text-button login-way" type="button" data-way="choose">${escape(this.options.words.back)}</button>`;
  }

  /** The way past logging in, for a computer on its own the first time. */
  private aloneWay(): string {
    const { words } = this.options;
    if (this.options.device || this.loggedIn()) return "";
    return `<p class="login-alone"><button class="text-button alone" type="button" title="${escape(words.aloneWhat)}">${escape(words.alone)}</button></p>`;
  }

  /** This computer's addresses as pills to choose: the physical ones with
   *  a way out chosen to start with. */
  private async drawAddresses(): Promise<void> {
    const slot = () => this.body.querySelector<HTMLElement>(".reach-chips");
    if (!this.addresses && this.options.addresses) {
      try {
        this.addresses = await this.options.addresses();
        const first = this.addresses.find((address) => address.physical) ?? this.addresses[0];
        if (first && !this.chosen.size) this.chosen.add(first.address);
      } catch {
        this.addresses = [];
      }
    }
    const here = slot();
    if (!here) return;
    const { words } = this.options;
    const list = this.addresses ?? [];
    here.innerHTML = list.length
      ? list
          .map(
            (address) =>
              `<button class="choice reach-chip${this.chosen.has(address.address) ? " chosen" : ""}" type="button" data-address="${escape(address.address)}" aria-pressed="${this.chosen.has(address.address)}" title="${escape(address.interface)}"><span class="mono">${escape(address.address)}</span>${address.physical ? "" : `<span class="reach-virtual">${escape(words.virtual)}</span>`}</button>`,
          )
          .join("")
      : `<span class="field-hint">${escape(words.noAddresses)}</span>`;
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
    slot.innerHTML = `<label class="field"><span class="field-label" title="${escape(words.nameHint)}">${escape(phone ? words.phoneName : words.name)}</span>
      <input name="name" class="mono" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${machineRule}"${phone ? " required" : ""} placeholder="${escape(phone ? "phone" : (world?.here ?? ""))}" value="${escape(typed)}"></label>`;
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
    // On a World just made, the way to let the next computer in comes first.
    const reached = this.created ? [...this.chosen, ...this.extraAddresses()].map((address) => `${address}:${this.typed.port}`) : [];
    const invite =
      this.created && this.options.invite
        ? `<section class="welcome-next">
          <h3 class="welcome-next-title">${escape(words.nextTitle)}</h3>
          <dl class="facts welcome-reached"><dt>${escape(words.hub)}</dt><dd class="mono">${escape(reached.join("  ") || "—")}</dd><dt>${escape(words.account)}</dt><dd class="mono">${escape(done.account)}</dd></dl>
          <button class="text-button invite-next" type="button">${glyph("plus", 16)}<span>${escape(words.inviteNext)}</span></button>
        </section>`
        : "";
    this.body.innerHTML = `
      <div class="welcome-form">
        <h2 class="welcome-form-title">${escape(this.created ? words.created(done.hub) : words.done(login, done.name))}</h2>
        <div class="welcome-kept">
          <p>${escape(this.options.device ? words.phoneDone : this.created ? words.createdSaid(done.account, done.name) : registered ? words.registered : words.onlyLoggedIn)}</p>
          ${kept}
        </div>
        ${invite}
        ${
          this.options.device || !this.options.applyHosts
            ? ""
            : `<section class="welcome-hosts">
          <h3 class="field-label" title="${escape(words.hostsWhat)}">${escape(words.hosts)}</h3>
          ${host("codex", words.codex)}${host("claude", words.claude)}
          <button class="text-button apply-now" type="button">${escape(words.applyGo)}</button>
          <p class="sheet-note" role="alert" hidden></p>
        </section>`
        }
        <footer class="welcome-foot"><span></span><button class="sheet-go enter" type="button">${escape(words.enter)}</button></footer>
      </div>`;
  }

  /** The address typed beside the pills, as an IP without its port. */
  private extraAddresses(): string[] {
    const extra = this.typed.extra.trim();
    if (!extra) return [];
    return [extra.replace(/:\d+$/, "")];
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
    const address = target.closest<HTMLElement>("[data-address]")?.dataset.address;
    if (address) {
      if (this.chosen.has(address)) this.chosen.delete(address);
      else this.chosen.add(address);
      return this.drawAddresses();
    }
    if (target.closest(".alone") || target.closest(".enter")) return this.options.enter();
    if (target.closest(".invite-next") && this.done && this.options.invite) {
      const login = loginsOf(this.options.store.world.peek()?.accounts).find((one) => one.hubName === this.done!.hub && one.account === this.done!.account);
      if (login) this.options.invite(login);
      return;
    }
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
    if (this.busy || this.step === "done" || this.step === "choose") return;
    const { words } = this.options;
    const field = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    const name = field("name")?.value.trim() || null;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const label = go.textContent ?? "";
    let act: () => Promise<LoggedIn>;
    this.keep();
    if (this.step === "token") {
      const token = this.body.querySelector<HTMLTextAreaElement>("[name=token]")?.value.trim() ?? "";
      if (!token) return;
      act = () => this.options.loginToken(token, name);
    } else if (this.step === "create") {
      const create = this.options.create;
      const world = field("world")?.value.trim() ?? "";
      const account = field("account")?.value.trim() ?? "";
      const password = field("password")?.value ?? "";
      if (!create || !world || !account || !password) return;
      if (password !== field("again")?.value) return this.note(words.passwordsDiffer);
      const reach = [...this.chosen, ...this.extraAddresses()].map((address) => `${address}:${this.typed.port.trim()}`);
      if (!reach.length) return this.note(words.noReach);
      const register = field("register")?.checked ?? false;
      act = () => create(world, account, password, reach, register, name);
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
    const creating = this.step === "create";
    this.busy = true;
    go.disabled = true;
    go.textContent = creating ? words.creating : words.going;
    this.note("");
    try {
      this.done = await act();
      this.created = creating;
      this.busy = false;
      await this.options.store.refresh();
      this.step = "done";
      this.draw();
      this.drawHere();
      void this.refreshSetup();
      this.body.querySelector<HTMLElement>(".invite-next, .sheet-go")?.focus({ preventScroll: true });
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

/** Joining: a ring drawn towards a knot that is there already. */
function joinMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 96 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round"><g class="ink-graphite" stroke-width="3"><circle cx="66" cy="32" r="7"/><circle cx="66" cy="32" r="2" fill="currentColor"/></g><g class="ink-clay" stroke-width="3"><circle cx="20" cy="32" r="11"/><path d="M34 32 C42 28 48 28 55 31"/><path d="M50 26 L56 31 L50 36"/></g></g></svg>`;
}

/** Creating: a knot with its first ring round it, the second being drawn. */
function createMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 96 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round"><g class="ink-graphite" stroke-width="3"><circle cx="48" cy="34" r="6"/><circle cx="20" cy="18" r="10"/><path d="M28 23 L43 31"/></g><g class="ink-clay" stroke-width="3"><path d="M70 36 A11 11 0 1 1 82 50"/><path d="M54 37 L62 40"/></g></g></svg>`;
}
