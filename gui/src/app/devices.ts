// Bringing a member into the World and sending one away
// (`world-network.md`, 邀请码): an invitation for a computer, a server or a
// phone, with what it may use once in, given as a few ready choices each said
// in a sentence, or set part by part; a one-use code shown as text, as a QR
// code for a phone to scan, or as the line a server runs. Revoking a member's
// key; this machine leaving. Revoking and leaving cannot be taken back, so
// each says what it does and waits for a second press before it does it.

import qrcode from "qrcode-generator";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Scope } from "../world/world";
import { Confirm, Sheet } from "./confirm";
import { describe, everything, ScopeEditor } from "./scope";
import type { DeviceWords, ScopeWords } from "./words";

export interface DevicesOptions {
  words: DeviceWords;
  scopeWords: ScopeWords;
  invite(name: string | null, hours: number, device: boolean, scope: Scope): Promise<string>;
  revoke(name: string, reason: string): Promise<void>;
  leave(): Promise<void>;
  /** The machines of the World, which a scope can name. */
  machines(): string[];
  /** The projects, by their short names. */
  projects(): string[];
  /** Whether this machine holds management rights: only then may it hand
   *  on the right to invite. */
  manages(): boolean;
  /** A moment as the interface says it. */
  when(at: Date): string;
  /** Something changed in the World: ask again. */
  changed(): void;
}

const validity = [1, 24, 72];

type Kind = "computer" | "server" | "phone";
type Preset = "all" | "self" | "one-agent" | "some" | "custom";

/** The ready choices for each kind of member, the first the default. */
const presets: Record<Kind, Preset[]> = {
  computer: ["all", "some", "custom"],
  server: ["self", "all", "custom"],
  phone: ["all", "one-agent", "some", "custom"],
};

/** A QR code as an SVG of dark squares on light, whatever the theme:
 *  scanners read dark on light, and the code is never drawn by a pen. */
