// Making a World on a server, or adding a server to a World, from this
// computer over SSH (`world-gui.md`, 新建 and 添加设备).
//
// A new World is asked for in four steps, and nothing is made on the server
// before the last: the server and how to sign in to it, with the
// fingerprint it shows the first time; the network's name and port; its one
// administrator; the names the server and this computer take in it. Signing
// in looks at the server, so a way in that does not work, a port taken and a
// World kept there already are said before anything else is asked. A World
// found there is finished, by its administrator, or taken down. Once the hub
// is made it stays: a step after it that fails is tried again, or the hub is
// taken down and the network asked for anew; a page left halfway finds it
// the next time the server is signed in to.
//
// The program this app carries for the server's architecture goes up to it;
// each step is ticked as it is done. Nothing typed here is kept.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { LoggedIn, NameProblem } from "../world/bridge";
import type { LoginWords } from "./account-words";
import { newPasswordFields, passwordReady, sayPassword } from "./new-password";
import { plainly } from "./problem";
import { progressSteps } from "./progress-steps";
import { stepMark as mark } from "../pen/step-mark";

export type RemoteAuth = { kind: "password"; password: string } | { kind: "key"; key: string; passphrase: string | null };

/** What the server showed of itself: where the connection went, and its key
 *  as this computer's own SSH records know it, if they do. */
export interface RemoteKey {
  address: string;
  fingerprint: string;
  known: "known" | "unknown" | "changed";
}

export interface RemoteServer {
  host: string;
  port: number;
  user: string;
  auth: RemoteAuth;
  /** The fingerprint the person confirmed; the connection is refused if
   *  the server shows another. */
  fingerprint: string;
  /** The password sudo asks for, where the user is not root. */
  sudo: string | null;
}

export interface RemoteHub extends RemoteServer {
  world: string;
  admin: string;
  password: string;
  hubPort: number;
}

/** What the server showed once signed in to. */
export interface Looked {
  /** The name it would take in the World, made from its host name. */
  machine: string;
  /** The ports something listens on there already. */
  busy: number[];
  /** A World's hub it keeps, as far as it could be read: one left half made
   *  reads as neither name nor port. */
  hub: { name: string | null; port: number | null; address: string | null } | null;
  /** It is a machine of a World, its node a service. */
  joined: boolean;
}

export type RemoteStep = "connect" | "check" | "upload" | "hub" | "join" | "arrive" | "withdraw";

export interface RemoteSeams {
  probe(host: string, port: number): Promise<RemoteKey>;
  /** Signs in and looks at the server; a way in that does not work fails
   *  here. */
  look(server: RemoteServer): Promise<Looked>;
  /** Connects, checks the system, uploads the program and makes the hub;
   *  the session stays open for the server to join. */
  hub(request: RemoteHub, step: (step: RemoteStep) => void): Promise<{ session: number; address: string; machine: string }>;
  /** Connects, checks the system and uploads the program, for a World there
   *  is; the session stays open for the server to join. */
  open(server: RemoteServer, step: (step: RemoteStep) => void): Promise<{ session: number; machine: string }>;
  /** Joins the server; kept open, the session stays for what follows. */
  join(session: number, token: string, step: (step: RemoteStep) => void, keepOpen?: boolean, proxy?: string | null): Promise<void>;
  /** Installs the service of the hub moving to the server. */
  arrive(session: number, step: (step: RemoteStep) => void): Promise<void>;
  /** Takes the World the server keeps off it. */
  withdraw(server: RemoteServer, step: (step: RemoteStep) => void): Promise<void>;
  /** This computer's login to a World, ended, before its hub is taken down. */
  logout(hub: string, account: string): Promise<void>;
  close(session: number): Promise<void>;
  /** This computer signs in to the World made, as its administrator, under
   *  `name`, or its own name when null. */
  login(address: string, admin: string, password: string, name: string | null): Promise<LoggedIn>;
  /** A one-time code for the server to join with, as a machine of `admin`. */
  token(hub: string, admin: string, machine: string): Promise<string>;
  /** What is wrong with a World's or a machine's name as typed. */
  nameProblem(kind: "machine" | "hub", text: string): Promise<NameProblem | null>;
}

/** What the server's own side says when sudo wants a password it was not
 *  given, and when only a line run there takes its World down
 *  (`src-tauri/src/remote_world/ssh.rs`). */
const SUDO_WANTED = "sudo needs a password";
const WITHDRAW_BY_HAND = "run in the server's terminal";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

const HUB_PORT = "7443";

type Stage = "server" | "key" | "found" | "withdraw" | "network" | "admin" | "machines" | "making";

/** Which of the four steps each stage is. */
const STAGE_OF: Record<Stage, number> = { server: 0, key: 0, found: 0, withdraw: 0, network: 1, admin: 2, machines: 3, making: 4 };

