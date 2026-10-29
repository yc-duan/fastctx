// A machine's own page (`world-gui.md`, 机器的详情): what it is on one line
// under its head, then three tabs on one solid sheet. 「状况」 is how it is
// doing now, drawn — how busy its processors and graphics cards are, how
// full its memory is — each disk, and who is at work on it; 「网络」 where it
// stands in each network, how its lines to the hubs held over a day, a
// week or a month, its cards and direct links; 「管理」 what can be done to
// it: the Fastctx on it, its name and group, whom it is shared with, the
// facts of it, and taking it off. Its name is changed where it is written.
// Only a machine the account may work on has a page: one whose agent only
// assists stays on the drawing (`world-network.md`).
//
// While the page is in view it asks the machine every two seconds, and
// nothing while it is not; the machine sends its lines' history whole once,
// then only what came after (`machine-history.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, names, type AccountChange, type HubAccounts, type Login, type ShareSeen } from "../world/accounts";
import type { NameProblem, WorldNow } from "../world/bridge";
import { agentName, osName, titleOf } from "../world/names";
import type { Card, LimitUse, MachineStats, StatsSeen } from "../world/stats";
import type { MachineFacts } from "../world/world";
import type { AccessWords, MachinesWords } from "./account-words";
import { AssistSheet, assistChange, assistancesIn, namesOn, type Assistance } from "./assist-sheet";
import type { Confirm } from "./confirm";
import { bytesSaid } from "./data-page";
import { redrawKeepingFocus } from "./focus";
import { diskBar, levelOf, Ring, Spark, Strip, type LineMark, type LineSpan } from "./machine-draw";
import { daysOf, heldOf, keptOf, LIMITS_MS, seenOf, SPAN_MS, take, tripsHeld, type Sample } from "./machine-history";
import type { MachinePageWords } from "./machine-page-words";
import { FastctxBlock, type FastctxSubject } from "./machine-fastctx";
import type { ReadLog } from "./machine-log";
import { holderLoginOf, removerLoginOf, updaterLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import { RemoveMachineSheet } from "./machine-remove";
import { removeMachines } from "./machine-removing";
import { GiveSheet } from "./machine-sheets";
import { linkSaid, linkWhy } from "./machine-spot";
import { formerNames, mapOf } from "./map-of";
import type { NameSaid } from "./name-words";
import { isManaged } from "./machine-list";
import { link, talkLink, type Place } from "./places";
import { detailOf } from "./problem";
import { ActsMenu } from "./acts-menu";
import { runsHub, holds, assistantsOn, groupsOf, inNoNetwork, isSuper, isVisitor, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameLogin, type Machine } from "./reach";
import { RenameField, renamerOf, type Renamer } from "./rename";
import type { Page } from "./shell";
import { effect, type Signal } from "./signals";
import { newestKnown, type UpdateCall, type UpdateState } from "../world/updates";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdateWords } from "./update-words";
import { jobsOf, residentsOf, type Store } from "./store";

export interface MachinePageOptions {
  store: Store;
  words: MachinePageWords;
  machines: MachinesWords;
  access: AccessWords;
  nameSaid: NameSaid;
  head(title: string, actions?: string): string;
  /** The machine, as the address names it. */
  name: string;
  /** When, as the front page says it: a clock time today, a day before. */
  when(at: Date, now: Date): string;
  /** How long ago, as the front page says it. */
  ago(ms: number): string;
  stats(machine: string | null, after: string | null, seen: StatsSeen | null): Promise<MachineStats>;
  nameProblem(text: string): Promise<NameProblem | null>;
  change(login: Login, change: AccountChange): Promise<void>;
  /** A hub's log, to follow the machine told to uninstall as it is taken off. */
  log?: ReadLog;
  renameHere(to: string): Promise<void>;
  /** Opens a new conversation with an agent on `machine`, as written. */
  talk(machine: string): void;
  confirm: Confirm;
  /** Goes to `place` in this page's stead: the machine renamed, removed, or
   *  one with no page of its own. */
  replace(place: Place): void;
  /** What this device knows of versions and the network's machines, and
   *  asking another machine about its updates. */
  updates?:
    | {
        state: Signal<UpdateState | null>;
        /** A machine of an earlier release updates to its newest. */
        machine(name: string): Promise<UpdateState>;
        remote(name: string, call: UpdateCall): Promise<UpdateState>;
        /** This device's own update card. */
        here(): void;
        words: UpdateWords;
        view: UpdateViewWords;
      }
    | undefined;
  manage: ManageWords;
  /** On a Mac, where Fastctx is let into what macOS keeps private. */
  allowPrivacy?: () => void;
  /** What the machine (this computer with none) keeps free in memory for
   *  its agents; set first when `bytes` is given. */
  memoryRoom?(machine: string | null, bytes: number | null): Promise<number>;
}

/** What a machine may keep free for its agents (`fastctx_types::stats`). */
const ROOMS = [0, 2 ** 30, 1.5 * 2 ** 30, 2 * 2 ** 30, 3 * 2 ** 30];

type Tab = "now" | "net" | "manage";
const TABS: Tab[] = ["now", "net", "manage"];

type Range = "day" | "week" | "month";
const RANGES: Record<Range, number> = { day: 86_400_000, week: 7 * 86_400_000, month: 30 * 86_400_000 };
/** Where this device remembers the span the lines are drawn over. */
const RANGE_KEY = "machine.range";

/** How often the page asks the machine while it is in view: as often as
 *  the World itself is asked. */
const EVERY_MS = 2_000;
/** A trace breaks where readings are further apart than a few askings. */
const READ_GAP_MS = 4.5 * EVERY_MS;
/** A machine reads its limits once a minute; a few missed break the trace. */
const LIMIT_GAP_MS = 3.5 * 60_000;
/** A line's round trip is measured each heartbeat, every ten seconds. */
const TRIP_GAP_MS = 35_000;
/** How often the lines' strips move on with the clock; a change on a line
 *  draws its strip at once. */
const STRIP_MS = 30_000;
/** How long a taking back can be undone. */
const UNDO_MS = 6_000;
/** A machine just renamed may not be on the hub's list under its new name
 *  yet: only past this is one not found said to be gone. */
const SETTLE_MS = 4_000;
/** As narrow as a phone (`machine-page.css`): what the identity and the
 *  vitals leave out there is said on a long press. */
const NARROW_PX = 560;

interface Vital {
  kind: "cpu" | "memory" | "gpu";
  element: HTMLElement;
  note: HTMLElement;
  sub: HTMLElement;
  ring: Ring;
  spark: Spark;
}

interface LimitRow {
  element: HTMLElement;
  figure: HTMLElement;
  spark: Spark;
}

interface LineView {
  element: HTMLElement;
  hub: HTMLElement;
  state: HTMLElement;
  trip: HTMLElement;
  held: HTMLElement;
  axis: HTMLElement;
  strip: Strip;
  spark: Spark;
  /** What its strip was last drawn from. */
  drawn: string;
}

