// Adding another device to an account (`world-network.md`, 登录与入网). A
// computer with a screen joins with the server's address, the account and
// its password, so the sheet shows the first two to copy. A phone scans a
// QR code holding a one-time login, valid an hour; a Linux host without a
// screen runs one line holding such a login, so no password ends up in
// its shell's history. Neither is shown again. From a computer a Linux host
// is added over SSH as well, as a World is made on one (`remote-world.ts`).
// A managed machine (受管机器) comes in by a code pasted on it: named, for
// as many computers and as long as the owner says, and shown with the three
// steps of using it and the line that does the same.
//
// Every way in is laid out alike: what to copy or fill in at the top, its
// three drawings in the middle of the room left, what it makes at the foot.

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { NameProblem } from "../world/bridge";
import type { LoginWords, TokenWords } from "./account-words";
import { Sheet } from "./confirm";
import { copyButton, copyFrom } from "./copy-button";
import { joinSteps, managedSteps } from "./managed-art";
import { plainly } from "./problem";
import { RemoteWorld, type RemoteSeams } from "./remote-world";

export type TokenKind = "computer" | "phone" | "server" | "managed";

export interface TokensOptions {
  words: TokenWords;
  /** The addresses the server of `login` publishes. */
  addresses(login: Login): string[];
  /** A one-time login to `login` for a phone. */
  token(login: Login, kind: "phone", register: boolean, name: string | null, hours: number): Promise<string>;
  /** A code that makes the computers using it managed machines of
   *  `login`'s account: `uses` of them at most, any number at zero. */
  managed(login: Login, label: string | null, uses: number, hours: number): Promise<string>;
  /** The line a Linux host runs, registering it as `login`'s machine, or
   *  as a managed machine of it. */
  server(login: Login, name: string | null, hours: number, managed: boolean): Promise<string>;
  /** Whether `login`'s hub registers managed machines. */
  manages(login: Login): boolean;
  /** A moment as the interface says it. */
  when(at: Date): string;
  /** What is wrong with a machine name as typed, where the shell can say. */
  nameProblem?: ((kind: "machine", text: string) => Promise<NameProblem | null>) | undefined;
  /** Adding a Linux host over SSH, where this device can: a code that
   *  registers it as `login`'s machine `machine`, made once it is reached. */
  remote?: { seams: RemoteSeams; words: LoginWords; token(login: Login, machine: string, managed: boolean): Promise<string>; added(): void } | undefined;
}

const validity = [1, 24, 72];
/** How many days a code for managed machines stays valid. */
const days = [1, 7, 30];
/** At most this many computers by one code, short of any number. */
const MOST = 9999;

/** A QR code as an SVG of dark squares on light, whatever the theme:
 *  scanners read dark on light, and the code is never drawn by a pen. */
export function qrSvg(text: string, label: string): string {
  const code = qrcode(0, "L");
  // Upper case letters and digits fit the alphanumeric mode, which packs
  // them densest; a login code is written that way on purpose.
  code.addData(text, /^[0-9A-Z $%*+\-./:]*$/.test(text) ? "Alphanumeric" : "Byte");
  code.make();
  const count = code.getModuleCount();
  const quiet = 4;
  const size = count + quiet * 2;
  let path = "";
  for (let row = 0; row < count; row += 1) {
    for (let col = 0; col < count; col += 1) {
      if (code.isDark(row, col)) path += `M${col + quiet} ${row + quiet}h1v1h-1z`;
    }
  }
  return `<svg class="qr" viewBox="0 0 ${size} ${size}" shape-rendering="crispEdges" role="img" aria-label="${escape(label)}"><rect width="${size}" height="${size}" fill="#fff"/><path d="${path}" fill="#111"/></svg>`;
}

export class Tokens {
  private readonly sheet = new Sheet("device-sheet token-sheet");
  private login: Login | null = null;
  private kind: TokenKind = "computer";
  /** How a Linux host comes in: over SSH from here, or by its line. */
  private way: "ssh" | "line" = "ssh";
  private ssh: RemoteWorld | null = null;
  private hours = 1;
  /** How many computers a code for managed machines lets in, and for how
   *  many days. */
  private uses: "one" | "any" | "some" = "one";
  private days = 7;
  /** Whether a Linux host comes in as a managed machine. */
  private managed = false;
  /** Whether a way in was given this time the sheet was open. */
  private given = false;
  private closed: ((given: boolean) => void) | null = null;

