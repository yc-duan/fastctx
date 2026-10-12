// Making a World on a server, or adding a server to a World, from this
// computer over SSH (`world-gui.md`, 新建 and 添加设备): the server's address
// and how to sign in to it, the fingerprint it shows the first time, then,
// for a new World, its name, its administrator and port. The program this
// app carries for the server's architecture goes up to it; the server
// becomes the World's hub as a service and this computer signs in as its
// administrator, or, for a World there is, a code is made for it; and the
// server joins as one of its machines. Each step is ticked as it is done;
// one that fails says which and why, and the steps already done are not
// done again. Nothing typed here is kept.

import { escape } from "../conversation/prose";
import type { LoggedIn, NameProblem } from "../world/bridge";
import type { LoginWords } from "./account-words";
import { passwordProblem } from "./password";
import { plainly } from "./problem";
import { mark } from "./removal";

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

export type RemoteStep = "connect" | "check" | "upload" | "hub" | "join";

export interface RemoteSeams {
  probe(host: string, port: number): Promise<RemoteKey>;
  /** Connects, checks the system, uploads the program and makes the hub;
   *  the session stays open for the server to join. */
  hub(request: RemoteHub, step: (step: RemoteStep) => void): Promise<{ session: number; address: string; machine: string }>;
  /** Connects, checks the system and uploads the program, for a World there
   *  is; the session stays open for the server to join. */
  open(server: RemoteServer, step: (step: RemoteStep) => void): Promise<{ session: number; machine: string }>;
  join(session: number, token: string, step: (step: RemoteStep) => void): Promise<void>;
  close(session: number): Promise<void>;
  /** This computer signs in to the World just made, as its administrator. */
  login(address: string, admin: string, password: string, name: string | null): Promise<LoggedIn>;
  /** A one-time code for the server to join with, as a machine of `admin`. */
  token(hub: string, admin: string, machine: string): Promise<string>;
  /** What is wrong with a server name as typed. */
  nameProblem(kind: "hub", text: string): Promise<NameProblem | null>;
}

/** What the server's own side says when sudo wants a password it was not
 *  given (`src-tauri/src/remote_world.rs`). */
const SUDO_WANTED = "sudo needs a password";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

type Stage = "server" | "key" | "world" | "making";

interface Progress {
  step: RemoteStep | "login";
  state: "doing" | "done" | "failed";
}

export class RemoteWorld {
  private stage: Stage = "server";
  private typed = { host: "", sshPort: "22", user: "root", by: "password" as "password" | "key", world: "", admin: "", hubPort: "7443" };
  private key: { text: string; name: string } | null = null;
  private secret = { login: "", passphrase: "", sudo: "", password: "", again: "" };
  private shown: RemoteKey | null = null;
  private wantsSudo = false;
  private progress: Progress[] = [];
  /** Steps already done stay done when the person tries again. */
  private made: { session: number; address: string; machine: string } | null = null;
  private opened: { session: number; machine: string } | null = null;
  private signedIn: LoggedIn | null = null;
  private joined = false;
  private busy = false;

  constructor(
    private readonly body: HTMLElement,
    private readonly options: {
      words: LoginWords;
      seams: RemoteSeams;
      /** This computer's name in the World, where it is chosen now. */
      name(): string | null;
      /** The way back to the ways in. */
      back: string;
      done?(loggedIn: LoggedIn): void;
      /** Adding the server to a World there is, instead of making one: a
       *  code for it to join with, by the name it will have, and word once
       *  it has. */
      adding?: { token(machine: string): Promise<string>; added(machine: string): void };
    },
  ) {
    body.addEventListener("click", this.clicked);
    body.addEventListener("change", this.changed);
    body.addEventListener("submit", this.submitted);
    this.draw();
  }

  /** The enclosing form was submitted: the step this one is on goes on. */
  submit(): void {
    void this.next();
  }

