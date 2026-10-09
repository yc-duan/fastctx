// What the owner of a machine sets on it (`world-network.md`, 一台机器给几个
// 账号用; 转接站): which other accounts on the hub may work on it or use its
// agents, each said plainly with what the owner sees of them; and the relays
// it connects to the hub through, in order, the hub always last. Each
// change is the account's own, made at once; the hub refuses what it may
// not do, and the sheet stays open with why.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { GrantSeen, Relay, Rights } from "../world/accounts";
import type { AccessWords } from "./account-words";
import { Sheet } from "./confirm";
import { redrawKeepingFocus } from "./focus";

const accountRule = "[a-z][a-z0-9\\-]{0,63}";

/** IPv4 or bracketed IPv6, with a port: what a relay's address must be. */
const socketAddress = /^(?:\d{1,3}(?:\.\d{1,3}){3}|\[[0-9a-fA-F:.]+\]):\d{1,5}$/;

export class GrantSheet {
  private readonly sheet = new Sheet("device-sheet grant-sheet");
  private machine = "";
  private grants: GrantSeen[] = [];
  private grant: (to: string, rights: Rights) => Promise<GrantSeen[]> = async () => [];

  constructor(private readonly words: AccessWords) {
    this.sheet.body.addEventListener("change", (event) => {
      const box = (event.target as HTMLElement).closest<HTMLInputElement>("[data-to]");
      if (!box) return;
      const row = box.closest<HTMLElement>(".grant-row")!;
      const rights = {
        hands: row.querySelector<HTMLInputElement>("[data-right=hands]")!.checked,
        agents: row.querySelector<HTMLInputElement>("[data-right=agents]")!.checked,
      };
      void this.change(box.dataset.to!, rights);
    });
    this.sheet.body.addEventListener("click", (event) => {
      const back = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-take-back]");
      if (back && !back.disabled) void this.change(back.dataset.takeBack!, { hands: false, agents: false });
    });
    this.sheet.body.addEventListener("submit", (event) => {
      event.preventDefault();
      const field = (name: string) => this.sheet.body.querySelector<HTMLInputElement>(`[name="${name}"]`)!;
      const to = field("to").value.trim();
      const rights = { hands: field("hands").checked, agents: field("agents").checked };
      if (!to || (!rights.hands && !rights.agents)) return;
      void this.change(to, rights, true);
    });
  }

  /** Opens the sheet for `machine`, lent as `grants` say; `grant` sets what
   *  one account may do and answers with the grants as they then stand. */
  open(machine: string, grants: GrantSeen[], grant: (to: string, rights: Rights) => Promise<GrantSeen[]>): void {
    this.machine = machine;
    this.grants = grants;
    this.grant = grant;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLInputElement>("[name=to]")?.focus();
  }

  private draw(): void {
    const { words, sheet } = this;
    const typed = sheet.body.querySelector<HTMLInputElement>("[name=to]")?.value ?? "";
    const check = (attributes: string, on: boolean, title: string, what?: string) =>
      `<label class="check-row${what ? "" : " compact"}"><input type="checkbox" ${attributes}${on ? " checked" : ""}><span><span class="check-title">${escape(title)}</span>${what ? `<span class="field-hint">${escape(what)}</span>` : ""}</span></label>`;
    const rows = this.grants
      .map(
        (grant) => `<li class="grant-row">
          <span class="grant-who">${escape(grant.account)}</span>
          ${check(`data-to="${escape(grant.account)}" data-right="hands"`, grant.rights.hands, words.hands)}
          ${check(`data-to="${escape(grant.account)}" data-right="agents"`, grant.rights.agents, words.agents)}
          <button class="text-button quiet-button" type="button" data-take-back="${escape(grant.account)}">${escape(words.takeBack)}</button>
        </li>`,
      )
      .join("");
    sheet.body.innerHTML = `
      ${sheet.head(words.grantTitle(this.machine), words.close)}
      <p class="confirm-what">${escape(words.grantWhat)}</p>
      <p class="field-hint grant-sees">${glyph("shield", 14)}<span>${escape(words.grantSees)}</span></p>
      <section class="grant-list">
        <h3 class="field-label">${escape(words.lentTo)}</h3>
        ${rows ? `<ul class="grant-rows">${rows}</ul>` : `<p class="field-hint">${escape(words.nobodyYet)}</p>`}
      </section>
      <div class="grant-new">
        <label class="field"><span class="field-label">${escape(words.to)}</span>
          <input name="to" autocomplete="off" spellcheck="false" autocapitalize="off" pattern="${accountRule}" placeholder="${escape(words.toHint)}" value="${escape(typed)}"></label>
        ${check(`name="hands"`, true, words.hands, words.handsWhat)}
        ${check(`name="agents"`, true, words.agents, words.agentsWhat)}
      </div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(words.grantGo)}</button></footer>`;
  }

  private async change(to: string, rights: Rights, adding = false): Promise<void> {
    const { sheet, words } = this;
    if (sheet.busy) return;
    const note = sheet.body.querySelector<HTMLElement>(".sheet-note")!;
    const controls = [...sheet.body.querySelectorAll<HTMLInputElement | HTMLButtonElement>("input, button:not(.sheet-close)")];
    sheet.busy = true;
    for (const control of controls) control.disabled = true;
    note.hidden = true;
    try {
      this.grants = await this.grant(to, rights);
      sheet.busy = false;
      redrawKeepingFocus(sheet.body, () => this.draw());
      if (adding) {
        const field = sheet.body.querySelector<HTMLInputElement>("[name=to]")!;
        field.value = "";
        field.focus();
      }
    } catch (error) {
      sheet.busy = false;
      for (const control of controls) control.disabled = false;
      // A switch that was not changed goes back to what it was.
      if (!adding) redrawKeepingFocus(sheet.body, () => this.draw());
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
