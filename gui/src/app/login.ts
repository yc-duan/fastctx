// Coming into a World (`world-gui.md`, 账号、机器、共享与协助). A computer
// that is in none yet is offered two ways: join one with the server's
// address, an account and its password, or create one with this computer
// as its server and the person its administrator. A phone scans the code a
// computer shows, or logs in with an account and password. A computer that
// logs in joins as one of the account's machines; a phone only logs in.
// A computer may join by a code pasted instead (`world-network.md`, 受管机器):
// what the code lets it into is asked of its server first and shown on a
// card, and a managed machine's card says when this Fastctx was used too
// much to become one. Once in, the page only confirms it; on a World just
// made another device can be added at once.

import { escape } from "../conversation/prose";
import type { KeptNow, LoggedIn, NameProblem, Setup } from "../world/bridge";
import type { Login, TokenPeek } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { LoginWords } from "./account-words";
import { managedMachine } from "./managed-art";
import { newPasswordFields, passwordReady, sayPassword } from "./new-password";
import { plainly } from "./problem";
import { loginLabel } from "./reach";
import { DrawnOnce } from "./drawn-once";
import { RemoteWorld, type RemoteSeams } from "./remote-world";
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
  /** Asked for from the sidebar's networks: joining one, or making a new
   *  one (here or on a server); with neither, every way is offered. By a
   *  code, when a managed machine comes back to a network. */
  way?: "join" | "new" | "code";
  head(title: string, actions?: string): string;
  /** This interface runs on a phone: it logs in as a device, and nothing
   *  is set up on it. */
  device: boolean;
  /** Where this computer stands, where the shell can say. */
  setup?: (() => Promise<Setup>) | undefined;
  place?: (() => Promise<string>) | undefined;
  login(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn>;
  /** Logs a phone in with the code it scanned, or a computer with the code
   *  pasted. */
  loginToken(token: string, name: string | null): Promise<LoggedIn>;
  /** What a pasted code lets this computer into, asked of its server; a
   *  computer that cannot ask has no way in by a code. */
  peekToken?: ((token: string) => Promise<TokenPeek | null>) | undefined;
  /** Opens the removal of Fastctx from this computer, on taking
   *  everything. */
  remove?: (() => void) | undefined;
  /** Opening an account, where this interface can: a computer can, a
   *  phone logs in only. */
  openAccount?: ((hub: string, account: string, password: string, register: boolean, name: string | null) => Promise<LoggedIn>) | undefined;
  /** Creating a World with this computer its server, where it can: this
   *  computer's addresses, and the World made and logged in to. */
  addresses?: (() => Promise<HereAddress[]>) | undefined;
  create?: ((world: string, admin: string, password: string, public_: string[], register: boolean, name: string | null) => Promise<LoggedIn>) | undefined;
  /** Making a World on a server over SSH, where this computer can. */
  remote?: RemoteSeams | undefined;
  /** Opens the sheet that adds another device to `login`. */
  invite?: ((login: Login) => void) | undefined;
  /** What is wrong with a machine or server name as typed, where the shell
   *  can say; nothing is checked before sending otherwise. */
  nameProblem?: ((kind: "machine" | "hub", text: string) => Promise<NameProblem | null>) | undefined;
  /** Reads a QR code with the camera, where the shell can; null when the
   *  person turned back. */
  scan?: (() => Promise<string | null>) | undefined;
  /** What a phone is called in its own settings, offered as its name the
   *  first time it logs in. */
  deviceName?: (() => Promise<string>) | undefined;
  /** Installs the startup service where it is missing. */
  keep?: (() => Promise<KeptNow>) | undefined;
  /** The World's drawing, with this computer marked on it once it is in. */
  drawing?: { element: HTMLElement; mark(machine: string | null): void; stop(): void } | undefined;
  /** Done here: on to the World. */
  enter(): void;
}

type Step = "choose" | "password" | "code" | "confirm" | "open" | "scan" | "create" | "remote" | "done";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";
/** The port a server made here listens on unless told otherwise. */
const hubPort = 7443;