/** A server added to a World there is. */
export interface Adding {
  token(machine: string): Promise<string>;
  /** What is done on the server once it joined, over the same session,
   *  as the hub moving to it: given the address typed for it. */
  then?(session: number, machine: string, host: string, step: (step: RemoteStep) => void): Promise<void>;
  added(machine: string): void;
}

interface Progress {
  step: RemoteStep | "login";
  state: "ahead" | "doing" | "done" | "failed";
}

export class RemoteWorld {
  private stage: Stage = "server";
  private typed = { host: "", sshPort: "22", user: "root", by: "password" as "password" | "key", world: "", admin: "", hubPort: HUB_PORT, name: "", here: "", confirm: "", proxy: "" };
  private key: { text: string; name: string } | null = null;
  private secret = { login: "", passphrase: "", sudo: "", password: "", again: "" };
  private shown: RemoteKey | null = null;
  /** The server as last looked at. */
  private looked: Looked | null = null;
  private wantsSudo = false;
  private progress: Progress[] = [];
  /** The World asked for is the one the server keeps already. */
  private resume = false;
  /** Steps already done stay done when the person tries again. */
  private made: { session: number; address: string; machine: string } | null = null;
  private opened: { session: number; machine: string } | null = null;
  private signedIn: LoggedIn | null = null;
  private joined = false;
  /** What follows the joining is done, and the session it kept closed. */
  private followed = false;
  /** The last try at making failed: it is tried again, or taken back. */
  private failed = false;
  private busy = false;

  constructor(
    private readonly body: HTMLElement,
    private readonly options: {
      words: LoginWords;
      seams: RemoteSeams;
      /** This computer's name, and whether it is fixed, as it is once it is
       *  in a network; a new World asks it where it is not. */
      here?(): { name: string; fixed: boolean };
      /** The logins this computer has to the World reached at `address`. */
      loginsAt?(address: string): { hub: string; account: string }[];
      /** The way back to the ways in. */
      back: string;
      done?(loggedIn: LoggedIn): void;
      /** Adding the server to a World there is, instead of making one: a
       *  code for it to join with, by the name it will have, and word once
       *  it has. */
      adding?: Adding;
    },
  ) {
    body.addEventListener("click", this.clicked);
    body.addEventListener("change", this.changed);
    body.addEventListener("submit", this.submitted);
    body.addEventListener("input", this.typedIn);
    this.draw();
  }

  /** What is typed that the page answers as it is typed: a new password's
   *  strength, the address a port makes. */
  private readonly typedIn = (event: Event): void => {
    const input = event.target as HTMLInputElement;
    if (input.name === "password" || input.name === "admin") {
      if (!this.resume) sayPassword(this.body, this.options.words, this.body.querySelector<HTMLInputElement>("[name=admin]")?.value.trim() ?? "");
    } else if (input.name === "hubPort") {
      const at = this.body.querySelector<HTMLElement>(".remote-at");
      if (at) at.textContent = this.options.words.networkAt(this.reachedAt(input.value.trim() || HUB_PORT));
    }
  };

  /** The enclosing form was submitted: the step this one is on goes on. */
  submit(): void {
    void this.next();
  }

  leave(): void {
    this.body.removeEventListener("click", this.clicked);
    this.body.removeEventListener("change", this.changed);
    this.body.removeEventListener("submit", this.submitted);
    this.body.removeEventListener("input", this.typedIn);
    this.closeSessions();
  }

  /** The sessions still open, closed: the server's temporary directory goes
   *  with them; a hub made stays. */
  private closeSessions(): void {
    const kept = !!this.options.adding?.then && !this.followed;
    for (const open of [this.made, this.opened]) {
      if (open && (!this.joined || kept)) void this.options.seams.close(open.session).catch((error: unknown) => console.warn("the session with the server could not be closed", error));
    }
  }

  private readonly clicked = (event: Event): void => {
    const target = event.target as HTMLElement;
    const by = target.closest<HTMLElement>("[data-by]")?.dataset.by;
    if (by === "password" || by === "key") {
      this.keep();
      this.typed.by = by;
      this.draw();
      return;
    }
    if (target.closest("[data-remote=choose-key]")) {
      this.body.querySelector<HTMLInputElement>("[name=keyfile]")?.click();
      return;
    }
    if (this.busy) return;
    const to = target.closest<HTMLElement>("[data-remote-to]")?.dataset.remoteTo as Stage | undefined;
    if (to) {
      this.keep();
      this.go(to);
    }
  };

