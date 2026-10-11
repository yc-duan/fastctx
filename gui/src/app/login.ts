// Coming into a World (`world-gui.md`, 账号、机器、共享与借出). A computer
// that is in none yet is offered two ways: join one with the server's
// address, an account and its password, or create one with this computer
// as its server and the person its administrator. A phone scans the code a
// computer shows, or logs in with an account and password. A computer that
// logs in joins as one of the account's machines; a phone only logs in.
// Once in, the page only confirms it; on a World just made another device
// can be added at once.

import { escape } from "../conversation/prose";
import type { KeptNow, LoggedIn, Setup } from "../world/bridge";
import type { Login } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { LoginWords } from "./account-words";
import { passwordProblem, passwordStrength } from "./password";
import { plainly } from "./problem";
import { loginLabel } from "./reach";
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
  head(title: string, actions?: string): string;
  /** This interface runs on a phone: it logs in as a device, and nothing
   *  is set up on it. */
  device: boolean;
  /** Where this computer stands, where the shell can say. */
  setup?: (() => Promise<Setup>) | undefined;
  place?: (() => Promise<string>) | undefined;
  login(hub: string, account: string, password: string, register: boolean, name: string | null): Promise<LoggedIn>;
  /** Logs a phone in with the code it scanned. */
  loginToken(token: string, name: string | null): Promise<LoggedIn>;
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
  /** Reads a QR code with the camera, where the shell can; null when the
   *  person turned back. */
  scan?: (() => Promise<string | null>) | undefined;
  /** Installs the startup service where it is missing. */
  keep?: (() => Promise<KeptNow>) | undefined;
  /** The World's drawing, with this computer marked on it once it is in. */
  drawing?: { element: HTMLElement; mark(machine: string | null): void; stop(): void } | undefined;
  /** Done here: on to the World. */
  enter(): void;
}

