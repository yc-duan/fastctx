// Importing MCP servers (`world-user-config.md`, 一键导入): what Claude
// Desktop, Claude Code and Cursor on a machine already have, read there by
// its node and listed by the tool that has it, each with a box to tick. A
// server the account already has by that name is listed unticked; ticked, it
// replaces the one there. Those a command starts are kept on the machine
// they were read on; those at an address, on the account. What is listed
// is a name and a kind: an address, a header or a command leaves the
// machine only for the server ticked, and only into the account.

import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { Login } from "../world/accounts";
import { freeName, nameProblem, type ImportCandidate } from "../world/profile";
import type { Pickable } from "./agent-machines";
import { Sheet } from "./confirm";
import type { ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";

export interface ImportOpening {
  taken: string[];
  machines: Pickable[];
  calls: ProfileCalls;
  login: Login;
  done(): void;
}

/** One server read on the machine: under what name it would be kept, and
 *  whether it is ticked. */
interface Candidate {
  found: ImportCandidate;
  name: string;
  had: boolean;
  ticked: boolean;
}

type Reading = { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "read"; machine: string; candidates: Candidate[] };

export class McpImport {
  private readonly sheet = new Sheet("mcp-sheet mcp-import");
  private opening: ImportOpening | null = null;
  private machine: string | null = null;
  private reading: Reading = { kind: "reading" };
  private problem: string | null = null;
  /** What the kernel said of it, for the pointer. */
  private problemWhy = "";

  constructor(
    private readonly words: ProfileWords,
    private readonly close: string,
  ) {
    this.sheet.body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const machine = target.closest<HTMLElement>("[data-machine]");
      if (machine && !(machine as HTMLButtonElement).disabled && machine.dataset.machine !== this.machine) {
        this.machine = machine.dataset.machine!;
        void this.read();
        return;
      }
      const tick = target.closest<HTMLElement>("[data-tick]");
      if (tick && this.reading.kind === "read") {
        const candidate = this.reading.candidates[Number(tick.dataset.tick)];
        if (candidate) candidate.ticked = !candidate.ticked;
        this.draw();
      }
    });
    this.sheet.body.onsubmit = (event) => {
      event.preventDefault();
      void this.submit();
    };
  }

  open(opening: ImportOpening): void {
    this.opening = opening;
    this.problem = null;
    this.machine = opening.machines.find((one) => one.here)?.written ?? opening.machines.find((one) => one.online === true)?.written ?? null;
    this.reading = { kind: "reading" };
    this.draw();
    this.sheet.show();
    void this.read();
  }

  private async read(): Promise<void> {
    const opening = this.opening;
    const machine = this.machine;
    if (!opening || !machine) return;
    this.reading = { kind: "reading" };
    this.problem = null;
    this.draw();
    try {
      const reply = await opening.calls.mcp(opening.login.hubName, opening.login.account, { do: "candidates", machine });
      if (this.machine !== machine) return;
      if (reply.reply !== "candidates") throw new Error(reply.reply);
      const names = new Set(opening.taken);
      const candidates = reply.servers.map((found): Candidate => {
        const had = opening.taken.includes(found.name);
        // The same name read from two tools is kept under two.
        const name = had ? found.name : freeName(found.name, "", (one) => names.has(one) || nameProblem(one) !== null);
        names.add(name);
        return { found, name, had, ticked: !had && nameProblem(name) === null };
      });
      this.reading = { kind: "read", machine: reply.machine, candidates };
    } catch (error) {
      if (this.machine === machine) this.reading = { kind: "failed", detail: detailOf(error) };
    }
    this.draw();
    this.sheet.anew();
  }

  private draw(): void {
    const opening = this.opening;
    if (!opening) return;
    const { words } = this;
    const machines = opening.machines
      .map((machine) => {
        const usable = machine.here || machine.online === true;
        const on = machine.written === this.machine;
        return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-machine="${escape(machine.written)}"${usable ? "" : " disabled"}>${glyph(machine.form, 15)}${escape(machine.written)}</button>`;
      })
      .join("");
    const reading = this.reading;
    let list = "";
    if (reading.kind === "reading") list = `<p class="prof-quiet">${escape(words.import.reading)}</p>`;
    else if (reading.kind === "failed") list = `<p class="prof-quiet" title="${escape(reading.detail)}">${escape(words.readFailed)}</p>`;
    else if (!reading.candidates.length) list = `<p class="prof-quiet">${escape(words.import.none)}</p>`;
    else {
      const sources = [...new Set(reading.candidates.map((one) => sourceName(one.found, words)))];
      list = sources
        .map((source) => {
          const rows = reading.candidates
            .map((one, index) => (sourceName(one.found, words) === source ? this.row(one, index) : ""))
            .join("");
          return `<section class="mcp-source"><h3 class="picker-title">${escape(source)}</h3><ul class="mcp-picks">${rows}</ul></section>`;
        })
        .join("");
    }
    const count = reading.kind === "read" ? reading.candidates.filter((one) => one.ticked).length : 0;
    this.sheet.body.innerHTML = `${this.sheet.head(words.import.title, this.close)}
      <div class="field mcp-on"><span class="field-label">${escape(words.import.from)}</span><div class="choice-options">${machines}</div></div>
      <div class="mcp-import-list">${list}</div>
      <p class="sheet-note" role="alert"${this.problem ? "" : " hidden"} title="${escape(this.problemWhy)}">${escape(this.problem ?? "")}</p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit"${count ? "" : " disabled"}>${escape(words.import.go(count))}</button></footer>`;
  }

  private row(candidate: Candidate, index: number): string {
    const { words } = this;
    return `<li><button type="button" class="mcp-pick${candidate.ticked ? " ticked" : ""}" role="checkbox" aria-checked="${candidate.ticked}" data-tick="${index}">
        <span class="mcp-check" aria-hidden="true">${candidate.ticked ? glyph("check", 14) : ""}</span>
        <span class="mcp-kind">${glyph(candidate.found.kind === "command" ? "terminal" : "link", 16)}</span>
        <span class="mcp-name">${escape(candidate.name)}</span>
        ${candidate.had ? `<span class="mcp-tag">${escape(words.import.had)}</span>` : ""}
      </button></li>`;
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    const reading = this.reading;
    if (!opening || reading.kind !== "read" || this.sheet.busy) return;
    const ticked = reading.candidates.filter((one) => one.ticked);
    if (!ticked.length) return;
    const button = this.sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.sheet.busy = true;
    button.disabled = true;
    button.textContent = this.words.import.importing;
    let kept = 0;
    try {
      for (const one of ticked) {
        // The machine it was found on takes it: one a command starts stays
        // there, one at an address it reads again for the account.
        await opening.calls.mcp(opening.login.hubName, opening.login.account, {
          do: "import",
          machine: reading.machine,
          source: one.found.source,
          name: one.found.name,
          as_name: one.name,
        });
        one.ticked = false;
        one.had = true;
        kept += 1;
      }
      this.sheet.busy = false;
      this.sheet.close();
      opening.done();
    } catch (error) {
      this.sheet.busy = false;
      this.problem = this.words.import.failed;
      this.problemWhy = detailOf(error);
      if (kept) opening.done();
      this.draw();
    }
  }
}

function sourceName(found: ImportCandidate, words: ProfileWords): string {
  const source = found.source;
  if (source.app === "claude_desktop") return words.import.claudeDesktop;
  if (source.app === "cursor") return words.import.cursor;
  const home = source.home.replace(/[\\/]+$/, "");
  const last = home.split(/[\\/]/).pop() ?? home;
  return `${words.import.claudeCode} ${last}`;
}