  private readonly changed = (event: Event): void => {
    const input = event.target as HTMLInputElement;
    if (input.name !== "keyfile" || !input.files?.[0]) return;
    const file = input.files[0];
    void file.text().then((text) => {
      this.key = { text, name: file.name };
      const shown = this.body.querySelector<HTMLElement>(".remote-key-name");
      if (shown) shown.textContent = file.name;
    });
  };

  private readonly submitted = (event: Event): void => {
    event.preventDefault();
    void this.next();
  };

  /** To another stage; going back to the server forgets what it showed, as
   *  the way in may change. */
  private go(stage: Stage): void {
    if (stage === "server") {
      this.looked = null;
      this.resume = false;
    }
    if (stage === "withdraw") this.typed.confirm = "";
    this.stage = stage;
    this.draw();
  }

  private keep(): void {
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`)?.value;
    for (const name of ["host", "sshPort", "user", "world", "admin", "hubPort", "name", "here", "confirm", "proxy"] as const) this.typed[name] = value(name) ?? this.typed[name];
    this.secret.login = value("login") ?? this.secret.login;
    this.secret.passphrase = value("passphrase") ?? this.secret.passphrase;
    this.secret.sudo = value("sudo") ?? this.secret.sudo;
    this.secret.password = value("password") ?? this.secret.password;
    this.secret.again = value("again") ?? this.secret.again;
  }

  private draw(): void {
    const { words } = this.options;
    // Within another form, as the sheet that adds a device, a form of its
    // own would be dropped: there it is a block, and the sheet hands its
    // submission on (`submit`).
    const form = this.options.adding ? "div" : "form";
    // The way back to the ways in, from the first step; in the sheet that
    // adds a device its own switch over the form is that way.
    const back = (to: Stage | null, label = words.back) =>
      to
        ? `<button class="text-button login-way" type="button" data-remote-to="${to}">${escape(label)}</button>`
        : this.options.adding
          ? ""
          : `<button class="text-button login-way" type="button" data-way="choose">${escape(this.options.back)}</button>`;
    const foot = (go: string, ways: string, kind = "") =>
      `<p class="sheet-note" role="alert" hidden></p><footer class="welcome-foot"><span class="login-ways">${ways}</span><button class="sheet-go${kind}" type="submit"${this.busy ? " disabled" : ""}>${escape(go)}</button></footer>`;
    const field = (name: string, label: string, attributes: string, value: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span><input name="${name}" ${attributes} value="${escape(value)}"></label>`;
    const sudo = this.wantsSudo ? `<label class="field"><span class="field-label">${escape(words.sudoPassword)}</span><input name="sudo" type="password" required autocomplete="off" value="${escape(this.secret.sudo)}"></label>` : "";
    const steps = `<ol class="removal-list remote-steps"${this.progress.length ? "" : " hidden"}></ol>`;
    const open = (what = "") => `<${form} class="welcome-form remote-form" autocomplete="off">${this.head()}${what ? `<p class="welcome-form-what">${what}</p>` : ""}`;
    const close = `</${form}>`;
    switch (this.stage) {
      case "server": {
        const password = this.typed.by === "password";
        this.body.innerHTML = `${open(this.options.adding ? "" : escape(words.remoteHub))}
          <div class="field-pair remote-address">
            ${field("host", words.server, 'class="mono" required spellcheck="false" autocapitalize="off"', this.typed.host)}
            ${field("sshPort", words.sshPort, 'class="mono" inputmode="numeric" pattern="[0-9]{1,5}" required', this.typed.sshPort)}
          </div>
          <div class="field-pair">
            ${field("user", words.sshUser, 'class="mono" required spellcheck="false" autocapitalize="off"', this.typed.user)}
            <div class="field remote-by-field"><span class="field-label">${escape(words.signInBy)}</span>
              <div class="segmented remote-by" role="radiogroup" aria-label="${escape(words.signInBy)}">
                <button class="segment${password ? " chosen" : ""}" type="button" role="radio" aria-checked="${password}" data-by="password">${escape(words.password)}</button>
                <button class="segment${password ? "" : " chosen"}" type="button" role="radio" aria-checked="${!password}" data-by="key">${escape(words.keyFile)}</button>
              </div>
            </div>
          </div>
          ${
            password
              ? `<label class="field"><span class="field-label">${escape(words.password)}</span><input name="login" type="password" required autocomplete="off" value="${escape(this.secret.login)}"></label>`
              : `<div class="field-pair"><div class="field remote-key"><span class="field-label">${escape(words.keyFile)}</span><span class="remote-key-pick"><input name="keyfile" type="file" hidden><button class="pick-file" type="button" data-remote="choose-key">${glyph("file", 16)}<span>${escape(words.chooseKey)}</span></button><span class="remote-key-name mono">${escape(this.key?.name ?? "")}</span></span></div>
                 <label class="field"><span class="field-label">${escape(words.keyPassword)}<span class="field-optional">${escape(words.optional)}</span></span><input name="passphrase" type="password" autocomplete="off" value="${escape(this.secret.passphrase)}"></label></div>`
          }
          ${
            this.options.adding
              ? `<label class="field" title="${escape(words.proxyWhat)}"><span class="field-label">${escape(words.proxy)}<span class="field-optional">${escape(words.optional)}</span></span><input name="proxy" class="mono" spellcheck="false" autocapitalize="off" autocomplete="off" placeholder="http://host:port" value="${escape(this.typed.proxy)}"></label>`
              : ""
          }
          ${foot(this.busy ? words.looking : words.next, back(null))}${close}`;
        break;
      }
      case "key": {
        const shown = this.shown!;
        this.body.innerHTML = `<${form} class="welcome-form remote-form">
          <h2 class="welcome-form-title">${escape(words.firstTime)}</h2>
          <p class="remote-fingerprint mono" title="${escape(words.fingerprintWhat)}">${escape(shown.fingerprint)}</p>
          <p class="field-hint mono">${escape(shown.address)}</p>
          ${foot(this.busy ? words.looking : words.trust, back("server"))}${close}`;
        break;
      }
      case "found": {
        const hub = this.looked!.hub!;
        const at = hub.address ?? (hub.port ? this.reachedAt(String(hub.port)) : "");
        const go = hub.name ? (this.looked!.joined ? words.signInThere : words.resumeGo) : null;
        this.body.innerHTML = `${open()}
          <div class="remote-found">
            <p class="remote-found-title">${escape(hub.name ? words.found(hub.name) : words.foundHalf)}</p>
            ${at ? `<p class="remote-found-at mono">${escape(at)}</p>` : ""}
          </div>
          <p class="sheet-note" role="alert" hidden></p>
          <footer class="welcome-foot"><span class="login-ways">${back("server")}<button class="text-button login-way danger-text" type="button" data-remote-to="withdraw">${escape(words.withdrawGo)}</button></span>${go ? `<button class="sheet-go" type="submit">${escape(go)}</button>` : "<span></span>"}</footer>${close}`;
        break;
      }
      case "withdraw": {
        const name = this.held();
        this.body.innerHTML = `${open()}
          <div class="welcome-slip remote-withdraw" role="alert"><span><b>${escape(words.withdrawTitle(name))}</b>${escape(words.withdrawWhat)}</span></div>
          ${name ? field("confirm", words.withdrawType(name), 'class="mono" required spellcheck="false" autocapitalize="off" autocomplete="off"', this.typed.confirm) : ""}
          ${sudo}
          ${steps}
          ${foot(this.busy ? words.withdrawing : words.withdrawConfirm, this.busy ? "" : back(this.made || this.failed ? "making" : "found"), " danger")}${close}`;
        this.drawSteps();
        break;
      }
      case "network":
        this.body.innerHTML = `${open(`<span class="remote-at">${escape(words.networkAt(this.reachedAt(this.typed.hubPort.trim() || HUB_PORT)))}</span>`)}
          <div class="field-pair remote-network">
            ${field("world", words.worldName, 'class="mono" required spellcheck="false" autocapitalize="off" placeholder="home"', this.typed.world)}
            ${field("hubPort", words.port, 'class="mono" inputmode="numeric" pattern="[0-9]{1,5}" required', this.typed.hubPort)}
          </div>
          ${foot(words.next, back("server"))}${close}`;
        break;
      case "admin":
        this.body.innerHTML = `${open(escape(this.resume ? words.adminAgain : words.adminWhat))}
          ${field("admin", words.admin, `class="mono" required spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="me"`, this.typed.admin)}
          ${
            this.resume
              ? `<label class="field"><span class="field-label">${escape(words.password)}</span><input name="password" type="password" required autocomplete="current-password" value="${escape(this.secret.password)}"></label>`
              : newPasswordFields(words, { password: this.secret.password, again: this.secret.again })
          }
          ${foot(words.next, back(this.resume ? "found" : "network"))}${close}`;
        if (!this.resume) sayPassword(this.body, words, this.typed.admin.trim());
        break;
      case "machines": {
        if (this.options.adding) {
          // A World there is: nothing to name but the server, and sudo's
          // password where the server wants one.
          this.body.innerHTML = `<${form} class="welcome-form remote-form" autocomplete="off">
            <p class="remote-host mono">${escape(this.typed.user)}@${escape(this.typed.host)}</p>
            ${this.nameField()}
            ${sudo}
            ${steps}
            ${foot(words.addGo, back(null))}${close}`;
          this.drawSteps();
          break;
        }
        const here = this.options.here?.();
        this.body.innerHTML = `${open()}
          <div class="remote-machines">
            ${this.serverJoins() ? this.nameField() : ""}
            ${
              here && !here.fixed
                ? `<label class="field"><span class="field-label">${glyph("desktop", 14)}<span>${escape(words.hereName)}</span></span><input name="here" class="mono" required spellcheck="false" autocapitalize="off" autocomplete="off" value="${escape(this.typed.here || here.name)}"></label>`
                : ""
            }
          </div>
          ${foot(this.resume ? (this.looked?.joined ? words.signInThere : words.resumeGo) : words.createGo, back("admin"))}${close}`;
        break;
      }
      case "making": {
        if (this.options.adding) {
          this.body.innerHTML = `<${form} class="welcome-form remote-form" autocomplete="off">
            <p class="remote-host mono">${escape(this.typed.user)}@${escape(this.typed.host)}</p>
            ${this.nameField()}
            ${steps}
            ${foot(words.adding, "")}${close}`;
          this.drawSteps();
          (this.body.querySelector("input[name=name]") as HTMLInputElement).disabled = true;
          break;
        }
        const hubThere = !!this.made || this.resume;
        const ways = this.busy ? "" : hubThere ? `<button class="text-button login-way danger-text" type="button" data-remote-to="withdraw">${escape(words.withdrawGo)}</button>` : back("network", words.backToEdit);
        this.body.innerHTML = `${open()}
          ${this.facts()}
          ${steps}
          ${this.failed ? sudo : ""}
          ${foot(this.busy ? (this.resume ? words.going : words.creating) : words.retry, ways)}${close}`;
        this.drawSteps();
        break;
      }
    }
  }