export class MachinePage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly heading: HTMLElement;
  private readonly acts: HTMLElement;
  private readonly board: HTMLElement;
  private readonly id: HTMLElement;
  private readonly note: HTMLElement;
  private readonly tabsBox: HTMLElement;
  private readonly sheet: HTMLElement;
  private readonly panes: Record<Tab, HTMLElement>;
  private readonly privacyBox: HTMLElement;
  /** The vitals' part of 「状况」, and the grid they stand in. */
  private readonly vitalsSec: HTMLElement;
  private readonly vitalsBox: HTMLElement;
  private readonly disksBox: HTMLElement;
  /** How far it is toward its system's limits: each limit's row by its
   *  kind, its trace kept from one reading to the next. */
  private readonly limitsBox: HTMLElement;
  private limitRows = new Map<string, LimitRow>();
  private readonly presentBox: HTMLElement;
  private readonly whereBox: HTMLElement;
  private readonly linesBox: HTMLElement;
  private readonly cardsBox: HTMLElement;
  private readonly directBox: HTMLElement;
  private readonly fastctxBox: HTMLElement;
  private readonly namingBox: HTMLElement;
  private readonly roomBox: HTMLElement;
  private roomMenu: ActsMenu | null = null;
  private readonly sharingBox: HTMLElement;
  private readonly factsBox: HTMLElement;
  private readonly dangerBox: HTMLElement;
  private readonly undoBar: HTMLElement;
  private readonly giving: GiveSheet;
  private readonly assistSheet: AssistSheet;
  private readonly removing: RemoveMachineSheet;
  private readonly fastctx: FastctxBlock | null;
  private readonly stop: () => void;
  private readonly born = Date.now();
  private machine: Machine | null = null;
  /** The tab shown; none until the machine is known, which decides it. */
  private tab: Tab | null = null;
  private vitals: Vital[] = [];
  private vitalsShape = "";
  private lines: LineView[] = [];
  private linesShape: string | null = null;
  private cardsShape = "";
  /** The trace of what the card it follows receives and sends. */
  private flow: Spark | null = null;
  private range: Range = keptRange();
  private timer = 0;
  private readonly moving: number;
  private asking = false;
  private left = false;
  /** Why the machine could not be asked the last time. */
  private problem: string | null = null;
  private renaming: RenameField | null = null;
  private undoing: { timer: number; redo: (() => Promise<void>) | null } | null = null;
  /** Each part's markup as last drawn, so an unchanged one is left alone. */
  private readonly drawn = new Map<string, string>();
  private narrow = false;
  private readonly sized = new ResizeObserver(() => {
    const narrow = this.element.clientWidth <= NARROW_PX;
    if (narrow === this.narrow) return;
    this.narrow = narrow;
    this.render();
  });

  constructor(private readonly options: MachinePageOptions) {
    this.title = options.name;
    this.giving = new GiveSheet(options.access);
    this.assistSheet = new AssistSheet(options.access);
    this.removing = new RemoveMachineSheet(options.manage);
    const updates = options.updates;
    this.fastctx = updates
      ? new FastctxBlock({
          words: options.manage,
          view: updates.view,
          local: updates.state,
          remote: updates.remote,
          older: updates.machine,
          here: updates.here,
          kept: (service) => (service ? options.words.service : options.words.noService),
          changed: () => this.render(),
        })
      : null;
    this.element = document.createElement("div");
    this.element.className = "machine-page";
    const { words } = options;
    const tab = (one: Tab) =>
      `<button class="mp-tab" type="button" role="tab" id="mp-tab-${one}" aria-controls="mp-pane-${one}" aria-selected="false" tabindex="-1" data-tab="${one}">${escape(words.tabs[one])}</button>`;
    this.element.innerHTML = `${options.head(`<div class="mp-heading"></div>`, `<div class="mp-acts"></div>`)}
      <div class="mp-body"><div class="mp-board">
        <section class="mp-id"></section>
        <p class="mp-note" hidden></p>
        <div class="mp-tabs" role="tablist" aria-label="${escape(words.tabsLabel)}" hidden>${TABS.map(tab).join("")}</div>
        <div class="mp-sheet scroller" hidden>
          <div class="mp-pane" id="mp-pane-now" role="tabpanel" aria-labelledby="mp-tab-now" data-pane="now" hidden>
            <section class="mp-sec mp-privacy" hidden></section>
            <section class="mp-sec mp-vitals-sec" hidden><div class="mp-vitals"></div></section>
            <section class="mp-sec mp-disks" hidden></section>
            <section class="mp-sec mp-limits" hidden></section>
            <section class="mp-sec mp-present" hidden></section>
          </div>
          <div class="mp-pane" id="mp-pane-net" role="tabpanel" aria-labelledby="mp-tab-net" data-pane="net" hidden>
            <section class="mp-sec mp-where" hidden></section>
            <section class="mp-sec mp-lines" hidden></section>
            <section class="mp-sec mp-cards" hidden></section>
            <section class="mp-sec mp-direct" hidden></section>
          </div>
          <div class="mp-pane" id="mp-pane-manage" role="tabpanel" aria-labelledby="mp-tab-manage" data-pane="manage" hidden>
            <section class="mp-sec mp-fastctx" hidden></section>
            <section class="mp-sec mp-naming" hidden></section>
            <section class="mp-sec mp-room" hidden></section>
            <section class="mp-sec mp-sharing" hidden></section>
            <section class="mp-sec mp-about" hidden></section>
            <section class="mp-sec mp-danger" hidden></section>
          </div>
        </div>
      </div></div>
      <div class="mp-undo" role="status" hidden></div>`;
    const part = (selector: string) => this.element.querySelector<HTMLElement>(selector)!;
    this.heading = part(".mp-heading");
    this.acts = part(".mp-acts");
    this.board = part(".mp-board");
    this.id = part(".mp-id");
    this.note = part(".mp-note");
    this.tabsBox = part(".mp-tabs");
    this.sheet = part(".mp-sheet");
    this.panes = { now: part("[data-pane=now]"), net: part("[data-pane=net]"), manage: part("[data-pane=manage]") };
    this.privacyBox = part(".mp-privacy");
    this.vitalsSec = part(".mp-vitals-sec");
    this.vitalsBox = part(".mp-vitals");
    this.disksBox = part(".mp-disks");
    this.limitsBox = part(".mp-limits");
    this.presentBox = part(".mp-present");
    this.whereBox = part(".mp-where");
    this.linesBox = part(".mp-lines");
    this.cardsBox = part(".mp-cards");
    this.directBox = part(".mp-direct");
    this.fastctxBox = part(".mp-fastctx");
    this.namingBox = part(".mp-naming");
    this.roomBox = part(".mp-room");
    this.sharingBox = part(".mp-sharing");
    this.factsBox = part(".mp-about");
    this.dangerBox = part(".mp-danger");
    this.undoBar = part(".mp-undo");
    this.heading.innerHTML = `<h1 class="page-title mp-name">${escape(options.name)}</h1>`;
    this.element.addEventListener("click", (event) => this.click(event));
    this.tabsBox.addEventListener("keydown", (event) => this.tabKey(event));
    document.addEventListener("visibilitychange", this.visible);
    this.sized.observe(this.element);
    // What this device knows of releases and of the network's machines can
    // come after the World: the page is drawn again when it does.
    const stopWorld = effect(() => this.draw(options.store.world.get()));
    const stopUpdates = updates
      ? effect(() => {
          updates.state.get();
          this.render();
        })
      : () => {};
    this.stop = () => {
      stopWorld();
      stopUpdates();
    };
    this.moving = window.setInterval(() => {
      if (document.visibilityState === "visible") this.drawLines();
    }, STRIP_MS);
    void this.ask();
  }

  leave(): void {
    this.left = true;
    this.stop();
    clearTimeout(this.timer);
    clearInterval(this.moving);
    if (this.undoing) clearTimeout(this.undoing.timer);
    document.removeEventListener("visibilitychange", this.visible);
    this.sized.disconnect();
    this.renaming?.dispose();
    this.fastctx?.leave();
    for (const vital of this.vitals) {
      vital.ring.dispose();
      vital.spark.dispose();
    }
    for (const row of this.limitRows.values()) row.spark.dispose();
    this.flow?.dispose();
    for (const line of this.lines) {
      line.strip.dispose();
      line.spark.dispose();
    }
  }

  private readonly visible = (): void => {
    if (document.visibilityState === "visible") void this.ask();
  };

  /** Asks the machine how it is doing, and again two seconds after it was
   *  asked, while the page is in view. One that cannot be asked now, being
   *  offline or of an older fastctx, is looked at again as often: the World
   *  says when it can. */
  private async ask(): Promise<void> {
    clearTimeout(this.timer);
    if (this.left || this.asking || document.visibilityState !== "visible") return;
    const machine = this.machine;
    if (!machine || !askable(machine)) {
      this.timer = window.setTimeout(() => void this.ask(), EVERY_MS);
      return;
    }
    this.asking = true;
    const began = Date.now();
    const kept = keptOf(machine.name);
    try {
      const asked = ++kept.asked;
      // Asked by how it is written: of two hubs' machines of one name, this one.
      const stats = await this.options.stats(machine.here ? null : machine.written, kept.last?.at ?? null, seenOf(kept.last));
      take(machine.name, stats, !!kept.last, asked);
      this.problem = null;
    } catch (error) {
      this.problem = detailOf(error);
    } finally {
      this.asking = false;
    }
    if (this.left) return;
    this.render();
    if (document.visibilityState === "visible") this.timer = window.setTimeout(() => void this.ask(), Math.max(0, EVERY_MS - (Date.now() - began)));
  }

  /** The machine the address names, as this device reaches it. A name it
   *  had leads to it as it is called now; the machine of another whose
   *  agent only assists has no page. */
  private found(world: WorldNow): { machine: Machine | null; now: string | null; assist: boolean } {
    const name = this.options.name;
    const machines = machinesOf(world);
    const named = machines.find((one) => one.written === name) ?? machines.find((one) => one.name === name);
    if (named) return { machine: named, now: null, assist: false };
    for (const hub of world.accounts.hubs) {
      const member = hub.members.find((one) => one.kind === "computer" && names(one, name));
      const machine = member ? machines.find((one) => one.name === member.name && one.on.some((on) => on.hub === hub.hub)) : undefined;
      if (machine) return { machine, now: machine.written, assist: false };
    }
    const here = machines.find((one) => one.here);
    if (here && (world.accounts.was ?? []).includes(name)) return { machine: here, now: here.written, assist: false };
    return { machine: null, now: null, assist: assistantsOn(world, name).length > 0 };
  }

  private draw(world: WorldNow | null): void {
    if (!world) return;
    // A computer in no network has no details to show, and is not watched:
    // the machines page tells what a network is.
    if (inNoNetwork(world)) return this.options.replace({ kind: "machines" });
    const { machine, now, assist } = this.found(world);
    if (now) return this.options.replace({ kind: "machine", name: now });
    if (assist) return this.options.replace({ kind: "machines", focus: this.options.name });
    const first = !this.machine && !!machine;
    this.machine = machine;
    this.render();
    if (first) void this.ask();
  }

  /** Draws every part from the World and what the machine last said; each
   *  part is touched only where it changed. */
  private render(): void {
    const world = this.options.store.world.peek();
    if (!world || this.left) return;
    const machine = this.machine;
    this.drawHeading(machine, world);
    this.drawActs(machine, world);
    if (!machine) {
      const gone = Date.now() - this.born > SETTLE_MS;
      this.note.hidden = !gone;
      this.note.textContent = this.options.words.gone;
      this.id.hidden = true;
      this.tabsBox.hidden = true;
      this.sheet.hidden = true;
      this.fastctx?.show(null, false);
      return;
    }
    const kept = keptOf(machine.name);
    const stats = kept.last;
    this.id.hidden = false;
    this.tabsBox.hidden = false;
    this.sheet.hidden = false;
    // A machine too old to say how it is doing opens on what can be done to
    // it: its update is then before the user's eyes.
    this.tab ??= askable(machine) || machine.online === false ? "now" : "manage";
    this.drawTabs();
    this.drawIdentity(machine, stats);
    this.drawNote(machine, stats, kept.heard);
    this.vitalsSec.hidden = !stats;
    if (stats) {
      this.drawVitals(stats, kept.samples);
      this.drawCards(stats, kept.samples);
      this.drawDisks(stats);
      this.drawLimits(stats);
      this.drawLines();
    } else {
      this.cardsBox.hidden = true;
      this.disksBox.hidden = true;
      this.limitsBox.hidden = true;
      this.linesBox.hidden = true;
    }
    this.drawPrivacy(machine, stats);
    this.drawPresent(machine, stats, world);
    this.drawWhere(machine, world);
    this.drawDirect(machine, world);
    this.drawFastctx(machine, stats, world);
    this.drawNaming(machine, world);
    this.drawRoom(machine, stats, world);
    this.drawSharing(machine, world);
    this.drawFacts(machine, stats);
    this.drawDanger(machine, world);
  }

  /** The three tabs, the one shown on the sheet as its own top. */
  private drawTabs(): void {
    const shown = this.tab ?? "now";
    for (const button of this.tabsBox.querySelectorAll<HTMLButtonElement>("[data-tab]")) {
      const on = button.dataset.tab === shown;
      button.setAttribute("aria-selected", String(on));
      button.tabIndex = on ? 0 : -1;
    }
    for (const one of TABS) this.panes[one].hidden = one !== shown;
    // The sheet's corner under the first tab is the tab's own.
    this.sheet.classList.toggle("first", shown === TABS[0]);
  }

  private choose(tab: Tab, focus: boolean): void {
    if (this.tab === tab) return;
    this.tab = tab;
    this.drawTabs();
    this.sheet.scrollTop = 0;
    this.render();
    if (focus) this.tabsBox.querySelector<HTMLElement>(`[data-tab=${tab}]`)?.focus();
  }

  /** The left and right keys go from tab to tab, as Home and End go to the
   *  first and the last. */
  private tabKey(event: KeyboardEvent): void {
    const at = TABS.indexOf(this.tab ?? "now");
    const to =
      event.key === "ArrowRight" ? (at + 1) % TABS.length : event.key === "ArrowLeft" ? (at + TABS.length - 1) % TABS.length : event.key === "Home" ? 0 : event.key === "End" ? TABS.length - 1 : -1;
    if (to === -1) return;
    event.preventDefault();
    event.stopPropagation();
    this.choose(TABS[to]!, true);
  }

  /** Replaces `box`'s markup when it changed. */
  private put(key: string, box: HTMLElement, html: string): void {
    if (this.drawn.get(key) === html) return;
    this.drawn.set(key, html);
    redrawKeepingFocus(box, () => {
      box.innerHTML = html;
    });
  }

  /** The name, a dot for whether it is online, the way to rename it, the
   *  names it had, and whose it is. */
  private drawHeading(machine: Machine | null, world: WorldNow): void {
    if (this.renaming) return;
    const { words, machines } = this.options;
    if (!machine) return this.put("heading", this.heading, `<h1 class="page-title mp-name">${escape(this.options.name)}</h1>`);
    const logins = loginsOf(world.accounts);
    const owner = logins.find((login) => owns(machine, login)) ?? null;
    const mine = machine.here || !!owner;
    const ownerName = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
    const viewer = logins.find((login) => machine.on.some((on) => on.hub === login.hub)) ?? null;
    const shared = !!viewer && !mine && sharedWith(world, viewer, machine.name).includes(viewer.account);
    const bySuper = !!viewer && !mine && !shared && isSuper(world, viewer);
    const reaching = !mine && logins.some((login) => machineActsFor(world, machine.name, login));
    const tags = [
      machine.here ? tag(machines.here) : "",
      owner && !machine.here ? tag(machines.yours, "tag-own") : "",
      ownerName ? tag(bySuper ? machines.superInShort(ownerName) : machines.sharedInShort(ownerName), "", bySuper ? machines.superIn(ownerName) : machines.sharedIn(ownerName)) : "",
      reaching ? tag(machines.reaching, "tag-reach", machines.reachingWhat) : "",
      isManaged(world, machine.name) ? tag(machines.managed, "", machines.managedWhat) : "",
    ].join("");
    const state = this.stateOf(machine);
    const former = formerNames(world, machine);
    const late = machine.here ? world.accounts.hubs.filter((hub) => hub.me && hub.me !== machine.name).map((hub) => hub.name) : [];
    const rename = this.renamer(machine, world)
      ? `<button class="icon-button mp-rename" type="button" data-act="rename" aria-label="${escape(words.rename)}" title="${escape(words.rename)}">${glyph("edit", 15)}</button>`
      : "";
    this.put(
      "heading",
      this.heading,
      `<h1 class="page-title mp-name ${state.kind}" title="${escape(state.said)}">${escape(machine.written)}</h1>${rename}${
        former.length ? `<span class="mp-former" tabindex="0" title="${escape(words.formerly(former))}">${glyph("history", 14)}</span>` : ""
      }${late.length ? `<span class="mp-late" tabindex="0" title="${escape(words.notFollowed(late))}">${glyph("relay", 14)}</span>` : ""}<span class="mp-tags">${tags}</span>`,
    );
  }

  /** What is done to it from here: a conversation, its agents, sharing
   *  it on, and the drawing with it chosen. */
  private drawActs(machine: Machine | null, world: WorldNow): void {
    const { words, machines } = this.options;
    if (!machine) return this.put("acts", this.acts, "");
    const giver = this.giver(machine, world);
    const button = (act: string, icon: Parameters<typeof glyph>[0], label: string) =>
      `<button class="text-button quiet-button mp-act" type="button" data-act="${act}" title="${escape(label)}">${glyph(icon, 15)}<span>${escape(label)}</span></button>`;
    // No one's agents live on a managed machine (`world-network.md`, 受管
    // 机器); a visitor shares nothing it holds, nor anyone the hub's machine.
    const managed = isManaged(world, machine.name);
    this.put(
      "acts",
      this.acts,
      [
        managed ? "" : button("talk", "plus", machines.talk),
        managed ? "" : `<a class="text-button quiet-button mp-act" href="${link({ kind: "agents", machine: machine.written })}" title="${escape(machines.hosts)}">${glyph("agents", 15)}<span>${escape(machines.hosts)}</span></a>`,
        giver ? button("share", "colleague", machines.share) : "",
        `<a class="icon-button mp-act-map" href="${link({ kind: "machines", focus: machine.written })}" aria-label="${escape(words.onMap)}" title="${escape(words.onMap)}">${glyph("machine", 17)}</a>`,
      ].join(""),
    );
  }

  /** What it is and how long it has been up, and the Fastctx on it and
   *  whether that is the newest; when it says nothing of itself, what the
   *  World knows and whether it is online. */
  private drawIdentity(machine: Machine, stats: MachineStats | null): void {
    const { words } = this.options;
    const facts = factsOf(machine);
    const form = facts?.form ?? (facts?.os === "linux" ? "server" : "desktop");
    const whole = stats?.system || (facts ? osName(facts.os) : "");
    const [system, build] = withBuild(whole);
    const version = stats?.node.version ?? facts?.version ?? this.options.updates?.state.peek()?.network?.machines?.[machine.name]?.kernel ?? null;
    const fastctx = version ? this.fastctxSaid(version) : [];
    const sizes = stats ? [words.threads(stats.cpu.threads), stats.memory.total ? bytesSaid(stats.memory.total) : "", stats.arch] : [facts?.arch ?? ""];
    const hardware = [stats?.cpu.model ?? "", ...sizes, ...fastctx];
    // Each fact keeps to itself on a narrow screen: the line breaks between
    // them, the dot after the one before.
    const bits = hardware.filter(Boolean);
    // A phone shows the system's name alone: held, it says the rest.
    const held = [whole, stats?.cpu.model ?? "", sizes.filter(Boolean).join("  "), fastctx.join("  ")].filter(Boolean).join("\n");
    const tip = this.narrow ? held : (stats?.kernel ?? "");
    let right: string;
    if (stats?.boot) {
      const at = Date.parse(stats.at);
      const boot = Date.parse(stats.boot);
      right = `<div class="mp-uptime" title="${escape(words.bootAt(words.stamp(new Date(boot), new Date(at))))}"><span class="mp-uptime-label">${escape(words.upFor)}</span><span class="mp-uptime-value">${escape(words.span(at - boot))}</span></div>`;
    } else {
      const state = this.stateOf(machine);
      right = `<div class="mp-uptime ${state.kind}"><span class="mp-uptime-label">${escape(state.said)}</span></div>`;
    }
    this.put(
      "id",
      this.id,
      `<span class="mp-id-mark" aria-hidden="true">${glyph(form, 28)}</span><div class="mp-id-text"><p class="mp-id-system"${tip ? ` title="${escape(tip)}"` : ""}>${escape(system)}${build ? `<span class="mp-id-build"> ${escape(build)}</span>` : ""}</p><p class="mp-id-hardware">${bits
        .map((bit, at) => `<span class="mp-id-bit">${escape(bit)}${at < bits.length - 1 ? `<span class="sep"></span>` : ""}</span>`)
        .join("")}</p></div>${right}`,
    );
  }

  /** Why nothing, or nothing new, is drawn: offline, too old to be asked,
   *  the asking failed, or the first answer is on its way. */
  private drawNote(machine: Machine, stats: MachineStats | null, heard: number): void {
    const { words } = this.options;
    let text = "";
    let title = "";
    // Offline with nothing read, the head already says since when.
    if (!machine.here && machine.online === false) text = stats ? words.offline : "";
    else if (!askable(machine)) text = words.older;
    else if (this.problem) {
      text = words.failed;
      title = this.problem;
    } else if (!stats) text = words.reading;
    if (text && stats) text = `${text} ${words.lastRead(this.options.ago(Date.now() - heard))}`;
    this.note.hidden = !text;
    set(this.note, text);
    this.note.title = title;
    this.board.classList.toggle("stale", !!text && !!stats);
  }

  /** The rings and traces of how busy and how full it is: its processors,
   *  its memory and each graphics card, in one row while four fit. */
  private drawVitals(stats: MachineStats, samples: Sample[]): void {
    const { words } = this.options;
    const gpus = stats.gpus ?? [];
    const shape = `${stats.machine}|${gpus.length}`;
    if (shape !== this.vitalsShape) {
      this.vitalsShape = shape;
      for (const vital of this.vitals) {
        vital.ring.dispose();
        vital.spark.dispose();
      }
      this.vitals = [
        this.vital("cpu", words.cpu, `${stats.machine}:cpu`),
        this.vital("memory", words.memory, `${stats.machine}:memory`),
        ...gpus.map((_, index) => this.vital("gpu", words.gpu, `${stats.machine}:gpu:${index}`)),
      ];
      this.vitalsBox.replaceChildren(...this.vitals.map((vital) => vital.element));
      this.vitalsBox.dataset.count = String(this.vitals.length);
    }
    const at = Date.parse(stats.at);
    const from = at - SPAN_MS;
    const series = (pick: (sample: Sample) => number | null): [number, number | null][] => samples.map((sample) => [sample.at, pick(sample)]);
    const windows = stats.system.startsWith("Windows");
    let gpu = 0;
    for (const vital of this.vitals) {
      switch (vital.kind) {
        case "cpu": {
          const busy = stats.cpu.busy ?? null;
          vital.ring.set(busy, percentHtml(busy));
          note(vital.note, words.threads(stats.cpu.threads));
          const load = stats.cpu.load;
          const points = series((sample) => sample.cpu);
          // Where the system keeps load averages they say more; elsewhere,
          // the highest the trace reached.
          let peak: number | null = null;
          for (const [when, value] of points) if (when >= from && value !== null && (peak === null || value > peak)) peak = value;
          set(vital.sub, load ? words.load(...(load.map((value) => value.toFixed(2)) as [string, string, string])) : peak === null ? "" : words.peak(`${Math.round(peak * 100)}%`));
          vital.spark.draw([{ points, max: 1, kind: "clay", area: true, gap: READ_GAP_MS }], from, at);
          break;
        }
        case "memory": {
          const { total, used, swap_total, swap_used, room } = stats.memory;
          const share = total ? used / total : null;
          vital.ring.set(share, percentHtml(share));
          // Past the line where what is left is the room kept for agents,
          // new turns wait.
          const line = room && total > room ? (total - room) / total : null;
          vital.ring.mark(line);
          if (room) vital.ring.element.title = words.roomKept(bytesSaid(room));
          else vital.ring.element.removeAttribute("title");
          note(vital.note, swap_total ? words.swap(windows, bytesSaid(swap_used), bytesSaid(swap_total)) : "");
          set(vital.sub, words.of(bytesSaid(used), bytesSaid(total)));
          vital.spark.draw([{ points: series((sample) => sample.memory), max: 1, kind: "clay", area: true, gap: READ_GAP_MS }], from, at, line);
          break;
        }
        case "gpu": {
          const index = gpu;
          gpu += 1;
          const one = gpus[index];
          if (!one) break;
          const busy = one.busy ?? null;
          vital.ring.set(busy, percentHtml(busy));
          note(vital.note, one.name);
          set(vital.sub, one.memory_total ? words.vram(bytesSaid(one.memory_used), bytesSaid(one.memory_total)) : "");
          vital.spark.draw([{ points: series((sample) => sample.gpus[index] ?? null), max: 1, kind: "clay", area: true, gap: READ_GAP_MS }], from, at);
          break;
        }
      }
      // A phone shows the ring alone: held, it says the rest.
      tip(vital.element, this.narrow ? [vital.note.textContent, vital.sub.textContent].filter(Boolean).join("\n") : "");
    }
  }

  /** One of the vitals: its name and a note on its line, its ring, its
   *  trace, and a line under it. */
  private vital(kind: Vital["kind"], label: string, seed: string): Vital {
    const element = document.createElement("article");
    element.className = `mp-vital mp-vital-${kind}`;
    element.innerHTML = `<header class="mp-vital-head"><h2 class="mp-vital-label">${escape(label)}</h2><span class="mp-vital-note"></span></header><div class="mp-vital-body"></div><p class="mp-vital-sub"></p>`;
    const ring = new Ring(seed);
    const spark = new Spark();
    element.querySelector<HTMLElement>(".mp-vital-body")!.append(ring.element, spark.element);
    return {
      kind,
      element,
      note: element.querySelector<HTMLElement>(".mp-vital-note")!,
      sub: element.querySelector<HTMLElement>(".mp-vital-sub")!,
      ring,
      spark,
    };
  }

  /** Each network card: its name, its link's speed, whether a program made
   *  it, where it is reached, and what it moves now and has moved; the one
   *  it uses most with a trace of what it received and sent. */
  private drawCards(stats: MachineStats, samples: Sample[]): void {
    const { words } = this.options;
    const cards = stats.cards;
    // A phone leaves out the cards not connected.
    this.cardsBox.hidden = !(this.narrow ? cards.some((card) => card.up) : cards.length);
    if (!cards.length) return;
    const main = mainCard(cards);
    const shape = `${main?.name ?? ""};${cards.map((card) => [card.name, card.up, card.hardware, card.speed, (card.addresses ?? []).join(",")].join("|")).join(";")}`;
    if (shape !== this.cardsShape) {
      this.cardsShape = shape;
      this.flow?.dispose();
      this.flow = null;
      this.cardsBox.innerHTML = `<h2 class="mp-sec-title">${escape(words.cards)}</h2><ul class="mp-list">${cards
        .map(
          (card, index) => `<li class="mp-net${card.up ? "" : " off"}${card === main ? " mp-net-followed" : ""}" data-card="${index}">
            <div class="mp-net-head"><span class="mp-net-name" title="${escape(card.name)}">${escape(card.name)}</span>${card.speed ? `<span class="tag">${escape(bits(card.speed))}</span>` : ""}${
              card.hardware ? "" : `<span class="tag mp-tag-quiet">${escape(words.virtualCard)}</span>`
            }${card.up ? `<span class="mp-net-rates"><span class="mp-rate down"><span class="mp-arrow" aria-hidden="true">↓</span><span class="mp-rate-value"></span></span><span class="mp-rate up"><span class="mp-arrow" aria-hidden="true">↑</span><span class="mp-rate-value"></span></span></span>` : `<span class="mp-net-off">${escape(words.cardOff)}</span>`}</div>
            ${(card.addresses ?? []).length ? `<p class="mp-net-at">${(card.addresses ?? []).map(escape).join(`<span class="sep"></span>`)}</p>` : ""}
            ${card.up ? `<p class="mp-net-moved"></p>` : ""}
          </li>`,
        )
        .join("")}</ul>`;
      if (main) {
        this.flow = new Spark("mp-spark-flow");
        this.cardsBox.querySelector(".mp-net-followed")?.append(this.flow.element);
      }
    }
    cards.forEach((card, index) => {
      if (!card.up) return;
      const row = this.cardsBox.querySelector<HTMLElement>(`[data-card="${index}"]`);
      if (!row) return;
      const [down, up] = row.querySelectorAll<HTMLElement>(".mp-rate-value");
      if (down) set(down, card.receiving === undefined ? "—" : rate(card.receiving));
      if (up) set(up, card.sending === undefined ? "—" : rate(card.sending));
      const moved = row.querySelector<HTMLElement>(".mp-net-moved");
      if (moved) set(moved, words.moved(bytesSaid(card.received), bytesSaid(card.sent)));
      // A phone shows its name and rates: held, it says the rest.
      const held = [card.name, card.speed ? bits(card.speed) : "", (card.addresses ?? []).join("  "), moved?.textContent ?? ""];
      tip(row, this.narrow ? held.filter(Boolean).join("\n") : "");
    });
    if (!main || !this.flow) return;
    const at = Date.parse(stats.at);
    const from = at - SPAN_MS;
    const series = (side: 0 | 1): [number, number | null][] => samples.map((sample) => [sample.at, sample.cards[main.name]?.[side] ?? null]);
    const downs = series(0);
    const ups = series(1);
    let top = 1024;
    for (const [when, value] of [...downs, ...ups]) if (when >= from && value !== null && value > top) top = value;
    this.flow.draw(
      [
        { points: downs, max: top * 1.15, kind: "clay", area: true, gap: READ_GAP_MS },
        { points: ups, max: top * 1.15, kind: "graphite", area: false, gap: READ_GAP_MS },
      ],
      from,
      at,
    );
  }

  /** Each disk as a bar of how full it is, with what is free. */
  private drawDisks(stats: MachineStats): void {
    const { words } = this.options;
    this.disksBox.hidden = !stats.disks.length;
    const rows = stats.disks
      .map((disk) => {
        // A disk's share moves by the byte; drawn to a five-hundredth.
        const used = disk.total ? Math.round((1 - disk.free / disk.total) * 500) / 500 : 0;
        // A phone leaves out what kind it is; held, the disk says it.
        const kind = [disk.label, disk.fs].filter(Boolean).join(" ");
        return `<li class="mp-disk"${this.narrow && kind ? ` title="${escape(kind)}"` : ""}><div class="mp-disk-head"><span class="mp-disk-at">${escape(diskName(disk.at))}</span><span class="mp-disk-kind">${escape(kind)}</span><span class="mp-disk-free">${escape(
          words.free(bytesSaid(disk.free), bytesSaid(disk.total)),
        )}</span></div>${diskBar(used, `${stats.machine}:${disk.at}`)}</li>`;
      })
      .join("");
    this.put("disks", this.disksBox, `<h2 class="mp-sec-title">${escape(words.disks)}</h2><ul class="mp-list">${rows}</ul>`);
  }

  /** Each line to a hub: up or down and for how long, its round trip now
   *  and over the hour, and its days as a strip with how it held. */
  private drawLines(): void {
    const machine = this.machine;
    const stats = machine ? keptOf(machine.name).last : null;
    if (!stats) return;
    const { words } = this.options;
    this.linesBox.hidden = !stats.lines.length;
    if (!stats.lines.length) return;
    const now = Date.parse(stats.at);
    const from = now - RANGES[this.range];
    const shape = `${stats.machine}|${stats.lines.map((line) => line.hub).join("|")}`;
    if (shape !== this.linesShape) this.buildLines(stats, shape);
    for (const button of this.linesBox.querySelectorAll<HTMLButtonElement>("[data-range]")) {
      const on = button.dataset.range === this.range;
      button.classList.toggle("chosen", on);
      button.setAttribute("aria-checked", String(on));
    }
    const stamp = (at: number) => words.stamp(new Date(at), new Date(now));
    stats.lines.forEach((line, index) => {
      const view = this.lines[index];
      if (!view) return;
      set(view.hub, line.hub);
      const lasted = words.span(Math.max(0, now - Date.parse(line.since)));
      set(view.state, line.up ? [words.lineUp(lasted), line.through ? words.lineVia(line.through) : ""].filter(Boolean).join(" ") : words.lineDown(lasted));
      view.element.classList.toggle("down", !line.up);
      const trips = tripsHeld(line);
      set(view.trip, trips.last === null ? "" : `${trips.last} ms`);
      view.trip.title = trips.median === null ? "" : words.roundTrip(`${trips.median} ms`, `${trips.highest} ms`);
      const points = line.trips.map((trip): [number, number] => [Date.parse(trip.at), trip.ms]);
      view.spark.draw([{ points, max: Math.max(10, trips.highest ?? 0) * 1.2, kind: "graphite", area: true, gap: TRIP_GAP_MS }], now - 3_600_000, now);
      const key = `${this.range}|${line.changes.length}|${line.changes.at(-1)?.at ?? ""}|${line.up}|${Math.floor(now / STRIP_MS)}`;
      if (key === view.drawn) return;
      view.drawn = key;
      const { stretches, moments } = daysOf(line, from, now);
      const said = (kind: string, start: number, end: number, why: string) =>
        kind === "up"
          ? `${words.lineUp(words.span(end - start))} ${stamp(start)}–${stamp(end)}`
          : kind === "down"
            ? end >= now && !line.up
              ? words.stillDown(stamp(start), why)
              : words.wentDown(stamp(start), words.span(end - start), why)
            : words.notRecorded;
      const spans: LineSpan[] = stretches.map((stretch) => ({ kind: stretch.kind, from: stretch.from, to: stretch.to, title: said(stretch.kind, stretch.from, stretch.to, stretch.why) }));
      const marks: LineMark[] = moments.map((moment) => {
        if (moment.kind === "start") return { kind: "start", at: moment.at, title: words.started(stamp(moment.at)) };
        const down = stretches.find((stretch) => stretch.kind === "down" && stretch.from === moment.at);
        return { kind: "drop", at: moment.at, title: down ? said("down", down.from, down.to, down.why) : words.stillDown(stamp(moment.at), moment.why) };
      });
      view.strip.draw(spans, marks, from, now);
      const held = heldOf(stretches, moments);
      set(view.held, [held.online === null ? "" : words.available(percentOf(held.online)), words.drops(held.drops), held.drops ? words.longest(words.span(held.longest)) : ""].filter(Boolean).join(" "));
    });
    const axis = this.axis(from, now);
    for (const view of this.lines) if (view.axis.innerHTML !== axis) view.axis.innerHTML = axis;
  }

  private buildLines(stats: MachineStats, shape: string): void {
    const { words } = this.options;
    this.linesShape = shape;
    for (const view of this.lines) {
      view.strip.dispose();
      view.spark.dispose();
    }
    this.linesBox.innerHTML = `<header class="mp-sec-head"><h2 class="mp-sec-title">${escape(words.connection)}</h2><div class="mp-ranges" role="radiogroup" aria-label="${escape(words.connection)}">${(Object.keys(RANGES) as Range[])
      .map((range) => `<button type="button" class="mp-range" role="radio" data-range="${range}">${escape(words.ranges[range])}</button>`)
      .join("")}</div></header><div class="mp-line-list"></div>`;
    const list = this.linesBox.querySelector<HTMLElement>(".mp-line-list")!;
    this.lines = stats.lines.map((line) => {
      const element = document.createElement("article");
      element.className = "mp-line";
      element.innerHTML = `<header class="mp-line-head"><span class="mp-line-hub"></span><span class="mp-line-state"></span><span class="mp-line-trip" tabindex="0"></span></header><div class="mp-axis" aria-hidden="true"></div><p class="mp-line-held"></p>`;
      const head = element.querySelector<HTMLElement>(".mp-line-head")!;
      const strip = new Strip(`${stats.machine}:${line.hub}`);
      const spark = new Spark("mp-spark-trip");
      head.append(spark.element);
      head.after(strip.element);
      list.append(element);
      return {
        element,
        hub: head.querySelector<HTMLElement>(".mp-line-hub")!,
        state: head.querySelector<HTMLElement>(".mp-line-state")!,
        trip: head.querySelector<HTMLElement>(".mp-line-trip")!,
        held: element.querySelector<HTMLElement>(".mp-line-held")!,
        axis: element.querySelector<HTMLElement>(".mp-axis")!,
        strip,
        spark,
        drawn: "",
      };
    });
  }

  /** The marks under the strips: the hours of a day, the days of a week
   *  or a month. */
  private axis(from: number, to: number): string {
    const { words } = this.options;
    const day = this.range === "day";
    const step = day ? 6 * 3_600_000 : this.range === "week" ? 86_400_000 : 5 * 86_400_000;
    const first = new Date(from);
    if (day) {
      first.setMinutes(0, 0, 0);
      first.setHours(Math.ceil((first.getHours() + (from > first.getTime() ? 1 : 0)) / 6) * 6);
    } else first.setHours(24, 0, 0, 0);
    const marks: string[] = [];
    for (let at = first.getTime(); at < to - step / 4; at += step) {
      const left = ((at - from) / (to - from)) * 100;
      marks.push(`<span style="left:${left.toFixed(2)}%">${escape(day ? words.hour(new Date(at)) : words.day(new Date(at)))}</span>`);
    }
    return marks.join("");
  }

  /** The Fastctx on it as its identity line says it: the version, and
   *  whether a newer one is out on its line, as this device knows. */
  private fastctxSaid(version: string): string[] {
    const { words } = this.options;
    const state = this.options.updates?.state.peek();
    if (!state) return [words.fastctxRuns(version)];
    const newer = newestKnown(state, version);
    return [words.fastctxRuns(version), newer ? words.fastctxBehind(newer) : words.fastctxLatest];
  }

  /** Facts the head does not say, one a row, the names in one column and
   *  the values in the next: its own host name, its kernel, its home and
   *  when it started. */
  private drawFacts(machine: Machine, stats: MachineStats | null): void {
    const { words } = this.options;
    const facts = factsOf(machine);
    const rows: { label: string; value: string; code?: boolean; title?: string }[] = [];
    if (stats?.host) rows.push({ label: words.host, value: stats.host });
    if (stats?.kernel) rows.push({ label: words.kernel, value: stats.kernel });
    if (facts?.home) rows.push({ label: words.home, value: facts.home, code: true });
    if (stats?.boot) rows.push({ label: words.booted, value: words.stamp(new Date(stats.boot), new Date(stats.at)) });
    if (stats) rows.push({ label: words.node, value: words.nodeFor(words.span(Date.parse(stats.at) - Date.parse(stats.node.started))) });
    this.factsBox.hidden = !rows.length;
    this.put("facts", this.factsBox, `<h2 class="mp-sec-title">${escape(words.about)}</h2>${factRows(rows)}`);
  }

  /** Where it stands in each network: how this computer reaches each hub
   *  and as whom, or whose machine another is there. */
  private drawWhere(machine: Machine, world: WorldNow): void {
    const { words, machines } = this.options;
    const networks = machine.here
      ? world.nodes.hubs.map((hub) => {
          const stand = hub.home ? machines.registeredUnder(hub.home) : hub.logins.length ? machines.onlyLogins(hub.logins.join(machines.separator)) : "";
          return { label: hub.name, value: [linkSaid(hub.link, machines), stand].filter(Boolean).join(" "), title: linkWhy(hub.link) };
        })
      : machine.on.filter((on) => on.owner).map((on) => ({ label: on.hubName, value: machines.registeredUnder(on.owner!), title: "" }));
    this.whereBox.hidden = !networks.length;
    this.put("where", this.whereBox, `<h2 class="mp-sec-title">${escape(words.networks)}</h2>${factRows(networks)}`);
  }

  /** How far it is toward its system's limits over the last day: each
   *  limit's name, how much of it is used now, and its trace with the limit
   *  at the top. A machine whose system bounds none of them has no part. */
  private drawLimits(stats: MachineStats): void {
    const { words } = this.options;
    const readings = stats.limits ?? [];
    const last = readings[readings.length - 1];
    const uses = (last?.uses ?? []).filter((use): use is LimitUse & { kind: keyof typeof words.limit } => use.kind in words.limit);
    this.limitsBox.hidden = !uses.length;
    if (!uses.length) return;
    const shape = uses.map((use) => use.kind).join("|");
    if (shape !== [...this.limitRows.keys()].join("|")) {
      for (const row of this.limitRows.values()) row.spark.dispose();
      this.limitRows = new Map(
        uses.map((use) => {
          const element = document.createElement("li");
          element.className = "mp-limit";
          element.innerHTML = `<div class="mp-limit-head"><span class="mp-limit-name" title="${escape(words.limit[use.kind].why)}">${escape(words.limit[use.kind].name)}</span><span class="mp-limit-figure"></span></div>`;
          const spark = new Spark("mp-limit-trace");
          element.append(spark.element);
          return [use.kind, { element, figure: element.querySelector<HTMLElement>(".mp-limit-figure")!, spark }];
        }),
      );
      this.limitsBox.innerHTML = `<header class="mp-sec-head"><h2 class="mp-sec-title">${escape(words.limits)}</h2><span class="mp-limits-span">${escape(words.limitsSpan)}</span></header><ul class="mp-list"></ul>`;
      this.limitsBox.querySelector("ul")!.append(...[...this.limitRows.values()].map((row) => row.element));
    }
    const at = Date.parse(last!.at);
    for (const use of uses) {
      const row = this.limitRows.get(use.kind)!;
      const share = use.limit ? use.used / use.limit : null;
      row.element.dataset.level = levelOf(share);
      set(row.figure, words.limitUsed(use.used.toLocaleString(), use.limit.toLocaleString()));
      const points: [number, number | null][] = readings.map((reading) => [Date.parse(reading.at), reading.uses.find((one) => one.kind === use.kind)?.used ?? null]);
      row.spark.draw([{ points, max: use.limit, kind: "clay", area: true, gap: LIMIT_GAP_MS }], at - LIMITS_MS, at, use.limit);
    }
  }

  /** Its name and the group its owner put it in, on each hub that says so. */
  private drawNaming(machine: Machine, world: WorldNow): void {
    const { words } = this.options;
    const grouped = [...new Set(world.accounts.hubs.flatMap((hub) => hub.members.filter((member) => member.kind === "computer" && names(member, machine.name) && member.group).map((member) => member.group!)))];
    const rename = this.renamer(machine, world)
      ? `<button class="text-button quiet-button" type="button" data-act="rename">${glyph("edit", 14)}<span>${escape(words.rename)}</span></button>`
      : "";
    const rows = [
      `<dt>${escape(words.name)}</dt><dd><span>${escape(machine.written)}</span>${rename}</dd>`,
      grouped.length ? `<dt>${escape(words.group)}</dt><dd><span>${escape(grouped.join(" "))}</span></dd>` : "",
    ].join("");
    this.namingBox.hidden = false;
    this.put("naming", this.namingBox, `<dl class="mp-rows">${rows}</dl>`);
  }

  /** What it keeps free in memory for its agents, for whoever holds it,
   *  where its node keeps any. */
  private drawRoom(machine: Machine, stats: MachineStats | null, world: WorldNow): void {
    const { words } = this.options;
    const room = stats?.memory.room;
    const shown = room !== undefined && !!this.options.memoryRoom && (machine.here || !!this.holder(machine, world));
    this.roomBox.hidden = !shown;
    if (!shown) return;
    const said = room ? bytesSaid(room) : words.roomNone;
    this.put(
      "room",
      this.roomBox,
      `<dl class="mp-rows"><dt>${escape(words.room)}</dt><dd><button class="text-button quiet-button mp-room-pick" type="button" data-act="room" data-menu-key="room" aria-haspopup="menu" aria-expanded="false"><span>${escape(said)}</span>${glyph("chevron", 13)}</button></dd></dl>`,
    );
  }

  /** Chooses what the machine keeps free for its agents. */
  private pickRoom(anchor: HTMLElement, machine: Machine): void {
    const { words } = this.options;
    const set = this.options.memoryRoom;
    const now = keptOf(machine.name).last?.memory.room;
    if (!set || now === undefined) return;
    this.roomMenu ??= new ActsMenu();
    const entries = ROOMS.map((bytes) => ({ act: String(bytes), label: bytes ? bytesSaid(bytes) : words.roomNone, checked: bytes === now }));
    this.roomMenu.open(anchor, words.room, entries, (act) => {
      const bytes = Number(act);
      if (bytes === now) return;
      void set(machine.here ? null : machine.written, bytes).then(
        (kept) => {
          const last = keptOf(machine.name).last;
          if (last) last.memory.room = kept;
          this.render();
        },
        (error: unknown) => this.say(words.roomFailed(detailOf(error)), null),
      );
    });
  }

  /** The Fastctx on it: what it runs and can take, its update and choices. */
  private drawFastctx(machine: Machine, stats: MachineStats | null, world: WorldNow): void {
    const block = this.fastctx;
    this.fastctxBox.hidden = !block;
    if (!block) return;
    const manages = updaterLoginOf(machine, world);
    const subject: FastctxSubject = {
      machine,
      version: stats?.node.version ?? factsOf(machine)?.version ?? null,
      service: stats ? stats.node.service : null,
      manages,
    };
    block.show(subject, this.tab === "manage" && document.visibilityState === "visible");
    this.put("fastctx", this.fastctxBox, block.html(subject));
  }

  /** At the foot of 「管理」: taking it off, for whoever may; on the machine
   *  that runs the network's hub, the way to move the hub or dissolve the
   *  network instead of uninstalling it. */
  private drawDanger(machine: Machine, world: WorldNow): void {
    const { manage } = this.options;
    const remover = removerLoginOf(machine, world);
    const hub = this.options.updates?.state.peek()?.network?.machines?.[machine.name]?.hub_here === true;
    const parts: string[] = [];
    if (hub) parts.push(`<p class="mp-danger-note">${escape(manage.hubHere)}</p><a class="text-button quiet-button" href="${link({ kind: "machines" })}">${escape(manage.hubGo)}</a>`);
    if (remover) parts.push(`<button class="text-button quiet-button danger" type="button" data-act="remove">${glyph("close", 14)}<span>${escape(manage.remove)}</span></button>`);
    this.dangerBox.hidden = !parts.length;
    this.put("danger", this.dangerBox, `<div class="mp-danger-acts">${parts.join("")}</div>`);
  }

  /** Who is at work on it now and what runs there, as this device's
   *  logins see it; and what of its own work is stuck, as it says. */
  private drawPresent(machine: Machine, stats: MachineStats | null, world: WorldNow): void {
    const { words, machines } = this.options;
    const viewer = loginsOf(world.accounts).find((login) => machine.on.some((on) => on.hub === login.hub)) ?? null;
    const group = viewer ? (groupsOf(world).find((one) => sameLogin(one.login, viewer)) ?? null) : null;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine.written && (r.now?.activity === "busy" || r.now?.waiting_for_user || !!r.now?.asked) && (!group || residentIn(world, group, r)),
    );
    const jobs = jobsOf(world).filter((job) => job.machine === machine.written);
    // Its owner is told how many are at work or waiting there, whosever
    // they are; those of other accounts go without their names.
    const answered = world.who.machines.find((one) => one.kind === "answered" && one.reply.machine === machine.name);
    const busy = answered?.kind === "answered" ? (answered.reply.busy ?? 0) : 0;
    const others = Math.max(0, busy - residents.length);
    const stuck = stats?.node.stuck ?? [];
    const health = stats?.node.health ?? [];
    const waiting = stats?.memory.waiting ?? 0;
    this.presentBox.hidden = !residents.length && !jobs.length && !others && !stuck.length && !health.length && !waiting;
    // Its own warnings in plain words, as it said them a hover away.
    const healthSaid = (text: string) => {
      const rows = /has (\d+) unreadable rows/.exec(text);
      return rows ? words.ledgerRows(Number(rows[1])) : /cannot be read or written/.test(text) ? words.ledgerBroken : words.healthOther;
    };
    const warned = [
      ...stuck.map((one) => `<li title="${escape(`${one.doing}: ${one.what}`)}">${escape(words.stuck(words.span(one.seconds * 1000)))}</li>`),
      ...health.map((one) => `<li title="${escape(one.text)}">${escape(healthSaid(one.text))}</li>`),
    ];
    const stuckSaid = warned.length ? `<ul class="mp-stuck">${warned.join("")}</ul>` : "";
    const agents = residents
      .map(
        (r) =>
          `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : "mark-busy"}" aria-hidden="true"></span>${escape(
            [agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" "),
          )}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
      )
      .join("");
    // Who runs how many jobs on it, as it says: a row of dots each, the
    // faint ones what its owner still lets that account run.
    const owner = this.holder(machine, world)?.account ?? words.ownJobs;
    const byAccount = (stats?.jobs ?? []).filter((one) => one.running > 0);
    const jobsSaid = byAccount.length
      ? `<ul class="mp-jobs">${byAccount
          .map((one) => {
            const who = one.account?.account ?? owner;
            const most = one.most ?? null;
            const shown = Math.min(one.running, JOB_DOTS);
            const room = most !== null && most <= JOB_DOTS ? Math.max(0, most - one.running) : 0;
            const dots = `${"<i class=\"on\"></i>".repeat(shown)}${"<i></i>".repeat(room)}`;
            const count = one.running > JOB_DOTS || (most !== null && most > JOB_DOTS) ? `<span class="mp-jobs-n">${most ? `${one.running}/${most}` : one.running}</span>` : "";
            return `<li title="${escape(words.jobsOf(who, one.running, most))}"><span class="mp-jobs-who">${escape(who)}</span><span class="mp-jobs-dots" aria-hidden="true">${dots}</span>${count}</li>`;
          })
          .join("")}</ul>`
      : jobs.length
        ? `<p class="machine-jobs">${escape(machines.jobs(jobs.length))}</p>`
        : "";
    this.presentBox.hidden &&= !byAccount.length;
    this.put(
      "present",
      this.presentBox,
      `<h2 class="mp-sec-title">${escape(words.present)}</h2>${agents ? `<ul class="machine-agents">${agents}</ul>` : ""}${others ? `<p class="machine-jobs">${escape(words.othersAtWork(others))}</p>` : ""}${
        waiting ? `<p class="machine-jobs mp-waiting" title="${escape(words.roomKept(bytesSaid(stats?.memory.room ?? 0)))}">${escape(words.waitingMemory(waiting))}</p>` : ""
      }${jobsSaid}${stuckSaid}`,
    );
  }

  /** What macOS kept private from its node: on this Mac with the way to
   *  allow Fastctx in (`world-gui.md`, 设置). */
  private drawPrivacy(machine: Machine, stats: MachineStats | null): void {
    const { words } = this.options;
    const refused = stats?.node.refused;
    this.privacyBox.hidden = !refused;
    if (!refused) return;
    const allow = machine.here ? this.options.allowPrivacy : undefined;
    this.put(
      "privacy",
      this.privacyBox,
      `<div class="mp-privacy-card"><span>${escape(machine.here ? words.privacyHere(refused.path) : words.privacyThere(refused.path))}</span>${
        allow ? `<button class="text-button quiet-button" type="button" data-act="allow-privacy">${escape(words.privacyGo)}</button>` : ""
      }</div>`,
    );
  }

  /** Its direct links, in 「网络」. */
  private drawDirect(machine: Machine, world: WorldNow): void {
    const { words, machines } = this.options;
    const direct = directSaid(machine, world, machines);
    this.directBox.hidden = !direct;
    if (direct) this.put("direct", this.directBox, `<h2 class="mp-sec-title">${escape(words.direct)}</h2>${direct}`);
  }

  /** Whom it is shared with and whom its agents assist, for those who hold
   *  it, each taken back with its cross; one's own share is given back. */
  private drawSharing(machine: Machine, world: WorldNow): void {
    const { words, machines, access } = this.options;
    const owner = this.holder(machine, world);
    this.sharingBox.hidden = !owner;
    if (!owner) return;
    const now = this.options.store.clock.peek();
    const chip = (label: string, take: string, said = machines.takeBack(label)) =>
      `<span class="chip">${escape(label)}<button class="chip-x" type="button" ${take} aria-label="${escape(said)}" title="${escape(said)}">${glyph("close", 12)}</button></span>`;
    // Its holders see every share, each taken back; a visitor's own share is
    // the only one it gives back, as it gives none.
    const giver = this.giver(machine, world);
    const to = sharedWith(world, owner, machine.name);
    const shared =
      giver || to.includes(owner.account)
        ? `<div class="given-row"><span class="given-label">${escape(machines.sharedTo)}</span>${to
            .filter((one) => giver || one === owner.account)
            .map((one) => {
              if (one === owner.account) return chip(one, `data-act="leave"`, machines.leave);
              const jobs = shareOf(world, owner, machine.name, one)?.jobs;
              return chip(one, `data-act="unshare" data-to="${escape(one)}"${jobs ? ` data-jobs="${jobs}"` : ""}`);
            })
            .join("")}${giver ? `<button class="text-button quiet-button" type="button" data-act="share">${glyph("plus", 13)}<span>${escape(machines.share)}</span></button>` : ""}</div>`
        : "";
    const assists = owner ? (hubOf(world, owner)?.assists ?? []).filter((assist) => assist.machine === machine.name) : [];
    const assisting = assists.length
      ? `<div class="given-row"><span class="given-label">${escape(machines.assisting)}</span>${assists
          .map((assist) => {
            const label = `${agentName(assist.agent)} ${assist.account}`;
            const at = `data-to="${escape(assist.account)}" data-agent="${escape(assist.agent)}"${assist.home ? ` data-home="${escape(assist.home)}"` : ""}${assist.until ? ` data-until="${escape(assist.until)}"` : ""}${assist.daily ? ` data-daily="${assist.daily}"` : ""}${assist.weekly ? ` data-weekly="${assist.weekly}"` : ""}${assist.five_hour ? ` data-five-hour="${assist.five_hour}"` : ""}${assist.turns ? ` data-turns="${assist.turns}"` : ""}`;
            const until = assist.until ? new Date(assist.until) : null;
            const said = until ? access.moment.until(until, now) : access.when.none;
            const day = until ? `<span class="assist-tag-until">${glyph("history", 12)}${escape(access.moment.short(until, now))}</span>` : "";
            return `<span class="chip assist-chip"><button class="assist-tag-open" type="button" data-act="assist-edit" ${at} title="${escape(`${access.changeUntil}\n${said}`)}">${escape(label)}${day}</button><button class="chip-x" type="button" data-act="assist-end" ${at} aria-label="${escape(machines.endAssist(assist.account))}" title="${escape(machines.endAssist(assist.account))}">${glyph("close", 12)}</button></span>`;
          })
          .join("")}</div>`
      : "";
    this.put("sharing", this.sharingBox, `<h2 class="mp-sec-title">${escape(words.sharing)}</h2>${shared}${assisting}`);
  }

  private stateOf(machine: Machine): { kind: string; said: string } {
    const { machines } = this.options;
    const now = this.options.store.clock.peek();
    const at = machine.since ? this.options.when(new Date(machine.since), now) : null;
    if (machine.here || machine.online === true) return { kind: "online", said: at && !machine.here ? machines.online(at) : machines.onlineNow };
    if (machine.online === false) return { kind: "offline", said: machines.offline(at) };
    return { kind: "unknown", said: machines.unknown };
  }

  /** The login it is in the hands of here: its owner's first. */
  private holder(machine: Machine, world: WorldNow): Login | undefined {
    return holderLoginOf(machine, world);
  }

  /** The login that shares it on: one holding it that is no visitor, where
   *  it does not run the hub. */
  private giver(machine: Machine, world: WorldNow): Login | undefined {
    const logins = loginsOf(world.accounts).filter((login) => holds(machine, login, world));
    return logins.find((login) => !isVisitor(world, login) && !runsHub(machine, login, world));
  }

  private renamer(machine: Machine, world: WorldNow): Renamer | null {
    return renamerOf(machine, world, this.options.change, this.options.renameHere);
  }

  private startRename(): void {
    const world = this.options.store.world.peek();
    const machine = this.machine;
    if (!world || !machine || this.renaming) return;
    const rename = this.renamer(machine, world);
    if (!rename) return;
    const { words } = this.options;
    const field = new RenameField({
      words,
      nameSaid: this.options.nameSaid,
      name: machine.name,
      note: !machine.here && machine.online === false ? words.renameOffline(machine.written) : undefined,
      check: (text) => this.options.nameProblem(text),
      rename: async (to) => {
        await rename(to);
        await this.options.store.refresh();
      },
      done: (to) => {
        field.dispose();
        this.renaming = null;
        this.options.replace({ kind: "machine", name: to });
      },
      cancel: () => {
        field.dispose();
        this.renaming = null;
        this.drawn.delete("heading");
        this.render();
      },
    });
    this.renaming = field;
    this.heading.replaceChildren(field.element);
    field.focus();
  }

  private click(event: Event): void {
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-act], [data-range], [data-tab]");
    if (!target || (target as HTMLButtonElement).disabled) return;
    if (target.dataset.tab) {
      this.choose(target.dataset.tab as Tab, false);
      return;
    }
    if (this.fastctx?.click(target)) return;
    if (target.dataset.range) {
      this.range = target.dataset.range as Range;
      keepRange(this.range);
      this.drawLines();
      return;
    }
    const world = this.options.store.world.peek();
    const machine = this.machine;
    if (!world || !machine) return;
    const owner = this.holder(machine, world);
    const button = target as HTMLButtonElement;
    switch (target.dataset.act) {
      case "rename":
        return this.startRename();
      case "room":
        return this.pickRoom(target, machine);
      case "talk":
        return this.options.talk(machine.written);
      case "share": {
        const giver = this.giver(machine, world);
        if (giver) this.share(giver, machine);
        return;
      }
      case "leave":
        if (owner) this.giveBack(owner, machine);
        return;
      case "unshare": {
        const to = target.dataset.to ?? "";
        // Given again by the undo, it keeps its limit of jobs.
        const jobs = target.dataset.jobs ? { jobs: Number(target.dataset.jobs) } : {};
        if (owner) void this.takeBack(button, owner, to, { change: "share", machine: machine.name, to, shared: false }, { change: "share", machine: machine.name, to, shared: true, ...jobs });
        return;
      }
      case "assist-edit":
        if (owner) this.assist(owner, machine, target.dataset.agent ?? "", target.dataset.home ?? null, target.dataset.to ?? null);
        return;
      case "assist-end": {
        const to = target.dataset.to ?? "";
        const agent = target.dataset.agent ?? "";
        const home = target.dataset.home ? { home: target.dataset.home } : {};
        // Given again by the undo, it ends when it was to, and keeps its allowances.
        const until = target.dataset.until ? { until: target.dataset.until } : {};
        const caps = {
          ...(target.dataset.daily ? { daily: Number(target.dataset.daily) } : {}),
          ...(target.dataset.weekly ? { weekly: Number(target.dataset.weekly) } : {}),
          ...(target.dataset.fiveHour ? { five_hour: Number(target.dataset.fiveHour) } : {}),
          ...(target.dataset.turns ? { turns: Number(target.dataset.turns) } : {}),
        };
        if (owner)
          void this.takeBack(
            button,
            owner,
            `${agentName(agent)} ${to}`,
            { change: "assist", machine: machine.name, to, agent, ...home, assisting: false },
            { change: "assist", machine: machine.name, to, agent, ...home, assisting: true, ...until, ...caps },
          );
        return;
      }
      case "remove":
        this.remove(machine, world);
        return;
      case "undo":
        void this.undo();
        return;
      case "allow-privacy":
        this.options.allowPrivacy?.();
        return;
    }
  }

  private share(login: Login, machine: Machine): void {
    const { access, store } = this.options;
    this.giving.open(
      { title: access.shareTitle(machine.written), what: access.shareWhat, sees: access.shareSees, given: access.sharedTo, go: access.shareGo },
      sharedWith(store.world.peek(), login, machine.name),
      async (to, shared) => {
        await this.options.change(login, { change: "share", machine: machine.name, to, shared });
        await store.refresh();
        return sharedWith(store.world.peek(), login, machine.name);
      },
      {
        of: (account) => shareOf(store.world.peek(), login, machine.name, account)?.jobs ?? null,
        set: async (account, jobs) => {
          await this.options.change(login, { change: "share", machine: machine.name, to: account, shared: true, ...(jobs ? { jobs } : {}) });
          await store.refresh();
          return sharedWith(store.world.peek(), login, machine.name);
        },
      },
    );
  }

  /** The assist sheet for `agent`'s login `home` on `machine`, with
   *  `editing`'s deadline open. */
  private assist(login: Login, machine: Machine, agent: string, home: string | null, editing: string | null): void {
    const { options } = this;
    const assistances = (): Assistance[] => assistancesIn(hubOf(options.store.world.peek(), login)?.assists ?? [], machine.name, agent, home);
    const name = home ? `${agentName(agent)} ${home.split(/[\\/]/).filter(Boolean).pop() ?? home}` : agentName(agent);
    this.assistSheet.open({
      title: options.access.assistTitle(name, machine.written),
      assistances: assistances(),
      assist: async (to, terms) => {
        await options.change(login, assistChange(machine.name, agent, home, to, terms));
        await options.store.refresh();
        return assistances();
      },
      editing,
      costs: agent === "claude",
      names: namesOn(hubOf(options.store.world.peek(), login), login),
    });
  }

  /** Takes `machine` off the network, one of three ways
   *  (`world-network.md`, 移出网络与卸载). */
  private remove(machine: Machine, world: WorldNow): void {
    const { access, manage } = this.options;
    removeMachines([machine], world, manage.removeTitle(machine.written), {
      manage,
      access,
      sheet: this.removing,
      running: (name) => this.options.updates?.state.peek()?.network?.machines?.[name],
      change: this.options.change,
      ...(this.options.log ? { log: this.options.log } : {}),
      done: () => {
        void this.options.store.refresh();
        this.options.replace({ kind: "machines" });
      },
    });
  }

  /** Takes back a share or ends an assistance at once; for a moment it
   *  can be given again from the foot of the page. */
  /** Gives back the share of `machine` `login` holds, once asked: it is
   *  another's to share again, so there is nothing to undo, and the page
   *  goes back to the machines. */
  private giveBack(login: Login, machine: Machine): void {
    const { machines } = this.options;
    const owner = ownerOf(machine, login) ?? "";
    this.options.confirm.ask({
      title: machines.leaveTitle(machine.written),
      what: machines.leaveWhat(owner),
      go: machines.leave,
      act: () => this.options.change(login, { change: "share", machine: machine.name, to: login.account, shared: false }),
      done: () => {
        void this.options.store.refresh();
        location.hash = link({ kind: "machines" });
      },
    });
  }

  private async takeBack(button: HTMLButtonElement, login: Login, label: string, take: AccountChange, give: AccountChange): Promise<void> {
    button.disabled = true;
    try {
      await this.options.change(login, take);
    } catch (error) {
      button.disabled = false;
      this.say(this.options.access.failed(detailOf(error)), null);
      return;
    }
    this.say(this.options.machines.tookBack(label), () => this.options.change(login, give));
    await this.options.store.refresh();
  }

  /** A line at the foot of the page for a moment, with a way to undo what
   *  was just done when there is one. */
  private say(said: string, redo: (() => Promise<void>) | null): void {
    if (this.undoing) clearTimeout(this.undoing.timer);
    this.undoing = { timer: window.setTimeout(() => this.endUndo(), UNDO_MS), redo };
    this.undoBar.innerHTML = `<span>${escape(said)}</span>${redo ? `<button class="text-button mp-undo-go" type="button" data-act="undo">${escape(this.options.machines.undo)}</button>` : ""}`;
    this.undoBar.hidden = false;
  }

  private async undo(): Promise<void> {
    const redo = this.undoing?.redo;
    this.endUndo();
    if (!redo) return;
    try {
      await redo();
    } catch (error) {
      this.say(this.options.access.failed(detailOf(error)), null);
    }
    await this.options.store.refresh();
  }

  private endUndo(): void {
    if (this.undoing) clearTimeout(this.undoing.timer);
    this.undoing = null;
    this.undoBar.hidden = true;
    this.undoBar.innerHTML = "";
  }
}

/** Whether a machine says how it is doing: this computer always; another
 *  while online, with a fastctx that answers it. */
function askable(machine: Machine): boolean {
  if (machine.here) return true;
  if (machine.online === false) return false;
  return !!factsOf(machine)?.stats;
}

function factsOf(machine: Machine): MachineFacts | null {
  return "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
}

/** Rows of facts, the names in one column and the values in the next. */
function factRows(rows: { label: string; value: string; code?: boolean; title?: string }[]): string {
  return `<dl class="mp-rows">${rows
    .map((row) => `<dt>${escape(row.label)}</dt><dd${row.title ? ` title="${escape(row.title)}"` : ""}${row.code ? ` class="code"` : ""}><span>${escape(row.value)}</span></dd>`)
    .join("")}</dl>`;
}

function hubOf(world: WorldNow | null, login: Login): HubAccounts | null {
  return world?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
}

/** The accounts `machine` is shared with, as `login`'s hub says. */
/** The share of `machine` with `account`, as its holder's hub keeps it. */
function shareOf(world: WorldNow | null, login: Login, machine: string, account: string): ShareSeen | undefined {
  return (hubOf(world, login)?.shares ?? []).find((share) => share.machine === machine && share.account === account);
}

/** The most dots an account's jobs are drawn with; past it, a count. */
const JOB_DOTS = 12;

function sharedWith(world: WorldNow | null, login: Login, machine: string): string[] {
  return (hubOf(world, login)?.shares ?? []).filter((share) => share.machine === machine).map((share) => share.account);
}

/** A machine's direct links: with whom they are up and how fast, which are
 *  down and why, and where others connect to it when it takes them. */
function directSaid(machine: Machine, world: WorldNow, words: MachinesWords): string {
  const { nodes, links } = mapOf(world, words);
  const key = `m:${machine.written}`;
  const name = (other: string) => nodes.find((node) => node.key === other)?.name ?? other.replace(/^m:/, "");
  const mine = links.filter((one) => one.a === key || one.b === key).sort((a, b) => Number(b.up) - Number(a.up));
  const at = nodes.find((node) => node.key === key)?.directAt ?? [];
  if (!mine.length && !at.length) return "";
  const said = mine.map((one) => {
    const peer = name(one.a === key ? one.b : one.a);
    return one.up ? `<span class="direct-up">${escape(words.directUp(peer, one.ms))}</span>` : `<span class="direct-down">${escape(words.directDown(peer, one.why))}</span>`;
  });
  return `<div class="panel-direct"><span class="panel-direct-list">${said.join(`<span class="sep"></span>`)}</span>${
    at.length ? `<p class="panel-direct-at">${escape(words.takesDirectAt(at.join("、")))}</p>` : ""
  }</div>`;
}

/** The card the page follows: of those up, the machine's own before one a
 *  program made, then the one that moved the most since it came up, which
 *  keeps the choice the same from one reading to the next. */
function mainCard(cards: Card[]): Card | null {
  return [...cards].filter((card) => card.up).sort((a, b) => Number(!!b.hardware) - Number(!!a.hardware) || b.received + b.sent - (a.received + a.sent))[0] ?? null;
}

function tag(text: string, kind = "", title = ""): string {
  return `<span class="tag${kind ? ` ${kind}` : ""}"${title ? ` title="${escape(title)}"` : ""}>${kind === "tag-reach" ? glyph("shield", 12) : ""}${escape(text)}</span>`;
}

/** Sets an element's text where it changed. */
function set(element: HTMLElement, text: string): void {
  if (element.textContent !== text) element.textContent = text;
}

/** A vital's note, whole under the pointer where its cell cuts it short. */
function note(element: HTMLElement, text: string): void {
  set(element, text);
  if (element.title !== text) element.title = text;
}

/** Says `text` when `element` is held or rested on; nothing, where empty. */
function tip(element: HTMLElement, text: string): void {
  if (!text) element.removeAttribute("title");
  else if (element.title !== text) element.title = text;
}

/** A system's name and the build it ends with in brackets, apart:
 *  `Windows 11 Pro 24H2`, `(26200.6584)`. */
function withBuild(system: string): [string, string] {
  const found = /^(.+?)\s*(\([^()]*\))$/.exec(system);
  return found ? [found[1]!, found[2]!] : [system, ""];
}

/** A share as a ring says it: a whole percent, its sign small. */
function percentHtml(share: number | null): string {
  return share === null ? "—" : `${Math.round(share * 100)}<small>%</small>`;
}

/** A share of time online: whole, but to a tenth near the top, where the
 *  difference matters; a hundred only when it never went down. */
function percentOf(share: number): string {
  if (share >= 1) return "100%";
  return share >= 0.99 ? `${(Math.floor(share * 1000) / 10).toFixed(1)}%` : `${Math.round(share * 100)}%`;
}

const rate = (perSecond: number) => `${bytesSaid(Math.round(perSecond))}/s`;

/** A card's link speed, in bits a second as cards are sold. */
function bits(speed: number): string {
  const [value, unit] = speed >= 1e9 ? [speed / 1e9, "Gb/s"] : speed >= 1e6 ? [speed / 1e6, "Mb/s"] : [speed / 1e3, "Kb/s"];
  return `${Number(value.toFixed(value >= 10 ? 0 : 1))} ${unit}`;
}

/** A disk as its machine names it: `C:` for a Windows drive, a mount point
 *  as it is. */
function diskName(at: string): string {
  return /^[A-Za-z]:[\\/]?$/.test(at) ? at.slice(0, 2) : at;
}

function keptRange(): Range {
  try {
    const kept = localStorage.getItem(RANGE_KEY);
    return kept === "week" || kept === "month" ? kept : "day";
  } catch {
    return "day";
  }
}

function keepRange(range: Range): void {
  try {
    localStorage.setItem(RANGE_KEY, range);
  } catch {
    // Not kept: the span holds while the page is open.
  }
}