export function qrSvg(text: string, label: string): string {
  const code = qrcode(0, "L");
  // Upper case letters and digits fit the alphanumeric mode, which packs
  // them densest; an invitation code is written that way on purpose.
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

export class Devices {
  private readonly invitation = new Sheet("device-sheet");
  private readonly confirm: Confirm;
  private hours = 1;
  private kind: Kind = "computer";
  private preset: Preset = "all";
  /** The machines chosen for "some machines", and the one agent chosen. */
  private some: string[] = [];
  private agent = { machine: "", agent: "codex" };
  private editor: ScopeEditor | null = null;

  constructor(private readonly options: DevicesOptions) {
    this.confirm = new Confirm(options.words);
    this.invitation.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const kind = target.closest<HTMLButtonElement>("[data-kind]");
      if (kind) {
        this.kind = kind.dataset.kind as Kind;
        this.preset = presets[this.kind][0];
        this.redraw();
        return;
      }
      const preset = target.closest<HTMLButtonElement>("[data-preset]");
      if (preset) {
        this.preset = preset.dataset.preset as Preset;
        this.redraw();
        return;
      }
      const some = target.closest<HTMLButtonElement>("[data-some]");
      if (some) {
        const machine = some.dataset.some!;
        this.some = this.some.includes(machine) ? this.some.filter((m) => m !== machine) : [...this.some, machine];
        this.redraw();
        return;
      }
      const pick = target.closest<HTMLButtonElement>("[data-pick-machine], [data-pick-agent]");
      if (pick) {
        if (pick.dataset.pickMachine) this.agent.machine = pick.dataset.pickMachine;
        if (pick.dataset.pickAgent) this.agent.agent = pick.dataset.pickAgent;
        this.redraw();
        return;
      }
      const hours = target.closest<HTMLButtonElement>("[data-hours]");
      if (hours) {
        this.hours = Number(hours.dataset.hours);
        for (const button of this.invitation.body.querySelectorAll<HTMLButtonElement>("[data-hours]")) {
          const chosen = Number(button.dataset.hours) === this.hours;
          button.classList.toggle("chosen", chosen);
          button.setAttribute("aria-checked", String(chosen));
        }
      }
      if (target.closest(".copy-code")) {
        const button = target.closest<HTMLElement>(".copy-code")!;
        const text = button.dataset.copy ?? "";
        void navigator.clipboard.writeText(text).then(() => {
          const label = button.querySelector("span");
          if (label) label.textContent = options.words.copied;
        });
      }
      if (target.closest(".make-again")) this.invite();
    });
    this.invitation.body.addEventListener("input", (event) => {
      if ((event.target as HTMLInputElement).name === "name") this.say();
    });
    this.invitation.body.addEventListener("submit", (event) => {
      event.preventDefault();
      void this.make();
    });
  }

  /** The sheet that makes an invitation. */
  invite(): void {
    this.some = [];
    this.agent = { machine: this.options.machines()[0] ?? "", agent: "codex" };
    this.editor = null;
    this.draw("");
    this.invitation.show();
    this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.focus();
  }

  /** Draws the sheet again, keeping the name typed so far. */
  private redraw(): void {
    const name = this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.value ?? "";
    if (this.preset !== "custom") this.editor = null;
    this.draw(name);
  }

  private draw(name: string): void {
    const { words } = this.options;
    const choices = <T extends string>(label: string, attribute: string, items: T[], chosen: T | ((item: T) => boolean), title: (item: T) => string, role = "radio") =>
      `<div class="choice-options" role="${role === "radio" ? "radiogroup" : "group"}" aria-label="${escape(label)}">${items
        .map((item) => {
          const on = typeof chosen === "function" ? chosen(item) : item === chosen;
          const state = role === "radio" ? `role="radio" aria-checked="${on}"` : `aria-pressed="${on}"`;
          return `<button type="button" class="choice${on ? " chosen" : ""}" ${state} ${attribute}="${escape(item)}">${escape(title(item))}</button>`;
        })
        .join("")}</div>`;
    const kindTitle = (kind: Kind) => (kind === "computer" ? words.aComputer : kind === "server" ? words.aServer : words.aPhone);
    const presetTitle = (preset: Preset) =>
      ({ all: words.presetAll, self: words.presetSelf, "one-agent": words.presetOneAgent, some: words.presetSome, custom: words.presetCustom })[preset];
    const hint = this.kind === "computer" ? words.computerHint : this.kind === "server" ? words.serverHint : words.phoneHint;
    this.invitation.body.innerHTML = `
      ${this.invitation.head(words.inviteTitle, words.close)}
      <div class="field"><span class="field-label">${escape(words.inviteWhat)}</span>
        ${choices(words.inviteWhat, "data-kind", ["computer", "server", "phone"] as Kind[], this.kind, kindTitle)}
        <span class="field-hint">${escape(hint)}</span></div>
      <label class="field"><span class="field-label">${escape(words.inviteName)}</span>
        <input name="name" autocomplete="off" spellcheck="false" class="mono" pattern="[a-z][a-z0-9-]{0,31}" value="${escape(name)}">
        <span class="field-hint">${escape(words.inviteNameHint)}</span></label>
      <div class="field"><span class="field-label">${escape(words.scope)}</span>
        ${choices(words.scope, "data-preset", presets[this.kind], this.preset, presetTitle)}
        ${this.presetFields()}
        <p class="field-hint invite-says" aria-live="polite"></p></div>
      <div class="field"><span class="field-label">${escape(words.valid)}</span>
        ${choices(words.valid, "data-hours", validity.map(String), String(this.hours), (hours) => words.hours(Number(hours)))}</div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.make)}</button></footer>`;
    const custom = this.invitation.body.querySelector<HTMLElement>(".invite-custom");
    if (custom && this.editor) this.editor.hook(custom);
    this.say();
  }

  /** The fields a preset asks for beyond itself. */
  private presetFields(): string {
    const { words } = this.options;
    const machines = this.options.machines();
    const chips = (attribute: string, items: string[], on: (item: string) => boolean, title: (item: string) => string = (item) => item) =>
      `<div class="choice-options invite-detail">${items
        .map((item) => `<button type="button" class="choice${on(item) ? " chosen" : ""}" aria-pressed="${on(item)}" ${attribute}="${escape(item)}">${escape(title(item))}</button>`)
        .join("")}</div>`;
    if (this.preset === "some") return chips("data-some", machines, (m) => this.some.includes(m));
    if (this.preset === "one-agent") {
      return `<span class="field-hint">${escape(words.oneAgentOn)}</span>
        ${chips("data-pick-machine", machines, (m) => m === this.agent.machine)}
        ${chips("data-pick-agent", ["codex", "claude"], (a) => a === this.agent.agent, (a) => (a === "codex" ? "Codex" : "Claude Code"))}`;
    }
    if (this.preset === "custom") {
      this.editor ??= new ScopeEditor(
        this.options.scopeWords,
        { machines, projects: this.options.projects(), invite: this.options.manages(), manage: null },
        everything(),
      );
      return `<div class="invite-custom">${this.editor.html()}</div>`;
    }
    return "";
  }

  private name(): string | null {
    return this.invitation.body.querySelector<HTMLInputElement>("[name=name]")?.value.trim() || null;
  }

  /** The scope the choices make; `null` with why when they make none yet. */
  private scope(): { scope: Scope } | { why: string } {
    const all = everything();
    switch (this.preset) {
      case "all":
        return { scope: all };
      case "self": {
        const name = this.name();
        return name ? { scope: { ...all, machines: [name] } } : { why: this.options.words.selfNeedsName };
      }
      case "some":
        return { scope: { ...all, machines: [...this.some] } };
      case "one-agent":
        return {
          scope: { ...all, machines: [this.agent.machine], agents: [this.agent.agent], actions: ["talk", "w", "read", "page"] },
        };
      case "custom":
        return { scope: this.editor?.value() ?? all };
    }
  }

  /** Says, under the choices, what the member may do once in. */
  private say(): void {
    const says = this.invitation.body.querySelector<HTMLElement>(".invite-says");
    if (!says) return;
    const made = this.scope();
    says.textContent = "scope" in made ? this.options.words.presetSays(describe(made.scope, this.options.scopeWords)) : made.why;
    // The custom fields say it themselves, as they change.
    says.hidden = this.preset === "custom";
  }

  private async make(): Promise<void> {
    const { words } = this.options;
    if (this.invitation.busy) return;
    const name = this.name();
    const go = this.invitation.body.querySelector<HTMLButtonElement>(".sheet-go");
    const note = this.invitation.body.querySelector<HTMLElement>(".sheet-note");
    if (!go || !note) return;
    const made = this.scope();
    if (!("scope" in made)) {
      note.textContent = made.why;
      note.hidden = false;
      return;
    }
    this.invitation.busy = true;
    go.disabled = true;
    go.textContent = words.making;
    try {
      const code = await this.options.invite(name, this.hours, this.kind === "phone", made.scope);
      const until = this.options.when(new Date(Date.now() + this.hours * 3_600_000));
      const said = words.presetSays(describe(made.scope, this.options.scopeWords));
      const copy = (text: string, label: string) =>
        `<button class="text-button copy-code" type="button" data-copy="${escape(text)}">${glyph("copy", 16)}<span>${escape(label)}</span></button>`;
      const command = `fastctx join ${code}`;
      const body =
        this.kind === "phone"
          ? `<div class="invitation">
              <div class="qr-frame">${qrSvg(code, words.qr)}</div>
              <div class="invitation-text">
                <p class="field-label">${escape(words.code)}</p>
                <p class="invite-code mono">${escape(code)}</p>
                <p class="field-hint">${escape(words.until(until))}</p>
                ${copy(code, words.copy)}
              </div>
            </div>`
          : `<div class="invitation invitation-wide">
              <div class="invitation-text">
                <p class="field-label">${escape(this.kind === "server" ? words.serverCommand : words.code)}</p>
                <p class="invite-code mono">${escape(this.kind === "server" ? command : code)}</p>
                <p class="field-hint">${escape(words.until(until))}</p>
                ${copy(this.kind === "server" ? command : code, words.copy)}
              </div>
            </div>`;
      const steps = this.kind === "phone" ? words.phoneSteps : this.kind === "server" ? words.serverSteps : words.howToSteps;
      this.invitation.body.innerHTML = `
        ${this.invitation.head(words.inviteTitle, words.close)}
        ${body}
        <p class="field-hint invite-says">${escape(said)}</p>
        <section class="how-to"><h3 class="field-label">${escape(words.howTo)}</h3><ol>${steps.map((step) => `<li>${escape(step)}</li>`).join("")}</ol></section>
        <footer class="sheet-foot"><span></span><button class="text-button make-again" type="button">${escape(words.again)}</button></footer>`;
    } catch (error) {
      note.textContent = words.failed(String(error));
      note.hidden = false;
      go.disabled = false;
      go.textContent = words.make;
    } finally {
      this.invitation.busy = false;
    }
  }

  /** Asks once more before revoking `name`; `what` says why, when it is
   *  not the plain revoking of a lost device. */
  revoke(name: string, what?: string, done?: () => void): void {
    const { words } = this.options;
    this.ask(
      words.revokeTitle(name),
      what ?? words.revokeWhat(name),
      `<label class="field"><span class="field-label">${escape(words.reason)}</span><input name="reason" autocomplete="off" value="${escape(words.reasonDefault)}"></label>`,
      words.revokeGo,
      () => this.options.revoke(name, this.confirm.body.querySelector<HTMLInputElement>("[name=reason]")?.value.trim() || words.reasonDefault),
      done,
    );
  }

  /** Asks once more before this machine leaves the World. */
  leave(): void {
    const { words } = this.options;
    this.ask(words.leaveTitle, words.leaveWhat, "", words.leaveGo, () => this.options.leave());
  }

  private ask(title: string, what: string, fields: string, go: string, act: () => Promise<void>, done?: () => void): void {
    this.confirm.ask({
      title,
      what,
      fields,
      go,
      act,
      done: () => {
        done?.();
        this.options.changed();
      },
    });
  }
}