  /** Over every stage of a new World: what it is and on which server, and
   *  the four steps, the one this is washed and those done ticked. */
  private head(): string {
    if (this.options.adding) return "";
    const { words } = this.options;
    const host = this.stage === "server" ? "" : this.typed.host.trim();
    return `<h2 class="welcome-form-title">${escape(words.remote)}${host ? ` <span class="mono">${escape(host)}</span>` : ""}</h2>
      ${progressSteps(words.remoteStages, STAGE_OF[this.stage], words.remote)}`;
  }

  /** What is being made, above its steps. */
  private facts(): string {
    const { words } = this.options;
    const here = this.options.here?.();
    const port = this.looked?.hub?.port ? String(this.looked.hub.port) : this.typed.hubPort.trim();
    const rows: [string, string][] = [
      [words.remoteStages[1], `${this.typed.world.trim()} ${this.looked?.hub?.address ?? this.reachedAt(port)}`],
      [words.remoteStages[2], this.typed.admin.trim()],
    ];
    if (this.serverJoins()) rows.push([words.remoteStages[0], this.typed.name.trim()]);
    if (here) rows.push([words.thisComputer, here.fixed ? here.name : this.typed.here.trim() || here.name]);
    return `<dl class="facts remote-facts">${rows.map(([what, value]) => `<dt>${escape(what)}</dt><dd class="mono">${escape(value)}</dd>`).join("")}</dl>`;
  }

