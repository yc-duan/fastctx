// Adding an MCP server, or editing one (`world-user-config.md`, 三种配法): a
// sheet with two ways in, pasting the `mcpServers` JSON every tool writes or
// filling in a form. Where a server a command starts runs is chosen first, at
// its top: this computer, or another the user may run commands on, as a
// server they would not log in to for this. Pasted, each server it names is
// a line, its name a field that takes a free one when the account already
// has it, and its end says where it goes: the machine chosen, or the address
// for one reached at one. Each is kept in turn; one refused stops the rest
// and says why.

import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { Login } from "../world/accounts";
import { freeName, nameProblem, parseServers, type Server } from "../world/profile";
import type { Pickable } from "./agent-machines";
import { Sheet } from "./confirm";
import type { ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";

interface Common {
  /** The names the account's servers already have. */
  taken: string[];
  /** Where a server a command starts may run. */
  machines: Pickable[];
  calls: ProfileCalls;
  login: Login;
  /** Something was kept: the list is read again. */
  done(): void;
}

/** Adding, on `machine` where it was asked from a machine's group; or
 *  editing one the account has. */
export type SheetOpening = ({ mode: "add"; machine?: string } & Common) | ({ mode: "edit"; name: string; server: Server; machine?: string } & Common);

type Way = "paste" | "form";

/** A server as pasted: its name as the user leaves it. */
interface Pasted {
  name: string;
  server: Server;
}

export class McpSheet {
  private readonly sheet = new Sheet("mcp-sheet");
  private opening: SheetOpening | null = null;
  private way: Way = "paste";
  private pasted: Pasted[] = [];
  private parsed: "json" | "none" | "shape" | null = null;
  private kind: "command" | "remote" = "command";
  private machine: string | null = null;
  private problem: { text: string; detail: string } | null = null;

  constructor(
    private readonly words: ProfileWords,
    private readonly close: string,
  ) {
    const body = this.sheet.body;
    body.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const way = target.closest<HTMLElement>("[data-way]");
      if (way) {
        this.way = way.dataset.way as Way;
        this.draw();
        return;
      }
      const sse = target.closest<HTMLElement>("[data-sse]");
      if (sse) {
        sse.setAttribute("aria-checked", String(sse.getAttribute("aria-checked") !== "true"));
        return;
      }
      const kind = target.closest<HTMLElement>("[data-kind]");
      if (kind) {
        this.kind = kind.dataset.kind as "command" | "remote";
        this.draw();
        return;
      }
      const machine = target.closest<HTMLElement>("[data-machine]");
      if (machine && !(machine as HTMLButtonElement).disabled) {
        this.machine = machine.dataset.machine!;
        for (const one of body.querySelectorAll<HTMLElement>("[data-machine]")) {
          const on = one.dataset.machine === this.machine;
          one.classList.toggle("chosen", on);
          one.setAttribute("aria-checked", String(on));
        }
        const pasted = body.querySelector<HTMLTextAreaElement>(".mcp-paste");
        if (pasted?.value.trim()) this.parse(pasted.value);
        this.drawFound();
        this.drawGo();
      }
    });
    body.addEventListener("input", (event) => {
      const target = event.target as HTMLElement;
      if (target.matches(".mcp-paste")) {
        this.parse((target as HTMLTextAreaElement).value);
        this.drawFound();
      } else if (target.matches(".mcp-found-name")) {
        const index = Number(target.dataset.index);
        const pasted = this.pasted[index];
        if (pasted) pasted.name = (target as HTMLInputElement).value.trim();
        this.drawFoundNotes();
      }
      this.drawGo();
    });
    body.onsubmit = (event) => {
      event.preventDefault();
      void this.submit();
    };
  }

  open(opening: SheetOpening): void {
    this.opening = opening;
    this.problem = null;
    this.pasted = [];
    this.parsed = null;
    if (opening.mode === "edit") {
      this.way = "form";
      this.kind = opening.server.kind;
      this.machine = opening.machine ?? null;
    } else {
      this.way = "paste";
      this.kind = "command";
      this.machine = opening.machine ?? null;
    }
    this.machine ??= opening.machines.find((one) => one.here && one.online !== false)?.written ?? opening.machines.find((one) => one.online === true)?.written ?? null;
    this.draw();
    this.sheet.show();
    this.sheet.body.querySelector<HTMLElement>(opening.mode === "edit" ? "input" : ".mcp-paste")?.focus();
  }

  private draw(): void {
    const opening = this.opening;
    if (!opening) return;
    const { words } = this;
    const editing = opening.mode === "edit";
    const ways = editing
      ? ""
      : `<div class="choice-options mcp-ways" role="tablist">${(["paste", "form"] as const)
          .map((way) => `<button type="button" class="choice${this.way === way ? " chosen" : ""}" role="tab" aria-selected="${this.way === way}" data-way="${way}">${escape(way === "paste" ? words.add.paste : words.add.form)}</button>`)
          .join("")}</div>`;
    const pane = this.way === "paste" ? this.pastePane() : this.formPane(opening);
    this.sheet.body.innerHTML = `${this.sheet.head(editing ? words.add.editTitle(opening.name) : words.add.title, this.close)}
      ${ways}
      ${this.machinesField()}
      <div class="mcp-pane">${pane}</div>
      <p class="sheet-note" role="alert"${this.problem ? "" : " hidden"}>${this.problem ? `${escape(this.problem.text)}<span class="sheet-note-detail">${escape(this.problem.detail)}</span>` : ""}</p>
      <footer class="sheet-foot"><span></span><button class="sheet-go" type="submit">${escape(editing ? words.add.save : words.add.go(this.count()))}</button></footer>`;
    this.drawFound();
    this.drawGo();
    this.sheet.anew();
  }

  private pastePane(): string {
    return `<textarea class="mcp-paste" rows="7" spellcheck="false" autocomplete="off" placeholder="${escape(this.words.add.pastePlaceholder)}" aria-label="${escape(this.words.add.paste)}"></textarea>
      <ul class="mcp-found"></ul>`;
  }

  private formPane(opening: SheetOpening): string {
    const { words } = this;
    const editing = opening.mode === "edit";
    const server = editing ? opening.server : null;
    const command = server?.kind === "command" ? server : null;
    const remote = server?.kind === "remote" ? server : null;
    const kinds = editing
      ? ""
      : `<div class="field"><span class="field-label">${escape(words.add.kind)}</span><div class="choice-options">${(["command", "remote"] as const)
          .map((kind) => `<button type="button" class="choice${this.kind === kind ? " chosen" : ""}" role="radio" aria-checked="${this.kind === kind}" data-kind="${kind}">${glyph(kind === "command" ? "terminal" : "link", 15)}${escape(kind === "command" ? words.add.kindCommand : words.add.kindRemote)}</button>`)
          .join("")}</div></div>`;
    const name = `<label class="field"><span class="field-label">${escape(words.add.name)}</span><input name="name" type="text" spellcheck="false" autocomplete="off" value="${escape(editing ? opening.name : "")}"${editing ? " readonly" : ""}><span class="field-hint mcp-name-note" hidden></span></label>`;
    const fields =
      this.kind === "command"
        ? `<label class="field"><span class="field-label">${escape(words.add.command)}</span><input name="command" type="text" spellcheck="false" autocomplete="off" value="${escape(command ? [command.command, ...(command.args ?? [])].map(quoted).join(" ") : "")}"></label>
          <label class="field"><span class="field-label">${escape(words.add.env)}</span><textarea name="env" rows="2" spellcheck="false" placeholder="${escape(words.add.envPlaceholder)}">${escape(lines(command?.env, "="))}</textarea></label>
          <label class="field"><span class="field-label">${escape(words.add.cwd)}</span><input name="cwd" type="text" spellcheck="false" autocomplete="off" value="${escape(command?.cwd ?? "")}"></label>`
        : `<label class="field"><span class="field-label">${escape(words.add.url)}<span class="field-tip" tabindex="0" aria-label="${escape(words.mcp.remoteReaches)}">?<span class="field-tip-card" role="tooltip">${escape(words.mcp.remoteReaches)}</span></span></span><input name="url" type="url" spellcheck="false" autocomplete="off" value="${escape(remote?.url ?? "")}"></label>
          <label class="field"><span class="field-label">${escape(words.add.headers)}</span><textarea name="headers" rows="2" spellcheck="false" placeholder="${escape(words.add.headersPlaceholder)}">${escape(lines(remote?.headers, ": "))}</textarea></label>
          <label class="mcp-sse"><button type="button" role="switch" aria-checked="${!!remote?.sse}" data-sse><span class="switch-track"><span class="switch-knob"></span></span></button><span>${escape(words.add.sse)}</span></label>`;
    return `${kinds}${name}${fields}`;
  }

  /** Where a server a command starts runs, chosen first: hidden only when
   *  what is in hand starts none. Editing, it is said, not chosen. */
  private machinesField(): string {
    const opening = this.opening;
    if (!opening) return "";
    const { words } = this;
    const label = `<span class="field-label">${escape(words.add.on)}<span class="field-tip" tabindex="0" aria-label="${escape(words.add.onTip)}">?<span class="field-tip-card" role="tooltip">${escape(words.add.onTip)}</span></span></span>`;
    if (opening.mode === "edit") {
      if (opening.server.kind !== "command" || !opening.machine) return `<div class="field mcp-on" hidden></div>`;
      const machine = opening.machines.find((one) => one.name === opening.machine || one.written === opening.machine);
      return `<div class="field mcp-on">${label}<span class="mcp-on-fixed">${glyph(machine?.form ?? "desktop", 15)}${escape(machine?.written ?? opening.machine)}</span></div>`;
    }
    const commands = this.way === "paste" ? !this.pasted.length || this.pasted.some((one) => one.server.kind === "command") : this.kind === "command";
    if (!commands) return `<div class="field mcp-on" hidden></div>`;
    const online = opening.machines.filter((one) => one.here || one.online === true);
    const choices = online.length
      ? opening.machines
          .map((machine) => {
            const usable = machine.here || machine.online === true;
            const on = usable && machine.written === this.machine;
            return `<button type="button" class="choice${on ? " chosen" : ""}" role="radio" aria-checked="${on}" data-machine="${escape(machine.written)}"${usable ? "" : " disabled"}>${glyph(machine.form, 15)}${escape(machine.written)}</button>`;
          })
          .join("")
      : `<span class="choice-none">${escape(words.add.noMachine)}</span>`;
    return `<div class="field mcp-on">${label}<div class="choice-options">${choices}</div></div>`;
  }

  private parse(text: string): void {
    const opening = this.opening;
    if (!opening) return;
    if (!text.trim()) {
      this.pasted = [];
      this.parsed = null;
      return;
    }
    const parsed = parseServers(text);
    if ("problem" in parsed) {
      this.pasted = [];
      this.parsed = parsed.problem;
      return;
    }
    this.parsed = null;
    const names = new Set(opening.taken);
    const machine = this.machine ?? "";
    this.pasted = parsed.servers.map(([name, server]) => {
      const free = freeName(name, server.kind === "command" ? machine : "", (one) => names.has(one) || nameProblem(one) !== null);
      names.add(free);
      return { name: free, server };
    });
  }

  private drawFound(): void {
    const list = this.sheet.body.querySelector<HTMLElement>(".mcp-found");
    const { words } = this;
    if (list) {
      list.innerHTML = this.pasted
        .map(
          (one, index) => `<li class="mcp-found-row">
            <span class="mcp-kind">${glyph(one.server.kind === "command" ? "terminal" : "link", 16)}</span>
            <input class="mcp-found-name" type="text" data-index="${index}" value="${escape(one.name)}" spellcheck="false" autocomplete="off" aria-label="${escape(words.add.name)}">
            <span class="mcp-detail">${escape(serverLine(one.server))}</span>
            <span class="mcp-found-on">${one.server.kind === "command" ? `${glyph(this.form(), 14)}${escape(this.machine ?? "")}` : `${glyph("web", 14)}${escape(words.mcp.remote)}`}</span>
            <span class="mcp-found-note"></span>
          </li>`,
        )
        .join("");
      if (this.parsed) list.innerHTML = `<li class="mcp-found-problem">${escape(this.parsed === "json" ? words.add.notJson : this.parsed === "none" ? words.add.noServers : words.add.shape)}</li>`;
      this.drawFoundNotes();
    }
    // The machines' field comes and goes with servers a command starts.
    const field = this.sheet.body.querySelector<HTMLElement>(".mcp-on");
    const html = this.machinesField();
    if (field && field.outerHTML !== html) field.outerHTML = html;
  }

  /** What kind of computer the machine chosen is, for its mark. */
  private form(): "desktop" | "laptop" | "server" {
    return this.opening?.machines.find((one) => one.written === this.machine)?.form ?? "desktop";
  }

  /** Beside each pasted name, why it cannot be kept, when it cannot. */
  private drawFoundNotes(): void {
    const opening = this.opening;
    if (!opening) return;
    const notes = this.sheet.body.querySelectorAll<HTMLElement>(".mcp-found-note");
    this.pasted.forEach((one, index) => {
      const note = notes[index];
      if (!note) return;
      const problem = this.nameNote(one.name, index);
      note.textContent = problem ?? "";
      note.classList.toggle("caution", !!problem);
    });
  }

  /** Why `name` cannot be the `index`th pasted name, or null. */
  private nameNote(name: string, index: number | null): string | null {
    const opening = this.opening;
    if (!opening) return null;
    const { words } = this;
    const problem = nameProblem(name);
    if (problem === "rule") return words.add.nameRule;
    if (problem === "reserved") return words.add.reserved;
    const editing = opening.mode === "edit" ? opening.name : null;
    if (name !== editing && opening.taken.includes(name)) return words.add.taken;
    if (index !== null && this.pasted.some((other, at) => at !== index && other.name === name)) return words.add.taken;
    return null;
  }

  private count(): number {
    return this.way === "paste" ? this.pasted.length : 1;
  }

  private drawGo(): void {
    const button = this.sheet.body.querySelector<HTMLButtonElement>(".sheet-go");
    if (!button || this.sheet.busy) return;
    const opening = this.opening;
    if (opening?.mode !== "edit") button.textContent = this.words.add.go(this.count());
    const commands = this.way === "paste" ? this.pasted.some((one) => one.server.kind === "command") : this.kind === "command";
    const placed = !commands || opening?.mode === "edit" || this.machine !== null;
    const named = this.way === "paste" ? this.pasted.length > 0 && this.pasted.every((one, index) => this.nameNote(one.name, index) === null) : true;
    button.disabled = !placed || !named;
  }

  /** The servers the sheet would keep, or why it cannot. */
  private gathered(): { servers: Pasted[] } | { problem: string } {
    const { words } = this;
    if (this.way === "paste") return { servers: this.pasted };
    const body = this.sheet.body;
    const value = (name: string) => body.querySelector<HTMLInputElement | HTMLTextAreaElement>(`[name="${name}"]`)?.value.trim() ?? "";
    const name = value("name");
    const nameNote = this.nameNote(name, null);
    const note = body.querySelector<HTMLElement>(".mcp-name-note");
    if (note) {
      note.hidden = !nameNote;
      note.textContent = nameNote ?? "";
    }
    if (nameNote) return { problem: nameNote };
    if (this.kind === "command") {
      const [command, ...args] = splitArgs(value("command"));
      if (!command) return { problem: words.add.command };
      const env = pairs(value("env"), "=");
      const cwd = value("cwd");
      return { servers: [{ name, server: { kind: "command", command, ...(args.length ? { args } : {}), ...(env ? { env } : {}), ...(cwd ? { cwd } : {}) } }] };
    }
    const url = value("url");
    if (!/^https?:\/\/\S+$/.test(url)) return { problem: words.add.urlRule };
    const headers = pairs(value("headers"), ":");
    const sse = body.querySelector<HTMLElement>("[data-sse]")?.getAttribute("aria-checked") === "true";
    return { servers: [{ name, server: { kind: "remote", url, ...(sse ? { sse } : {}), ...(headers ? { headers } : {}) } }] };
  }

  private async submit(): Promise<void> {
    const opening = this.opening;
    if (!opening || this.sheet.busy) return;
    const { words } = this;
    const gathered = this.gathered();
    if ("problem" in gathered) {
      this.problem = { text: gathered.problem, detail: "" };
      this.drawProblem();
      return;
    }
    const button = this.sheet.body.querySelector<HTMLButtonElement>(".sheet-go")!;
    this.sheet.busy = true;
    button.disabled = true;
    button.textContent = words.add.adding;
    let kept = 0;
    try {
      for (const one of gathered.servers) {
        const machine = one.server.kind === "command" ? (opening.mode === "edit" ? opening.machine : (this.machine ?? undefined)) : undefined;
        await opening.calls.mcp(opening.login.hubName, opening.login.account, { do: "keep", name: one.name, server: one.server, ...(machine ? { machine } : {}) });
        kept += 1;
      }
      this.sheet.busy = false;
      this.sheet.close();
      opening.done();
    } catch (error) {
      this.sheet.busy = false;
      this.problem = { text: words.add.failed, detail: detailOf(error) };
      // What was kept before the refusal is kept; the rest stays here.
      if (this.way === "paste") this.pasted = this.pasted.slice(kept);
      if (kept) opening.done();
      this.draw();
    }
  }

  private drawProblem(): void {
    const note = this.sheet.body.querySelector<HTMLElement>(".sheet-note");
    if (!note) return;
    note.hidden = !this.problem;
    note.innerHTML = this.problem ? `${escape(this.problem.text)}${this.problem.detail ? `<span class="sheet-note-detail">${escape(this.problem.detail)}</span>` : ""}` : "";
  }
}

