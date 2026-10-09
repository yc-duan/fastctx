// Letting another device into an account (`world-network.md`, 登录与入网): a
// one-time token, valid an hour, a day or three. A phone scans it as a QR
// code; a computer pastes it, registering as one of the account's machines
// when the account may register them; a Linux server runs one line that
// installs fastctx and logs in with it. The token is shown once and goes
// nowhere else.

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login } from "../world/accounts";
import type { TokenWords } from "./account-words";
import { Sheet } from "./confirm";
import { loginLabel } from "./reach";

export type TokenKind = "phone" | "computer" | "server";

export interface TokensOptions {
  words: TokenWords;
  /** A token for `kind` as `login`; a computer's registers it when asked. */
  token(login: Login, kind: "phone" | "computer", register: boolean, name: string | null, hours: number): Promise<string>;
  /** The line a server runs, registering it as `login`'s machine. */
  server(login: Login, name: string | null, hours: number): Promise<string>;
  /** Whether `login`'s account may register machines. */
  mayRegister(login: Login): boolean;
  /** A moment as the interface says it. */
  when(at: Date): string;
}

const validity = [1, 24, 72];
const machineRule = "[a-z][a-z0-9\\-]{0,31}";

/** A QR code as an SVG of dark squares on light, whatever the theme:
 *  scanners read dark on light, and the code is never drawn by a pen. */
export function qrSvg(text: string, label: string): string {
  const code = qrcode(0, "L");
  // Upper case letters and digits fit the alphanumeric mode, which packs
  // them densest; a token is written that way on purpose.
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
  private kind: TokenKind = "phone";
  private hours = 1;
  private register = true;

  constructor(private readonly options: TokensOptions) {
    const { body } = this.sheet;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const kind = target.closest<HTMLButtonElement>("[data-kind]");
      if (kind) {
        this.kind = kind.dataset.kind as TokenKind;
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
    body.addEventListener("change", (event) => {
      const box = event.target as HTMLInputElement;
      if (box.name === "register") this.register = box.checked;
    });
    body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.make();
    });
  }

  /** Opens the sheet for `login`, ready to make a token for `kind`. */
  open(login: Login, kind: TokenKind = "phone"): void {
    this.login = login;
    this.kind = kind;
    this.register = true;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(".choice.chosen")?.focus();
  }

  private draw(): void {
    const { words } = this.options;
    const login = this.login!;
    const registers = this.options.mayRegister(login);
    const choices = (label: string, attribute: string, items: string[], chosen: string, title: (item: string) => string) =>
      `<div class="choice-options" role="radiogroup" aria-label="${escape(label)}">${items
        .map(
          (item) =>
            `<button type="button" class="choice${item === chosen ? " chosen" : ""}" role="radio" aria-checked="${item === chosen}" ${attribute}="${escape(item)}">${escape(title(item))}</button>`,
        )
        .join("")}</div>`;
    const kinds: TokenKind[] = registers ? ["phone", "computer", "server"] : ["phone", "computer"];
    if (!kinds.includes(this.kind)) this.kind = "phone";
    // A name is the one a registered machine takes; a phone names itself.
    const named = this.kind === "server" || (this.kind === "computer" && registers && this.register);
    this.sheet.body.innerHTML = `
      ${this.sheet.head(words.title(loginLabel(login)), words.close)}
      <div class="field"><span class="field-label">${escape(words.what)}</span>
        ${choices(words.what, "data-kind", kinds, this.kind, (kind) => words.kinds[kind as TokenKind])}
        <span class="field-hint">${escape(words.hints[this.kind])}</span>
        ${registers ? "" : `<span class="field-hint">${escape(words.mayNotRegister)}</span>`}</div>
      ${
        this.kind === "computer" && registers
          ? `<label class="check-row"><input type="checkbox" name="register"${this.register ? " checked" : ""}><span><span class="check-title">${escape(words.register)}</span><span class="field-hint">${escape(words.registerWhat)}</span></span></label>`
          : ""
      }
      ${
        named
          ? `<label class="field"><span class="field-label">${escape(words.name)}</span>
        <input name="name" autocomplete="off" spellcheck="false" autocapitalize="off" class="mono" pattern="${machineRule}">
        <span class="field-hint">${escape(words.nameHint)}</span></label>`
          : ""
      }
      <div class="field"><span class="field-label">${escape(words.valid)}</span>
        ${choices(words.valid, "data-hours", validity.map(String), String(this.hours), (hours) => words.hours(Number(hours)))}</div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.make)}</button></footer>`;
  }

  private async make(): Promise<void> {
    const { words } = this.options;
    const { sheet } = this;
    const login = this.login;
    if (sheet.busy || !login) return;
    const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    const name = sheet.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
    const kind = this.kind;
    const hours = this.hours;
    const register = kind === "computer" && this.options.mayRegister(login) && this.register;
    sheet.busy = true;
    go.disabled = true;
    go.textContent = words.making;
    try {
      const made = kind === "server" ? await this.options.server(login, name, hours) : await this.options.token(login, kind, register, name, hours);
      const until = this.options.when(new Date(Date.now() + hours * 3_600_000));
      const copy = `<button class="text-button copy-code" type="button" data-copy="${escape(made)}">${glyph("copy", 16)}<span>${escape(words.copy)}</span></button>`;
      const text = `<div class="invitation-text">
          <p class="field-label">${escape(kind === "server" ? words.command : words.token)}</p>
          <p class="invite-code mono">${escape(made)}</p>
          <p class="field-hint">${escape(words.until(until))}</p>
          ${copy}
        </div>`;
      sheet.body.innerHTML = `
        ${sheet.head(words.title(loginLabel(login)), words.close)}
        ${kind === "phone" ? `<div class="invitation"><div class="qr-frame">${qrSvg(made, words.qr)}</div>${text}</div>` : `<div class="invitation invitation-wide">${text}</div>`}
        <section class="how-to"><h3 class="field-label">${escape(words.howTo)}</h3><ol>${words.steps[kind].map((step) => `<li>${escape(step)}</li>`).join("")}</ol></section>
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
    } catch (error) {
      note.textContent = words.failed(error instanceof Error ? error.message : String(error));
      note.hidden = false;
      go.disabled = false;
      go.textContent = words.make;
    } finally {
      sheet.busy = false;
    }
  }
}