  /** The name the server takes in the World: its host name to begin with,
   *  which the person may change. */
  private nameField(): string {
    const { words } = this.options;
    const icon = this.options.adding ? "" : glyph("server", 14);
    return `<label class="field"><span class="field-label">${icon}<span>${escape(words.serverName)}</span></span><input name="name" class="mono" required spellcheck="false" autocapitalize="off" autocomplete="off" value="${escape(this.typed.name)}"></label>`;
  }

  /** Where machines reach the network on `port`: the address the server was
   *  reached at over SSH. */
  private reachedAt(port: string): string {
    const address = this.shown?.address ?? this.typed.host.trim();
    const host = address.includes(":") && !address.startsWith("[") && address.split(":").length > 2 ? `[${address}]` : address.replace(/:\d+$/, "");
    return `${host}:${port}`;
  }

  /** The name of the World the server keeps, or made here. */
  private held(): string | null {
    return this.looked?.hub?.name ?? (this.made ? this.typed.world.trim() : null);
  }

  /** Whether the server is still to join: not when it is a machine of the
   *  World it keeps already. */
  private serverJoins(): boolean {
    return !(this.resume && this.looked?.joined);
  }

  /** The steps a try will take, all listed before the first begins so the
   *  list is ticked in place and never grows; those done before stay done,
   *  one that failed is to come again. */
  private plan(steps: Progress["step"][]): void {
    this.progress = steps.map((step) => {
      const known = this.progress.find((one) => one.step === step);
      return { step, state: known && known.state !== "failed" ? known.state : "ahead" };
    });
    this.drawSteps();
  }

