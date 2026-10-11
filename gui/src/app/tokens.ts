// Adding another device to an account (`world-network.md`, 登录与入网). A
// computer with a screen joins with the server's address, the account and
// its password, so the sheet shows the first two to copy. A phone scans a
// QR code holding a one-time login, valid an hour; a Linux host without a
// screen runs one line holding such a login, so no password ends up in
// its shell's history. Neither is shown again. From a computer a Linux host
// is added over SSH as well, as a World is made on one (`remote-world.ts`).

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { LoginWords, TokenWords } from "./account-words";
import { Sheet } from "./confirm";
import { plainly } from "./problem";
import { RemoteWorld, type RemoteSeams } from "./remote-world";

export type TokenKind = "computer" | "phone" | "server";

export interface TokensOptions {
  words: TokenWords;
  /** The addresses the server of `login` publishes. */
  addresses(login: Login): string[];
  /** A one-time login to `login` for a phone. */
  token(login: Login, kind: "phone", register: boolean, name: string | null, hours: number): Promise<string>;
  /** The line a Linux host runs, registering it as `login`'s machine. */
  server(login: Login, name: string | null, hours: number): Promise<string>;
  /** A moment as the interface says it. */
  when(at: Date): string;
  /** Adding a Linux host over SSH, where this device can: a code that
   *  registers it as `login`'s machine `machine`, made once it is reached. */
  remote?: { seams: RemoteSeams; words: LoginWords; token(login: Login, machine: string): Promise<string>; added(): void } | undefined;
}

const validity = [1, 24, 72];
const machineRule = "[a-z][a-z0-9\\-]{0,31}";

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
        this.kind = kind.dataset.kind as TokenKind;
        this.draw();
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
        for (const button of body.querySelectorAll<HTMLButtonElement>("[data-hours]")) {
          const chosen = Number(button.dataset.hours) === this.hours;
          button.classList.toggle("chosen", chosen);
          button.setAttribute("aria-checked", String(chosen));
        }
        return;
      }
      const copy = target.closest<HTMLElement>(".copy-code");
      if (copy) {
        void navigator.clipboard.writeText(copy.dataset.copy ?? "").then(() => {
          const label = copy.querySelector("span");
          if (label) label.textContent = options.words.copied;
        });
        return;
      }
      if (target.closest(".make-again")) this.draw();
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
    this.kind = kind;
    this.way = "ssh";
    this.given = false;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(".choice.chosen")?.focus();
    return new Promise((resolve) => {
      this.closed = resolve;
    });
  }

  private copy(text: string): string {
    return `<button class="text-button copy-code" type="button" data-copy="${escape(text)}">${glyph("copy", 16)}<span>${escape(this.options.words.copy)}</span></button>`;
  }

  private draw(): void {
    const { words } = this.options;
    const login = this.login!;
    const choices = (label: string, attribute: string, items: string[], chosen: string, title: (item: string) => string) =>
      `<div class="choice-options" role="radiogroup" aria-label="${escape(label)}">${items
        .map(
          (item) =>
            `<button type="button" class="choice${item === chosen ? " chosen" : ""}" role="radio" aria-checked="${item === chosen}" ${attribute}="${escape(item)}">${escape(title(item))}</button>`,
        )
        .join("")}</div>`;
    const kinds: TokenKind[] = ["computer", "phone", "server"];
    this.ssh?.leave();
    this.ssh = null;
    const remote = this.options.remote;
    const ssh = this.kind === "server" && !!remote && this.way === "ssh";
    const ways = this.kind === "server" && remote ? choices(words.title, "data-server-way", ["ssh", "line"], this.way, (way) => words.ways[way as "ssh" | "line"]) : "";
    let body: string;
    if (ssh) body = `<div class="device-remote"></div>`;
    else if (this.kind === "computer") {
      // Joined from that computer's own World with the account's password.
      const address = this.options.addresses(login).join("  ");
      body = `<dl class="facts device-facts">
          <dt>${escape(words.hub)}</dt><dd><span class="mono">${escape(address || "—")}</span>${address ? this.copy(this.options.addresses(login)[0]!) : ""}</dd>
          <dt>${escape(words.account)}</dt><dd><span class="mono">${escape(login.account)}</span></dd>
        </dl>`;
    } else {
      body = `
        ${
          this.kind === "server"
            ? `<label class="field"><span class="field-label">${escape(words.name)}</span>
          <input name="name" autocomplete="off" spellcheck="false" autocapitalize="off" class="mono" pattern="${machineRule}" placeholder="${escape(words.nameAuto)}"></label>
        <div class="field"><span class="field-label">${escape(words.valid)}</span>
          ${choices(words.valid, "data-hours", validity.map(String), String(this.hours), (hours) => words.hours(Number(hours)))}</div>`
            : ""
        }
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.kind === "phone" ? words.makeCode : words.makeCommand)}</button></footer>`;
    }
    this.sheet.body.innerHTML = `
      ${this.sheet.head(words.title, words.close)}
      ${choices(words.title, "data-kind", kinds, this.kind, (kind) => words.kinds[kind as TokenKind])}
      ${ways}
      <div class="device-body">${body}</div>`;
    const place = this.sheet.body.querySelector<HTMLElement>(".device-remote");
    if (ssh && remote && place) {
      this.ssh = new RemoteWorld(place, {
        words: remote.words,
        seams: remote.seams,
        name: () => null,
        back: words.ways.line,
        adding: {
          token: (machine) => remote.token(login, machine),
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

  private async make(): Promise<void> {
    const { words } = this.options;
    const { sheet } = this;
    const login = this.login;
    if (sheet.busy || !login || this.kind === "computer") return;
    const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    const name = sheet.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
    const kind = this.kind;
    const hours = kind === "phone" ? 1 : this.hours;
    sheet.busy = true;
    go.disabled = true;
    go.textContent = words.making;
    try {
      const made = kind === "server" ? await this.options.server(login, name, hours) : await this.options.token(login, "phone", false, null, hours);
      this.given = true;
      const until = words.until(this.options.when(new Date(Date.now() + hours * 3_600_000)));
      const shown =
        kind === "phone"
          ? `<div class="invitation"><div class="qr-frame">${qrSvg(made, words.qr)}</div><p class="field-hint">${escape(until)}</p></div>`
          : `<div class="invitation invitation-wide"><p class="field-label">${escape(words.command)}</p><p class="invite-code mono">${escape(made)}</p><p class="field-hint">${escape(until)}</p>${this.copy(made)}</div>`;
      sheet.body.querySelector(".device-body")!.innerHTML = `${shown}
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
    } catch (error) {
      const { text, detail } = plainly(error, words.problems);
      note.textContent = text;
      note.title = detail;
      note.hidden = false;
      go.disabled = false;
      go.textContent = kind === "phone" ? words.makeCode : words.makeCommand;
    } finally {
      sheet.busy = false;
    }
  }
}
