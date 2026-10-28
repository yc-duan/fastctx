// Adding another device to an account (`world-network.md`, 登录与入网). A
// computer with a screen joins with the server's address, the account and
// its password, so the sheet shows the first two to copy. The other ways in
// are codes (入网码): a phone scans one as a QR code; a Linux host without
// a screen runs one line holding one, so no password ends up in its
// shell's history; a managed machine (受管机器) has one pasted on it. Each
// lets in as many devices and for as long as its maker says, never as long
// as it is not revoked, and is shown once only: the hub keeps no more of it
// than what tells it again. From a computer a Linux host is added over SSH
// as well, as a World is made on one (`remote-world.ts`).
//
// Every way in is laid out alike: what to copy or fill in at the top, its
// three drawings in the middle of the room left, what it makes at the foot.
// The codes made that still let devices in are one more face of the sheet,
// beside its title, each revoked from its row (`code-rows.ts`).

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Login, TokenSeen } from "../world/accounts";
import type { NameProblem } from "../world/bridge";
import type { LoginWords, TokenWords } from "./account-words";
import { codeRow, type CodeWords } from "./code-rows";
import { Sheet } from "./confirm";
import { copyButton, copyFrom } from "./copy-button";
import { joinSteps, managedSteps } from "./managed-art";
import { plainly } from "./problem";
import { RemoteWorld, type RemoteSeams } from "./remote-world";

export type TokenKind = "computer" | "phone" | "server" | "managed";
/** What the sheet shows: a way in, or the codes that still let devices in. */
type Face = TokenKind | "codes";

/** A code as its maker asks for it. */
export interface CodeAsked {
  /** How many devices it lets in: one, or any number at zero. */
  uses: number;
  /** Until when it lets them in; as long as it is not revoked at null. */
  expires: Date | null;
  /** What it is called; the machines of a code for several are grouped
   *  under it. */
  label: string | null;
  /** For one device: the name it takes. */
  name: string | null;
  /** The computers that use it become managed machines. */
  managed: boolean;
  /** A Windows computer comes in by the line, not by the code pasted. */
  windows?: boolean;
  /** A Mac comes in by its line. */
  mac?: boolean;
}