  /** Each step with a pencil tick when done, a cross when it failed, a dot
   *  while it is under way; those to come are faint. */
  private drawSteps(): void {
    const list = this.body.querySelector<HTMLElement>(".remote-steps");
    if (!list) return;
    const { words } = this.options;
    list.hidden = !this.progress.length;
    // The pencil ticks of the removal list: a step under way is a faint
    // dot until it is done. Only a step that changed is drawn again, so a
    // tick drawn once is not drawn over each time the next step begins.
    this.progress.forEach((one, index) => {
      const shown = list.children[index] as HTMLElement | undefined;
      if (shown?.dataset.step === one.step && shown.dataset.state === one.state) return;
      const item = document.createElement("li");
      item.dataset.step = one.step;
      item.dataset.state = one.state;
      if (one.state === "done" || one.state === "failed") item.dataset.result = one.state;
      item.innerHTML = `<span class="removal-mark" aria-hidden="true">${mark(one.state === "done" || one.state === "failed" ? one.state : null)}</span><span class="removal-what">${escape(words.remoteSteps[one.step])}</span>`;
      if (shown) shown.replaceWith(item);
      else list.append(item);
    });
    while (list.children.length > this.progress.length) list.lastElementChild!.remove();
  }

  private mark(step: Progress["step"], state: Progress["state"]): void {
    const known = this.progress.find((one) => one.step === step);
    if (known) known.state = state;
    else this.progress.push({ step, state });
    // A step begun means the ones before it are done.
    if (state === "doing") for (const one of this.progress) if (one.step !== step && one.state === "doing") one.state = "done";
    this.drawSteps();
  }

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

  /** What the server's side said, in the person's words, under the form. */
  private said(error: unknown, sudoWrong = false): void {
    const message = error instanceof Error ? error.message : String(error);
    const said = sshSaid(message, this.options.words, sudoWrong);
    if (said) this.note(said, message);
    else this.problem(error);
  }

  private auth(): RemoteAuth | null {
    if (this.typed.by === "password") return this.secret.login ? { kind: "password", password: this.secret.login } : null;
    return this.key ? { kind: "key", key: this.key.text, passphrase: this.secret.passphrase || null } : null;
  }

  /** The server as the person gave it, once its key is known. */
  private server(): RemoteServer | null {
    const auth = this.auth();
    if (!auth || !this.shown) return null;
    return {
      host: this.typed.host.trim(),
      port: Number(this.typed.sshPort),
      user: this.typed.user.trim(),
      auth,
      fingerprint: this.shown.fingerprint,
      // Signed in with a password, sudo is usually asked the same.
      sudo: this.secret.sudo || (auth.kind === "password" ? auth.password : null),
    };
  }

  /** A machine's name checked, `label` the field it was typed in; null, with
   *  the reason said under the form, when it will not do. */
  private async checkedName(text: string, label: string): Promise<string | null> {
    const { words, seams } = this.options;
    const name = text.trim();
    if (!name) {
      this.note(words.inField(label, words.nameSaid({ problem: "empty" })));
      return null;
    }
    this.busy = true;
    const problem = await seams.nameProblem("machine", name).finally(() => (this.busy = false));
    if (problem) {
      this.note(words.inField(label, words.nameSaid(problem)));
      return null;
    }
    return name;
  }

  /** The name this computer is asked to take, where it is asked one and
   *  another than its own was typed. */
  private hereName(): string | null {
    const here = this.options.here?.();
    if (!here || here.fixed) return null;
    const typed = this.typed.here.trim();
    return typed && typed !== here.name ? typed : null;
  }

  private async next(): Promise<void> {
    if (this.busy) return;
    this.keep();
    const { words, seams } = this.options;
    switch (this.stage) {
      case "server": {
        if (!this.typed.host.trim() || !this.typed.user.trim() || !this.auth()) return this.note(this.typed.by === "key" && !this.key ? words.keyMissing : "");
        this.busy = true;
        this.draw();
        try {
          this.shown = await seams.probe(this.typed.host.trim(), Number(this.typed.sshPort));
        } catch (error) {
          this.busy = false;
          this.draw();
          return this.said(error);
        }
        this.busy = false;
        if (this.shown.known === "changed") {
          this.draw();
          return this.note(words.fingerprintChanged, this.shown.fingerprint);
        }
        if (this.shown.known === "known") return this.look();
        this.go("key");
        return;
      }
      case "key":
        return this.look();
      case "found": {
        const hub = this.looked!.hub!;
        if (!hub.name) return;
        this.resume = true;
        this.typed.world = hub.name;
        if (hub.port) this.typed.hubPort = String(hub.port);
        this.go("admin");
        return;
      }
      case "withdraw":
        return this.withdraw();
      case "network": {
        const world = this.typed.world.trim();
        const port = Number(this.typed.hubPort.trim());
        if (!Number.isInteger(port) || port < 1 || port > 65535) return this.note(words.portRange);
        if (this.looked?.busy.includes(port)) return this.note(words.portTaken);
        this.busy = true;
        const named = await seams.nameProblem("hub", world).finally(() => (this.busy = false));
        if (named) return this.note(words.inField(words.worldName, words.nameSaid(named)));
        this.typed.hubPort = String(port);
        this.go("admin");
        return;
      }
      case "admin":
        if (this.resume ? !this.typed.admin.trim() || !this.secret.password : !passwordReady(this.body, words, this.typed.admin.trim())) return;
        this.go("machines");
        return;
      case "machines": {
        if (this.options.adding) {
          const machine = await this.checkedName(this.typed.name, words.serverName);
          if (machine) await this.add(this.options.adding, machine);
          return;
        }
        const server = this.serverJoins() ? await this.checkedName(this.typed.name, words.serverName) : "";
        if (server === null) return;
        const here = this.options.here?.();
        if (here && !here.fixed) {
          const name = await this.checkedName(this.typed.here || here.name, words.hereName);
          if (name === null) return;
          if (server && name.toLowerCase() === server.toLowerCase()) return this.note(words.sameNames);
        }
        return this.make();
      }
      case "making":
        return this.options.adding ? undefined : this.make();
    }
  }

