// What the owner of a machine sets on it (`world-network.md`, 共享机器与借出
// 智能体; 转接站): whom the machine is shared with, working on it and every
// agent there; whom one login of an agent on it is lent to, that login
// alone; each said plainly with what the owner gives and sees. And the
// relays it connects to the hub through, in order, the hub always last.
// Each change is the account's own, made at once; the hub refuses what it
// may not do, and the sheet stays open with why.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Relay } from "../world/accounts";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** IPv4 or bracketed IPv6, with a port: what a relay's address must be. */
const socketAddress = /^(?:\d{1,3}(?:\.\d{1,3}){3}|\[[0-9a-fA-F:.]+\]):\d{1,5}$/;

/** What a sheet that gives something to other accounts says. */
export interface GiveWords {
  title: string;
  what: string;
  sees: string;
  given: string;
  go: string;
  /** A line under it all, when there is one to say. */
  after?: string;
}

/** Giving a machine, or one login of an agent on it, to other accounts on
 *  the hub: those it is given to, each with a way to take it back, then
 *  whom to give it to. */
export class GiveSheet {
  private readonly sheet = new Sheet("device-sheet give-sheet");
  private said: GiveWords | null = null;
  private given: string[] = [];
  private give: (to: string, given: boolean) => Promise<string[]> = async () => [];

  constructor(private readonly words: AccessWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const back = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-take-back]");
      if (back && !back.disabled) void this.change(back.dataset.takeBack!, false);
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      const to = this.sheet.body.querySelector<HTMLInputElement>("[name=to]")!.value.trim();
      if (to) void this.change(to, true);
    });
  }

  /** Opens the sheet, saying `said`, given to `given` now; `give` gives it
   *  to an account or takes it back, and answers whom it is given to then. */
  open(said: GiveWords, given: string[], give: (to: string, given: boolean) => Promise<string[]>): void {
    this.said = said;
    this.given = given;
    this.give = give;
    this.sheet.body.innerHTML = "";
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private draw(): void {
    const { words, sheet, said } = this;
    if (!said) return;
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const rows = this.given
      .map(
        (account) => `<li class="give-row">
          <span class="give-who">${glyph("person", 14)}<span>${escape(account)}</span></span>
          <button class="text-button quiet-button" type="button" data-take-back="${escape(account)}">${escape(words.takeBack)}</button>
        </li>`,
      )
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(said.title, words.close)}
      <p class="confirm-what">${escape(said.what)}</p>
      <p class="field-hint give-sees">${glyph("shield", 14)}<span>${escape(said.sees)}</span></p>
      <section class="give-list">
        <h3 class="field-label">${escape(said.given)}</h3>
        ${rows ? `<ul class="give-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      <div class="give-new">
        <label class="field"><span class="field-label">${escape(words.to)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
      </div>
      ${said.after ? `<p class="field-hint give-after">${escape(said.after)}</p>` : ""}
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(said.go)}</button></footer>`;
  }

  private async change(to: string, given: boolean): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    note.hidden = true;
    try {
      this.given = await this.give(to, given);
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (given) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        field.focus();
      }
    } catch (error) {
      sheet.busy = false;
      for (const control of controls) control.disabled = false;
      const shown = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      shown.textContent = words.failed(error instanceof Error ? error.message : String(error));
      shown.hidden = false;
    }
  }
}

export class RelaysSheet {
  private readonly sheet = new Sheet("device-sheet relays-sheet");
  private relays: { label: string; addresses: string }[] = [];

  constructor(private readonly words: AccessWords) {
    this.sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest(".add-relay")) {
        this.read();
        this.relays.push({ label: "", addresses: "" });
        this.draw();
        this.sheet.body.querySelector<HTMLInputElement>(".relay-row:last-child .relay-label")?.focus();
        return;
      }
      const remove = target.closest<HTMLButtonElement>("[data-remove]");
      if (remove) {
        this.read();
        this.relays.splice(Number(remove.dataset.remove), 1);
        redrawKeepingFocus(this.sheet.body, () => this.draw());
      }
    });
  }

  /** Opens the sheet for `machine` as it connects now; `set` keeps the
   *  relays, in order. */
  open(machine: string, relays: Relay[], set: (relays: Relay[]) => Promise<void>, done: () => void): void {
    const { sheet, words } = this;
    this.relays = relays.map((relay) => ({ label: relay.label, addresses: relay.addresses.join(", ") }));
    this.machine = machine;
    this.draw();
    sheet.body.onsubmit = (event) => {
      event.preventDefault();
      if (sheet.busy) return;
      this.read();
      const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
      const say = (text: string) => {
        note.textContent = text;
        note.hidden = !text;
      };
      const rows = this.relays.filter((relay) => relay.label.trim() || relay.addresses.trim());
      const made: Relay[] = [];
      for (const row of rows) {
        const label = row.label.trim();
        if (!label) return say(words.needsLabel);
        const addresses = row.addresses.split(/[\s,，、]+/).filter(Boolean);
        const wrong = addresses.find((address) => !socketAddress.test(address));
        if (wrong || !addresses.length) return say(words.badAddress(wrong ?? row.addresses));
        made.push({ label, addresses });
      }
      const go = sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
      sheet.busy = true;
      go.disabled = true;
      go.textContent = words.working;
      say("");
      set(made)
        .then(() => {
          sheet.busy = false;
          sheet.close();
          done();
        })
        .catch((error: unknown) => {
          sheet.busy = false;
          go.disabled = false;
          go.textContent = words.relaysGo;
          say(words.failed(error instanceof Error ? error.message : String(error)));
        });
    };
    sheet.show();
  }

  private machine = "";

  /** Keeps what was typed before the rows are drawn again. */
  private read(): void {
    this.relays = [...this.sheet.body.querySelectorAll<HTMLElement>(".relay-row")].map((row) => ({
      label: row.querySelector<HTMLInputElement>(".relay-label")!.value,
      addresses: row.querySelector<HTMLInputElement>(".relay-addresses")!.value,
    }));
  }

  private draw(): void {
    const { words, sheet } = this;
    const rows = this.relays
      .map(
        (relay, index) => `<li class="relay-row">
          <span class="relay-order" aria-hidden="true">${index + 1}</span>
          <input class="relay-label" aria-label="${escape(words.relayLabel)}" placeholder="${escape(words.relayLabel)}" autocomplete="off" value="${escape(relay.label)}">
          <input class="relay-addresses mono" aria-label="${escape(words.relayAddresses)}" placeholder="203.0.113.5:7443" autocomplete="off" spellcheck="false" value="${escape(relay.addresses)}">
          <button class="icon-button" type="button" data-remove="${index}" aria-label="${escape(words.removeRelay)}">${glyph("close", 16)}</button>
        </li>`,
      )
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(words.relaysTitle(this.machine), words.close)}
      <p class="confirm-what">${escape(words.relaysWhat)}</p>
      <div class="field">
        ${rows ? `<ol class="relay-rows">${rows}</ol>` : `<p class="field-hint">${escape(words.noRelays)}</p>`}
        ${rows ? `<span class="field-hint">${escape(words.relayAddressesHint)}</span>` : ""}
        <button class="text-button add-relay" type="button">${glyph("plus", 16)}<span>${escape(words.addRelay)}</span></button>
        ${rows ? `<p class="relay-last">${glyph("relay", 14)}<span>${escape(words.hubLast)}</span></p>` : ""}
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.relaysGo)}</button></footer>`;
  }
}
