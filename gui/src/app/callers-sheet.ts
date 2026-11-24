// The callers of model collaboration, chosen (`world-colleagues.md`): the
// agents whose conversations are given the tools that ask a colleague. One
// row a login, under its machine; an account with a hundred machines finds
// one by what it types, and takes or leaves all that are listed at once.
// The list scrolls in its own place: the head, the search and the foot
// stay.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { sameAgentLogin, type AgentLogin } from "../world/collab";
import { agentName } from "../world/names";
import { Sheet } from "./confirm";
import type { FeaturesWords } from "./features-words";
import { detailOf } from "./problem";

/** A login an agent can work in, as its machine tells of it. */
export interface CallerLogin {
  login: AgentLogin;
  /** Whose login it is: the account's address, or its directory's name. */
  who: string;
}

export interface CallersOpening {
  /** Every login the account's machines tell of now. */
  known: CallerLogin[];
  /** The callers as kept. */
  chosen: AgentLogin[];
  save(callers: AgentLogin[]): Promise<void>;
}

export class CallersSheet {
  private readonly sheet = new Sheet("cs-sheet");
  private opening: CallersOpening | null = null;
  /** Every login listed: those told of, then those kept that no machine
   *  tells of now. */
  private listed: (CallerLogin & { away: boolean })[] = [];
  private chosen = new Set<number>();
  private sought = "";

  constructor(private readonly words: FeaturesWords) {
    const body = this.sheet.body;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const row = target.closest<HTMLElement>("[data-cs-login]");
      if (row) {
        const at = Number(row.dataset.csLogin);
        if (this.chosen.has(at)) this.chosen.delete(at);
        else this.chosen.add(at);
        const on = this.chosen.has(at);
        row.classList.toggle("chosen", on);
        row.setAttribute("aria-checked", String(on));
        this.drawFoot();
        return;
      }
      const all = target.closest<HTMLElement>("[data-cs-all]");
      if (all) {
        const on = all.dataset.csAll === "on";
        for (const at of this.shown()) {
          if (on) this.chosen.add(at);
          else this.chosen.delete(at);
        }
        this.drawList();
        this.drawFoot();
      }
    });
    body.addEventListener("input", (event) => {
      const field = event.target as HTMLInputElement;
      if (!field.matches("[name=seek]")) return;
      this.sought = field.value.trim().toLowerCase();
      this.drawList();
    });
    body.onsubmit = (event) => {
      event.preventDefault();
      void this.submit();
    };
  }

  open(opening: CallersOpening): void {
    this.opening = opening;
    const away = opening.chosen
      .filter((login) => !opening.known.some((one) => sameAgentLogin(one.login, login)))
      .map((login) => ({ login, who: login.home ?? "", away: true }));
    this.listed = [...opening.known.map((one) => ({ ...one, away: false })), ...away];
    this.chosen = new Set(this.listed.flatMap((one, at) => (opening.chosen.some((login) => sameAgentLogin(login, one.login)) ? [at] : [])));
    this.sought = "";
    const { words } = this;
    const c = words.callersSheet;
    // Few enough to see at once need no search.
    const seek = this.listed.length > 8 ? `<input class="cs-seek" name="seek" type="search" autocomplete="off" spellcheck="false" placeholder="${escape(c.seek)}" aria-label="${escape(c.seek)}">` : "";
    this.sheet.body.innerHTML = `${this.sheet.head(words.callers, c.close)}
      <p class="cs-said">${escape(words.callersSaid)}</p>
      ${seek}
      <div class="cs-list scroller" role="group" aria-label="${escape(words.callers)}"></div>
      <p class="sheet-note" role="alert" hidden></p>
      <footer class="sheet-foot cs-foot"><span class="cs-all"><button class="text-button quiet-button" type="button" data-cs-all="on">${escape(c.all)}</button><button class="text-button quiet-button" type="button" data-cs-all="off">${escape(c.none)}</button></span><span class="cs-count" role="status"></span><button class="sheet-go" type="submit">${escape(words.save)}</button></footer>`;
    this.drawList();
    this.drawFoot();
    this.sheet.show();
  }

  /** The logins the search leaves, by their place in the list. */
  private shown(): number[] {
    const sought = this.sought;
    return this.listed.flatMap((one, at) => (!sought || `${one.login.machine} ${agentName(one.login.agent)} ${one.who}`.toLowerCase().includes(sought) ? [at] : []));
  }

  private drawList(): void {
    const list = this.sheet.body.querySelector<HTMLElement>(".cs-list");
    if (!list) return;
    const { words } = this;
    const c = words.callersSheet;
    const shown = this.shown();
    if (!shown.length) {
      list.innerHTML = `<p class="cs-none">${escape(this.listed.length ? c.noMatch : words.noLogins)}</p>`;
      return;
    }
    const machines: string[] = [];
    for (const at of shown) if (!machines.includes(this.listed[at].login.machine)) machines.push(this.listed[at].login.machine);
    list.innerHTML = machines
      .map((machine) => {
        const rows = shown
          .filter((at) => this.listed[at].login.machine === machine)
          .map((at) => {
            const one = this.listed[at];
            const on = this.chosen.has(at);
            return `<button type="button" class="cs-login${on ? " chosen" : ""}" role="checkbox" aria-checked="${on}" data-cs-login="${at}">${glyph(one.login.agent, 16)}<span class="cs-agent">${escape(agentName(one.login.agent))}</span><span class="cs-who">${escape(one.away ? c.away : one.who)}</span><span class="cs-mark" aria-hidden="true">${glyph("check", 14)}</span></button>`;
          })
          .join("");
        return `<section class="cs-machine"><h3 class="cs-machine-name">${escape(machine)}</h3>${rows}</section>`;
      })
      .join("");
  }

  private drawFoot(): void {
    const count = this.sheet.body.querySelector<HTMLElement>(".cs-count");
    if (count) count.textContent = this.words.callersSheet.count(this.chosen.size);
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    if (!opening || this.sheet.busy) return;
    const callers = this.listed.filter((_, at) => this.chosen.has(at)).map((one) => one.login);
    this.sheet.busy = true;
    const go = this.sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    if (go) go.disabled = true;
    try {
      await opening.save(callers);
      this.sheet.busy = false;
      this.sheet.close();
    } catch (error) {
      this.sheet.busy = false;
      if (go) go.disabled = false;
      const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note");
      if (note) {
        note.innerHTML = `${escape(this.words.failed(detailOf(error)))}<span class="sheet-note-detail">${escape(detailOf(error))}</span>`;
        note.hidden = false;
      }
    }
  }
}