/** A server on one quiet line: its command and arguments, or its address. */
export function serverLine(server: Server): string {
  return server.kind === "command" ? [server.command, ...(server.args ?? [])].map(quoted).join(" ") : server.url;
}

/** A word as a shell would read it back: quoted where it holds a space or a
 *  quote. */
function quoted(word: string): string {
  return /^[^\s"'\\]+$/.test(word) ? word : `"${word.replace(/(["\\])/g, "\\$1")}"`;
}

/** A command line split into its words, as a shell splits one: on spaces,
 *  keeping what is quoted together. */
export function splitArgs(text: string): string[] {
  const words: string[] = [];
  let word = "";
  let quote: string | null = null;
  let begun = false;
  for (let i = 0; i < text.length; i += 1) {
    const c = text[i]!;
    if (quote) {
      if (c === quote) quote = null;
      else if (c === "\\" && quote === '"' && (text[i + 1] === '"' || text[i + 1] === "\\")) word += text[++i];
      else word += c;
    } else if (c === '"' || c === "'") {
      quote = c;
      begun = true;
    } else if (/\s/.test(c)) {
      if (begun) words.push(word);
      word = "";
      begun = false;
    } else {
      word += c;
      begun = true;
    }
  }
  if (begun) words.push(word);
  return words;
}

/** `KEY=value` or `Name: value` lines as a table; none when there are none. */
function pairs(text: string, separator: string): Record<string, string> | null {
  const entries = text
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const at = line.indexOf(separator);
      return at > 0 ? ([line.slice(0, at).trim(), line.slice(at + separator.length).trim()] as [string, string]) : null;
    })
    .filter((entry): entry is [string, string] => entry !== null);
  return entries.length ? Object.fromEntries(entries) : null;
}

function lines(table: Record<string, string> | undefined, separator: string): string {
  return Object.entries(table ?? {})
    .map(([key, value]) => `${key}${separator}${value}`)
    .join("\n");
}