/** A name the World takes, made from what a phone calls itself
 *  (`world-network.md`, 机器的名字): full-width letters made half-width,
 *  a hyphen where anything but letters and digits of any language stood,
 *  and at most 32 characters and 64 bytes. "REDMI K80 Ultra" is
 *  REDMI-K80-Ultra. The World's own rule still checks it before it is sent. */
function nameFrom(said: string): string {
  let name = said.normalize("NFKC").replace(/[^\p{L}\p{N}]+/gu, "-").replace(/^-+|-+$/g, "");
  const bytes = (text: string) => new TextEncoder().encode(text).length;
  while ([...name].length > 32 || bytes(name) > 64) name = [...name].slice(0, -1).join("").replace(/-+$/, "");
  return name;
}

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
  private typed = { hub: "", account: "", world: "", port: String(hubPort), extra: "", code: "", name: "" };
  /** This computer's addresses, once asked, and those chosen. */
  private addresses: HereAddress[] | null = null;
  private chosen = new Set<string>();
  private readonly stop: () => void;
  /** The way through a server over SSH, while it is open. */
  private remote: RemoteWorld | null = null;
  /** The name a phone is offered, made from what it calls itself. */
  private suggested = "";
  /** The page is drawn again on every change: its drawings are drawn in
   *  once, and the card of being in rises in once. */
  private readonly drawn = new DrawnOnce();
  private risen = false;
  /** What the code pasted lets this computer into, as its server said;
   *  null when it said nothing of it. */
  private peeked: TokenPeek | null = null;

  constructor(private readonly options: LoginOptions) {
    const { words } = options;
    this.title = words.title;
    this.step = options.device
      ? options.scan
        ? "scan"
        : "password"
      : options.way === "code" && options.peekToken
        ? "code"
        : options.way === "join"
          ? "password"
          : options.way === "new" || !this.loggedIn()
            ? "choose"
            : "password";
    this.element = document.createElement("div");
    this.element.className = "welcome-page login-page";
    this.element.innerHTML = `
      ${options.head("")}
      <div class="welcome scroller">
        <header class="welcome-hero">
          ${heroMark()}
          <h1 class="welcome-title">Fastctx</h1>
          <p class="welcome-lede">${escape(words.lede)}</p>
        </header>
        <div class="welcome-body"></div>
        <section class="welcome-here"></section>
      </div>`;
    this.here = this.element.querySelector<HTMLElement>(".welcome-here")!;
    this.body = this.element.querySelector<HTMLElement>(".welcome-body")!;
    this.element.addEventListener("click", (event) => void this.click(event));
    this.element.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.name === "password" || input.name === "account") this.drawStrength();
    });
    this.element.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    // A code is one line however it wraps: Enter goes on.
    this.element.addEventListener("keydown", (event) => {
      const field = event.target as HTMLElement;
      if (event.key !== "Enter" || event.shiftKey || event.isComposing || !field.matches("[name=code]")) return;
      event.preventDefault();
      field.closest("form")?.requestSubmit();
    });
    this.draw();
    void this.refreshSetup();
    if (options.device && options.deviceName) {
      options.deviceName().then(
        (name) => {
          this.suggested = nameFrom(name);
          const field = this.body.querySelector<HTMLInputElement>("[name=name]");
          if (field && !field.value) field.value = this.suggested;
        },
        (error: unknown) => console.error("reading the phone's name", error),
      );
    }
    // The logins this computer has, as the World answers.
    let shown = "";
    this.stop = effect(() => {
      const world = options.store.world.get();
      const said = JSON.stringify([world?.here, loginsOf(world?.accounts).map(loginLabel)]);
      if (said === shown) return;
      shown = said;
      this.drawHere();
      if (this.step === "password" || this.step === "code" || this.step === "open" || this.step === "create" || this.step === "scan") this.drawName();
    });
  }

  leave(): void {
    this.remote?.leave();
    this.stop();
    this.options.drawing?.stop();
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

  /** At the foot: the logins this computer has, and the one thing that
   *  may be missing for it to take part at all. */
  private drawHere(): void {
    const { words, store } = this.options;
    const logins = loginsOf(store.world.peek()?.accounts).map(loginLabel);
    const missing = this.setup && !this.setup.installed && this.setup.carried;
    this.here.innerHTML = `
      ${logins.length && this.step !== "done" ? `<p class="welcome-legacy">${escape(words.already(logins.join(words.separator)))}</p>` : ""}
      ${missing ? `<p class="welcome-legacy">${escape(words.placeMissing)}<button class="text-button place-now" type="button">${escape(words.place)}</button></p>` : ""}`;
  }

  /** Moves to another step; its first field, or its first button, takes
   *  focus, as a new page would. */
  private go(step: Step): void {
    this.remote?.leave();
    this.remote = null;
    this.keep();
    this.step = step;
    this.draw();
    this.drawHere();
    this.body.querySelector<HTMLElement>("input:not([type=hidden]):not([type=checkbox]), .scan-code, .sheet-go, .welcome-way")?.focus({ preventScroll: true });
  }

  /** Keeps what was typed across the ways in. */
  private keep(): void {
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    this.typed.hub = value("hub")?.value ?? this.typed.hub;
    this.typed.account = value("account")?.value ?? this.typed.account;
    this.typed.world = value("world")?.value ?? this.typed.world;
    this.typed.port = value("port")?.value ?? this.typed.port;
    this.typed.extra = value("extra")?.value ?? this.typed.extra;
    this.typed.code = value("code")?.value ?? this.typed.code;
    this.typed.name = value("name")?.value ?? this.typed.name;
  }

  private loggedIn(): boolean {
    return loginsOf(this.options.store.world.peek()?.accounts).length > 0;
  }

  private draw(): void {
    const { words } = this.options;
    const foot = (ways: string, go: string, title = "") =>
      `<p class="sheet-note" role="alert" hidden></p>
      <footer class="welcome-foot"><span class="login-ways">${ways}</span><button class="sheet-go" type="submit"${title ? ` title="${escape(title)}"` : ""}>${escape(go)}</button></footer>`;
    const way = (to: Step, label: string) => `<button class="text-button login-way" type="button" data-way="${to}">${escape(label)}</button>`;
    const newPassword = (pair: boolean) => newPasswordFields(words, { pair });
    // A computer joins with an account and its password, or by a code.
    const tabs = (on: "password" | "code") =>
      this.options.device || !this.options.peekToken
        ? ""
        : `<div class="segmented login-tabs" role="tablist">${(["password", "code"] as const)
            .map((tab) => `<button class="segment${tab === on ? " chosen" : ""}" type="button" role="tab" aria-selected="${tab === on}" data-way="${tab}">${escape(tab === "password" ? words.byPassword : words.byCode)}</button>`)
            .join("")}</div>`;
    switch (this.step) {
      case "choose":
        this.body.innerHTML = `
          <div class="welcome-ways">
            ${this.options.way === "new" ? "" : `<button class="welcome-way" type="button" data-way="password">${this.drawn.mark("join", joinMark())}<span class="welcome-way-title">${escape(words.join)}</span><span class="welcome-way-what">${escape(words.joinWhat)}</span></button>`}
            ${
              this.options.create
                ? `<button class="welcome-way" type="button" data-way="create">${this.drawn.mark("create", createMark())}<span class="welcome-way-title">${escape(words.create)}</span><span class="welcome-way-what">${escape(words.createWhat)}</span></button>`
                : ""
            }
            ${
              this.options.remote
                ? `<button class="welcome-way" type="button" data-way="remote">${this.drawn.mark("remote", remoteMark())}<span class="welcome-way-title">${escape(words.remote)}</span><span class="welcome-way-what">${escape(words.remoteWhat)}</span></button>`
                : ""
            }
          </div>
          <p class="welcome-hub-is">${escape(words.hubIs)}</p>`;
        break;
      case "password":
      case "open": {
        const opening = this.step === "open";
        const ways = opening
          ? way("password", words.back)
          : [this.options.device && this.options.scan ? way("scan", words.scan) : "", this.options.openAccount ? way("open", words.signUp) : "", this.backWay()].join("");
        this.body.innerHTML = `
          <form class="login-form welcome-form" autocomplete="on">
            <h2 class="welcome-form-title">${escape(opening ? words.signUp : this.loggedIn() ? words.another : words.join)}</h2>
            ${opening ? "" : tabs("password")}
            <label class="field"><span class="field-label">${escape(words.hub)}</span>
              <input name="hub" class="mono" required autocomplete="url" spellcheck="false" autocapitalize="off" placeholder="192.168.1.10:7443" value="${escape(this.typed.hub)}"></label>
            <label class="field"><span class="field-label">${escape(words.account)}</span>
              <input name="account" required autocomplete="username" spellcheck="false" autocapitalize="off" pattern="${accountRule}" value="${escape(this.typed.account)}"></label>
            ${
              opening
                ? newPassword(false)
                : `<label class="field"><span class="field-label">${escape(words.password)}</span>
              <input name="password" type="password" required autocomplete="current-password"></label>`
            }
            <div class="login-name"></div>
            ${foot(ways, opening ? words.signUpGo : words.go)}
          </form>`;
        this.drawName();
        break;
      }
      case "code":
        this.body.innerHTML = `
          <form class="code-form welcome-form" autocomplete="off">
            <h2 class="welcome-form-title">${escape(this.loggedIn() ? words.another : words.join)}</h2>
            ${tabs("code")}
            <label class="field"><span class="field-label">${escape(words.code)}</span>
              <textarea name="code" class="mono code-input" rows="3" required spellcheck="false" autocapitalize="off" placeholder="FX5…">${escape(this.typed.code)}</textarea></label>
            <div class="login-name"></div>
            ${foot(this.backWay(), words.next)}
          </form>`;
        this.drawName();
        break;
      case "confirm":
        this.drawConfirm();
        break;
      case "scan":
        this.body.innerHTML = `
          <div class="welcome-form scan-form">
            <div class="login-name"></div>
            <button class="scan-code" type="button">${this.drawn.mark("scan", scanMark())}<span>${escape(words.scan)}</span></button>
            <p class="sheet-note" role="alert" hidden></p>
            <footer class="welcome-foot"><span class="login-ways">${way("password", words.withPassword)}</span><span></span></footer>
          </div>`;
        this.drawName();
        break;
      case "create":
        this.body.innerHTML = `
          <form class="create-form welcome-form" autocomplete="off">
            <h2 class="welcome-form-title">${escape(words.create)}</h2>
            <p class="welcome-form-what">${escape(words.createHub)}</p>
            <div class="field-pair">
              <label class="field"><span class="field-label">${escape(words.worldName)}</span>
                <input name="world" class="mono" required spellcheck="false" autocapitalize="off" placeholder="home" value="${escape(this.typed.world)}"></label>
              <label class="field"><span class="field-label">${escape(words.admin)}</span>
                <input name="account" class="mono" required spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="me" value="${escape(this.typed.account)}"></label>
            </div>
            ${newPassword(true)}
            <div class="field reach-field"><span class="field-label" title="${escape(words.reachWhat)}">${escape(words.reach)}</span>
              <div class="reach-chips"></div>
              <div class="reach-more">
                <input name="extra" class="mono" spellcheck="false" autocapitalize="off" placeholder="${escape(words.extraHint)}" aria-label="${escape(words.extra)}" value="${escape(this.typed.extra)}">
                <label class="reach-port"><span>${escape(words.port)}</span><input name="port" class="mono" inputmode="numeric" pattern="[0-9]{2,5}" required value="${escape(this.typed.port)}"></label>
              </div>
            </div>
            <div class="login-name"></div>
            ${foot(this.backWay(), words.createGo, words.createAsks)}
          </form>`;
        this.drawName();
        void this.drawAddresses();
        break;
      case "remote":
        if (this.options.remote) {
          this.remote = new RemoteWorld(this.body, {
            words,
            seams: this.options.remote,
            // Its name is asked at its first login; after that it keeps it.
            here: () => {
              const world = this.options.store.whole.peek();
              return { name: world?.here ?? "", fixed: loginsOf(world?.accounts).length > 0 };
            },
            loginsAt: (address) =>
              (this.options.store.whole.peek()?.accounts.hubs ?? [])
                .filter((hub) => hub.addresses.includes(address))
                .flatMap((hub) => hub.logins.map((account) => ({ hub: hub.name, account }))),
            back: words.back,
            done: (done) => {
              this.done = done;
              this.created = true;
              void this.options.store.refresh().then(() => this.arrived());
            },
          });
        }
        break;
      case "done":
        this.drawDone();
        break;
    }
    this.drawn.seen(this.body);
    this.drawStrength();
  }

  /** Under a new password: the bar before it is typed, then what it
   *  still lacks, or how strong it is. */
  private drawStrength(): void {
    sayPassword(this.body, this.options.words, this.body.querySelector<HTMLInputElement>("[name=account]")?.value.trim() ?? "");
  }

  /** Back to the two ways, for a computer that has none yet. */
  private backWay(): string {
    if (this.options.device || this.loggedIn()) {
      const { words } = this.options;
      return [
        this.options.create && this.step !== "create" ? `<button class="text-button login-way" type="button" data-way="create">${escape(words.create)}</button>` : "",
        this.options.remote && this.step !== "remote" ? `<button class="text-button login-way" type="button" data-way="remote">${escape(words.remote)}</button>` : "",
      ].join("");
    }
    return `<button class="text-button login-way" type="button" data-way="choose">${escape(this.options.words.back)}</button>`;
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

  /** A name is asked where one is chosen now: the first login, a phone's
   *  or a computer's, which joins as one of the account's machines. A
   *  device keeps one name on every server after that. */
  private drawName(): void {
    const slot = this.body.querySelector<HTMLElement>(".login-name");
    if (!slot) return;
    const { words, store } = this.options;
    const world = store.whole.peek();
    const first = !loginsOf(world?.accounts).length;
    const typed = slot.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
    if (!first) {
      slot.innerHTML = "";
      return;
    }
    // A phone has no name until it first logs in, so it is offered what it
    // calls itself as a name it can change; a computer has its own, which
    // the field shows faintly and takes when left empty.
    const phone = this.options.device;
    const shown = phone ? "" : (world?.here ?? "");
    slot.innerHTML = `<label class="field"><span class="field-label">${escape(phone ? words.phoneName : words.name)}</span>
      <input name="name" class="mono" autocomplete="off" spellcheck="false" autocapitalize="off"${phone ? " required" : ""} placeholder="${escape(shown)}" value="${escape(typed || this.typed.name || (phone ? this.suggested : ""))}"></label>`;
  }

  /** What the code pasted lets this computer into, before it is used: whose
   *  machine it becomes and on which network; for a managed machine, what
   *  the owner's agents do here and that it uses nothing itself, or why
   *  this Fastctx cannot become one. */
  private drawConfirm(): void {
    const { words } = this.options;
    const peeked = this.peeked;
    const facts = peeked
      ? `<dl class="facts code-facts">
          <dt>${escape(words.codeOwner)}</dt><dd>${escape(peeked.account)}</dd>
          <dt>${escape(words.codeNetwork)}</dt><dd>${escape(peeked.hub)}</dd>
          ${peeked.label ? `<dt>${escape(words.code)}</dt><dd>${escape(peeked.label)}</dd>` : ""}
        </dl>`
      : "";
    const refused = peeked?.refused;
    const card = peeked?.managed
      ? `<div class="code-card managed">
          <div class="code-card-art">${managedMachine("managed-art code-art")}</div>
          <h2 class="code-card-title">${escape(words.managedTitle)}</h2>
          ${facts}
          <ul class="code-card-what"><li>${escape(words.managedCan(peeked.account))}</li><li>${escape(words.managedNot)}</li></ul>
        </div>`
      : `<div class="code-card">
          <h2 class="code-card-title">${escape(peeked ? words.codeAs(peeked.account) : words.codeUnknown)}</h2>
          ${facts}
        </div>`;
    const unfit = refused
      ? `<div class="welcome-slip code-unfit" role="alert"><span><b>${escape(words.fresh)}</b>${escape(words.unfit[refused.why])}</span>${this.options.remove ? `<button class="text-button remove-first" type="button">${escape(words.removeFirst)}</button>` : ""}</div>`
      : "";
    this.body.innerHTML = `
      <form class="code-confirm welcome-form">
        ${card}
        ${unfit}
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="welcome-foot"><span class="login-ways"><button class="text-button login-way" type="button" data-way="code">${escape(words.back)}</button></span><button class="sheet-go" type="submit"${refused ? " disabled" : ""}>${escape(words.codeGo)}</button></footer>
      </form>`;
  }

  /** What is left for the person once in: a startup service that did not
   *  install. Nothing when all is well. Agents are not set up here
   *  (`world-gui.md`, 账号、机器、共享与协助): their own page does that. */
  private wanting(): { kept: string } {
    const { words } = this.options;
    const kept = this.done?.kept;
    const said = this.options.device
      ? ""
      : this.done?.unkept
        ? words.kept.declined
        : !kept
          ? ""
          : kept.kind === "command"
          ? "command"
          : kept.kind === "declined" || kept.kind === "unsupported"
            ? words.kept[kept.kind]
            : "";
    return { kept: said };
  }

  /** In: the World's drawing with this computer on it, a short title, and
   *  only what needs the person. */
  private drawDone(): void {
    const { words } = this.options;
    const done = this.done!;
    const wanting = this.wanting();
    const kept = this.done?.kept;
    const slips = [
      wanting.kept === "command" && kept?.kind === "command"
        ? `<div class="welcome-slip"><span>${escape(words.keptCommand)}</span><pre class="welcome-command mono">${escape(kept.command)}</pre></div>`
        : wanting.kept
          ? `<div class="welcome-slip"><span title="${escape(done.unkept ?? "")}">${escape(wanting.kept)}</span>${this.options.keep && (kept?.kind === "declined" || done.unkept) ? `<button class="text-button keep-now" type="button">${escape(words.keepGo)}</button>` : ""}</div>`
          : "",
    ].join("");
    const who = [done.account, this.options.device ? "" : done.name].filter(Boolean).join(" · ");
    // A picture, not a place to work: the drawing is shown, never pointed
    // at or pressed (`world-gui.md`, 登录或新建成功之后).
    this.body.innerHTML = `
      <div class="welcome-done${this.risen ? " risen" : ""}">
        <div class="welcome-drawing" inert aria-hidden="true"></div>
        <h2 class="welcome-done-title">${escape(this.created ? words.created(done.hub) : words.joined(done.hub))}</h2>
        <p class="welcome-done-who">${escape(who)}</p>
        ${slips ? `<div class="welcome-slips">${slips}</div>` : ""}
        <p class="sheet-note" role="alert" hidden></p>
        <div class="welcome-done-acts"><button class="sheet-go enter" type="button">${escape(words.enter)}</button>${this.created && this.options.invite ? `<button class="text-button invite-next" type="button">${escape(words.addDevice)}</button>` : ""}</div>
      </div>`;
    this.risen = true;
    const drawing = this.options.drawing;
    if (drawing) {
      this.body.querySelector(".welcome-drawing")!.append(drawing.element);
      drawing.mark(this.options.device ? null : done.name);
    }
  }

  /** In: the drawing shows this device arriving in its place, with one
   *  line and the way in (`world-gui.md`, 登录或新建成功之后). A World made
   *  here also offers the next device. */
  private async arrived(): Promise<void> {
    this.remote?.leave();
    this.remote = null;
    await this.refreshSetup();
    this.element.classList.add("in");
    this.step = "done";
    this.draw();
    this.drawHere();
    this.body.querySelector<HTMLElement>(".invite-next, .enter")?.focus({ preventScroll: true });
  }

  /** The address typed beside the pills, as an IP without its port. */
  private extraAddresses(): string[] {
    const extra = this.typed.extra.trim();
    if (!extra) return [];
    return [extra.replace(/:\d+$/, "")];
  }

  /** Says what went wrong under the form; the original words of a problem
   *  stay a hover away, or a touch where there is no pointer to rest. */
  private note(text: string, detail = ""): void {
    const note = this.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.textContent = text;
    note.title = detail;
    note.dataset.detail = detail && detail !== text ? detail : "";
    note.hidden = !text;
  }

  /** The original words of the problem under the form, shown or put away. */
  private toggleDetail(note: HTMLElement): void {
    const detail = note.dataset.detail;
    if (!detail) return;
    const shown = note.querySelector(".sheet-note-detail");
    if (shown) return shown.remove();
    const line = document.createElement("span");
    line.className = "sheet-note-detail mono";
    line.textContent = detail;
    note.append(line);
  }

  /** This device's logins now, as `hub/account`, the store asked first. */
  private async loginsNow(): Promise<Set<string>> {
    await this.options.store.refresh();
    return new Set(loginsOf(this.options.store.world.peek()?.accounts).map((login) => `${login.hub}/${login.account}`));
  }

  /** After an error, whether this device is in all the same: an answer
   *  lost on its way back, or a step after the hub let it in that failed,
   *  leaves a login here that was not here `before`, to `account` where it
   *  was typed. That login, as a login's result; none where there is no
   *  such login. */
  private async inAnyway(before: Set<string>, account: string | null): Promise<LoggedIn | null> {
    await this.options.store.refresh();
    const world = this.options.store.world.peek();
    const login = loginsOf(world?.accounts).find(
      (one) => !before.has(`${one.hub}/${one.account}`) && (!account || one.account === account),
    );
    return login && world ? { hub: login.hubName, name: world.accounts.machine, account: login.account } : null;
  }

  /** In: where this device arrived, or for a managed machine its one page.
   *  The page that says so failing to draw leaves the person in all the
   *  same, on the World's own pages. */
  private async landed(managed: boolean): Promise<void> {
    try {
      await this.options.store.refresh();
      if (managed) return this.options.enter();
      await this.arrived();
    } catch {
      this.options.enter();
    }
  }

  private problem(error: unknown): void {
    const { text, detail } = plainly(error, this.options.words.problems);
    this.note(text, detail);
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
    const note = target.closest<HTMLElement>(".sheet-note");
    if (note) return this.toggleDetail(note);
    if (target.closest(".enter")) return this.options.enter();
    if (target.closest(".remove-first")) return this.options.remove?.();
    if (target.closest(".invite-next") && this.done && this.options.invite) {
      const login = loginsOf(this.options.store.world.peek()?.accounts).find((one) => one.hubName === this.done!.hub && one.account === this.done!.account);
      if (login) this.options.invite(login);
      return;
    }
    if (target.closest(".scan-code") && this.options.scan && !this.busy) {
      // A phone logs in with the code the camera read, under its own name:
      // the first time, the one asked for beside the button.
      const named = this.body.querySelector<HTMLInputElement>("[name=name]");
      if (named && !named.reportValidity()) return;
      this.busy = true;
      let token: string | null = null;
      let before = new Set<string>();
      try {
        token = await this.options.scan();
        if (!token) return;
        before = await this.loginsNow();
        this.done = await this.options.loginToken(token, named?.value.trim() || null);
      } catch (error) {
        const inAnyway = token ? await this.inAnyway(before, null) : null;
        if (!inAnyway) return this.problem(error);
        this.done = inAnyway;
      } finally {
        this.busy = false;
      }
      this.created = false;
      await this.landed(false);
      return;
    }
    if (target.closest(".place-now") && this.options.place) {
      await this.options.place().catch((error: unknown) => this.problem(error));
      return this.refreshSetup();
    }
    if (target.closest(".keep-now") && !this.busy && this.options.keep && this.done) {
      this.busy = true;
      try {
        const kept = await this.options.keep();
        this.done = { ...this.done, kept, unkept: null };
        this.draw();
      } catch (error) {
        this.problem(error);
      } finally {
        this.busy = false;
      }
      return;
    }
  }

  /** Asks the code's server what it lets this computer into, and shows it
   *  on a card before the code is used. */
  private async peek(go: HTMLButtonElement, label: string, name: string | null): Promise<void> {
    const { words } = this.options;
    const code = this.typed.code.replace(/\s+/g, "");
    const peek = this.options.peekToken;
    if (!code || !peek) return;
    this.typed.code = code;
    const check = this.options.nameProblem;
    this.busy = true;
    go.disabled = true;
    go.textContent = words.peeking;
    this.note("");
    try {
      const problem = name && check ? await check("machine", name) : null;
      if (problem) {
        this.note(words.nameSaid(problem));
        return;
      }
      this.peeked = await peek(code);
      this.busy = false;
      this.go("confirm");
    } catch (error) {
      this.problem(error);
    } finally {
      this.busy = false;
      if (this.step === "code") {
        go.disabled = false;
        go.textContent = label;
      }
    }
  }

  private async submit(): Promise<void> {
    if (this.busy || this.step === "done" || this.step === "choose" || this.step === "scan") return;
    const { words } = this.options;
    const field = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const label = go.textContent ?? "";
    let act: () => Promise<LoggedIn>;
    this.keep();
    const name = (this.step === "confirm" ? this.typed.name : field("name")?.value)?.trim() || null;
    if (this.step === "code") return this.peek(go, label, name);
    // A name is checked by the World's rule before anything is sent; the
    // page takes nothing else meanwhile. Creating a World asks for two
    // names, so there the note says which field it is about.
    const named = async (kind: "machine" | "hub", text: string | null): Promise<boolean> => {
      const check = this.options.nameProblem;
      if (!text || !check) return true;
      this.busy = true;
      const problem = await check(kind, text).finally(() => (this.busy = false));
      if (!problem) return true;
      const said = words.nameSaid(problem);
      const label = kind === "hub" ? words.worldName : this.options.device ? words.phoneName : words.name;
      this.note(this.step === "create" ? words.inField(label, said) : said);
      field(kind === "hub" ? "world" : "name")?.focus();
      return false;
    };
    if (this.step !== "confirm" && !(await named("machine", name))) return;
    // A new password must pass before anything is sent.
    const fresh = (account: string): boolean => passwordReady(this.body, words, account);
    if (this.step === "confirm") {
      if (this.peeked?.refused) return;
      const code = this.typed.code.trim();
      act = () => this.options.loginToken(code, name);
    } else if (this.step === "create") {
      const create = this.options.create;
      const world = field("world")?.value.trim() ?? "";
      const account = field("account")?.value.trim() ?? "";
      const password = field("password")?.value ?? "";
      if (!create || !world || !account || !password || !fresh(account)) return;
      if (!(await named("hub", world))) return;
      const reach = [...this.chosen, ...this.extraAddresses()].map((address) => `${address}:${this.typed.port.trim()}`);
      if (!reach.length) return this.note(words.noReach);
      act = () => create(world, account, password, reach, true, name);
    } else {
      const hub = field("hub")?.value.trim() ?? "";
      const account = field("account")?.value.trim() ?? "";
      const password = field("password")?.value ?? "";
      // A computer logging in joins as one of the account's machines; a
      // phone only logs in (`world-network.md`, 一台电脑和中枢).
      const register = !this.options.device;
      if (!hub || !account || !password) return;
      if (this.step === "open") {
        const open = this.options.openAccount;
        if (!open || !fresh(account)) return;
        act = () => open(hub, account, password, register, name);
      } else act = () => this.options.login(hub, account, password, register, name);
    }
    const creating = this.step === "create";
    const managed = this.step === "confirm" && !!this.peeked?.managed;
    this.busy = true;
    go.disabled = true;
    go.textContent = creating ? words.creating : words.going;
    this.note("");
    const typed = this.step === "confirm" ? null : (field("account")?.value.trim() ?? null);
    const before = await this.loginsNow();
    try {
      this.done = await act();
    } catch (error) {
      const inAnyway = await this.inAnyway(before, typed);
      if (!inAnyway) {
        this.busy = false;
        go.disabled = false;
        go.textContent = label;
        return this.problem(error);
      }
      this.done = inAnyway;
    }
    this.created = creating;
    this.busy = false;
    // A managed machine has nothing more to show here: its one page says
    // what it is.
    await this.landed(managed);
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

/** Scanning: the four corners of a code's frame, drawn with the marker. */
function scanMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 64 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round" stroke-width="3"><g class="ink-graphite"><path d="M10 22 V10 H22M42 10 H54 V22M54 42 V54 H42M22 54 H10 V42"/></g><g class="ink-clay"><path d="M20 32 H44"/></g></g></svg>`;
}

/** On a server: a box with a knot inside it, the line reaching it drawn. */
function remoteMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 96 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round"><g class="ink-graphite" stroke-width="3"><rect x="52" y="16" width="30" height="34" rx="4"/><circle cx="67" cy="33" r="5"/></g><g class="ink-clay" stroke-width="3"><circle cx="18" cy="33" r="9"/><path d="M29 33 C36 30 42 30 47 32"/></g></g></svg>`;
}
