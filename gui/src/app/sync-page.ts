// Where syncing stands, and why (`gui-sync-status-page`, design §28.9): the
// page one comes to when something has not reached a device, or to learn
// how the account's copies stand. It is made for an account of a few
// devices and for one of hundreds alike.
//
// At the top, this computer and the hub, drawn: a line each way, the way up
// saying what this computer has not sent and the way down what it has yet
// to take; the lines are broken while the two are cut off, which this
// computer knows by itself. Under the hub, what the hub holds.
//
// Below, the account's devices as one list, by how each stands: catching
// up, not reporting, offline, caught up. Each standing is a group with its
// own heads over its rows; a long group shows its first few and folds the
// rest; the standings above the list choose one of them, and a search
// finds a device by name. A row is a device's name, the one or two figures
// that matter for its standing and how far behind it stood through the
// span, small. Opened, it says in a sentence what the device is doing and
// why, then the facts under that, then its own line through the day or
// the week.
//
// The numbers are the hub's, asked again each minute while the page is
// open.

import "../ledger/ledger.css";
import { bytesSaid } from "../bytes";
import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { rule } from "../ledger/marks";
import { loginsOf, type HubId, type Login } from "../world/accounts";
import type { DeviceSync, LagPoint, Part, SyncStatus, SyncStatusReply } from "../world/sync";
import { redrawKeepingFocus } from "./focus";
import { detailOf } from "./problem";
import { followState, type Following } from "../world/topics";
import type { Page } from "./shell";
import type { Store } from "./store";
import { CAUGHT_UP, lagLines, strip, wayLine } from "./sync-draw";
import type { SyncWords } from "./sync-words";
import { parts } from "./usage/draw";
import { waiting } from "./waiting";

export interface SyncPageOptions {
  store: Store;
  words: SyncWords;
  head(title: string, actions?: string): string;
  /** How `account`'s devices stand on `hub` over the last `hours`; with
   *  `machine`, that device's line in full, the others' left out. */
  read(hub: HubId, account: string, hours: number, machine?: string): Promise<SyncStatusReply>;
  /** What a conversation is called here, by its id; none where this
   *  device does not know it. */
  named?(conversation: string): string | null;
  now(): number;
}

type Span = "day" | "week";
const SPAN_KEY = "sync.span";
const HOURS: Record<Span, number> = { day: 24, week: 24 * 7 };
/** How often the page asks again while it is open. */
const AGAIN_MS = 60_000;
/** The inks of what is left, by kind. */
const PART_INKS: Record<Part | "files", string> = {
  conversations: "var(--pen-graphite)",
  output: "var(--pen-sky)",
  other: "var(--pen-sage)",
  files: "var(--pen-rose)",
};

/** How a device stands, in the order the list tells them. */
type Standing = "syncing" | "silent" | "away" | "synced";
const ORDER: Standing[] = ["syncing", "silent", "away", "synced"];
/** The rows a group shows before it folds the rest. */
const FEW = 6;
/** From how many devices on a search is offered. */
const FIND_FROM = 9;

/** What the reader chose of one login's list. */
interface Seen {
  only: Standing | "all";
  query: string;
  /** The groups shown whole. */
  whole: Set<Standing>;
}

function keptSpan(): Span {
  try {
    return localStorage.getItem(SPAN_KEY) === "week" ? "week" : "day";
  } catch {
    return "day";
  }
}

function standingOf(device: DeviceSync): Standing {
  if (device.reported === undefined) return "silent";
  if (!device.online) return "away";
  return device.synced ? "synced" : "syncing";
}

/** The bytes a device has yet to take: its records and its files. */
const toTake = (device: DeviceSync) => (device.left ?? []).reduce((sum, [, bytes]) => sum + bytes, 0) + device.missing_bytes;

type Read = { reply: SyncStatusReply } | { problem: string };

