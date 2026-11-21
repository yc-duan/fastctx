// Moving a login's directory on a server to another disk from here, over
// SSH (the user, 2026-11-21: "前端也可以放个类似这种的东西，让用户可以从前端
// 进行SSH传一个命令的迁移"): the server's address and a way in, as adding a
// server asks them, the key it shows confirmed the first time; then its
// Fastctx moves the directory (`fastctx move-login` there, which stops the
// World there while it moves, so it is asked from outside it), what it
// writes shown as it comes; then what came of it, with deleting the
// original it set aside, or finishing a move cut short.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { pending, type MoveAsk, type Moved, type MoveRecord } from "../world/login-move";
import { Sheet } from "./confirm";
import { detailOf } from "./problem";
import type { RemoteKey, RemoteServer } from "./remote-world";

export interface LoginMoveWords {
  title: string;
  what: string;
  server: string;
  sshPort: string;
  user: string;
  signInBy: string;
  password: string;
  keyFile: string;
  chooseKey: string;
  keyPassword: string;
  optional: string;
  sudoPassword: string;
  /** sudo wanted a password it was not given. */
  sudoWanted: string;
  login: string;
  target: string;
  targetHint: string;
  stopAgents: string;
  stopAgentsWhat: string;
  next: string;
  looking: string;
  firstTime: string;
  changed: string;
  trust: string;
  back: string;
  running: string;
  close: string;
  moved(login: string, target: string, backup: string): string;
  notMoved(why: string): string;
  unfinished(login: string, phase: string): string;
  ended(code: number | null): string;
  resume: string;
  dropBackup: string;
  dropSure(backup: string): string;
  backupGone(backup: string): string;
  /** A field filled wrong. */
  wanting: { host: string; login: string; target: string; key: string };
}

export interface LoginMoveOptions {
  words: LoginMoveWords;
  probe(host: string, port: number): Promise<RemoteKey>;
  move(server: RemoteServer, machine: string, hub: string, ask: MoveAsk, told: (piece: string) => void): Promise<Moved>;
}

/** The machine a move is for: as the World writes it, by its name, and the
 *  network it is meant on — one of another network of that name is not it. */
export interface MoveTarget {
  written: string;
  name: string;
  hub: string;
}

type Stage = "form" | "key" | "running" | "done";

export class LoginMoveSheet {
  private readonly sheet = new Sheet("device-sheet login-move-sheet");
  private stage: Stage = "form";
  private typed = { host: "", port: "22", user: "root", by: "password" as "password" | "key", login: "", target: "", stopAgents: false };
  private secret = { password: "", passphrase: "", sudo: "" };
  private key: { name: string; text: string } | null = null;
  private wantsSudo = false;
  private probed: RemoteKey | null = null;
  private output = "";
  private moved: Moved | null = null;
  private problem: string | null = null;
  private logins: string[] = [];
  private target: MoveTarget = { written: "", name: "", hub: "" };
  /** The backup asked to be deleted, waiting for a second press. */
  private dropping: string | null = null;