type Step = "choose" | "password" | "open" | "scan" | "create" | "remote" | "done";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";
const machineRule = "[a-z][a-z0-9\\-]{0,31}";
const hubRule = "[a-z][a-z0-9\\-]{0,31}";
/** The port a server made here listens on unless told otherwise. */
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
  private typed = { hub: "", account: "", world: "", port: String(hubPort), extra: "" };
  /** This computer's addresses, once asked, and those chosen. */
  private addresses: HereAddress[] | null = null;
  private chosen = new Set<string>();
  private readonly stop: () => void;
  /** The way through a server over SSH, while it is open. */
  private remote: RemoteWorld | null = null;

  constructor(private readonly options: LoginOptions) {
    const { words } = options;
    this.title = words.title;
    this.step = options.device ? (options.scan ? "scan" : "password") : this.loggedIn() ? "password" : "choose";
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
    this.element.addEventListener("input", (event) => {
      const input = event.target as HTMLInputElement;
      if (input.name === "password" || input.name === "account") this.drawStrength();
    });
    this.element.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.submit();
    });
    this.draw();
    void this.refreshSetup();
    // The logins this computer has, as the World answers.
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
    const newPassword = (pair: boolean) => {
      const fields = `<label class="field"><span class="field-label">${escape(words.password)}</span>
        <input name="password" type="password" required autocomplete="new-password"></label>
      <label class="field"><span class="field-label">${escape(words.passwordAgain)}</span>
        <input name="again" type="password" required autocomplete="new-password"></label>`;
      return `${pair ? `<div class="field-pair">${fields}</div>` : fields}
      <p class="password-meter" aria-live="polite"><span class="password-bars"><i></i><i></i><i></i></span><span class="password-said"></span></p>`;
    };
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
            ${
              this.options.remote
                ? `<button class="welcome-way" type="button" data-way="remote">${remoteMark()}<span class="welcome-way-title">${escape(words.remote)}</span><span class="welcome-way-what">${escape(words.remoteWhat)}</span></button>`
                : ""
            }
          </div>
          ${this.aloneWay()}`;
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
      case "scan":
        this.body.innerHTML = `
          <div class="welcome-form scan-form">
            <button class="scan-code" type="button">${scanMark()}<span>${escape(words.scan)}</span></button>
            <p class="sheet-note" role="alert" hidden></p>
            <footer class="welcome-foot"><span class="login-ways">${way("password", words.withPassword)}</span><span></span></footer>
          </div>`;
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
            name: () => null,
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
    this.drawStrength();
  }

  /** Under a new password: what it still lacks, or how strong it is. */
  private drawStrength(): void {
    const meter = this.body.querySelector<HTMLElement>(".password-meter");
    if (!meter) return;
    const { words } = this.options;
    const account = this.body.querySelector<HTMLInputElement>("[name=account]")?.value.trim() ?? "";
    const password = this.body.querySelector<HTMLInputElement>("[name=password]")?.value ?? "";
    const problem = password ? passwordProblem(account, password) : null;
    const strength = passwordStrength(account, password);
    meter.dataset.strength = String(strength);
    meter.classList.toggle("wanting", problem !== null);
    meter.querySelector(".password-said")!.textContent = problem ? words.passwordRule[problem] : strength ? words.strength[strength - 1]! : "";
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

  /** A name is asked where one is chosen now: the first login, a phone's
   *  or a computer's, which joins as one of the account's machines. A
   *  device keeps one name on every server after that. */
  private drawName(): void {
    const slot = this.body.querySelector<HTMLElement>(".login-name");
    if (!slot) return;
    const { words, store } = this.options;
    const world = store.world.peek();
    const first = !loginsOf(world?.accounts).length;
    const typed = slot.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
    if (!first) {
      slot.innerHTML = "";
      return;
    }
    // A phone has no name until it first logs in; a computer has its own.
    const phone = this.options.device;
    slot.innerHTML = `<label class="field"><span class="field-label">${escape(phone ? words.phoneName : words.name)}</span>
      <input name="name" class="mono" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${machineRule}"${phone ? " required" : ""} placeholder="${escape(phone ? "phone" : (world?.here ?? ""))}" value="${escape(typed)}"></label>`;
  }

  /** What is left for the person once in: a startup service that did not
   *  install. Nothing when all is well. Agents are not set up here
   *  (`world-gui.md`, 账号、机器、共享与借出): their own page does that. */
  private wanting(): { kept: string } {
    const { words } = this.options;
    const kept = this.done?.kept;
    const said =
      !kept || this.options.device
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
          ? `<div class="welcome-slip"><span>${escape(wanting.kept)}</span>${this.options.keep && kept?.kind === "declined" ? `<button class="text-button keep-now" type="button">${escape(words.keepGo)}</button>` : ""}</div>`
          : "",
    ].join("");
    const who = [done.account, this.options.device ? "" : done.name].filter(Boolean).join(" · ");
    this.body.innerHTML = `
      <div class="welcome-done">
        <div class="welcome-drawing"></div>
        <h2 class="welcome-done-title">${escape(this.created ? words.created(done.hub) : words.joined(done.hub))}</h2>
        <p class="welcome-done-who mono">${escape(who)}</p>
        ${slips ? `<div class="welcome-slips">${slips}</div>` : ""}
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="welcome-foot"><span class="login-ways">${this.created && this.options.invite ? `<button class="text-button invite-next" type="button">${escape(words.addDevice)}</button>` : ""}</span><button class="sheet-go enter" type="button">${escape(words.enter)}</button></footer>
      </div>`;
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
   *  stay a hover away. */
  private note(text: string, detail = ""): void {
    const note = this.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.textContent = text;
    note.title = detail;
    note.hidden = !text;
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
    if (target.closest(".alone") || target.closest(".enter")) return this.options.enter();
    if (target.closest(".invite-next") && this.done && this.options.invite) {
      const login = loginsOf(this.options.store.world.peek()?.accounts).find((one) => one.hubName === this.done!.hub && one.account === this.done!.account);
      if (login) this.options.invite(login);
      return;
    }
    if (target.closest(".scan-code") && this.options.scan && !this.busy) {
      // A phone logs in with the code the camera read, under its own name.
      try {
        const token = await this.options.scan();
        if (!token) return;
        this.busy = true;
        this.done = await this.options.loginToken(token, null);
        this.created = false;
        await this.options.store.refresh();
        await this.arrived();
      } catch (error) {
        this.problem(error);
      } finally {
        this.busy = false;
      }
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
        this.done = { ...this.done, kept };
        this.draw();
      } catch (error) {
        this.problem(error);
      } finally {
        this.busy = false;
      }
      return;
    }
  }

  private async submit(): Promise<void> {
    if (this.busy || this.step === "done" || this.step === "choose" || this.step === "scan") return;
    const { words } = this.options;
    const field = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    const name = field("name")?.value.trim() || null;
    const go = this.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    const label = go.textContent ?? "";
    let act: () => Promise<LoggedIn>;
    this.keep();
    // A new password must pass before anything is sent.
    const fresh = (account: string, password: string): boolean => {
      const problem = passwordProblem(account, password);
      if (problem) {
        this.note(words.passwordRule[problem]);
        return false;
      }
      if (password !== field("again")?.value) {
        this.note(words.passwordsDiffer);
        return false;
      }
      return true;
    };
    if (this.step === "create") {
      const create = this.options.create;
      const world = field("world")?.value.trim() ?? "";
      const account = field("account")?.value.trim() ?? "";
      const password = field("password")?.value ?? "";
      if (!create || !world || !account || !password || !fresh(account, password)) return;
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
        if (!open || !fresh(account, password)) return;
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
      await this.arrived();
    } catch (error) {
      this.busy = false;
      go.disabled = false;
      go.textContent = label;
      this.problem(error);
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

/** Scanning: the four corners of a code's frame, drawn with the marker. */
function scanMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 64 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round" stroke-width="3"><g class="ink-graphite"><path d="M10 22 V10 H22M42 10 H54 V22M54 42 V54 H42M22 54 H10 V42"/></g><g class="ink-clay"><path d="M20 32 H44"/></g></g></svg>`;
}

/** On a server: a box with a knot inside it, the line reaching it drawn. */
function remoteMark(): string {
  return `<svg class="welcome-way-mark" viewBox="0 0 96 64" aria-hidden="true"><g filter="url(#pen-marker)" fill="none" stroke-linecap="round"><g class="ink-graphite" stroke-width="3"><rect x="52" y="16" width="30" height="34" rx="4"/><circle cx="67" cy="33" r="5"/></g><g class="ink-clay" stroke-width="3"><circle cx="18" cy="33" r="9"/><path d="M29 33 C36 30 42 30 47 32"/></g></g></svg>`;
}