  /** Signs in to look at the server: what it already holds decides where
   *  this goes; a way in that does not work goes back to where it was
   *  given, saying why. */
  private async look(): Promise<void> {
    const server = this.server();
    if (!server) return;
    this.busy = true;
    this.draw();
    try {
      const looked = await this.options.seams.look(server);
      this.busy = false;
      this.looked = looked;
      if (!this.typed.name) this.typed.name = looked.machine;
      // A port taken that is the default is moved off, so the default asks
      // nothing of the person unless it must.
      if (!this.options.adding && looked.busy.includes(Number(this.typed.hubPort)) && this.typed.hubPort === HUB_PORT) {
        const free = [7443, 7444, 7445, 17443, 27443].find((port) => !looked.busy.includes(port));
        if (free) this.typed.hubPort = String(free);
      }
      this.go(this.options.adding ? "machines" : looked.hub ? "found" : "network");
    } catch (error) {
      this.busy = false;
      this.stage = "server";
      this.draw();
      this.said(error);
    }
  }

  /** Takes the World the server keeps down: then the network is asked for
   *  anew, with the port it held free again. */
  private async withdraw(): Promise<void> {
    const { words, seams } = this.options;
    const name = this.held();
    if (name && this.typed.confirm.trim() !== name) return this.note(words.withdrawType(name));
    const server = this.server();
    if (!server) return;
    this.busy = true;
    this.progress = [];
    this.plan(["connect", "check", "upload", "withdraw"]);
    this.draw();
    try {
      // This computer's own logins to it end first, while its hub answers,
      // so none is left to a World that is gone.
      const address = this.looked?.hub?.address ?? this.made?.address ?? null;
      const logins = [...(address ? (this.options.loginsAt?.(address) ?? []) : []), ...(this.signedIn ? [{ hub: this.signedIn.hub, account: this.signedIn.account }] : [])];
      for (const login of new Map(logins.map((one) => [`${one.hub}\n${one.account}`, one])).values()) await seams.logout(login.hub, login.account);
      await seams.withdraw(server, (one) => this.mark(one, "doing"));
      this.mark("withdraw", "done");
      for (const open of [this.made, this.opened]) if (open) void seams.close(open.session).catch((error: unknown) => console.warn("the session with the server could not be closed", error));
      this.made = null;
      this.opened = null;
      this.signedIn = null;
      this.joined = false;
      this.busy = false;
      const port = this.looked?.hub?.port ?? Number(this.typed.hubPort);
      if (this.looked) this.looked = { ...this.looked, hub: null, joined: false, busy: this.looked.busy.filter((one) => one !== port) };
      this.resume = false;
      this.failed = false;
      this.progress = [];
      this.typed.confirm = "";
      this.go("network");
    } catch (error) {
      this.busy = false;
      const failed = this.progress.find((one) => one.state === "doing");
      if (failed) failed.state = "failed";
      const message = error instanceof Error ? error.message : String(error);
      if (message.includes(SUDO_WANTED)) this.wantsSudo = true;
      this.draw();
      const byHand = message.indexOf(WITHDRAW_BY_HAND);
      if (byHand >= 0) this.note(`${words.withdrawByHand} ${message.slice(byHand + WITHDRAW_BY_HAND.length + 2)}`, message);
      else this.said(error, /incorrect password attempt|Sorry, try again/.test(message));
    }
  }