  constructor(private readonly options: LoginMoveOptions) {
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.next();
    });
    this.sheet.body.addEventListener("click", (event) => this.click(event));
    this.sheet.body.addEventListener("change", (event) => this.picked(event));
  }

  /** Opens it for `target`, its logins' directories offered. Another machine
   *  than last time — of another network too — starts from nothing typed: no
   *  address, way in or key of one is used for another. */
  open(target: MoveTarget, logins: string[]): void {
    if (target.written !== this.target.written || target.hub !== this.target.hub) {
      this.probed = null;
      this.typed = { host: "", port: "22", user: "root", by: "password", login: "", target: "", stopAgents: false };
      this.secret = { password: "", passphrase: "", sudo: "" };
      this.key = null;
      this.wantsSudo = false;
    }
    this.target = target;
    this.logins = logins;
    this.stage = "form";
    this.output = "";
    this.moved = null;
    this.problem = null;
    this.dropping = null;
    if (!this.typed.login || !logins.includes(this.typed.login)) this.typed.login = logins[0] ?? "";
    this.draw();
    this.sheet.show();
  }

  /** What is typed, kept as it is drawn anew. */
  private keep(): void {
    const value = (name: string) => this.sheet.body.querySelector<HTMLInputElement>(`[name="${name}"]`);
    const typed = this.typed;
    typed.host = value("host")?.value ?? typed.host;
    typed.port = value("sshPort")?.value ?? typed.port;
    typed.user = value("user")?.value ?? typed.user;
    typed.login = value("login")?.value ?? typed.login;
    typed.target = value("target")?.value ?? typed.target;
    const stop = value("stopAgents");
    if (stop) typed.stopAgents = stop.checked;
    this.secret.password = value("password")?.value ?? this.secret.password;
    this.secret.passphrase = value("passphrase")?.value ?? this.secret.passphrase;
    this.secret.sudo = value("sudo")?.value ?? this.secret.sudo;
  }

  private server(): RemoteServer {
    const { typed, secret } = this;
    return {
      host: typed.host.trim(),
      port: Number(typed.port) || 22,
      user: typed.user.trim(),
      auth:
        typed.by === "password"
          ? { kind: "password", password: secret.password }
          : { kind: "key", key: this.key?.text ?? "", passphrase: secret.passphrase || null },
      fingerprint: this.probed?.fingerprint ?? "",
      sudo: secret.sudo || null,
    };
  }

  private moveAsk(): MoveAsk {
    return { do: "move", login: this.typed.login.trim(), target: this.typed.target.trim(), stop_agents: this.typed.stopAgents };
  }

  private async next(): Promise<void> {
    if (this.sheet.busy) return;
    this.keep();
    const { words } = this.options;
    if (this.stage === "form") {
      const { host, login, target, by } = this.typed;
      const wanting = !host.trim()
        ? words.wanting.host
        : !login.trim().startsWith("/")
          ? words.wanting.login
          : !target.trim().startsWith("/")
            ? words.wanting.target
            : by === "key" && !this.key
              ? words.wanting.key
              : null;
      if (wanting) {
        this.problem = wanting;
        this.draw();
        return;
      }
      this.sheet.busy = true;
      this.problem = null;
      this.draw();
      try {
        this.probed = await this.options.probe(host.trim(), Number(this.typed.port) || 22);
      } catch (error) {
        this.problem = detailOf(error);
        return;
      } finally {
        this.sheet.busy = false;
        this.draw();
      }
      if (this.probed.known === "known") return this.run(this.moveAsk());
      this.stage = "key";
      this.draw();
      return;
    }
    if (this.stage === "key") return this.run(this.moveAsk());
  }

  private async run(ask: MoveAsk): Promise<void> {
    const back = this.stage === "done" ? "done" : "form";
    this.stage = "running";
    this.output = "";
    this.problem = null;
    this.dropping = null;
    this.sheet.busy = true;
    this.draw();
    try {
      const { name, hub } = this.target;
      this.moved = await this.options.move(this.server(), name, hub, ask, (piece) => {
        this.output += piece;
        this.tail();
      });
      this.stage = "done";
    } catch (error) {
      const why = detailOf(error);
      if (why.includes("sudo needs a password")) {
        this.wantsSudo = true;
        this.stage = "form";
        this.problem = this.options.words.sudoWanted;
      } else {
        this.stage = back;
        this.problem = why;
      }
    } finally {
      this.sheet.busy = false;
      this.draw();
    }
  }

  /** What the command wrote, shown to its end as it comes. */
  private tail(): void {
    const pre = this.sheet.body.querySelector<HTMLElement>("pre.lm-output");
    if (!pre) return;
    pre.textContent = this.output;
    pre.scrollTop = pre.scrollHeight;
  }

  /** The move this asking was of, as the server records it now; none
   *  where it left none (refused before anything was recorded). */
  private record(): MoveRecord | null {
    const op = this.moved?.op;
    return op ? (this.moved?.records.find((one) => one.op === op) ?? null) : null;
  }

  private draw(): void {
    const { words } = this.options;
    const busy = this.sheet.busy ? " disabled" : "";
    const field = (name: string, label: string, attributes: string, value: string) =>
      `<label class="field"><span class="field-label">${escape(label)}</span><input name="${name}" ${attributes} value="${escape(value)}"></label>`;
    const note = this.problem ? `<p class="sheet-note" role="alert">${escape(this.problem)}</p>` : "";
    const head = this.sheet.head(`${words.title} · ${this.target.written}`, words.close);
    let body = "";
    switch (this.stage) {
      case "form": {
        const password = this.typed.by === "password";
        const logins = this.logins.map((dir) => `<option value="${escape(dir)}"></option>`).join("");
        body = `<p class="welcome-form-what">${escape(words.what)}</p>
          <div class="field-pair remote-address">
            ${field("host", words.server, 'class="mono" required spellcheck="false" autocapitalize="off"', this.typed.host)}
            ${field("sshPort", words.sshPort, 'class="mono" inputmode="numeric" pattern="[0-9]{1,5}" required', this.typed.port)}
          </div>
          <div class="field-pair">
            ${field("user", words.user, 'class="mono" required spellcheck="false" autocapitalize="off"', this.typed.user)}
            <div class="field remote-by-field"><span class="field-label">${escape(words.signInBy)}</span>
              <div class="segmented remote-by" role="radiogroup" aria-label="${escape(words.signInBy)}">
                <button class="segment${password ? " chosen" : ""}" type="button" role="radio" aria-checked="${password}" data-lm-by="password">${escape(words.password)}</button>
                <button class="segment${password ? "" : " chosen"}" type="button" role="radio" aria-checked="${!password}" data-lm-by="key">${escape(words.keyFile)}</button>
              </div>
            </div>
          </div>
          ${
            password
              ? `<label class="field"><span class="field-label">${escape(words.password)}</span><input name="password" type="password" autocomplete="off" value="${escape(this.secret.password)}"></label>`
              : `<div class="field-pair"><div class="field remote-key"><span class="field-label">${escape(words.keyFile)}</span><span class="remote-key-pick"><input name="keyfile" type="file" hidden><button class="pick-file" type="button" data-lm="choose-key">${glyph("file", 16)}<span>${escape(words.chooseKey)}</span></button><span class="remote-key-name mono">${escape(this.key?.name ?? "")}</span></span></div>
                 <label class="field"><span class="field-label">${escape(words.keyPassword)}<span class="field-optional">${escape(words.optional)}</span></span><input name="passphrase" type="password" autocomplete="off" value="${escape(this.secret.passphrase)}"></label></div>`
          }
          ${this.wantsSudo ? `<label class="field"><span class="field-label">${escape(words.sudoPassword)}</span><input name="sudo" type="password" autocomplete="off" value="${escape(this.secret.sudo)}"></label>` : ""}
          <label class="field"><span class="field-label">${escape(words.login)}</span><input name="login" class="mono" list="lm-logins" spellcheck="false" autocapitalize="off" value="${escape(this.typed.login)}"><datalist id="lm-logins">${logins}</datalist></label>
          <label class="field"><span class="field-label">${escape(words.target)}</span><input name="target" class="mono" spellcheck="false" autocapitalize="off" placeholder="/data/claude" value="${escape(this.typed.target)}"><span class="field-hint">${escape(words.targetHint)}</span></label>
          <label class="lm-stop"><input name="stopAgents" type="checkbox"${this.typed.stopAgents ? " checked" : ""}><span>${escape(words.stopAgents)}<span class="field-hint">${escape(words.stopAgentsWhat)}</span></span></label>
          ${note}<footer class="welcome-foot"><span class="login-ways"></span><button class="sheet-go" type="submit"${busy}>${escape(this.sheet.busy ? words.looking : words.next)}</button></footer>`;
        break;
      }
      case "key": {
        const probed = this.probed!;
        body = `<h2 class="welcome-form-title">${escape(probed.known === "changed" ? words.changed : words.firstTime)}</h2>
          <p class="remote-fingerprint mono">${escape(probed.fingerprint)}</p>
          <p class="field-hint mono">${escape(probed.address)}</p>
          ${note}<footer class="welcome-foot"><span class="login-ways"><button class="text-button login-way" type="button" data-lm="back">${escape(words.back)}</button></span><button class="sheet-go" type="submit"${busy}>${escape(words.trust)}</button></footer>`;
        break;
      }
      case "running":
        body = `<p class="welcome-form-what">${escape(words.running)}</p><pre class="lm-output mono"></pre>`;
        break;
      case "done": {
        const record = this.record();
        const status = this.moved?.status ?? null;
        let said = words.ended(status);
        const acts: string[] = [];
        if (record) {
          if (record.phase === "done" && record.service_restored) {
            said = record.backup_deleted ? words.backupGone(record.backup) : words.moved(record.login, record.target, record.backup);
            if (!record.backup_deleted) {
              acts.push(
                this.dropping === record.op
                  ? `<button class="text-button quiet-button danger" type="button" data-lm="drop-sure" data-op="${escape(record.op)}"${busy}>${escape(words.dropSure(record.backup))}</button>`
                  : `<button class="text-button quiet-button" type="button" data-lm="drop" data-op="${escape(record.op)}"${busy}>${escape(words.dropBackup)}</button>`,
              );
            }
          } else if (record.phase === "aborted" && record.service_restored) {
            said = words.notMoved(record.why ?? "");
          } else if (pending(record)) {
            said = words.unfinished(record.login, record.phase);
            acts.push(`<button class="text-button quiet-button" type="button" data-lm="resume"${busy}>${escape(words.resume)}</button>`);
          }
        }
        body = `<p class="welcome-form-what">${escape(said)}</p><pre class="lm-output mono">${escape(this.output)}</pre>${note}
          <footer class="welcome-foot"><span class="login-ways">${acts.join("")}</span><button class="sheet-go" type="button" data-lm="close"${busy}>${escape(words.close)}</button></footer>`;
        break;
      }
    }
    this.sheet.body.innerHTML = `${head}<div class="welcome-form remote-form login-move">${body}</div>`;
    this.sheet.anew();
    this.tail();
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-lm], [data-lm-by]");
    if (!target || this.sheet.busy) return;
    this.keep();
    const by = target.dataset.lmBy;
    if (by === "password" || by === "key") {
      this.typed.by = by;
      this.draw();
      return;
    }
    switch (target.dataset.lm) {
      case "choose-key":
        this.sheet.body.querySelector<HTMLInputElement>('input[name="keyfile"]')?.click();
        return;
      case "back":
        this.stage = "form";
        this.problem = null;
        this.draw();
        return;
      case "resume":
        void this.run({ do: "resume" });
        return;
      case "drop":
        this.dropping = target.dataset.op ?? null;
        this.draw();
        return;
      case "drop-sure":
        if (target.dataset.op) void this.run({ do: "drop_backup", op: target.dataset.op });
        return;
      case "close":
        this.sheet.close();
        return;
    }
  }

  /** A key file chosen: its text is used for the one connection, kept
   *  nowhere. */
  private picked(event: Event): void {
    const input = event.target as HTMLInputElement;
    if (input.name !== "keyfile") return;
    const file = input.files?.[0];
    if (!file) return;
    void file.text().then((text) => {
      this.keep();
      this.key = { name: file.name, text };
      this.draw();
    });
  }
}
