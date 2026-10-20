// The account's MCP servers (`world-user-config.md`, MCP): one list across
// its machines, grouped by where each runs. A server a command starts sits
// under its machine, this computer first; one reached at an address under
// 「网址」. Each says what it is on a quiet line and what its machine says
// of it; its 「⋯」 tries it, edits it, removes it. New ones are added by
// pasting the `mcpServers` JSON every tool writes, by filling in a form, or
// by importing what Claude Desktop, Claude Code and Cursor on a machine
// already have (`./mcp-sheet`, `./mcp-import`).

import { escape } from "../conversation/prose";
import { glyph } from "../conversation/glyphs";
import type { Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { Listed, Tested } from "../world/profile";
import { ActsMenu } from "./acts-menu";
import { pickables, type Pickable } from "./agent-machines";
import type { Confirm } from "./confirm";
import { McpImport } from "./mcp-import";
import { McpSheet } from "./mcp-sheet";
import { machinesOf } from "./reach";
import { AccountChoice, loginKey, type ProfileCalls } from "./profile-account";
import type { ProfileWords } from "./profile-words";
import { detailOf } from "./problem";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { Store } from "./store";

export interface McpPageOptions {
  store: Store;
  words: ProfileWords;
  head(title: string, actions?: string): string;
  calls: ProfileCalls;
  confirm: Confirm;
  /** Says what is close by, to close a sheet. */
  close: string;
}

type State = { kind: "none" } | { kind: "reading" } | { kind: "failed"; detail: string } | { kind: "older" } | { kind: "ready"; servers: Listed[] };

type Trial = { kind: "running" } | { kind: "done"; tested: Tested; open: boolean } | { kind: "failed"; detail: string };

/** How many of a server's tools are named after trying it; the rest are a
 *  count. */
const TOOLS_SHOWN = 12;

let shared: ActsMenu | null = null;
function rowsMenu(): ActsMenu {
  shared ??= new ActsMenu();
  return shared;
}

export class McpPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private readonly account: AccountChoice;
  private readonly stop: () => void;
  private readonly menu = rowsMenu();
  private readonly sheet: McpSheet;
  private readonly importing: McpImport;
  private state: State = { kind: "none" };
  private shownLogin: string | null = null;
  private trials = new Map<string, Trial>();
  private drawn = "";

  constructor(private readonly options: McpPageOptions) {
    const { words } = options;
    this.title = words.sections.mcp;
    this.account = new AccountChoice(words, (login) => void this.load(login));
    this.sheet = new McpSheet(words, options.close);
    this.importing = new McpImport(words, options.close);
    this.element = document.createElement("div");
    this.element.className = "prof-page mcp-page";
    const actions = `<button class="text-button quiet-button mcp-head-act" type="button" data-import title="${escape(words.mcp.import)}">${glyph("save", 16)}<span>${escape(words.mcp.import)}</span></button><button class="text-button mcp-head-act mcp-add" type="button" data-add title="${escape(words.mcp.add)}">${glyph("plus", 16)}<span>${escape(words.mcp.add)}</span></button>`;
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.sections.mcp)}</h1><span class="prof-account-place"></span>`, actions)}
      <div class="prof-body scroller"></div>`;
    this.element.querySelector(".prof-account-place")!.replaceWith(this.account.element);
    this.body = this.element.querySelector<HTMLElement>(".prof-body")!;
    this.element.addEventListener("click", (event) => void this.click(event));
    this.stop = effect(() => {
      const world = options.store.world.get();
      this.account.draw(world);
      const login = this.account.chosen();
      if (login && loginKey(login) !== this.shownLogin) void this.load(login);
      else this.draw();
    });
  }

  leave(): void {
    this.stop();
    this.menu.close();
    this.account.leave();
  }

  private async load(login: Login): Promise<void> {
    const key = loginKey(login);
    if (this.shownLogin !== key) this.trials.clear();
    this.shownLogin = key;
    if (this.state.kind !== "ready") {
      this.state = { kind: "reading" };
      this.draw();
    }
    try {
      const [view, listed] = await Promise.all([this.options.calls.read(login.hubName, login.account), this.options.calls.mcp(login.hubName, login.account, { do: "list" })]);
      if (this.shownLogin !== key) return;
      this.state = !view.kept ? { kind: "older" } : listed.reply === "listed" ? { kind: "ready", servers: listed.servers } : { kind: "failed", detail: listed.reply };
    } catch (error) {
      if (this.shownLogin === key) this.state = { kind: "failed", detail: detailOf(error) };
    }
    this.draw();
  }

  private reload(): void {
    const login = this.account.chosen();
    if (login) void this.load(login);
  }

  private draw(): void {
    const { words } = this.options;
    const world = this.options.store.world.peek();
    const state = this.state;
    const ready = state.kind === "ready";
    for (const button of this.element.querySelectorAll<HTMLButtonElement>(".mcp-head-act")) button.hidden = !ready || state.servers.length === 0;
    let html: string;
    switch (state.kind) {
      case "none":
        html = "";
        break;
      case "reading":
        html = `<p class="prof-quiet prof-board">${escape(words.reading)}</p>`;
        break;
      case "failed":
        html = `<p class="prof-quiet prof-board" title="${escape(state.detail)}">${escape(words.readFailed)}</p>`;
        break;
      case "older":
        html = `<div class="prof-empty">${glyph("server", 40)}<p class="prof-quiet">${escape(words.olderHub)}</p></div>`;
        break;
      case "ready":
        html = state.servers.length
          ? this.groups(state.servers, world)
          : `<div class="prof-empty">${glyph("plug", 40)}<div class="prof-empty-acts"><button class="text-button quiet-button" type="button" data-import>${escape(words.mcp.import)}</button><button class="sheet-go" type="button" data-add>${escape(words.mcp.add)}</button></div></div>`;
        break;
    }
    if (html === this.drawn) return;
    this.drawn = html;
    this.body.innerHTML = html;
  }

  /** The servers by where they run: each machine's, this computer first and
   *  those away last, then those reached at an address. */
  private groups(servers: Listed[], world: WorldNow | null): string {
    const { words } = this.options;
    const machines = pickables(world);
    const byMachine = new Map<string, Listed[]>();
    const remote: Listed[] = [];
    for (const listed of servers) {
      if (listed.server.kind === "command") {
        const machine = machineOf(machines, listed.server.machine)?.written ?? listed.server.machine;
        byMachine.set(machine, [...(byMachine.get(machine) ?? []), listed]);
      } else remote.push(listed);
    }
    const order = [...byMachine.keys()].sort((a, b) => rank(machineOf(machines, a)) - rank(machineOf(machines, b)) || a.localeCompare(b));
    const groups = order.map((name) => {
      const machine = machineOf(machines, name);
      const away = machine?.online !== true && !machine?.here;
      const windows = osOf(world, machine?.name ?? name) === "windows";
      const addable = !away && machine !== undefined && machine.hold !== "assist";
      const add = addable ? `<button class="mcp-group-add" type="button" data-add-on="${escape(name)}" aria-label="${escape(words.mcp.addOn(name))}" title="${escape(words.mcp.addOn(name))}">${glyph("plus", 14)}</button>` : "";
      const title = `<h2 class="picker-title mcp-group-title${away ? " away" : ""}"><span class="mcp-group-name">${glyph(machine?.form ?? "desktop", 16)}${escape(name)}${machine?.here ? `<span class="mcp-group-note">${escape(words.mcp.here)}</span>` : ""}</span>${add}</h2>`;
      return `<section class="mcp-group prof-board">${title}<ul class="mcp-rows">${(byMachine.get(name) ?? []).map((listed) => this.row(listed, away, windows)).join("")}</ul></section>`;
    });
    if (remote.length) {
      groups.push(
        `<section class="mcp-group prof-board"><h2 class="picker-title mcp-group-title"><span class="mcp-group-name">${glyph("web", 16)}${escape(words.mcp.remote)}</span></h2><ul class="mcp-rows">${remote.map((listed) => this.row(listed, false, false)).join("")}</ul></section>`,
      );
    }
    return groups.join("");
  }

  private row(listed: Listed, away: boolean, windows: boolean): string {
    const { words } = this.options;
    const server = listed.server;
    const trial = this.trials.get(listed.name);
    let detail = "";
    let tag = "";
    let hint = "";
    let kind = glyph("terminal", 16);
    if (server.kind === "remote") {
      kind = glyph("link", 16);
      detail = server.url;
      hint = words.mcp.remoteReaches;
    } else if (server.kind === "command") {
      const state = server.state;
      if (state.state === "missing") tag = `<span class="mcp-tag caution">${escape(words.mcp.missing)}</span>`;
      else if (state.state === "older") tag = `<span class="mcp-tag caution">${escape(words.mcp.older)}</span>`;
      else if (state.state === "failed") tag = `<span class="mcp-tag caution" title="${escape(state.problem)}">${escape(words.mcp.failed)}</span>`;
      if (windows) hint = words.mcp.sessionZero;
    }
    if (trial?.kind === "running") tag = `<span class="mcp-tag"><span class="pulse" aria-hidden="true"></span>${escape(words.mcp.testing)}</span>`;
    else if (trial?.kind === "done" && !trial.tested.problem) tag = `<span class="mcp-tag">${escape(trial.tested.tools.length ? words.mcp.tools(trial.tested.tools.length) : words.mcp.noTools)}</span>`;
    else if (trial?.kind === "done" || trial?.kind === "failed") tag = `<span class="mcp-tag wrong">${escape(words.mcp.didNotStart)}</span>`;
    const quiet = away || server.kind === "unknown";
    const row = `<li class="mcp-row${quiet ? " away" : ""}" data-name="${escape(listed.name)}"${hint ? ` title="${escape(hint)}"` : ""}>
        <span class="mcp-kind">${kind}</span>
        <span class="mcp-name">${escape(listed.name)}</span>
        <span class="mcp-detail">${escape(detail)}</span>
        ${tag}
        <button class="mcp-more" type="button" data-more aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.mcp.more)}" title="${escape(words.mcp.more)}">${glyph("more", 18)}</button>
      </li>`;
    return row + this.trialLine(listed.name, trial);
  }

  /** Under a server tried: its tools, or why it did not start and, opened,
   *  the end of what it wrote. */
  private trialLine(name: string, trial: Trial | undefined): string {
    const { words } = this.options;
    if (!trial || trial.kind === "running") return "";
    if (trial.kind === "failed") return `<li class="mcp-trial"><span class="mcp-trial-problem">${escape(trial.detail)}</span></li>`;
    const { tested } = trial;
    if (!tested.problem) {
      if (!tested.tools.length) return "";
      const shown = tested.tools.slice(0, TOOLS_SHOWN);
      const rest = tested.tools.length - shown.length;
      return `<li class="mcp-trial"><span class="mcp-tools">${shown.map((tool) => `<span class="mcp-tool">${escape(tool)}</span>`).join("")}${rest > 0 ? `<span class="mcp-tool more">+${rest}</span>` : ""}</span></li>`;
    }
    return `<li class="mcp-trial"><span class="mcp-trial-problem">${escape(tested.problem)}</span>${
      tested.log ? `<button class="text-button quiet-button mcp-log-toggle" type="button" data-log="${escape(name)}" aria-expanded="${trial.open}">${escape(words.mcp.log)}${glyph("chevron", 12)}</button>` : ""
    }${trial.open && tested.log ? `<pre class="mcp-log">${escape(tested.log)}</pre>` : ""}</li>`;
  }

  private listedOf(name: string): Listed | null {
    return this.state.kind === "ready" ? (this.state.servers.find((one) => one.name === name) ?? null) : null;
  }

  private async click(event: MouseEvent): Promise<void> {
    const target = event.target as HTMLElement;
    const login = this.account.chosen();
    if (!login) return;
    const taken = this.state.kind === "ready" ? this.state.servers.map((one) => one.name) : [];
    const world = this.options.store.world.peek();
    const on = target.closest<HTMLElement>("[data-add-on]");
    if (on) {
      this.sheet.open({ mode: "add", machine: on.dataset.addOn!, taken, machines: placeable(world), calls: this.options.calls, login, done: () => this.reload() });
      return;
    }
    if (target.closest("[data-add]")) {
      this.sheet.open({ mode: "add", taken, machines: placeable(world), calls: this.options.calls, login, done: () => this.reload() });
      return;
    }
    if (target.closest("[data-import]")) {
      this.importing.open({ taken, machines: placeable(world), calls: this.options.calls, login, done: () => this.reload() });
      return;
    }
    const log = target.closest<HTMLElement>("[data-log]");
    if (log) {
      const trial = this.trials.get(log.dataset.log!);
      if (trial?.kind === "done") trial.open = !trial.open;
      this.draw();
      return;
    }
    const more = target.closest<HTMLElement>("[data-more]");
    const name = more?.closest<HTMLElement>("[data-name]")?.dataset.name;
    if (!more || !name) return;
    const listed = this.listedOf(name);
    if (!listed) return;
    const { words } = this.options;
    // A command server's machine keeps its command to itself: it is filled
    // in anew, never shown.
    const editable = listed.server.kind === "remote" || (listed.server.kind === "command" && listed.server.state.state === "kept");
    const edit = listed.server.kind === "command" ? words.mcp.refill : words.mcp.edit;
    this.menu.open(more, name, [
      { act: "test", icon: "tool", label: words.mcp.test },
      ...(editable ? [{ act: "edit", icon: "edit" as const, label: edit }] : []),
      { rule: true },
      { act: "remove", icon: "trash", label: words.mcp.remove, danger: true },
    ], (act) => {
      if (act === "test") void this.test(login, name);
      else if (act === "edit") this.edit(login, listed, taken, world);
      else if (act === "remove") this.remove(login, listed, world);
    });
  }

  private async test(login: Login, name: string): Promise<void> {
    this.trials.set(name, { kind: "running" });
    this.draw();
    try {
      const reply = await this.options.calls.mcp(login.hubName, login.account, { do: "test", name });
      if (reply.reply === "tested") {
        const { reply: _, ...tested } = reply;
        this.trials.set(name, { kind: "done", tested, open: false });
      }
      else this.trials.set(name, { kind: "failed", detail: reply.reply });
    } catch (error) {
      this.trials.set(name, { kind: "failed", detail: detailOf(error) });
    }
    this.draw();
  }

  private edit(login: Login, listed: Listed, taken: string[], world: WorldNow | null): void {
    const server = listed.server;
    if (server.kind === "remote") {
      const { kind: _, ...remote } = server;
      this.sheet.open({ mode: "edit", name: listed.name, server: { kind: "remote", ...remote }, taken, machines: placeable(world), calls: this.options.calls, login, done: () => this.reload() });
    } else if (server.kind === "command" && server.state.state === "kept") {
      this.sheet.open({ mode: "edit", name: listed.name, server: { kind: "command", command: "" }, machine: server.machine, taken, machines: placeable(world), calls: this.options.calls, login, done: () => this.reload() });
    }
  }

  private remove(login: Login, listed: Listed, world: WorldNow | null): void {
    const { words } = this.options;
    const machine = listed.server.kind === "command" ? (machineOf(pickables(world), listed.server.machine)?.written ?? listed.server.machine) : null;
    this.options.confirm.ask({
      title: words.mcp.removeTitle(listed.name),
      what: machine ? words.mcp.removeCommand(machine) : words.mcp.removeRemote,
      go: words.mcp.removeGo,
      act: async () => {
        await this.options.calls.mcp(login.hubName, login.account, { do: "remove", name: listed.name });
      },
      done: () => {
        this.trials.delete(listed.name);
        this.reload();
      },
    });
  }
}

function machineOf(machines: Pickable[], name: string): Pickable | undefined {
  return machines.find((one) => one.name === name || one.written === name);
}

/** This computer first, then the user's own, then those shared; those away
 *  after all that answer. */
function rank(machine: Pickable | undefined): number {
  if (!machine) return 9;
  if (machine.here) return 0;
  const hold = machine.hold === "mine" ? 1 : machine.hold === "shared" ? 2 : 3;
  return machine.online === true ? hold : hold + 4;
}

function osOf(world: WorldNow | null, name: string): string | null {
  const machine = machinesOf(world).find((one) => one.name === name);
  const facts = machine && "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
  return facts?.os ?? null;
}

/** The machines a server a command starts may be kept on: those the user
 *  may run commands on, their own first. */
export function placeable(world: WorldNow | null): Pickable[] {
  return pickables(world)
    .filter((machine) => machine.hold === "mine" || machine.hold === "shared" || machine.hold === "others")
    .sort((a, b) => rank(a) - rank(b));
}