  /** Makes the World, or finishes the one the server keeps: the hub, this
   *  computer signed in to it, the server joined. What is done is not done
   *  again when it is tried again. */
  private async make(): Promise<void> {
    const { words, seams } = this.options;
    const server = this.server();
    if (!server) return;
    const world = this.typed.world.trim();
    const admin = this.typed.admin.trim();
    const { password } = this.secret;
    this.busy = true;
    this.failed = false;
    this.stage = "making";
    this.plan(this.resume ? (this.serverJoins() ? ["login", "connect", "check", "upload", "join"] : ["login"]) : ["connect", "check", "upload", "hub", "login", "join"]);
    this.draw();
    try {
      const step = (one: RemoteStep) => this.mark(one, "doing");
      let address: string;
      if (this.resume) {
        address = this.looked?.hub?.address ?? this.reachedAt(this.typed.hubPort);
      } else {
        this.made ??= await seams.hub({ ...server, world, admin, password, hubPort: Number(this.typed.hubPort) }, step);
        address = this.made.address;
      }
      this.mark("login", "doing");
      this.signedIn ??= await seams.login(address, admin, password, this.hereName());
      if (this.serverJoins() && !this.joined) {
        const session = this.made?.session ?? (this.opened ??= await seams.open(server, step)).session;
        this.mark("join", "doing");
        const token = await seams.token(this.signedIn.hub, admin, this.typed.name.trim());
        await seams.join(session, token, step);
        this.joined = true;
        this.mark("join", "done");
      } else {
        this.mark("login", "done");
      }
      this.busy = false;
      this.options.done?.(this.signedIn);
    } catch (error) {
      this.busy = false;
      this.failed = true;
      const failed = this.progress.find((one) => one.state === "doing");
      if (failed) failed.state = "failed";
      const message = error instanceof Error ? error.message : String(error);
      const sudoWrong = /incorrect password attempt|Sorry, try again/.test(message);
      if (message.includes(SUDO_WANTED) || sudoWrong) this.wantsSudo = true;
      this.draw();
      const { text, detail } = plainly(error, words.problems);
      // A hub just made that this computer does not reach is its port shut;
      // one there before answered, so what it said is the reason.
      const said = failed?.step === "login" && !this.resume ? words.portClosed(this.typed.hubPort) : (sshSaid(message, words, sudoWrong) ?? text);
      this.note(said, detail || message);
    }
  }

  /** A server added to a World there is: signed in to, checked, the
   *  program up; then a code by the name it will have, and it joins. */
  private async add(adding: Adding, machine: string): Promise<void> {
    const { words, seams } = this.options;
    const server = this.server();
    if (!server) return;
    this.busy = true;
    this.stage = "making";
    this.plan(["connect", "check", "upload", "join", ...(adding.then ? (["arrive"] as const) : [])]);
    this.draw();
    try {
      const step = (one: RemoteStep) => this.mark(one, "doing");
      this.opened ??= await seams.open(server, step);
      if (!this.joined) {
        this.mark("join", "doing");
        const token = await adding.token(machine);
        await seams.join(this.opened.session, token, step, !!adding.then, this.typed.proxy.trim() || null);
        this.joined = true;
      }
      this.mark("join", "done");
      if (adding.then && !this.followed) {
        this.mark("arrive", "doing");
        await adding.then(this.opened.session, machine, this.typed.host.trim(), step);
        await seams.close(this.opened.session);
        this.followed = true;
      }
      if (adding.then) this.mark("arrive", "done");
      this.busy = false;
      adding.added(machine);
    } catch (error) {
      this.busy = false;
      const failed = this.progress.find((one) => one.state === "doing");
      if (failed) failed.state = "failed";
      const message = error instanceof Error ? error.message : String(error);
      const sudoWrong = /incorrect password attempt|Sorry, try again/.test(message);
      if (message.includes(SUDO_WANTED) || sudoWrong) this.wantsSudo = true;
      this.stage = "machines";
      this.draw();
      const { text, detail } = plainly(error, words.problems);
      this.note(sshSaid(message, words, sudoWrong) ?? text, detail || message);
    }
  }
}

/** What the server's side said, in the person's words, where it is one of
 *  the things only this way in says (`src-tauri/src/remote_world/ssh.rs`). */
function sshSaid(message: string, words: LoginWords, sudoWrong: boolean): string | null {
  if (message.includes(SUDO_WANTED)) return words.sudoWanted;
  if (sudoWrong) return words.sudoWrong;
  if (/showed another key than the one confirmed/.test(message)) return words.fingerprintChanged;
  if (/did not accept .* with this password or key|reading the key file/.test(message)) return words.sshRefused;
  if (/^reaching \S+: /.test(message)) return words.sshUnreachable;
  if (/fastctx runs on x86_64 and aarch64 Linux/.test(message)) return words.notArch;
  if (/, not Linux$/.test(message)) return words.notLinux;
  return null;
}