  leave(): void {
    this.body.removeEventListener("click", this.clicked);
    this.body.removeEventListener("change", this.changed);
    this.body.removeEventListener("submit", this.submitted);
    const open = this.made ?? this.opened;
    if (open && !this.joined) void this.options.seams.close(open.session).catch((error: unknown) => console.warn("the session with the server could not be closed", error));
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
    if (target.closest("[data-remote=key-back]")) {
      this.stage = "server";
      this.draw();
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

  private keep(): void {
    const value = (name: string) => this.body.querySelector<HTMLInputElement>(`[name="${name}"]`)?.value;
    for (const name of ["host", "sshPort", "user", "world", "admin", "hubPort"] as const) this.typed[name] = value(name) ?? this.typed[name];
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
    const foot = (go: string, back = `<button class="text-button login-way" type="button" data-way="choose">${escape(this.options.back)}</button>`) =>
      `<p class="sheet-note" role="alert" hidden></p><footer class="welcome-foot"><span class="login-ways">${back}</span><button class="sheet-go" type="submit">${escape(go)}</button></footer>`;
    const field = (name: string, label: string, attributes: string, value: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span><input name="${name}" ${attributes} value="${escape(value)}"></label>`;
    switch (this.stage) {
      case "server": {
        const password = this.typed.by === "password";
        this.body.innerHTML = `<${form} class="welcome-form remote-form" autocomplete="off">
          ${this.options.adding ? "" : `<h2 class="welcome-form-title">${escape(words.remote)}</h2>`}
          <div class="field-pair remote-address">
            ${field("host", words.server, 'class="mono" required spellcheck="false" autocapitalize="off" placeholder="203.0.113.20"', this.typed.host)}
            ${field("sshPort", words.sshPort, 'class="mono" inputmode="numeric" pattern="[0-9]{1,5}" required', this.typed.sshPort)}
          </div>
          ${field("user", words.sshUser, 'class="mono" required spellcheck="false" autocapitalize="off"', this.typed.user)}
          <div class="choice-options remote-by" role="radiogroup" aria-label="${escape(words.signInBy)}">
            <button class="choice${password ? " chosen" : ""}" type="button" role="radio" aria-checked="${password}" data-by="password">${escape(words.password)}</button>
            <button class="choice${password ? "" : " chosen"}" type="button" role="radio" aria-checked="${!password}" data-by="key">${escape(words.keyFile)}</button>
          </div>
          ${
            password
              ? `<label class="field"><span class="field-label">${escape(words.password)}</span><input name="login" type="password" required autocomplete="off" value="${escape(this.secret.login)}"></label>`
              : `<label class="field remote-key"><span class="field-label">${escape(words.keyFile)}</span><span class="remote-key-pick"><input name="keyfile" type="file"><span class="remote-key-name mono">${escape(this.key?.name ?? "")}</span></span></label>
                 <label class="field"><span class="field-label">${escape(words.keyPassword)}</span><input name="passphrase" type="password" autocomplete="off" value="${escape(this.secret.passphrase)}"></label>`
          }
          ${foot(words.next)}
        </${form}>`;
        break;
      }
      case "key": {
        const shown = this.shown!;
        this.body.innerHTML = `<${form} class="welcome-form remote-form">
          <h2 class="welcome-form-title">${escape(words.firstTime)}</h2>
          <p class="remote-fingerprint mono" title="${escape(words.fingerprintWhat)}">${escape(shown.fingerprint)}</p>
          <p class="field-hint mono">${escape(shown.address)}</p>
          ${foot(words.trust, `<button class="text-button login-way" type="button" data-remote="key-back">${escape(words.back)}</button>`)}
        </${form}>`;
        break;
      }
      case "world":
      case "making": {
        const making = this.stage === "making";
        if (this.options.adding) {
          // A World there is: nothing to name, only sudo's password where
          // the server wants one.
          this.body.innerHTML = `<${form} class="welcome-form remote-form" autocomplete="off">
            <p class="remote-host mono">${escape(this.typed.user)}@${escape(this.typed.host)}</p>
            ${this.wantsSudo ? `<label class="field"><span class="field-label">${escape(words.sudoPassword)}</span><input name="sudo" type="password" required autocomplete="off" value="${escape(this.secret.sudo)}"></label>` : ""}
            <ol class="removal-list remote-steps"${this.progress.length ? "" : " hidden"}></ol>
            ${foot(making ? words.adding : words.addGo)}
          </${form}>`;
          this.drawSteps();
          if (making) this.body.querySelector<HTMLButtonElement>(".sheet-go")!.disabled = true;
          break;
        }
        this.body.innerHTML = `<${form} class="welcome-form remote-form" autocomplete="off">
          <h2 class="welcome-form-title">${escape(words.remote)} · <span class="mono">${escape(this.typed.host)}</span></h2>
          <div class="field-pair">
            ${field("world", words.worldName, `class="mono" required spellcheck="false" autocapitalize="off" placeholder="home"`, this.typed.world)}
            ${field("admin", words.admin, `class="mono" required spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="me"`, this.typed.admin)}
          </div>
          <div class="field-pair">
            <label class="field"><span class="field-label">${escape(words.password)}</span><input name="password" type="password" required autocomplete="new-password" value="${escape(this.secret.password)}"></label>
            <label class="field"><span class="field-label">${escape(words.passwordAgain)}</span><input name="again" type="password" required autocomplete="new-password" value="${escape(this.secret.again)}"></label>
          </div>
          ${field("hubPort", words.port, 'class="mono" inputmode="numeric" pattern="[0-9]{2,5}" required', this.typed.hubPort)}
          ${this.wantsSudo ? `<label class="field"><span class="field-label">${escape(words.sudoPassword)}</span><input name="sudo" type="password" required autocomplete="off" value="${escape(this.secret.sudo)}"></label>` : ""}
          <ol class="removal-list remote-steps"${this.progress.length ? "" : " hidden"}></ol>
          ${foot(making ? words.creating : words.createGo)}
        </${form}>`;
        this.drawSteps();
        if (making) this.body.querySelector<HTMLButtonElement>(".sheet-go")!.disabled = true;
        break;
      }
    }
  }

  /** Each step with a pencil tick when done, a cross when it failed. */
  private drawSteps(): void {
    const list = this.body.querySelector<HTMLElement>(".remote-steps");
    if (!list) return;
    const { words } = this.options;
    list.hidden = !this.progress.length;
    // The pencil ticks of the removal list: a step under way is a faint
    // dot until it is done. Only a step that changed is drawn again, so a
    // tick drawn once is not drawn over each time the next step begins.
    this.progress.forEach((one, index) => {
      const state = one.state === "doing" ? "" : one.state;
      const shown = list.children[index] as HTMLElement | undefined;
      if (shown?.dataset.step === one.step && (shown.dataset.result ?? "") === state) return;
      const item = document.createElement("li");
      item.dataset.step = one.step;
      if (state) item.dataset.result = state;
      item.innerHTML = `<span class="removal-mark" aria-hidden="true">${mark(state ? (state as "done" | "failed") : null)}</span><span class="removal-what">${escape(words.remoteSteps[one.step])}</span>`;
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

  private auth(): RemoteAuth | null {
    if (this.typed.by === "password") return this.secret.login ? { kind: "password", password: this.secret.login } : null;
    return this.key ? { kind: "key", key: this.key.text, passphrase: this.secret.passphrase || null } : null;
  }

  private async next(): Promise<void> {
    if (this.busy) return;
    this.keep();
    const { words, seams } = this.options;
    if (this.stage === "server") {
      const port = Number(this.typed.sshPort);
      if (!this.typed.host.trim() || !this.typed.user.trim() || !this.auth()) return this.note(this.typed.by === "key" && !this.key ? words.keyMissing : "");
      this.busy = true;
      try {
        this.shown = await seams.probe(this.typed.host.trim(), port);
        if (this.shown.known === "changed") return this.note(words.fingerprintChanged, this.shown.fingerprint);
        this.stage = this.shown.known === "known" ? "world" : "key";
        this.draw();
      } catch (error) {
        const message = error instanceof Error ? error.message : String(error);
        const said = sshSaid(message, words, false);
        if (said) this.note(said, message);
        else this.problem(error);
      } finally {
        this.busy = false;
      }
      return;
    }
    if (this.stage === "key") {
      this.stage = "world";
      this.draw();
      return;
    }
    if (this.stage !== "world") return;
    if (this.options.adding) return this.add(this.options.adding);
    const { password, again } = this.secret;
    const world = this.typed.world.trim();
    const admin = this.typed.admin.trim();
    this.busy = true;
    const named = await seams.nameProblem("hub", world).finally(() => (this.busy = false));
    if (named) return this.note(words.nameSaid(named));
    const problem = passwordProblem(admin, password);
    if (problem) return this.note(words.passwordRule[problem]);
    if (password !== again) return this.note(words.passwordsDiffer);
    const auth = this.auth()!;
    this.busy = true;
    this.stage = "making";
    this.draw();
    try {
      const step = (one: RemoteStep) => this.mark(one, "doing");
      this.made ??= await seams.hub(
        {
          host: this.typed.host.trim(),
          port: Number(this.typed.sshPort),
          user: this.typed.user.trim(),
          auth,
          fingerprint: this.shown!.fingerprint,
          // Signed in with a password, sudo is usually asked the same.
          sudo: this.secret.sudo || (auth.kind === "password" ? auth.password : null),
          world,
          admin,
          password,
          hubPort: Number(this.typed.hubPort),
        },
        step,
      );
      this.mark("login", "doing");
      this.signedIn ??= await seams.login(this.made.address, admin, password, this.options.name());
      const machine = this.made.machine;
      if (!this.joined) {
        this.mark("join", "doing");
        const token = await seams.token(this.signedIn.hub, admin, machine);
        await seams.join(this.made.session, token, step);
        this.joined = true;
      }
      this.mark("join", "done");
      this.busy = false;
      this.options.done?.(this.signedIn);
    } catch (error) {
      this.busy = false;
      const failed = this.progress.find((one) => one.state === "doing");
      if (failed) failed.state = "failed";
      const message = error instanceof Error ? error.message : String(error);
      const sudoWrong = /incorrect password attempt|Sorry, try again/.test(message);
      if (message.includes(SUDO_WANTED) || sudoWrong) this.wantsSudo = true;
      this.stage = "world";
      this.draw();
      const { text, detail } = plainly(error, words.problems);
      // The hub is made by then: this computer not reaching it is its port.
      const said = failed?.step === "login" ? words.portClosed(this.typed.hubPort) : (sshSaid(message, words, sudoWrong) ?? text);
      this.note(said, detail || message);
    }
  }
  /** A server added to a World there is: signed in to, checked, the
   *  program up; then a code by the name it will have, and it joins. */
  private async add(adding: { token(machine: string): Promise<string>; added(machine: string): void }): Promise<void> {
    const { words, seams } = this.options;
    const auth = this.auth()!;
    this.busy = true;
    this.stage = "making";
    this.draw();
    try {
      const step = (one: RemoteStep) => this.mark(one, "doing");
      this.opened ??= await seams.open(
        {
          host: this.typed.host.trim(),
          port: Number(this.typed.sshPort),
          user: this.typed.user.trim(),
          auth,
          fingerprint: this.shown!.fingerprint,
          sudo: this.secret.sudo || (auth.kind === "password" ? auth.password : null),
        },
        step,
      );
      const machine = this.opened.machine;
      if (!this.joined) {
        this.mark("join", "doing");
        const token = await adding.token(machine);
        await seams.join(this.opened.session, token, step);
        this.joined = true;
      }
      this.mark("join", "done");
      this.busy = false;
      adding.added(machine);
    } catch (error) {
      this.busy = false;
      const failed = this.progress.find((one) => one.state === "doing");
      if (failed) failed.state = "failed";
      const message = error instanceof Error ? error.message : String(error);
      const sudoWrong = /incorrect password attempt|Sorry, try again/.test(message);
      if (message.includes(SUDO_WANTED) || sudoWrong) this.wantsSudo = true;
      this.stage = "world";
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
