// A machine's own page (`world-gui.md`, 机器的详情): how it is doing now,
// drawn — how busy its processors and graphics cards are, how full its
// memory is, how fast its busiest card moves bytes — how long it has been
// up, each card and disk, and how its lines to the hubs held over a day, a
// week or a month; then what it is, who is at work on it, and whom it is
// shared with. Its name is changed where it is written. Only a machine the
// account may work on has a page: one whose agent only assists stays on
// the drawing (`world-network.md`).
//
// While the page is in view it asks the machine every two seconds, and
// nothing while it is not; the machine sends its lines' history whole once,
// then only what came after (`machine-history.ts`).

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { loginsOf, names, type AccountChange, type HubAccounts, type Login } from "../world/accounts";
import type { NameProblem, WorldNow } from "../world/bridge";
import { agentName, osName, titleOf } from "../world/names";
import type { Card, MachineStats } from "../world/stats";
import type { MachineFacts } from "../world/world";
import type { AccessWords, MachinesWords } from "./account-words";
import { AssistSheet, assistChange, assistancesIn, type Assistance } from "./assist-sheet";
import type { Confirm } from "./confirm";
import { bytesSaid } from "./data-page";
import { redrawKeepingFocus } from "./focus";
import { diskBar, Ring, Spark, Strip, type LineMark, type LineSpan } from "./machine-draw";
import { daysOf, heldOf, keptOf, SPAN_MS, take, tripsHeld, type Sample } from "./machine-history";
import type { MachinePageWords } from "./machine-page-words";
import { GiveSheet } from "./machine-sheets";
import { linkSaid, linkWhy } from "./machines";
import { formerNames, mapOf } from "./map-of";
import type { NameSaid } from "./name-words";
import { isManaged } from "./machine-list";
import { link, talkLink, type Place } from "./places";
import { detailOf } from "./problem";
import { assistantsOn, groupsOf, inNoNetwork, isSuper, isVisitor, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameLogin, type Machine } from "./reach";
import { RenameField, renamerOf, type Renamer } from "./rename";
import type { Page } from "./shell";
import { effect, type Signal } from "./signals";
import { compareVersions, type UpdateState } from "../world/updates";
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
  stats(machine: string | null, after: string | null): Promise<MachineStats>;
  nameProblem(text: string): Promise<NameProblem | null>;
  change(login: Login, change: AccountChange): Promise<void>;
  renameHere(to: string): Promise<void>;
  /** Opens a new conversation with an agent on `machine`, as written. */
  talk(machine: string): void;
  confirm: Confirm;
  /** Goes to `place` in this page's stead: the machine renamed, removed, or
   *  one with no page of its own. */
  replace(place: Place): void;
  /** What the network says of versions, and having a machine that runs no
   *  application update its World program itself. */
  updates?: { state: Signal<UpdateState | null>; machine(name: string): Promise<UpdateState>; words: UpdateWords } | undefined;
}

type Range = "day" | "week" | "month";
const RANGES: Record<Range, number> = { day: 86_400_000, week: 7 * 86_400_000, month: 30 * 86_400_000 };
/** Where this device remembers the span the lines are drawn over. */
const RANGE_KEY = "machine.range";

/** How often the page asks the machine while it is in view: as often as
 *  the World itself is asked. */
const EVERY_MS = 2_000;
/** A trace breaks where readings are further apart than a few askings. */
const READ_GAP_MS = 4.5 * EVERY_MS;
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

interface Vital {
  kind: "cpu" | "memory" | "gpu" | "network";
  element: HTMLElement;
  note: HTMLElement;
  sub: HTMLElement;
  ring: Ring | null;
  spark: Spark;
  down: HTMLElement | null;
  up: HTMLElement | null;
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
  private readonly vitalsBox: HTMLElement;
  private readonly note: HTMLElement;
  private readonly cardsBox: HTMLElement;
  private readonly disksBox: HTMLElement;
  private readonly linesBox: HTMLElement;
  private readonly aboutBox: HTMLElement;
  private readonly presentBox: HTMLElement;
  private readonly sharingBox: HTMLElement;
  private readonly columns: HTMLElement;
  private readonly undoBar: HTMLElement;
  private readonly giving: GiveSheet;
  private readonly assistSheet: AssistSheet;
  private readonly stop: () => void;
  private readonly born = Date.now();
  private machine: Machine | null = null;
  /** How having this machine update itself stands, while it is asked. */
  private machineUpdate: string | null = null;
  private vitals: Vital[] = [];
  private vitalsShape = "";
  private lines: LineView[] = [];
  private linesShape: string | null = null;
  private cardsShape = "";
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