export class SyncPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly body: HTMLElement;
  private span: Span = keptSpan();
  private readonly read = new Map<string, Read>();
  /** The logins asked now: one ask at a time each, whatever its span; and
   *  those to be asked again once it answers, the span having changed. */
  private readonly asking = new Map<string, Span>();
  private readonly again = new Set<string>();
  private readonly seen = new Map<string, Seen>();
  /** The device opened: its login and its name. */
  private opened: string | null = null;
  /** The opened device's own line in full, for the span it was read for:
   *  the list's lines are coarser where an account has many devices. */
  private line: { name: string; span: Span; status: SyncStatus } | null = null;
  /** A device the address asks to be opened, in a development build: for
   *  looking at an opened row without a hand to press it. */
  private wanted = import.meta.env.DEV ? new URLSearchParams(location.search).get("open") : null;
  /** Reads how the devices stand while the page is open and in view; none
   *  once it is left. */
  private following: Following | null = null;
  private readonly resize: ResizeObserver;
  private width = 0;

  constructor(private readonly options: SyncPageOptions) {
    const { words } = options;
    this.title = words.title;
    this.element = document.createElement("div");
    this.element.className = "settings-page sync-page";
    this.element.innerHTML = `${options.head(`<h1 class="page-title">${escape(words.title)}</h1>`)}<div class="sync-body scroller"><div class="lg-app sy" data-coming="page"></div></div>`;
    this.body = this.element.querySelector<HTMLElement>(".sy")!;
    this.element.addEventListener("click", (event) => this.click(event));
    this.element.addEventListener("input", (event) => {
      const field = event.target as HTMLInputElement;
      if (!field.matches(".sy-find input")) return;
      const key = field.closest<HTMLElement>("[data-login]")!.dataset.login!;
      this.seenOf(key).query = field.value;
      this.drawList(key);
    });
    this.element.addEventListener("keydown", (event) => {
      const target = event.target as HTMLElement;
      if ((event.key === "Enter" || event.key === " ") && target.matches("[role='button']")) {
        event.preventDefault();
        target.click();
      }
    });
    this.resize = new ResizeObserver(() => {
      const width = Math.round(this.body.clientWidth);
      if (width === this.width) return;
      this.width = width;
      this.draw();
    });
    this.resize.observe(this.body);
    this.draw();
    // At once, then every minute while the window is in view.
    this.following = followState(null, () => this.ask(), AGAIN_MS);
  }

  leave(): void {
    this.following?.();
    this.following = null;
    this.resize.disconnect();
  }

  private logins(): Login[] {
    return loginsOf(this.options.store.world.peek()?.accounts);
  }

  private seenOf(key: string): Seen {
    let seen = this.seen.get(key);
    if (!seen) this.seen.set(key, (seen = { only: "all", query: "", whole: new Set() }));
    return seen;
  }

  /** Asks how each login's devices stand, all at once, each drawn as it
   *  answers: a slow hub holds up only its own. A login still answering is
   *  not asked again over it: asked meanwhile for another span, it is asked
   *  for that one once it answers, and the answer for a span no longer
   *  chosen is let go. */
  private async ask(): Promise<void> {
    await Promise.all(this.logins().map((login) => this.askOne(login)));
  }

  private async askOne(login: Login): Promise<void> {
    const key = `${login.hub}\n${login.account}`;
    const asked = this.asking.get(key);
    if (asked !== undefined) {
      if (asked !== this.span) this.again.add(key);
      return;
    }
    const span = this.span;
    this.asking.set(key, span);
    let read: Read;
    try {
      read = { reply: await this.options.read(login.hub, login.account, HOURS[span]) };
    } catch (error) {
      read = { problem: detailOf(error) };
    } finally {
      this.asking.delete(key);
    }
    if (this.following === null) return;
    if (span === this.span) {
      this.read.set(key, read);
      if (this.wanted) {
        this.opened = `${key}\n${this.wanted}`;
        this.wanted = null;
      }
      this.draw();
      if (this.opened?.startsWith(`${key}\n`)) void this.readLine(login, this.opened.slice(key.length + 1));
    }
    if (this.again.delete(key) || span !== this.span) await this.askOne(login);
  }

  private click(event: MouseEvent): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-act]");
    if (!target) return;
    const key = target.closest<HTMLElement>("[data-login]")?.dataset.login;
    if (!key) return;
    const seen = this.seenOf(key);
    switch (target.dataset.act) {
      case "only":
        seen.only = target.dataset.value as Seen["only"];
        this.drawTools(key);
        this.drawList(key);
        break;
      case "whole":
        seen.whole.add(target.dataset.value as Standing);
        this.drawList(key);
        break;
      case "open":
        this.open(key, target.dataset.machine!, target.closest<HTMLElement>(".lg-item")!);
        break;
      case "span": {
        const span = target.dataset.value as Span;
        if (span === this.span) return;
        this.span = span;
        try {
          localStorage.setItem(SPAN_KEY, span);
        } catch {
          // Kept for this visit only.
        }
        this.draw();
        void this.ask();
        break;
      }
    }
  }

  /** Opens a device's row, or shuts the one open: what it opens is drawn
   *  as it opens, and the row before it shuts as this one does. */
  private open(key: string, machine: string, item: HTMLElement): void {
    const name = `${key}\n${machine}`;
    const shut = this.opened === name;
    this.body.querySelectorAll(".lg-item.open").forEach((one) => one.classList.remove("open"));
    this.opened = shut ? null : name;
    if (shut) return;
    const login = this.logins().find((one) => `${one.hub}\n${one.account}` === key);
    if (login) void this.readLine(login, machine);
    const read = this.read.get(key);
    const status = read && "reply" in read ? read.reply.hub : undefined;
    const device = status?.devices.find((one) => one.machine === machine);
    if (!status || !device) return;
    item.querySelector<HTMLElement>(".lg-fold-inner")!.innerHTML = this.detail(key, status, device);
    // Its fold is measured shut before it opens, so it is seen to open.
    void item.offsetHeight;
    item.classList.add("open");
  }

  private draw(): void {
    const { words } = this.options;
    const logins = this.logins();
    const html = logins.length
      ? logins
          .map((login) => {
            const key = `${login.hub}\n${login.account}`;
            const read = this.read.get(key);
            const named = logins.length > 1 ? `<h2 class="sy-login-name">${escape(login.account)}　${escape(this.hubName(login.hub))}</h2>` : "";
            const inside = !read
              ? waiting({ label: words.reading, shape: "rows", rows: 3 })
              : "problem" in read
                ? `<p class="feature-note" title="${escape(read.problem)}">${escape(words.failed)}</p>`
                : this.login(key, read.reply, login);
            return `<section class="sy-login" data-login="${escape(key)}">${named}${inside}</section>`;
          })
          .join("")
      : `<p class="feature-note">${escape(words.noneReported)}</p>`;
    const kept = this.body.parentElement!.scrollTop;
    redrawKeepingFocus(this.body, () => {
      this.body.innerHTML = html;
    });
    this.body.parentElement!.scrollTop = kept;
    // Only the first drawing comes up from below; what follows is in place.
    window.setTimeout(() => (this.body.dataset.coming = "quiet"), 600);
  }

  private hubName(hub: HubId): string {
    return this.options.store.world.peek()?.accounts.hubs.find((one) => one.hub === hub)?.name ?? hub;
  }

  /** One login's part: this computer and the hub, then its devices. */
  private login(key: string, reply: SyncStatusReply, login: Login): string {
    const { words } = this.options;
    const status = reply.hub;
    const picture = this.picture(reply, login);
    if (!status) return `${picture}<p class="feature-note sy-unreached"${reply.unreached ? ` title="${escape(reply.unreached)}"` : ""}>${escape(words.unreached)}</p>`;
    if (!status.devices.length) return `${picture}<p class="feature-note">${escape(words.noneReported)}</p>`;
    return `${picture}<div class="sy-tools">${this.tools(key, status)}</div><div class="sy-list">${this.list(key, status, reply.local.machine)}</div>`;
  }

  /** This computer and the hub, a line each way between them. */
  private picture(reply: SyncStatusReply, login: Login): string {
    const { words } = this.options;
    const here = reply.local;
    const status = reply.hub;
    const now = this.options.now();
    const mine = status?.devices.find((device) => device.machine === here.machine);
    const cut = here.cut_off !== undefined || !status;
    const up = here.unread
      ? `<b title="${escape(here.unread)}">${escape(words.failed)}</b>`
      : `<b>${escape(here.unsent_frames ? words.toUpload(here.unsent_frames, bytesSaid(here.unsent_bytes)) : words.nothingUnsent)}</b>`;
    let down: string;
    if (cut) down = `<b>${escape(words.resumes)}</b>`;
    else if (!mine || mine.reported === undefined) down = `<b>${escape(words.neverReported)}</b>`;
    else if (mine.synced) down = `<b>${escape(words.synced)}</b>`;
    else {
      const left = toTake(mine);
      const records = (mine.left ?? []).reduce((sum, [, bytes]) => sum + bytes, 0);
      const eta = records && mine.rate ? `<span>${escape(words.eta(words.duration((records / mine.rate) * 1000)))}</span>` : "";
      down = left ? `<b>${escape(words.catching(`${mine.about ? "≈ " : ""}${bytesSaid(left)}`))}</b>${eta}` : `<b>${escape(words.behindBy(words.duration(mine.behind)))}</b>`;
    }
    const upLine = wayLine(`${login.hub}:up`, cut);
    const downLine = wayLine(`${login.hub}:down`, cut);
    const kind = mine?.kind === "phone" ? "phone" : "desktop";
    const totals = status?.totals;
    const held = totals
      ? `<p class="sy-held" title="${escape(words.hubSaid)}"><span>${escape(words.conversations(totals.conversations))}</span><span>${escape(words.hubFiles(totals.files, bytesSaid(totals.file_bytes)))}</span><span>${escape(words.records(bytesSaid(totals.record_bytes)))}</span></p>`
      : "";
    // Why it is cut off is the link's own word for it, as it stands.
    const off = cut
      ? `<p class="sy-cut"${reply.unreached ? ` title="${escape(reply.unreached)}"` : ""}>${escape(here.cut_off !== undefined ? words.cutOff(words.duration(Math.max(0, now - here.cut_off))) : words.unreached)}</p>${here.why ? `<p class="sy-cut-why">${escape(here.why)}</p>` : ""}`
      : "";
    return `<div class="sy-top"><div class="sy-link${cut ? " cut" : ""}">
      <div class="sy-node"><span class="sy-node-box">${glyph(kind, 28)}</span><b>${escape(here.machine)}</b><small>${escape(words.here)}</small></div>
      <div class="sy-ways">
        <div class="sy-way"><p class="sy-way-said"><span>${escape(words.up)}</span>${up}</p><span class="sy-way-arrow">${upLine.line}${upLine.head}</span></div>
        <div class="sy-way sy-way-down"><span class="sy-way-arrow">${downLine.head}${downLine.line}</span><p class="sy-way-said"><span>${escape(words.down)}</span>${down}</p></div>
        ${off}
      </div>
      <div class="sy-node"><span class="sy-node-box">${glyph("server", 28)}</span><b>${escape(this.hubName(login.hub))}</b><small>${escape(words.hub)}</small></div>
    </div>${held}</div>`;
  }

  /** How many devices, the standings to choose among with how many stand
   *  so, and the search. */
  private tools(key: string, status: SyncStatus): string {
    const { words } = this.options;
    const seen = this.seenOf(key);
    // A few devices are all in sight under their groups' heads.
    if (status.devices.length < FIND_FROM) return "";
    const count = (standing: Standing) => status.devices.filter((device) => standingOf(device) === standing).length;
    const choices: [Seen["only"], string, number][] = [["all", words.standings.all, status.devices.length], ...ORDER.map((standing): [Standing, string, number] => [standing, words.standings[standing], count(standing)]).filter(([, , n]) => n > 0)];
    // One standing alone needs no choosing.
    const only = choices.length > 2 ? `<div class="choice-options">${choices.map(([value, word, n]) => `<button type="button" class="choice${seen.only === value ? " chosen" : ""}" data-act="only" data-value="${value}">${escape(word)}<small>${n}</small></button>`).join("")}</div>` : `<span class="sy-count">${escape(words.devices(status.devices.length))}</span>`;
    return `${only}<label class="sy-find">${glyph("search", 16)}<input type="search" placeholder="${escape(words.find)}" value="${escape(seen.query)}" autocomplete="off" spellcheck="false" data-keep="${escape(key)}"></label>`;
  }

  private drawTools(key: string): void {
    const section = this.sectionOf(key);
    const read = this.read.get(key);
    const tools = section?.querySelector<HTMLElement>(".sy-tools");
    if (!tools || !read || !("reply" in read) || !read.reply.hub) return;
    const status = read.reply.hub;
    redrawKeepingFocus(tools, () => {
      tools.innerHTML = this.tools(key, status);
    });
  }

  private drawList(key: string): void {
    const section = this.sectionOf(key);
    const read = this.read.get(key);
    const list = section?.querySelector<HTMLElement>(".sy-list");
    if (!list || !read || !("reply" in read) || !read.reply.hub) return;
    list.innerHTML = this.list(key, read.reply.hub, read.reply.local.machine);
  }

  private sectionOf(key: string): HTMLElement | null {
    return Array.from(this.body.querySelectorAll<HTMLElement>("[data-login]")).find((one) => one.dataset.login === key) ?? null;
  }

  /** The devices, a group for each standing that has any, each under its
   *  own heads. */
  private list(key: string, status: SyncStatus, here: string): string {
    const { words } = this.options;
    const seen = this.seenOf(key);
    const now = this.options.now();
    const query = seen.query.trim().toLowerCase();
    const from = status.at - status.hours * 3_600_000;
    const points = new Map<string, LagPoint[]>();
    for (const point of status.points) {
      const of = points.get(point.machine);
      if (of) of.push(point);
      else points.set(point.machine, [point]);
    }
    // One scale for every small line, so they read against each other.
    const ceiling = Math.max(10 * CAUGHT_UP, ...status.points.filter((point) => point.online).map((point) => point.behind));
    const found = status.devices.filter((device) => !query || device.machine.toLowerCase().includes(query));
    const groups = ORDER.filter((standing) => seen.only === "all" || seen.only === standing)
      .map((standing) => {
        const of = found.filter((device) => standingOf(device) === standing).sort((a, b) => this.before(a, b, standing, here));
        if (!of.length) return "";
        // A group chosen alone, or searched in, is shown whole.
        const whole = seen.only !== "all" || !!query || seen.whole.has(standing) || of.length <= FEW + 1;
        const shown = whole ? of : of.slice(0, FEW);
        const heads = this.headsOf(standing);
        const rest = whole ? "" : `<button type="button" class="lg-word sy-rest" data-act="whole" data-value="${standing}">${escape(words.rest(of.length - shown.length))}</button>`;
        return `<section class="lg-family sy-group">
          <div class="lg-family-head lg-cols"><h2${standing === "silent" ? ` title="${escape(words.silentSaid)}"` : ""}><span>${escape(words.standings[standing])}</span><small>${of.length}</small></h2><span>${escape(heads[0])}</span><span>${escape(heads[1])}</span><span class="sy-recent">${standing === "silent" ? "" : escape(words.heads.recent(this.span))}</span><span></span></div>
          ${rule(((ORDER.indexOf(standing) % 3) + 1) as 1 | 2 | 3)}
          ${shown.map((device) => this.row(key, status, device, standing, device.machine === here, points.get(device.machine) ?? [], from, ceiling, now)).join("")}
          ${rest}
        </section>`;
      })
      .join("");
    return groups || `<p class="lg-none">${escape(words.noMatch)}</p>`;
  }

  /** What stands over a standing's two figures. */
  private headsOf(standing: Standing): [string, string] {
    const { heads } = this.options.words;
    return standing === "syncing" ? [heads.left, heads.behind] : standing === "away" ? [heads.away, heads.onReturn] : ["", ""];
  }

  /** Which of two devices of one standing comes first: this computer, then
   *  the one with most to take, or longest away, then by name. */
  private before(a: DeviceSync, b: DeviceSync, standing: Standing, here: string): number {
    if ((a.machine === here) !== (b.machine === here)) return a.machine === here ? -1 : 1;
    if (standing === "syncing") return toTake(b) - toTake(a) || b.behind - a.behind || a.machine.localeCompare(b.machine);
    if (standing === "away") return (b.since ?? 0) - (a.since ?? 0) || a.machine.localeCompare(b.machine);
    return a.machine.localeCompare(b.machine);
  }

  private row(key: string, status: SyncStatus, device: DeviceSync, standing: Standing, here: boolean, points: LagPoint[], from: number, ceiling: number, now: number): string {
    const { words } = this.options;
    const left = toTake(device);
    const size = left ? `${device.about ? "≈ " : ""}${bytesSaid(left)}` : "";
    let first = "";
    let second = "";
    if (standing === "syncing") {
      first = size || (device.missing_objects ? words.fileCount(device.missing_objects, bytesSaid(device.missing_bytes)) : "");
      second = device.behind >= CAUGHT_UP ? words.duration(device.behind) : "";
    } else if (standing === "away") {
      first = words.duration(Math.max(0, now - (device.since ?? device.reported ?? now)));
      // Nothing left is said only where nothing is known to be left (I04, J04).
      const unknown = !!device.about || device.unpulled > 0 || (!device.synced && !device.missing_objects);
      second = size || (unknown ? "–" : words.nothingMissed);
    } else if (standing === "synced" && device.unsent_frames) {
      first = words.toUpload(device.unsent_frames, bytesSaid(device.unsent_bytes));
    }
    const kind = device.kind === "phone" ? "phone" : device.kind === "computer" ? "desktop" : "machine";
    const name = `${key}\n${device.machine}`;
    const open = this.opened === name;
    return `<div class="lg-item${open ? " open" : ""}">
      <div class="lg-row lg-cols" role="button" tabindex="0" aria-expanded="${open}" data-act="open" data-machine="${escape(device.machine)}">
        <span class="sy-name">${glyph(kind, 18)}<b>${escape(device.machine)}</b>${here ? `<small>${escape(words.thisOne)}</small>` : ""}</span>
        <span class="sy-first">${escape(first)}</span>
        <span class="sy-second">${escape(second)}</span>
        <span class="sy-recent">${standing === "silent" ? "" : strip(points, from, status.at, ceiling, device.machine)}</span>
        <span class="lg-chevron">${glyph("chevron", 12)}</span>
      </div>
      <div class="lg-fold"><div class="lg-fold-inner">${open ? this.detail(key, status, device) : ""}</div></div>
    </div>`;
  }

  /** What a scope is, by its key (`fastctx_types::assets::Readers::key_prefix`):
   *  a conversation by what it is called here, else by its id; an account's
   *  records; a shared machine's; a project's; a working copy. */
  private scopeSaid(key: string): string {
    const { scope } = this.options.words;
    const parts = key.split("/");
    switch (parts[0]) {
      case "c": {
        const id = parts[parts.length - 1] ?? key;
        return scope.talk(this.options.named?.(id) ?? id);
      }
      case "a":
        return scope.records;
      case "b":
        return scope.shared;
      case "p":
        return scope.project(parts[1] ?? "");
      case "w":
        return scope.working;
      default:
        return scope.hub;
    }
  }

  /** Reads the opened device's own line in full, and draws it in where the
   *  device is still the one opened. */
  private async readLine(login: Login, machine: string): Promise<void> {
    const key = `${login.hub}\n${login.account}`;
    const name = `${key}\n${machine}`;
    const span = this.span;
    let status: SyncStatus | undefined;
    try {
      status = (await this.options.read(login.hub, login.account, HOURS[span], machine)).hub;
    } catch {
      return;
    }
    if (!status || this.following === null || this.opened !== name || this.span !== span) return;
    this.line = { name, span, status };
    const read = this.read.get(key);
    const listed = read && "reply" in read ? read.reply.hub : undefined;
    const device = listed?.devices.find((one) => one.machine === machine);
    const item = this.body.querySelector<HTMLElement>(`.lg-item.open [data-machine="${CSS.escape(machine)}"]`)?.closest<HTMLElement>(".lg-item");
    const chart = item?.querySelector<HTMLElement>(".sy-line");
    if (!listed || !device || !chart) return;
    const drawn = document.createElement("div");
    drawn.innerHTML = this.detail(key, listed, device);
    const fresh = drawn.querySelector<HTMLElement>(".sy-line");
    if (fresh) chart.replaceWith(fresh);
  }

  /** A device opened: what it is doing and why, the facts, its own line. */
  private detail(key: string, status: SyncStatus, device: DeviceSync): string {
    const { words } = this.options;
    const now = this.options.now();
    const standing = standingOf(device);
    const records = (device.left ?? []).reduce((sum, [, bytes]) => sum + bytes, 0);
    const left = records + device.missing_bytes;
    const size = `${device.about ? "≈ " : ""}${bytesSaid(left)}`;
    const since = words.duration(Math.max(0, now - (device.since ?? device.reported ?? now)));
    let why: string;
    if (standing === "silent") why = words.whySilent;
    else if (standing === "synced") why = words.whySynced(words.ago(words.duration(Math.max(0, now - (device.reported ?? now)))));
    else if (standing === "away") {
      const unknown = !!device.about || device.unpulled > 0 || (!device.synced && !device.missing_objects);
      why = left ? words.away(since, size) : unknown ? words.awayUnknown(since) : words.away(since, null);
    } else if (!records && device.behind < CAUGHT_UP && !device.unpulled && device.missing_objects) why = words.whyFiles(device.missing_objects, bytesSaid(device.missing_bytes));
    else if (left) why = words.whySyncing(size, records && device.rate ? words.eta(words.duration((records / device.rate) * 1000)) : null);
    else why = words.whyBehind(words.duration(device.behind));
    const shares = [
      ...(device.left ?? []).map(([part, bytes]) => ({ value: bytes, ink: PART_INKS[part], said: `${words.parts[part]} ${bytesSaid(bytes)}`, name: words.parts[part], size: bytesSaid(bytes) })),
      ...(device.missing_bytes ? [{ value: device.missing_bytes, ink: PART_INKS.files, said: `${words.filesPart} ${bytesSaid(device.missing_bytes)}`, name: words.filesPart, size: words.fileCount(device.missing_objects, bytesSaid(device.missing_bytes)) }] : []),
    ].filter((share) => share.value > 0);
    const kinds = shares.length
      ? `<div class="sy-kinds">${shares.length > 1 ? parts(shares, `sync:${device.machine}`) : ""}<ul>${shares.map((share) => `<li><i style="background:${share.ink}" aria-hidden="true"></i><span>${escape(share.name)}</span><b>${escape(share.size)}</b></li>`).join("")}</ul></div>`
      : "";
    const facts: [string, string][] = [];
    if (device.missing_objects && !device.missing_bytes) facts.push([words.facts.files, String(device.missing_objects)]);
    if (device.unpulled) {
      // Which they are, where the hub tells: the first few by name.
      const named = (device.unpulled_scopes ?? []).slice(0, 3).map((scope) => this.scopeSaid(scope));
      const rest = device.unpulled - named.length;
      facts.push([words.facts.unpulled, named.length ? `${named.join("、")}${rest > 0 ? ` ${words.more(rest)}` : ""}` : words.scopes(device.unpulled)]);
    }
    if (device.unsent_frames) facts.push([words.facts.upload, words.frames(device.unsent_frames, bytesSaid(device.unsent_bytes))]);
    if (device.rate) facts.push([words.facts.rate, words.perSecond(bytesSaid(device.rate))]);
    if (device.reported !== undefined) {
      if (device.since !== undefined) facts.push([device.online ? words.facts.online : words.facts.offline, since]);
      if (standing !== "synced") facts.push([words.facts.reported, words.ago(words.duration(Math.max(0, now - device.reported)))]);
    }
    const figures = facts.length ? `<dl class="sy-facts">${facts.map(([name, value]) => `<div><dt>${escape(name)}</dt><dd>${escape(value)}</dd></div>`).join("")}</dl>` : "";
    // Its own line in full where it was read; else the list's.
    const own = this.line?.name === `${key}\n${device.machine}` && this.line.span === this.span ? this.line.status : status;
    const points = own.points.filter((point) => point.machine === device.machine);
    const width = Math.max(280, Math.min(this.width || 900, 1120) - 48);
    const line = points.length
      ? lagLines({ width, height: 180, from: own.at - own.hours * 3_600_000, to: own.at, step: own.step, span: this.span, lines: [{ machine: device.machine, name: device.machine, ink: "var(--pen-graphite)", points }], words, seed: `${key}:${device.machine}` })
      : `<p class="lg-none">${escape(words.lineEmpty)}</p>`;
    const spans = `<div class="ledger-switch" role="radiogroup" aria-label="${escape(words.spanLabel)}">${(["day", "week"] as const).map((span) => `<button type="button" role="radio" aria-checked="${span === this.span}" class="${span === this.span ? "current" : ""}" data-act="span" data-value="${span}">${escape(words.spans[span])}</button>`).join("")}</div>`;
    const chart = standing === "silent" ? "" : `<div class="sy-line"><div class="sy-line-head"><h3 title="${escape(words.lineSaid)}">${escape(words.lineTitle)}</h3>${spans}</div>${line}</div>`;
    return `<div class="sy-detail"><p class="sy-why">${escape(why)}</p>${kinds}${figures}${chart}</div>`;
  }
}