export interface TokensOptions {
  words: TokenWords;
  codeWords: CodeWords;
  /** The addresses the server of `login` publishes. */
  addresses(login: Login): string[];
  /** A code to `login`'s account, as asked: for phones to scan, or for
   *  computers to paste, which become its machines, or its managed ones. */
  code(login: Login, kind: "phone" | "computer", asked: CodeAsked): Promise<string>;
  /** The line Linux hosts run, registering each as `login`'s machine, or as
   *  a managed machine of it, with a code as asked in it. */
  server(login: Login, asked: CodeAsked): Promise<string>;
  /** The codes that still let devices in, as `login`'s network tells this
   *  device of them. */
  codes(login: Login): TokenSeen[];
  /** Asks once, then revokes `token`; whether it was revoked. */
  revoke(login: Login, token: TokenSeen): Promise<boolean>;
  /** This device's name, beside the codes it made. */
  here(): string;
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

/** How long a code may stay valid, in hours: an hour, a day, a week, thirty
 *  days, or as long as it is not revoked. */
const SPANS = [1, 24, 24 * 7, 24 * 30, null] as const;
type Span = (typeof SPANS)[number];
/** How long each way's code stays valid unless its maker says. */
const SPAN_FIRST: Record<"phone" | "server" | "managed", Span> = { phone: 1, server: 24, managed: 24 * 7 };
/** At most this many devices by one code, short of any number. */
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

/** How a managed machine comes in: by a code, or by a Windows or Mac line. */
type ManagedWay = "code" | "windows" | "mac";
const MANAGED_WAYS: readonly ManagedWay[] = ["code", "windows", "mac"];

export class Tokens {
  private readonly sheet = new Sheet("device-sheet token-sheet");
  private login: Login | null = null;
  private face: Face = "computer";
  /** How a Linux host comes in: over SSH from here, or by its line. */
  private way: "ssh" | "line" = "ssh";
  private ssh: RemoteWorld | null = null;
  /** How many devices the code being asked for lets in, and for how long;
   *  each way starts from its own. */
  private uses: "one" | "any" | "some" = "one";
  private span: Span = 1;
  /** Whether a Linux host comes in as a managed machine. */
  private managed = false;
  /** How a managed machine comes in: the code pasted into its Fastctx, or
   *  the line run on a Windows computer that has none. */
  private managedWay: ManagedWay = "code";
  /** Codes revoked here, gone from the list before the network says so. */
  private readonly revoked = new Set<string>();
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
      const given = this.given || this.face === "computer";
      this.closed?.(given);
      this.closed = null;
    });
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const face = target.closest<HTMLButtonElement>("[data-face]");
      if (face) {
        if (face.disabled) return;
        this.turn(face.dataset.face as Face);
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
      const managedWay = target.closest<HTMLButtonElement>("[data-managed-way]");
      if (managedWay) {
        this.managedWay = managedWay.dataset.managedWay as ManagedWay;
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
      const span = target.closest<HTMLButtonElement>("[data-span]");
      if (span) {
        this.span = span.dataset.span === "never" ? null : (Number(span.dataset.span) as Span);
        this.choose("[data-span]", (button) => button.dataset.span === String(this.span ?? "never"));
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
      const revoke = target.closest<HTMLButtonElement>("[data-act=revoke]");
      if (revoke && this.login) {
        const login = this.login;
        const token = options.codes(login).find((one) => one.key === revoke.dataset.key);
        if (token)
          void options.revoke(login, token).then((done) => {
            if (!done) return;
            this.revoked.add(token.key);
            if (this.face === "codes" && this.sheet.dialog.open) this.draw();
          });
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
    this.way = "ssh";
    this.managed = false;
    this.given = false;
    this.turn(kind === "managed" && !this.options.manages(login) ? "computer" : kind);
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(".choice.chosen")?.focus();
    return new Promise((resolve) => {
      this.closed = resolve;
    });
  }

  /** Shows `face`, the code asked for there starting from that way's own
   *  number and time. */
  private turn(face: Face): void {
    this.face = face;
    this.uses = "one";
    if (face === "phone" || face === "server" || face === "managed") this.span = SPAN_FIRST[face];
    this.draw();
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
    this.nameLine();
  }

  /** A Linux line for one host names that host; one for several names
   *  them together, as their group: the same field, said for which. */
  private nameLine(): void {
    const field = this.sheet.body.querySelector<HTMLElement>("[data-line-name]");
    if (!field) return;
    const { words } = this.options;
    const several = this.uses !== "one";
    field.querySelector(".field-label")!.textContent = several ? words.label : words.name;
    const input = field.querySelector<HTMLInputElement>("input")!;
    input.placeholder = several ? words.lineHint : words.nameAuto;
    input.classList.toggle("mono", !several);
  }

  private copy(text: string): string {
    return copyButton(text, this.options.words.copy);
  }

  /** The codes shown: those the network tells of, but those revoked here. */
  private codes(login: Login): TokenSeen[] {
    return this.options.codes(login).filter((token) => !this.revoked.has(token.key));
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
    const ssh = this.face === "server" && !!remote && this.way === "ssh";
    // How a Linux host or a managed machine comes in is a choice within its
    // face: a row of words under the ways of adding a device, the chosen
    // one circled as theirs is.
    const ways =
      this.face === "server" && remote
        ? `<div class="choice-options device-ways" role="radiogroup" aria-label="${escape(words.kinds.server)}">${(["ssh", "line"] as const)
            .map((way) => `<button class="choice${way === this.way ? " chosen" : ""}" type="button" role="radio" aria-checked="${way === this.way}" data-server-way="${way}">${escape(words.ways[way])}</button>`)
            .join("")}</div>`
        : this.face === "managed"
          ? `<div class="choice-options device-ways" role="radiogroup" aria-label="${escape(words.kinds.managed)}">${MANAGED_WAYS.map(
              (way) => `<button class="choice${way === this.managedWay ? " chosen" : ""}" type="button" role="radio" aria-checked="${way === this.managedWay}" data-managed-way="${way}">${escape(words.managedWays[way])}</button>`,
            ).join("")}</div>`
          : "";
    // A Linux host may come in as a managed machine, over SSH or by its line.
    const managed =
      this.face === "server"
        ? `<button class="device-managed" type="button" role="switch" aria-checked="${this.managed}" data-managed title="${escape(manages ? words.managedWhat : words.managedOld)}"${manages ? "" : " disabled"}>${glyph("shield", 16)}<span class="device-managed-label">${escape(words.managed)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`
        : "";
    const codes = this.codes(login);
    let body: string;
    if (this.face === "codes") body = this.codesFace(login, codes);
    else if (ssh) body = `<div class="device-remote"></div>`;
    else if (this.face === "computer") {
      // Joined from that computer's own World with the account's password:
      // each address the server is reached at, on a line of its own.
      const addresses = this.options.addresses(login);
      const reached = addresses.length ? addresses.map((address) => `<dd><span class="mono">${escape(address)}</span>${this.copy(address)}</dd>`).join("") : `<dd><span class="mono">—</span></dd>`;
      body = `<dl class="facts device-facts">
          <dt>${escape(words.hub)}</dt>${reached}
          <dt>${escape(words.account)}</dt><dd><span class="mono">${escape(login.account)}</span></dd>
        </dl>
        <div class="device-middle">${this.storyboard("computer")}</div>`;
    } else if (this.face === "managed") {
      body = `
        <label class="field"><span class="field-label">${escape(words.label)}</span>
          <input name="label" maxlength="40" autocomplete="off" spellcheck="false" placeholder="${escape(words.labelHint)}"></label>
        ${this.usesField()}
        ${this.spanField()}
        <div class="device-middle">${this.storyboard(this.managedWay === "code" ? "managed" : this.managedWay)}</div>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(this.managedWay === "code" ? words.makeManaged : words.makeCommand)}</button></footer>`;
    } else {
      const server = this.face === "server";
      body = `
        ${this.usesField()}
        ${
          server
            ? `<label class="field" data-line-name><span class="field-label">${escape(words.name)}</span>
          <input name="name" maxlength="40" autocomplete="off" spellcheck="false" autocapitalize="off" class="mono" placeholder="${escape(words.nameAuto)}"></label>`
            : ""
        }
        ${this.spanField()}
        <div class="device-middle">${this.storyboard(server ? "line" : "phone")}</div>
        <p class="sheet-note" role="alert" hidden></p>
        <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(server ? words.makeCommand : words.makeCode)}</button></footer>`;
    }
    // The codes made are beside the title while any lets devices in, and
    // while they are shown.
    const shown = this.face === "codes";
    const pill =
      codes.length || shown
        ? `<button class="choice codes-pill${shown ? " chosen" : ""}" type="button" data-face="codes" aria-pressed="${shown}">${glyph("key", 14)}<span>${escape(words.codes)}</span>${codes.length ? `<span class="codes-count">${codes.length}</span>` : ""}</button>`
        : "";
    this.sheet.body.innerHTML = `
      <header class="sheet-head"><h2 class="sheet-title">${escape(words.title)}</h2><span class="sheet-head-end">${pill}<button class="icon-button sheet-close" type="button" aria-label="${escape(words.close)}">${glyph("close", 18)}</button></span></header>
      ${choices(words.title, "data-face", kinds, this.face, (kind) => words.kinds[kind as TokenKind], (kind) => (kind === "managed" && !manages ? words.managedOld : ""))}
      <div class="device-body${shown ? " codes-face" : ""}">${ways}${managed}${body}</div>`;
    this.nameLine();
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

  /** How many devices the code lets in: one, any number, or a number typed
   *  into the third choice itself. */
  private usesField(): string {
    const { words } = this.options;
    const uses = (["one", "any"] as const)
      .map(
        (uses) =>
          `<button type="button" class="choice${this.uses === uses ? " chosen" : ""}" role="radio" aria-checked="${this.uses === uses}" data-uses="${uses}">${escape(uses === "one" ? words.usesOne : words.usesAny)}</button>`,
      )
      .join("");
    const some = this.uses === "some";
    return `<div class="field"><span class="field-label">${escape(words.uses)}</span>
          <div class="choice-options" role="radiogroup" aria-label="${escape(words.uses)}">${uses}<label class="choice choice-count${some ? " chosen" : ""}" role="radio" aria-checked="${some}" data-uses="some"><span class="count-word">${escape(words.usesSome)}</span><input name="count" inputmode="numeric" pattern="[0-9]*" maxlength="4" autocomplete="off" aria-label="${escape(words.usesSome)}">${words.unit ? `<span class="count-unit">${escape(words.unit)}</span>` : ""}</label></div></div>`;
  }

  /** How long the code lets devices in. */
  private spanField(): string {
    const { words } = this.options;
    const spans = SPANS.map((span, index) => {
      const chosen = span === this.span;
      return `<button type="button" class="choice${chosen ? " chosen" : ""}" role="radio" aria-checked="${chosen}" data-span="${span ?? "never"}">${escape(words.spans[index]!)}</button>`;
    }).join("");
    return `<div class="field"><span class="field-label">${escape(words.valid)}</span>
          <div class="choice-options" role="radiogroup" aria-label="${escape(words.valid)}">${spans}</div></div>`;
  }

  /** The codes that still let devices in, each revoked from its row. */
  private codesFace(login: Login, codes: TokenSeen[]): string {
    const { codeWords, words, when, here } = this.options;
    if (!codes.length) return `<p class="codes-none">${escape(words.codesNone)}</p>`;
    return `<ul class="me-rows codes-rows">${codes.map((token) => codeRow(token, login, here(), codeWords, when)).join("")}</ul>`;
  }

  /** How many devices the form asks a code to let in: any number at
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

  /** Until when, and for how many, a code made now lets devices in. */
  private validSaid(expires: Date | null, uses: number): string {
    const { words, when } = this.options;
    return expires ? words.codeUntil(when(expires), uses) : words.codeLasting(uses);
  }

  private async make(): Promise<void> {
    const { words } = this.options;
    const { sheet } = this;
    const login = this.login;
    if (sheet.busy || !login || this.face === "computer" || this.face === "codes") return;
    const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    const face = this.face;
    const label = go.textContent ?? "";
    const uses = this.usesAsked(note);
    if (uses === null) return;
    // A Linux line for one host names it; for several, their group.
    const typed = sheet.body.querySelector<HTMLInputElement>(face === "managed" ? "[name=label]" : "[name=name]")?.value.trim() || null;
    const name = face === "server" && uses === 1 ? typed : null;
    const codeName = face === "managed" || (face === "server" && uses !== 1) ? typed : null;
    const expires = this.span === null ? null : new Date(Date.now() + this.span * 3_600_000);
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
      // A managed Windows computer or Mac comes in by its own line.
      const line = face === "managed" && this.managedWay !== "code" ? this.managedWay : null;
      const asked: CodeAsked = {
        uses,
        expires,
        label: codeName,
        name,
        managed: face === "managed" || (face === "server" && this.managed),
        ...(line === "windows" ? { windows: true } : line === "mac" ? { mac: true } : {}),
      };
      const made = face === "server" || line ? await this.options.server(login, asked) : await this.options.code(login, face === "phone" ? "phone" : "computer", asked);
      this.given = true;
      const valid = this.validSaid(expires, uses);
      // The code to scan stands where the drawings stood; a line to copy
      // and a managed machine's code are read from the top.
      sheet.body.querySelector(".device-body")!.innerHTML = `${
        face === "managed" && !line
          ? this.managedCode(made, codeName, valid)
          : face === "phone"
            ? `<div class="device-middle"><div class="invitation"><div class="qr-frame">${qrSvg(made, words.qr)}</div><p class="field-hint">${escape(valid)}</p></div></div>`
            : `<div class="device-made"><p class="field-label">${escape(line === "windows" ? words.windowsCommand : line === "mac" ? words.macCommand : words.command)}</p><div class="code-box"><p class="invite-code mono">${escape(made)}</p>${this.copy(made)}</div><p class="field-hint">${escape(valid)}</p></div>`
      }
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
      // The codes made, with this one, are beside the title.
      this.redrawPill();
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

  /** The codes' way in beside the title, as many as the network tells of
   *  now; drawn again once one is made, the rest of the sheet kept. */
  private redrawPill(): void {
    const login = this.login;
    const end = this.sheet.body.querySelector(".sheet-head-end");
    if (!login || !end) return;
    const count = this.codes(login).length;
    const { words } = this.options;
    const pill = `<button class="choice codes-pill" type="button" data-face="codes" aria-pressed="false">${glyph("key", 14)}<span>${escape(words.codes)}</span>${count ? `<span class="codes-count">${count}</span>` : ""}</button>`;
    end.querySelector(".codes-pill")?.remove();
    end.insertAdjacentHTML("afterbegin", pill);
  }

  /** How the device comes in by what this sheet gives it, in three small
   *  drawings and a line each. */
  private storyboard(way: "computer" | "phone" | "line" | "windows" | "mac" | "managed"): string {
    const { words } = this.options;
    const art = way === "managed" ? managedSteps() : joinSteps(way === "windows" || way === "mac" ? "line" : way);
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