  constructor(private readonly options: MachinePageOptions) {
    this.title = options.name;
    this.giving = new GiveSheet(options.access);
    this.assistSheet = new AssistSheet(options.access);
    this.element = document.createElement("div");
    this.element.className = "machine-page";
    this.element.innerHTML = `${options.head(`<div class="mp-heading"></div>`, `<div class="mp-acts"></div>`)}
      <div class="mp-body scroller"><div class="mp-board">
        <section class="mp-id"></section>
        <p class="mp-note" hidden></p>
        <section class="mp-vitals" hidden></section>
        <div class="mp-columns">
          <div class="mp-main"><section class="mp-card mp-lines" hidden></section><section class="mp-card mp-cards" hidden></section></div>
          <div class="mp-side"><section class="mp-card mp-disks" hidden></section><section class="mp-card mp-present" hidden></section><section class="mp-card mp-sharing" hidden></section><section class="mp-card mp-about" hidden></section></div>
        </div>
      </div></div>
      <div class="mp-undo" role="status" hidden></div>`;
    const part = (selector: string) => this.element.querySelector<HTMLElement>(selector)!;
    this.heading = part(".mp-heading");
    this.acts = part(".mp-acts");
    this.board = part(".mp-board");
    this.id = part(".mp-id");
    this.vitalsBox = part(".mp-vitals");
    this.note = part(".mp-note");
    this.cardsBox = part(".mp-cards");
    this.disksBox = part(".mp-disks");
    this.linesBox = part(".mp-lines");
    this.aboutBox = part(".mp-about");
    this.presentBox = part(".mp-present");
    this.sharingBox = part(".mp-sharing");
    this.columns = part(".mp-columns");
    this.undoBar = part(".mp-undo");
    this.heading.innerHTML = `<h1 class="page-title mp-name">${escape(options.name)}</h1>`;
    this.element.addEventListener("click", (event) => this.click(event));
    document.addEventListener("visibilitychange", this.visible);
    this.stop = effect(() => this.draw(options.store.world.get()));
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
    this.renaming?.dispose();
    for (const vital of this.vitals) {
      vital.ring?.dispose();
      vital.spark.dispose();
    }
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
      const stats = await this.options.stats(machine.here ? null : machine.name, kept.last?.at ?? null);
      take(machine.name, stats, !!kept.last);
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
      for (const box of [this.id, this.vitalsBox, this.cardsBox, this.disksBox, this.linesBox, this.aboutBox, this.presentBox, this.sharingBox]) box.hidden = true;
      return;
    }
    const kept = keptOf(machine.name);
    const stats = kept.last;
    this.id.hidden = false;
    this.drawIdentity(machine, stats);
    this.drawNote(machine, stats, kept.heard);
    this.vitalsBox.hidden = !stats;
    if (stats) {
      this.drawVitals(stats, kept.samples);
      this.drawCards(stats);
      this.drawDisks(stats);
      this.drawLines();
    } else {
      this.cardsBox.hidden = true;
      this.disksBox.hidden = true;
      this.linesBox.hidden = true;
    }
    this.drawAbout(machine, stats, world);
    this.drawPresent(machine, world);
    this.drawSharing(machine, world);
    // With nothing in the wider column, the rest spreads across the page.
    this.columns.classList.toggle("single", this.linesBox.hidden && this.cardsBox.hidden);
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
      `<span class="mp-dot ${state.kind}" title="${escape(state.said)}"></span><h1 class="page-title mp-name">${escape(machine.written)}</h1>${rename}${
        former.length ? `<span class="mp-former" tabindex="0" title="${escape(words.formerly(former))}">${glyph("history", 14)}</span>` : ""
      }${late.length ? `<span class="mp-late" tabindex="0" title="${escape(words.notFollowed(late))}">${glyph("relay", 14)}</span>` : ""}<span class="mp-tags">${tags}</span>`,
    );
  }

  /** What is done to it from here: a conversation, its agents, sharing
   *  it, and the drawing with it chosen. */
  private drawActs(machine: Machine | null, world: WorldNow): void {
    const { words, machines } = this.options;
    if (!machine) return this.put("acts", this.acts, "");
    const owner = this.ownerLogin(machine, world);
    const button = (act: string, icon: Parameters<typeof glyph>[0], label: string) =>
      `<button class="text-button quiet-button mp-act" type="button" data-act="${act}" title="${escape(label)}">${glyph(icon, 15)}<span>${escape(label)}</span></button>`;
    // No one's agents live on a managed machine (`world-network.md`, 受管
    // 机器); a visitor shares none of its machines.
    const managed = isManaged(world, machine.name);
    this.put(
      "acts",
      this.acts,
      [
        managed ? "" : button("talk", "plus", machines.talk),
        managed ? "" : `<a class="text-button quiet-button mp-act" href="${link({ kind: "agents", machine: machine.written })}" title="${escape(machines.hosts)}">${glyph("agents", 15)}<span>${escape(machines.hosts)}</span></a>`,
        owner && !isVisitor(world, owner) ? button("share", "colleague", machines.share) : "",
        `<a class="icon-button mp-act-map" href="${link({ kind: "machines", focus: machine.written })}" aria-label="${escape(words.onMap)}" title="${escape(words.onMap)}">${glyph("machine", 17)}</a>`,
      ].join(""),
    );
  }

  /** What it is and how long it has been up; when it says nothing of
   *  itself, what the World knows and whether it is online. */
  private drawIdentity(machine: Machine, stats: MachineStats | null): void {
    const { words } = this.options;
    const facts = factsOf(machine);
    const form = facts?.form ?? (facts?.os === "linux" ? "server" : "desktop");
    const system = stats?.system || (facts ? osName(facts.os) : "");
    const hardware = stats
      ? [stats.cpu.model, words.threads(stats.cpu.threads), stats.memory.total ? bytesSaid(stats.memory.total) : "", stats.arch]
      : facts
        ? [facts.arch, `fastctx ${facts.version}`]
        : [];
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
      `<span class="mp-id-mark" aria-hidden="true">${glyph(form, 28)}</span><div class="mp-id-text"><p class="mp-id-system"${stats?.kernel ? ` title="${escape(stats.kernel)}"` : ""}>${escape(system)}</p><p class="mp-id-hardware">${hardware
        .filter(Boolean)
        .map(escape)
        .join(`<span class="sep">·</span>`)}</p></div>${right}`,
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
    if (text && stats) text = `${text} · ${words.lastRead(this.options.ago(Date.now() - heard))}`;
    this.note.hidden = !text;
    set(this.note, text);
    this.note.title = title;
    this.board.classList.toggle("stale", !!text && !!stats);
  }

  /** The rings and traces of how busy and how full it is: its processors,
   *  its memory, each graphics card, and the bytes of the card it uses. */
  private drawVitals(stats: MachineStats, samples: Sample[]): void {
    const { words } = this.options;
    const card = mainCard(stats.cards);
    const gpus = stats.gpus ?? [];
    const shape = `${stats.machine}|${gpus.length}|${card?.name ?? ""}`;
    if (shape !== this.vitalsShape) {
      this.vitalsShape = shape;
      for (const vital of this.vitals) {
        vital.ring?.dispose();
        vital.spark.dispose();
      }
      this.vitals = [
        this.vital("cpu", words.cpu, `${stats.machine}:cpu`),
        this.vital("memory", words.memory, `${stats.machine}:memory`),
        ...gpus.map((_, index) => this.vital("gpu", words.gpu, `${stats.machine}:gpu:${index}`)),
        ...(card ? [this.vital("network", words.network, `${stats.machine}:network`)] : []),
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
          vital.ring!.set(busy, percentHtml(busy));
          set(vital.note, words.threads(stats.cpu.threads));
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
          const { total, used, swap_total, swap_used } = stats.memory;
          const share = total ? used / total : null;
          vital.ring!.set(share, percentHtml(share));
          set(vital.note, swap_total ? words.swap(windows, bytesSaid(swap_used), bytesSaid(swap_total)) : "");
          set(vital.sub, words.of(bytesSaid(used), bytesSaid(total)));
          vital.spark.draw([{ points: series((sample) => sample.memory), max: 1, kind: "clay", area: true, gap: READ_GAP_MS }], from, at);
          break;
        }
        case "gpu": {
          const index = gpu;
          gpu += 1;
          const one = gpus[index];
          if (!one) break;
          const busy = one.busy ?? null;
          vital.ring!.set(busy, percentHtml(busy));
          set(vital.note, one.name);
          vital.note.title = one.name;
          set(vital.sub, one.memory_total ? words.vram(bytesSaid(one.memory_used), bytesSaid(one.memory_total)) : "");
          vital.spark.draw([{ points: series((sample) => sample.gpus[index] ?? null), max: 1, kind: "clay", area: true, gap: READ_GAP_MS }], from, at);
          break;
        }
        case "network": {
          if (!card) break;
          set(vital.down!, card.receiving === undefined ? "—" : rate(card.receiving));
          set(vital.up!, card.sending === undefined ? "—" : rate(card.sending));
          set(vital.note, [card.name, card.speed ? bits(card.speed) : ""].filter(Boolean).join(" · "));
          set(vital.sub, words.moved(bytesSaid(card.received), bytesSaid(card.sent)));
          const downs = series((sample) => sample.cards[card.name]?.[0] ?? null);
          const ups = series((sample) => sample.cards[card.name]?.[1] ?? null);
          let top = 1024;
          for (const [when, value] of [...downs, ...ups]) if (when >= from && value !== null && value > top) top = value;
          vital.spark.draw(
            [
              { points: downs, max: top * 1.15, kind: "clay", area: true, gap: READ_GAP_MS },
              { points: ups, max: top * 1.15, kind: "graphite", area: false, gap: READ_GAP_MS },
            ],
            from,
            at,
          );
          break;
        }
      }
    }
  }

  /** One of the vitals: its name and a note on its line, its ring (or the
   *  rates of a card), its trace, and a line under it. */
  private vital(kind: Vital["kind"], label: string, seed: string): Vital {
    const element = document.createElement("article");
    element.className = `mp-vital mp-vital-${kind}`;
    element.innerHTML = `<header class="mp-vital-head"><h2 class="mp-vital-label">${escape(label)}</h2><span class="mp-vital-note"></span></header><div class="mp-vital-body"></div><p class="mp-vital-sub"></p>`;
    const body = element.querySelector<HTMLElement>(".mp-vital-body")!;
    const spark = new Spark(kind === "network" ? "mp-spark-flow" : "");
    let ring: Ring | null = null;
    let down: HTMLElement | null = null;
    let up: HTMLElement | null = null;
    if (kind === "network") {
      const rates = document.createElement("div");
      rates.className = "mp-rates";
      rates.innerHTML = `<span class="mp-rate down"><span class="mp-arrow" aria-hidden="true">↓</span><span class="mp-rate-value"></span></span><span class="mp-rate up"><span class="mp-arrow" aria-hidden="true">↑</span><span class="mp-rate-value"></span></span>`;
      const values = rates.querySelectorAll<HTMLElement>(".mp-rate-value");
      down = values[0]!;
      up = values[1]!;
      body.append(rates);
    } else {
      ring = new Ring(seed);
      body.append(ring.element);
    }
    body.append(spark.element);
    return {
      kind,
      element,
      note: element.querySelector<HTMLElement>(".mp-vital-note")!,
      sub: element.querySelector<HTMLElement>(".mp-vital-sub")!,
      ring,
      spark,
      down,
      up,
    };
  }

  /** Each network card: its name, its link's speed, whether a program made
   *  it, where it is reached, and what it moves now and has moved. */
  private drawCards(stats: MachineStats): void {
    const { words } = this.options;
    const cards = stats.cards;
    this.cardsBox.hidden = !cards.length;
    if (!cards.length) return;
    const shape = cards.map((card) => [card.name, card.up, card.hardware, card.speed, (card.addresses ?? []).join(",")].join("|")).join(";");
    if (shape !== this.cardsShape) {
      this.cardsShape = shape;
      this.cardsBox.innerHTML = `<h2 class="mp-card-title">${escape(words.cards)}</h2><ul class="mp-list">${cards
        .map(
          (card, index) => `<li class="mp-net${card.up ? "" : " off"}" data-card="${index}">
            <div class="mp-net-head"><span class="mp-net-name" title="${escape(card.name)}">${escape(card.name)}</span>${card.speed ? `<span class="tag">${escape(bits(card.speed))}</span>` : ""}${
              card.hardware ? "" : `<span class="tag mp-tag-quiet">${escape(words.virtualCard)}</span>`
            }${card.up ? `<span class="mp-net-rates"><span class="mp-rate down"><span class="mp-arrow" aria-hidden="true">↓</span><span class="mp-rate-value"></span></span><span class="mp-rate up"><span class="mp-arrow" aria-hidden="true">↑</span><span class="mp-rate-value"></span></span></span>` : `<span class="mp-net-off">${escape(words.cardOff)}</span>`}</div>
            ${(card.addresses ?? []).length ? `<p class="mp-net-at">${(card.addresses ?? []).map(escape).join(`<span class="sep">·</span>`)}</p>` : ""}
            ${card.up ? `<p class="mp-net-moved"></p>` : ""}
          </li>`,
        )
        .join("")}</ul>`;
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
    });
  }

  /** Each disk as a bar of how full it is, with what is free. */
  private drawDisks(stats: MachineStats): void {
    const { words } = this.options;
    this.disksBox.hidden = !stats.disks.length;
    const rows = stats.disks
      .map((disk) => {
        // A disk's share moves by the byte; drawn to a five-hundredth.
        const used = disk.total ? Math.round((1 - disk.free / disk.total) * 500) / 500 : 0;
        return `<li class="mp-disk"><div class="mp-disk-head"><span class="mp-disk-at">${escape(diskName(disk.at))}</span><span class="mp-disk-kind">${escape([disk.label, disk.fs].filter(Boolean).join(" · "))}</span><span class="mp-disk-free">${escape(
          words.free(bytesSaid(disk.free), bytesSaid(disk.total)),
        )}</span></div>${diskBar(used, `${stats.machine}:${disk.at}`)}</li>`;
      })
      .join("");
    this.put("disks", this.disksBox, `<h2 class="mp-card-title">${escape(words.disks)}</h2><ul class="mp-list">${rows}</ul>`);
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
      set(view.state, line.up ? [words.lineUp(lasted), line.through ? words.lineVia(line.through) : ""].filter(Boolean).join(" · ") : words.lineDown(lasted));
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
          ? `${words.lineUp(words.span(end - start))} · ${stamp(start)}–${stamp(end)}`
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
      set(view.held, [held.online === null ? "" : words.available(percentOf(held.online)), words.drops(held.drops), held.drops ? words.longest(words.span(held.longest)) : ""].filter(Boolean).join(" · "));
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
    this.linesBox.innerHTML = `<header class="mp-card-head"><h2 class="mp-card-title">${escape(words.connection)}</h2><div class="mp-ranges" role="radiogroup" aria-label="${escape(words.connection)}">${(Object.keys(RANGES) as Range[])
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

  /** What it is that the head of the page does not say, one fact a row,
   *  the names in one column and the values in the next: its own host
   *  name, its kernel, its home, when it started, the fastctx on it, and
   *  where it stands in each network; last, removing it, for its owner. */
  private drawAbout(machine: Machine, stats: MachineStats | null, world: WorldNow): void {
    const { words, machines } = this.options;
    const facts = factsOf(machine);
    const rows: { label: string; lines: string[]; title?: string; code?: boolean; act?: string }[] = [];
    if (stats?.host) rows.push({ label: words.host, lines: [stats.host] });
    if (stats?.kernel) rows.push({ label: words.kernel, lines: [stats.kernel] });
    if (facts?.home) rows.push({ label: words.home, lines: [facts.home], code: true });
    if (stats?.boot) rows.push({ label: words.booted, lines: [words.stamp(new Date(stats.boot), new Date(stats.at))] });
    const version = stats?.node.version ?? facts?.version;
    if (version) {
      const kept = stats ? (stats.node.service ? words.service : words.noService) : "";
      const running = stats ? words.nodeFor(words.span(Date.parse(stats.at) - Date.parse(stats.node.started))) : "";
      rows.push({ label: words.version, lines: [[version, kept].filter(Boolean).join(" · ")], title: running, act: this.updateAct(machine, version, world) });
    }
    // Where it stands in each network: how this computer reaches each hub
    // and as whom, or whose machine another is there.
    const networks = machine.here
      ? world.nodes.hubs.map((hub) => {
          const stand = hub.home ? machines.registeredUnder(hub.home) : hub.logins.length ? machines.onlyLogins(hub.logins.join(machines.separator)) : "";
          return { line: [hub.name, linkSaid(hub.link, machines), stand].filter(Boolean).join(" · "), why: linkWhy(hub.link) };
        })
      : machine.on.filter((on) => on.owner).map((on) => ({ line: `${on.hubName} · ${machines.registeredUnder(on.owner!)}`, why: "" }));
    if (networks.length) rows.push({ label: words.networks, lines: networks.map((one) => one.line), title: networks.map((one) => one.why).filter(Boolean).join("\n") });
    // The group its owner put it in, on each hub that says so.
    const grouped = [...new Set(world.accounts.hubs.flatMap((hub) => hub.members.filter((member) => member.kind === "computer" && names(member, machine.name) && member.group).map((member) => member.group!)))];
    if (grouped.length) rows.push({ label: words.group, lines: grouped });
    const owner = this.ownerLogin(machine, world);
    const remove =
      owner && !machine.here
        ? `<div class="mp-own"><button class="text-button quiet-button danger" type="button" data-act="remove">${glyph("close", 14)}<span>${escape(machines.remove)}</span></button></div>`
        : "";
    this.aboutBox.hidden = !rows.length && !remove;
    this.put(
      "about",
      this.aboutBox,
      `<h2 class="mp-card-title">${escape(words.about)}</h2><dl class="mp-facts">${rows
        .map(
          (row) =>
            `<div class="mp-fact"><dt>${escape(row.label)}</dt><dd${row.title ? ` title="${escape(row.title)}"` : ""}${row.code ? ` class="code"` : ""}>${row.lines
              .map((line) => `<span>${escape(line)}</span>`)
              .join("")}${row.act ?? ""}</dd></div>`,
        )
        .join("")}</dl>${remove}`,
    );
  }

  /** Who is at work on it now and what runs there, as this device's
   *  logins see it. */
  private drawPresent(machine: Machine, world: WorldNow): void {
    const { words, machines } = this.options;
    const viewer = loginsOf(world.accounts).find((login) => machine.on.some((on) => on.hub === login.hub)) ?? null;
    const group = viewer ? (groupsOf(world).find((one) => sameLogin(one.login, viewer)) ?? null) : null;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine.name && (r.now?.activity === "busy" || r.now?.waiting_for_user || !!r.now?.asked) && (!group || residentIn(world, group, r)),
    );
    const jobs = jobsOf(world).filter((job) => job.machine === machine.name);
    // Its owner is told how many are at work or waiting there, whosever
    // they are; those of other accounts go without their names.
    const answered = world.who.machines.find((one) => one.kind === "answered" && one.reply.machine === machine.name);
    const busy = answered?.kind === "answered" ? (answered.reply.busy ?? 0) : 0;
    const others = Math.max(0, busy - residents.length);
    this.presentBox.hidden = !residents.length && !jobs.length && !others;
    const agents = residents
      .map(
        (r) =>
          `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : "mark-busy"}" aria-hidden="true"></span>${escape(
            [agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" · "),
          )}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
      )
      .join("");
    this.put(
      "present",
      this.presentBox,
      `<h2 class="mp-card-title">${escape(words.present)}</h2>${agents ? `<ul class="machine-agents">${agents}</ul>` : ""}${others ? `<p class="machine-jobs">${escape(words.othersAtWork(others))}</p>` : ""}${jobs.length ? `<p class="machine-jobs">${escape(machines.jobs(jobs.length))}</p>` : ""}`,
    );
  }

  /** Whom it is shared with and whom its agents assist, for its owner,
   *  each taken back with its cross; and its direct links. */
  private drawSharing(machine: Machine, world: WorldNow): void {
    const { words, machines, access } = this.options;
    const owner = this.ownerLogin(machine, world);
    const direct = directSaid(machine, world, machines);
    this.sharingBox.hidden = !owner && !direct;
    if (!owner && !direct) return;
    const now = this.options.store.clock.peek();
    const chip = (label: string, take: string) =>
      `<span class="chip">${escape(label)}<button class="chip-x" type="button" ${take} aria-label="${escape(machines.takeBack(label))}" title="${escape(machines.takeBack(label))}">${glyph("close", 12)}</button></span>`;
    const shared = owner && !isVisitor(world, owner)
      ? `<div class="given-row"><span class="given-label">${escape(machines.sharedTo)}</span>${sharedWith(world, owner, machine.name)
          .map((to) => chip(to, `data-act="unshare" data-to="${escape(to)}"`))
          .join("")}<button class="text-button quiet-button" type="button" data-act="share">${glyph("plus", 13)}<span>${escape(machines.share)}</span></button></div>`
      : "";
    const assists = owner ? (hubOf(world, owner)?.assists ?? []).filter((assist) => assist.machine === machine.name) : [];
    const assisting = assists.length
      ? `<div class="given-row"><span class="given-label">${escape(machines.assisting)}</span>${assists
          .map((assist) => {
            const label = `${agentName(assist.agent)} · ${assist.account}`;
            const at = `data-to="${escape(assist.account)}" data-agent="${escape(assist.agent)}"${assist.home ? ` data-home="${escape(assist.home)}"` : ""}${assist.until ? ` data-until="${escape(assist.until)}"` : ""}`;
            const until = assist.until ? new Date(assist.until) : null;
            const said = until ? access.moment.until(until, now) : access.when.none;
            const day = until ? `<span class="assist-tag-until">${glyph("history", 12)}${escape(access.moment.short(until, now))}</span>` : "";
            return `<span class="chip assist-chip"><button class="assist-tag-open" type="button" data-act="assist-edit" ${at} title="${escape(`${access.changeUntil} · ${said}`)}">${escape(label)}${day}</button><button class="chip-x" type="button" data-act="assist-end" ${at} aria-label="${escape(machines.endAssist(assist.account))}" title="${escape(machines.endAssist(assist.account))}">${glyph("close", 12)}</button></span>`;
          })
          .join("")}</div>`
      : "";
    this.put("sharing", this.sharingBox, `<h2 class="mp-card-title">${escape(words.sharing)}</h2>${shared}${assisting}${direct}`);
  }

  private stateOf(machine: Machine): { kind: string; said: string } {
    const { machines } = this.options;
    const now = this.options.store.clock.peek();
    const at = machine.since ? this.options.when(new Date(machine.since), now) : null;
    if (machine.here || machine.online === true) return { kind: "online", said: at && !machine.here ? machines.online(at) : machines.onlineNow };
    if (machine.online === false) return { kind: "offline", said: machines.offline(at) };
    return { kind: "unknown", said: machines.unknown };
  }

  /** The login a machine is the account's own under: this computer's on a
   *  hub it is registered on, or the one that registered another. */
  /** The way to have another machine that runs no application update
   *  itself, for its owner, or a managed one's super administrator, while
   *  the network knows a newer release than it runs. */
  private updateAct(machine: Machine, version: string, world: WorldNow): string {
    const updates = this.options.updates;
    if (!updates || machine.here) return "";
    const said = this.machineUpdate;
    if (said) return `<span class="mp-update-said">${escape(said)}</span>`;
    const state = updates.state.peek();
    const running = state?.network?.machines?.[machine.name];
    const offer = state?.offer;
    if (!offer || (running && running.app) || compareVersions(offer.version, version) <= 0) return "";
    const owner = this.ownerLogin(machine, world);
    const administered = isManaged(world, machine.name) && loginsOf(world.accounts).some((login) => isSuper(world, login));
    if (!owner && !administered) return "";
    return `<button class="text-button quiet-button" type="button" data-act="update-machine" title="${escape(updates.words.machineBehind(offer.version))}">${escape(updates.words.updateMachine)}</button>`;
  }

  private redrawAbout(): void {
    const world = this.options.store.world.peek();
    if (world && this.machine) this.drawAbout(this.machine, keptOf(this.machine.name).last, world);
  }

  private updateMachine(machine: Machine): void {
    const updates = this.options.updates;
    if (!updates || this.machineUpdate) return;
    this.machineUpdate = updates.words.machineUpdating;
    this.redrawAbout();
    updates.machine(machine.name).then(
      () => {
        this.machineUpdate = null;
        this.redrawAbout();
      },
      (error: unknown) => {
        this.machineUpdate = detailOf(String(error));
        this.redrawAbout();
      },
    );
  }

  private ownerLogin(machine: Machine, world: WorldNow): Login | undefined {
    const logins = loginsOf(world.accounts);
    if (machine.here) return logins.find((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account));
    return logins.find((login) => owns(machine, login));
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
    const target = (event.target as HTMLElement).closest<HTMLElement>("[data-act], [data-range]");
    if (!target || (target as HTMLButtonElement).disabled) return;
    if (target.dataset.range) {
      this.range = target.dataset.range as Range;
      keepRange(this.range);
      this.drawLines();
      return;
    }
    const world = this.options.store.world.peek();
    const machine = this.machine;
    if (!world || !machine) return;
    const owner = this.ownerLogin(machine, world);
    const button = target as HTMLButtonElement;
    switch (target.dataset.act) {
      case "rename":
        return this.startRename();
      case "talk":
        return this.options.talk(machine.written);
      case "share":
        if (owner) this.share(owner, machine);
        return;
      case "unshare": {
        const to = target.dataset.to ?? "";
        if (owner) void this.takeBack(button, owner, to, { change: "share", machine: machine.name, to, shared: false }, { change: "share", machine: machine.name, to, shared: true });
        return;
      }
      case "assist-edit":
        if (owner) this.assist(owner, machine, target.dataset.agent ?? "", target.dataset.home ?? null, target.dataset.to ?? null);
        return;
      case "assist-end": {
        const to = target.dataset.to ?? "";
        const agent = target.dataset.agent ?? "";
        const home = target.dataset.home ? { home: target.dataset.home } : {};
        // Given again by the undo, it ends when it was to.
        const until = target.dataset.until ? { until: target.dataset.until } : {};
        if (owner)
          void this.takeBack(
            button,
            owner,
            `${agentName(agent)} · ${to}`,
            { change: "assist", machine: machine.name, to, agent, ...home, assisting: false },
            { change: "assist", machine: machine.name, to, agent, ...home, assisting: true, ...until },
          );
        return;
      }
      case "remove":
        if (owner) this.remove(owner, machine);
        return;
      case "update-machine":
        this.updateMachine(machine);
        return;
      case "undo":
        void this.undo();
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
    );
  }

  /** The assist sheet for `agent`'s login `home` on `machine`, with
   *  `editing`'s deadline open. */
  private assist(login: Login, machine: Machine, agent: string, home: string | null, editing: string | null): void {
    const { options } = this;
    const assistances = (): Assistance[] => assistancesIn(hubOf(options.store.world.peek(), login)?.assists ?? [], machine.name, agent, home);
    const name = home ? `${agentName(agent)} · ${home.split(/[\\/]/).filter(Boolean).pop() ?? home}` : agentName(agent);
    this.assistSheet.open(
      options.access.assistTitle(name, machine.written),
      assistances(),
      async (to, until, weekly, assisting) => {
        await options.change(login, assistChange(machine.name, agent, home, to, until, weekly, assisting));
        await options.store.refresh();
        return assistances();
      },
      editing,
      agent === "claude",
    );
  }

  private remove(login: Login, machine: Machine): void {
    const { access } = this.options;
    this.options.confirm.ask({
      title: access.removeTitle(machine.written),
      what: access.removeWhat(machine.written),
      go: access.removeGo,
      act: () => this.options.change(login, { change: "remove", name: machine.name, reason: access.reasonDefault }),
      done: () => {
        void this.options.store.refresh();
        this.options.replace({ kind: "machines" });
      },
    });
  }

  /** Takes back a share or ends an assistance at once; for a moment it
   *  can be given again from the foot of the page. */
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

function hubOf(world: WorldNow | null, login: Login): HubAccounts | null {
  return world?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
}

/** The accounts `machine` is shared with, as `login`'s hub says. */
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
  return `<div class="panel-direct"><span class="given-label">${escape(words.directRow)}</span><span class="panel-direct-list">${said.join(`<span class="sep">·</span>`)}</span>${
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