  constructor(private readonly options: TokensOptions) {
    const { body } = this.sheet;
    this.sheet.dialog.addEventListener("close", () => {
      this.ssh?.leave();
      this.ssh = null;
      // The computer's way in is the address and account shown: given once
      // the sheet closes on it.
      const given = this.given || this.kind === "computer";
      this.closed?.(given);
      this.closed = null;
    });
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const kind = target.closest<HTMLButtonElement>("[data-kind]");
      if (kind) {
        if (kind.disabled) return;
        this.kind = kind.dataset.kind as TokenKind;
        this.draw();
        return;
      }
      // Turned on or off in place: what was typed in the form stays.
      const managed = target.closest<HTMLButtonElement>("[data-managed]");
      if (managed) {
        if (this.login && options.manages(this.login)) {
          this.managed = !this.managed;
          managed.setAttribute("aria-checked", String(this.managed));
        }
        return;
      }
      const way = target.closest<HTMLButtonElement>("[data-server-way]");
      if (way) {
        this.way = way.dataset.serverWay as "ssh" | "line";
        this.draw();
        return;
      }
      // The SSH form's way back is to the line.
      if (target.closest(".device-remote [data-way=choose]")) {
        this.way = "line";
        this.draw();
        return;
      }
      const hours = target.closest<HTMLButtonElement>("[data-hours]");
      if (hours) {
        this.hours = Number(hours.dataset.hours);
        this.choose("[data-hours]", (button) => Number(button.dataset.hours) === this.hours);
        return;
      }
      const day = target.closest<HTMLButtonElement>("[data-days]");
      if (day) {
        this.days = Number(day.dataset.days);
        this.choose("[data-days]", (button) => Number(button.dataset.days) === this.days);
        return;
      }
      const uses = target.closest<HTMLElement>("[data-uses]");
      if (uses) {
        this.chooseUses(uses.dataset.uses as "one" | "any" | "some");
        if (this.uses === "some" && !target.closest("input")) body.querySelector<HTMLInputElement>("[name=count]")?.focus();
        return;
      }
      const copy = target.closest<HTMLElement>(".copy-button");
      if (copy) {
        void copyFrom(copy, options.words.copied);
        return;
      }
      if (target.closest(".make-again")) this.draw();
    });
    // Typing a number is choosing it.
    body.addEventListener("focusin", (event) => {
      if ((event.target as HTMLElement).matches("[name=count]")) this.chooseUses("some");
    });
    body.addEventListener("submit", (event) => {
      event.preventDefault();
      // Over SSH, the flow in the sheet takes the submission.
      if (this.ssh) return this.ssh.submit();
      void this.make();
    });
  }

  /** Opens the sheet for `login`, on `kind`; once it closes, whether a way
   *  in was given: a code or a command made, or a computer's shown. */
  open(login: Login, kind: TokenKind = "computer"): Promise<boolean> {
    this.closed?.(this.given);
    this.login = login;
    this.kind = kind === "managed" && !this.options.manages(login) ? "computer" : kind;
    this.way = "ssh";
    this.managed = false;
    this.uses = "one";
    this.days = 7;
    this.given = false;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(".choice.chosen")?.focus();
    return new Promise((resolve) => {
      this.closed = resolve;
    });
  }

  /** Marks the one of `selector` that `chosen` picks, in place. */
  private choose(selector: string, chosen: (button: HTMLElement) => boolean): void {
    for (const button of this.sheet.body.querySelectorAll<HTMLElement>(selector)) {
      const is = chosen(button);
      button.classList.toggle("chosen", is);
      button.setAttribute("aria-checked", String(is));
    }
  }

  private chooseUses(uses: "one" | "any" | "some"): void {
    this.uses = uses;
    this.choose("[data-uses]", (button) => button.dataset.uses === uses);
  }

  private copy(text: string): string {
    return copyButton(text, this.options.words.copy);
  }

  private draw(): void {
    const { words } = this.options;
    const login = this.login!;
    const choices = (label: string, attribute: string, items: string[], chosen: string, title: (item: string) => string, off: (item: string) => string = () => "") =>
      `<div class="choice-options" role="radiogroup" aria-label="${escape(label)}">${items
        .map((item) => {
          const why = off(item);
          return `<button type="button" class="choice${item === chosen ? " chosen" : ""}" role="radio" aria-checked="${item === chosen}" ${attribute}="${escape(item)}"${why ? ` disabled title="${escape(why)}"` : ""}>${escape(title(item))}</button>`;
        })
        .join("")}</div>`;
    const kinds: TokenKind[] = ["computer", "phone", "server", "managed"];
    const manages = this.options.manages(login);
    if (!manages) this.managed = false;
    this.ssh?.leave();
    this.ssh = null;
    const remote = this.options.remote;
    const ssh = this.kind === "server" && !!remote && this.way === "ssh";
    const ways = this.kind === "server" && remote ? choices(words.title, "data-server-way", ["ssh", "line"], this.way, (way) => words.ways[way as "ssh" | "line"]) : "";
    // A Linux host may come in as a managed machine, over SSH or by its line.
    const managed =
      this.kind === "server"
        ? `<button class="device-managed" type="button" role="switch" aria-checked="${this.managed}" data-managed title="${escape(manages ? words.managedWhat : words.managedOld)}"${manages ? "" : " disabled"}>${glyph("shield", 16)}<span class="device-managed-label">${escape(words.managed)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`
        : "";
    let body: string;
    if (ssh) body = `<div class="device-remote"></div>`;
    else if (this.kind === "computer") {
      // Joined from that computer's own World with the account's password:
      // each address the server is reached at, on a line of its own.
      const addresses = this.options.addresses(login);
      const reached = addresses.length ? addresses.map((address) => `<dd><span class="mono">${escape(address)}</span>${this.copy(address)}</dd>`).join("") : `<dd><span class="mono">—</span></dd>`;
      body = `<dl class="facts device-facts">
          <dt>${escape(words.hub)}</dt>${reached}
          <dt>${escape(words.account)}</dt><dd><span class="mono">${escape(login.account)}</span></dd>
        </dl>
        <div class="device-middle">${this.storyboard("computer")}</div>`;
    } else if (this.kind === "managed") {
      const uses = (["one", "any"] as const)
        .map(
          (uses) =>
            `<button type="button" class="choice${this.uses === uses ? " chosen" : ""}" role="radio" aria-checked="${this.uses === uses}" data-uses="${uses}">${escape(uses === "one" ? words.usesOne : words.usesAny)}</button>`,
        )
        .join("");
      body = `
        <label class="field"><span class="field-label">${escape(words.label)}</span>
          <input name="label" maxlength="40" autocomplete="off" spellcheck="false" placeholder="${escape(words.labelHint)}"></label>
        <div class="field"><span class="field-label">${escape(words.uses)}</span>
          <div class="choice-options" role="radiogroup" aria-label="${escape(words.uses)}">${uses}<label class="choice choice-count${this.uses === "some" ? " chosen" : ""}" role="radio" aria-checked="${this.uses === "some"}" data-uses="some"><input name="count" inputmode="numeric" pattern="[0-9]*" maxlength="4" autocomplete="off" placeholder="${escape(words.usesSome)}" aria-label="${escape(words.usesSome)}">${words.unit ? `<span>${escape(words.unit)}</span>` : ""}</label></div></div>
        <div class="field"><span class="field-label">${escape(words.valid)}</span>
          ${choices(words.valid, "data-days", days.map(String), String(this.days), (n) => words.days(Number(n)))}</div>
        <div class="device-middle">${this.storyboard("managed")}</div>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.makeManaged)}</button></footer>`;
    } else {
      body = `
        ${
          this.kind === "server"
            ? `<label class="field"><span class="field-label">${escape(words.name)}</span>
          <input name="name" autocomplete="off" spellcheck="false" autocapitalize="off" class="mono" placeholder="${escape(words.nameAuto)}"></label>
        <div class="field"><span class="field-label">${escape(words.valid)}</span>
          ${choices(words.valid, "data-hours", validity.map(String), String(this.hours), (hours) => words.hours(Number(hours)))}</div>`
            : ""
        }
        <div class="device-middle">${this.storyboard(this.kind === "phone" ? "phone" : "line")}</div>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.kind === "phone" ? words.makeCode : words.makeCommand)}</button></footer>`;
    }
    this.sheet.body.innerHTML = `
      ${this.sheet.head(words.title, words.close)}
      ${choices(words.title, "data-kind", kinds, this.kind, (kind) => words.kinds[kind as TokenKind], (kind) => (kind === "managed" && !manages ? words.managedOld : ""))}
      ${ways}
      ${managed}
      <div class="device-body">${body}</div>`;
    const place = this.sheet.body.querySelector<HTMLElement>(".device-remote");
    if (ssh && remote && place) {
      this.ssh = new RemoteWorld(place, {
        words: remote.words,
        seams: remote.seams,
        back: words.ways.line,
        adding: {
          token: (machine) => remote.token(login, machine, this.managed),
          added: (machine) => {
            this.given = true;
            this.ssh?.leave();
            this.ssh = null;
            place.innerHTML = `<p class="device-added">${escape(words.added(machine))}</p>`;
            remote.added();
          },
        },
      });
    }
  }

  /** How many computers the form asks a code to let in: any number at
   *  zero; null, with the note said, when the number typed is none. */
  private usesAsked(note: HTMLElement): number | null {
    if (this.uses === "one") return 1;
    if (this.uses === "any") return 0;
    const typed = this.sheet.body.querySelector<HTMLInputElement>("[name=count]")?.value.trim() ?? "";
    const count = /^[0-9]+$/.test(typed) ? Number(typed) : NaN;
    if (!Number.isInteger(count) || count < 1 || count > MOST) {
      note.textContent = this.options.words.usesSaid(MOST);
      note.title = "";
      note.hidden = false;
      this.sheet.body.querySelector<HTMLInputElement>("[name=count]")?.focus();
      return null;
    }
    return count;
  }

  private async make(): Promise<void> {
    const { words } = this.options;
    const { sheet } = this;
    const login = this.login;
    if (sheet.busy || !login || this.kind === "computer") return;
    const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    const kind = this.kind;
    const label = go.textContent ?? "";
    const name = sheet.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
    const codeName = sheet.body.querySelector<HTMLInputElement>("[name=label]")?.value.trim() || null;
    const uses = kind === "managed" ? this.usesAsked(note) : 1;
    if (uses === null) return;
    const hours = kind === "phone" ? 1 : kind === "managed" ? this.days * 24 : this.hours;
    sheet.busy = true;
    const check = this.options.nameProblem;
    const problem = name && check ? await check("machine", name).finally(() => (sheet.busy = false)) : null;
    if (problem) {
      note.textContent = words.nameSaid(problem);
      note.title = "";
      note.hidden = false;
      return;
    }
    sheet.busy = true;
    go.disabled = true;
    go.textContent = words.making;
    try {
      const made =
        kind === "server"
          ? await this.options.server(login, name, hours, this.managed)
          : kind === "managed"
            ? await this.options.managed(login, codeName, uses, hours)
            : await this.options.token(login, "phone", false, null, hours);
      this.given = true;
      const at = this.options.when(new Date(Date.now() + hours * 3_600_000));
      // The code to scan stands where the drawings stood; a line to copy
      // and a managed machine's code are read from the top.
      sheet.body.querySelector(".device-body")!.innerHTML = `${
        kind === "managed"
          ? this.managedCode(made, codeName, words.codeUntil(at, uses))
          : kind === "phone"
            ? `<div class="device-middle"><div class="invitation"><div class="qr-frame">${qrSvg(made, words.qr)}</div><p class="field-hint">${escape(words.until(at))}</p></div></div>`
            : `<div class="device-made"><p class="field-label">${escape(words.command)}</p><div class="code-box"><p class="invite-code mono">${escape(made)}</p>${this.copy(made)}</div><p class="field-hint">${escape(words.until(at))}</p></div>`
      }
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
    } catch (error) {
      const { text, detail } = plainly(error, words.problems);
      note.textContent = text;
      note.title = detail;
      note.hidden = false;
      go.disabled = false;
      go.textContent = label;
    } finally {
      sheet.busy = false;
    }
  }

  /** How the device comes in by what this sheet gives it, in three small
   *  drawings and a line each. */
  private storyboard(way: "computer" | "phone" | "line" | "managed"): string {
    const { words } = this.options;
    const art = way === "managed" ? managedSteps() : joinSteps(way);
    const steps = way === "managed" ? words.steps : words.joinSteps[way];
    return `<ol class="managed-steps">${steps.map((step, index) => `<li class="managed-step">${art[index]}<span><b>${index + 1}</b>${escape(step)}</span></li>`).join("")}</ol>`;
  }

  /** A code for managed machines, made: the code to copy, how long and for
   *  how many it lets computers in, the three steps of using it, and the
   *  line that does the same, for a tool that runs one on many computers. */
  private managedCode(code: string, name: string | null, until: string): string {
    const { words } = this.options;
    const line = `fastctx login --token ${code}`;
    return `<div class="managed-code">
        ${name ? `<p class="managed-code-name">${escape(name)}</p>` : ""}
        <div class="code-box"><p class="invite-code mono">${escape(code)}</p>${this.copy(code)}</div>
        <p class="field-hint">${escape(until)}</p>
        ${this.storyboard("managed")}
        <div class="managed-line"><span class="field-label">${escape(words.line)}</span><div class="code-box code-line"><code class="mono" title="${escape(line)}">${escape(line)}</code>${this.copy(line)}</div></div>
      </div>`;
  }
}
