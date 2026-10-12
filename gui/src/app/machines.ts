// The machines a login may use (`world-gui.md`, 账号、机器、共享与借出),
// drawn: the account's World as a hand-drawn map that moves under the hand
// (`machine-map.ts`). Choosing a hub or a machine floats its actions beside
// it, small icons that say what they do when pointed at; the right button,
// or a long press, lists them with their names. What is known of the one
// chosen stands in a panel that opens only when asked for. Beside each hub
// a dashed ring adds a machine: it says one is on its way once a way in was
// given, and the machine is circled as it arrives. A machine of the
// account's own is shared and removed from here; whom it is shared with is
// taken back with a press. Long sayings stand behind a
// pointer's rest, not on the page.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import type { Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded } from "../pen/shapes";
import type { AccountChange, HubAccounts, Login, MemberSeen } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, osName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet } from "./machine-sheets";
import { MachineLog, type ReadLog } from "./machine-log";
import { MachineMap, type LineSeen } from "./machine-map";
import { RING } from "./map-layout";
import { mapOf } from "./map-of";
import type { MapNode } from "./network-map";
import { link, talkLink } from "./places";
import { borrowedOf, groupsOf, isComputer, isSuper, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameLogin, superOf, type Borrowed, type Machine } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import { jobsOf, residentsOf, type Store } from "./store";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  access: AccessWords;
  head(title: string, actions?: string): string;
  /** The machine to bring into view, when one was named. */
  focus: string | null;
  /** When, as the front page says it: a clock time today, a day before. */
  when(at: Date, now: Date): string;
  /** Makes a change as a login. */
  change(login: Login, change: AccountChange): Promise<void>;
  /** Opens the sheet that lets another device into `login`; once it
   *  closes, whether a way in was given. */
  invite(login: Login): Promise<boolean> | void;
  /** Opens a new conversation with an agent on `machine`, as written. */
  talk(machine: string): void;
  confirm: Confirm;
  /** Reads the hub's record of changes, a page at a time. */
  log: ReadLog;
}

/** One thing on the map, as the page knows it. */
type Spot = { kind: "machine"; machine: Machine } | { kind: "lent"; lent: Borrowed[] } | { kind: "hub"; hub: HubAccounts };

type GlyphName = Parameters<typeof glyph>[0];

/** What can be done to a hub or machine, as an icon beside it and a line
 *  in its menu. */
interface Act {
  act: string;
  label: string;
  icon: GlyphName;
  danger?: boolean;
}


/** How long a taking back can be undone from the drawing's corner. */
const UNDO_MS = 6_000;

/** How long a machine that just arrived stays circled: the drawing-in of
 *  the circle and a moment to see it. */
const ARRIVED_MS = 6_000;

/** Past this many machines others' groups start folded. */
const FOLD_PAST = 24;

/** The port offered first for direct connections: the one after the hub's
 *  own, 7443. */
const DIRECT_PORT = 7444;

export class MachinesPage implements Page {
  readonly element: HTMLElement;
  readonly title: string;
  private readonly panel: HTMLElement;
  private readonly canvas: HTMLElement;
  private readonly map: MachineMap;
  /** The actions floating beside the one chosen, the menu, the tags of
   *  whom each machine is given to, and the moment to undo a taking back. */
  private readonly acts: HTMLElement;
  private readonly menu: HTMLElement;
  private readonly undoBar: HTMLElement;
  private undoing: { timer: number; redo: () => Promise<void> } | null = null;
  /** The card asking to confirm a change made on the drawing, or to fill
   *  in what one needs. */
  private readonly card: HTMLElement;
  private cardFor: string | null = null;
  private drawn = "";
  private floated = "";
  private readonly stop: () => void;
  /** The spot chosen, by its key on the map. */
  private selected: string | null;
  private detailsOpen: boolean;
  /** On a narrow page the chosen one's sheet shows its name and actions;
   *  pulled up, everything known of it. */
  private grown = false;
  /** When the sheet was last pulled, so the press that pulled it is not
   *  also taken for a tap. */
  private pulled = 0;
  /** Redraws when the page turns narrow or wide: the sheet and what floats
   *  beside the one chosen depend on it. */
  private readonly resized: ResizeObserver;
  private readonly log: MachineLog;
  /** The one to bring into view once the drawing has drawn its next
   *  frame at its size: until then where it stands is not yet known. */
  private bring: string | null = null;
  private menuFor: { key: string | null; at: Point; sheet: boolean } | null = null;
  private scale = 1;
  private note: { machine: string; text: string } | null = null;
  private readonly giving: GiveSheet;
  /** Hubs a machine is on its way to, with the machines each had when a
   *  way in was given. */
  private readonly waiting = new Map<string, Set<string>>();
  private readonly fresh = new Set<string>();
  /** Machines direct connections were just allowed to, while the hub
   *  checks it reaches them. */
  private readonly opening = new Set<string>();
  private readonly tip: HTMLElement;
  /** Machines framed together with the mouse, and those inside the frame
   *  while it is drawn. */
  private framed: string[] = [];
  private framing: string[] = [];
  /** Groups the user folded or opened, by key; the rest follow the
   *  default. */
  private readonly folds = new Map<string, boolean>();

  constructor(private readonly options: MachinesOptions) {
    const { words } = options;
    this.giving = new GiveSheet(options.access);
    this.title = options.focus ?? words.title;
    this.selected = options.focus ? `m:${options.focus}` : null;
    // Come to a machine from elsewhere, what is known of it is open.
    this.detailsOpen = !!options.focus;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(words.title)}</h1>`,
        `<button class="text-button add-device" type="button" hidden>${glyph("plus", 16)}<span>${escape(words.add)}</span></button>`,
      )}
      <div class="machines-layout"><div class="machines-canvas"><div class="map-undo" role="status" hidden></div><div class="map-acts" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><div class="map-menu" role="menu" aria-label="${escape(words.actions)}" hidden></div><form class="map-card" tabindex="-1" hidden></form><div class="map-tip" role="tooltip" hidden></div></div><aside class="machine-panel scroller" aria-live="polite" hidden></aside></div>`;
    this.panel = this.element.querySelector<HTMLElement>(".machine-panel")!;
    this.canvas = this.element.querySelector<HTMLElement>(".machines-canvas")!;
    this.acts = this.element.querySelector<HTMLElement>(".map-acts")!;
    this.menu = this.element.querySelector<HTMLElement>(".map-menu")!;
    this.undoBar = this.element.querySelector<HTMLElement>(".map-undo")!;
    this.card = this.element.querySelector<HTMLElement>(".map-card")!;
    this.tip = this.element.querySelector<HTMLElement>(".map-tip")!;
    this.map = new MachineMap({
      words,
      storageKey: "machines-map",
      event: (event) => {
        if (event.kind === "select") this.choose(event.key);
        else if (event.kind === "marquee") this.frame(event.keys, event.done);
        else if (event.kind === "context") this.openMenu(event.key, event.at, event.input !== "mouse" || matchMedia("(max-width: 760px)").matches);
      },
      frame: (frame) => {
        this.scale = frame.view.scale;
        this.float();
        if (this.bring && frame.view.height > 0) {
          const key = this.bring;
          this.bring = null;
          this.bringIntoView(key);
        }
      },
    });
    this.canvas.prepend(this.map.element);
    this.log = new MachineLog(words, options.log, () => loginsOf(options.store.world.peek()?.accounts), (at) => options.when(at, options.store.clock.peek()));
    this.canvas.append(this.log.button, this.log.panel);
    // A pointer resting on a line is told which way it is and how fast.
    this.map.element.addEventListener("pointermove", (event) => this.tell(event));
    this.map.element.addEventListener("pointerleave", () => this.untell());
    // A ring pressed twice opens what is known of it.
    this.map.element.addEventListener("dblclick", (event) => {
      const key = (event.target as HTMLElement).closest<HTMLElement>("[data-item]")?.dataset.item;
      if (key && !key.startsWith("add:")) this.details(key);
    });
    const add = this.element.querySelector<HTMLButtonElement>(".add-device")!;
    // Another device comes into an account: offered where there is one.
    effect(() => {
      add.hidden = !loginsOf(options.store.world.get()?.accounts).length;
    });
    effect(() => {
      this.log.button.hidden = !loginsOf(options.store.world.get()?.accounts).length;
    });
    add.addEventListener("click", () => {
      const [first] = loginsOf(options.store.world.peek()?.accounts);
      if (first) this.invite(first);
    });
    this.panel.addEventListener("click", (event) => this.click(event));
    this.panel.addEventListener("click", (event) => this.clickAct(event));
    this.panel.addEventListener("pointerdown", (event) => this.pull(event));
    this.acts.addEventListener("click", (event) => this.clickAct(event));
    this.menu.addEventListener("click", (event) => this.clickAct(event));
    this.undoBar.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".map-undo-go")) void this.undo();
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.menuFor) {
        event.stopPropagation();
        this.closeMenu();
      } else if (event.key === "Escape" && this.cardFor) {
        event.stopPropagation();
        this.closeCard();
      } else if (event.key === "Escape" && this.framed.length) {
        event.stopPropagation();
        this.framed = [];
        this.redraw();
      }
    });
    this.card.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-card=cancel]")) this.closeCard();
    });
    document.addEventListener("pointerdown", this.outside, true);
    this.stop = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
    let narrow: boolean | null = null;
    this.resized = new ResizeObserver(() => {
      if (narrow === this.narrow()) return;
      narrow = this.narrow();
      this.redraw();
      // Once the drawing has fitted itself to its new size.
      if (narrow && this.selected) {
        this.bring = this.selected;
        this.map.refresh();
      }
    });
    this.resized.observe(this.element);
  }

  leave(): void {
    this.stop();
    this.resized.disconnect();
    this.log.dispose();
    if (this.undoing) clearTimeout(this.undoing.timer);
    document.removeEventListener("pointerdown", this.outside, true);
    this.map.dispose();
  }

  /** A press anywhere but the menu closes it. */
  private readonly outside = (event: Event): void => {
    if (this.menuFor && !this.menu.contains(event.target as Node)) this.closeMenu();
  };

  private loginOf(element: HTMLElement): Login | undefined {
    const at = element.closest<HTMLElement>("[data-hub]");
    return loginsOf(this.options.store.world.peek()?.accounts).find((l) => l.hub === at?.dataset.hub && l.account === at?.dataset.account);
  }

  private click(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-act]");
    if (!button || button.disabled) return;
    const login = this.loginOf(button);
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    switch (button.dataset.act) {
      case "share":
        if (login) this.share(login, machine);
        return;
      case "unshare":
        if (login) void this.act(button, login, { change: "share", machine, to: button.dataset.to ?? "", shared: false });
        return;
      case "unlend": {
        const home = button.dataset.home ? { home: button.dataset.home } : {};
        if (login) void this.act(button, login, { change: "lend", machine, to: button.dataset.to ?? "", agent: button.dataset.agent ?? "", ...home, lent: false });
        return;
      }
      case "invite":
        if (login) this.invite(login);
        return;
      case "remove":
        if (login) this.remove(login, machine);
        return;
      case "framed-pick":
        this.choose(button.dataset.key ?? null);
        return;
      case "framed-clear":
        this.framed = [];
        this.redraw();
        return;
      case "framed-share":
        this.shareFramed();
        return;
      case "framed-remove":
        this.removeFramed();
        return;
      case "close-details":
        // On a narrow page the sheet is the choice itself.
        if (this.narrow()) {
          this.selected = null;
          this.grown = false;
        } else this.detailsOpen = false;
        this.redraw();
        return;
      case "grow":
        if (Date.now() - this.pulled < 400) return;
        this.grown = !this.grown;
        this.redraw();
        return;
    }
  }

  /** Whether the page is narrow enough that what is known of the one
   *  chosen rises as a sheet from below (`machines.css`, 860 pixels). */
  private narrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width <= 860;
  }

  /** The sheet pulled by its grip: up shows everything, down shows less or
   *  lets the choice go. */
  private pull(event: PointerEvent): void {
    if (!this.narrow() || !(event.target as HTMLElement).closest(".sheet-grip")) return;
    const from = event.clientY;
    const up = (ended: PointerEvent) => {
      const dy = ended.clientY - from;
      if (Math.abs(dy) < 24) return;
      this.pulled = Date.now();
      if (dy < 0) this.grown = true;
      else if (this.grown) this.grown = false;
      else this.selected = null;
      this.redraw();
    };
    window.addEventListener("pointerup", up, { once: true });
  }

  /** On a narrow page the one chosen is brought to the middle of what the
   *  sheet leaves of the drawing. */
  private bringIntoView(key: string): void {
    const at = this.map.screenOf(key);
    if (!at || this.panel.hidden) return;
    const box = this.canvas.getBoundingClientRect();
    // The sheet stands on the bottom of the window; its height, not where
    // it is while it still rises.
    const top = document.documentElement.clientHeight - this.panel.offsetHeight;
    const room = Math.max(0, Math.min(box.height, top - box.top));
    this.map.glide(box.width / 2 - at[0], room / 2 - at[1]);
  }

  /** The hub or machine chosen on the drawing: its actions float beside it. */
  /** The groups folded to their knot. Past two dozen machines the drawing
   *  fitted to the page is too small to read them all, so others' groups
   *  start folded and the account's own open; what the user opened or
   *  folded stays so. */
  private foldedOf(nodes: MapNode[]): Set<string> {
    const many = nodes.length > FOLD_PAST;
    const folded = new Set<string>();
    for (const node of nodes) {
      const group = node.group?.key ?? "mine";
      for (const hub of node.hubs) {
        const key = `group:${hub}:${group}`;
        if (this.folds.get(key) ?? (many && group !== "mine")) folded.add(key);
      }
    }
    return folded;
  }

  /** A frame drawn round machines: one chosen, or several together. */
  private frame(keys: string[], done: boolean): void {
    const machines = keys.filter((key) => key.startsWith("m:"));
    if (!done) {
      if (JSON.stringify(machines) === JSON.stringify(this.framing)) return;
      this.framing = machines;
      const world = this.options.store.world.peek();
      if (world) this.drawMap(world);
      return;
    }
    this.framing = [];
    if (machines.length < 2) return this.choose(machines[0] ?? null);
    this.closeMenu();
    this.closeCard();
    this.framed = machines;
    this.selected = null;
    this.redraw();
  }

  private choose(key: string | null): void {
    // A group's knot folds its machines away, or opens them again.
    if (key?.startsWith("group:")) {
      const world = this.options.store.world.peek();
      const nodes = world ? mapOf(world, this.options.words).nodes : [];
      this.folds.set(key, !this.foldedOf(nodes).has(key));
      this.redraw();
      return;
    }
    this.framed = [];
    this.closeMenu();
    // The dashed ring is itself the way to add a machine.
    if (key?.startsWith("add:")) {
      this.selected = null;
      const login = this.hubLogin(key.slice(4));
      if (login) this.invite(login);
      this.redraw();
      return;
    }
    this.selected = key;
    this.note = null;
    this.grown = false;
    this.redraw();
    if (key && this.narrow()) requestAnimationFrame(() => this.bringIntoView(key));
  }

  private details(key: string): void {
    this.selected = key;
    this.detailsOpen = true;
    this.closeMenu();
    this.redraw();
    this.panel.scrollTop = 0;
  }

  /** The first login on a hub, as which a device is let in there. */
  private hubLogin(hub: string): Login | null {
    return loginsOf(this.options.store.world.peek()?.accounts).find((login) => login.hub === hub) ?? null;
  }

  /** Opens the sheet that lets another device in; once a way in was given,
   *  the hub's dashed ring says a machine is on its way. */
  private invite(login: Login): void {
    const given = this.options.invite(login);
    void Promise.resolve(given).then((yes) => {
      if (!yes) return;
      const world = this.options.store.world.peek();
      this.waiting.set(login.hub, new Set(world ? this.machineKeysOn(world, login.hub) : []));
      this.redraw();
    });
  }

  /** The keys of the machines drawn on a hub. */
  private machineKeysOn(world: WorldNow, hub: string): string[] {
    return mapOf(world, this.options.words)
      .nodes.filter((node) => node.hubs.includes(hub))
      .map((node) => node.key);
  }

  private hubOf(login: Login): HubAccounts | null {
    return this.options.store.world.peek()?.accounts.hubs.find((hub) => hub.hub === login.hub) ?? null;
  }

  /** The accounts `machine` is shared with, as `login`'s hub says. */
  private sharedWith(login: Login, machine: string): string[] {
    return (this.hubOf(login)?.shares ?? []).filter((share) => share.machine === machine).map((share) => share.account);
  }

  /** The sheet that shares `machine` with other accounts. */
  private share(login: Login, machine: string): void {
    const { access } = this.options;
    this.giving.open(
      { title: access.shareTitle(machine), what: access.shareWhat, sees: access.shareSees, given: access.sharedTo, go: access.shareGo },
      this.sharedWith(login, machine),
      async (to, shared) => {
        await this.options.change(login, { change: "share", machine, to, shared });
        await this.options.store.refresh();
        return this.sharedWith(login, machine);
      },
    );
  }

  private remove(login: Login, machine: string): void {
    const { access } = this.options;
    this.options.confirm.ask({
      title: access.removeTitle(machine),
      what: access.removeWhat(machine),
      go: access.removeGo,
      act: () => this.options.change(login, { change: "remove", name: machine, reason: access.reasonDefault }),
      done: () => void this.options.store.refresh(),
    });
  }

  /** A change made at once, with nothing to read first: taking back a
   *  share or a loan, which can be given again as easily. */
  private async act(button: HTMLButtonElement, login: Login, change: AccountChange): Promise<void> {
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    button.disabled = true;
    try {
      await this.options.change(login, change);
      this.note = null;
    } catch (error) {
      this.note = { machine, text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
    }
    await this.options.store.refresh();
    this.redraw();
  }

  private redraw(): void {
    this.drawn = "";
    this.floated = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** Everything on the map, by its key. */
  private spots(world: WorldNow): Map<string, Spot> {
    const spots = new Map<string, Spot>();
    for (const hub of world.accounts.hubs) if (hub.logins.length) spots.set(`hub:${hub.hub}`, { kind: "hub", hub });
    for (const machine of machinesOf(world)) spots.set(`m:${machine.written}`, { kind: "machine", machine });
    for (const lent of borrowedOf(world)) {
      const key = `lent:${lent.written}`;
      const known = spots.get(key);
      if (known?.kind === "lent") known.lent.push(lent);
      else spots.set(key, { kind: "lent", lent: [lent] });
    }
    return spots;
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const spots = this.spots(world);
    // A machine named in the address may be one an agent is lent on.
    if (this.selected?.startsWith("m:") && !spots.has(this.selected) && spots.has(`lent:${this.selected.slice(2)}`)) this.selected = `lent:${this.selected.slice(2)}`;
    if (this.selected && !spots.has(this.selected)) this.selected = null;
    this.drawMap(world);
    const spot = this.selected ? spots.get(this.selected) : undefined;
    // On a narrow page nothing floats beside the one chosen: its sheet rises
    // at once, its actions a row in it.
    const narrow = this.narrow();
    const framed = this.framed.filter((key) => spots.get(key)?.kind === "machine");
    const open = (!!spot && (narrow || this.detailsOpen)) || framed.length > 1;
    this.panel.hidden = !open;
    this.panel.classList.toggle("peek", narrow && !this.grown);
    const alone = !loginsOf(world.accounts).length && isComputer(world);
    this.canvas.classList.toggle("alone", alone);
    const acts = narrow && this.selected ? this.sheetActs(this.selected, world) : "";
    const grip = narrow
      ? `<button class="sheet-grip" type="button" data-act="grow" aria-expanded="${this.grown}" aria-label="${escape(this.grown ? words.closeDetails : words.details)}"></button>`
      : "";
    const html = alone
      ? `<p class="machines-alone">${escape(words.alone)} <a class="text-link" href="${link({ kind: "login" })}">${escape(words.logIn)}</a></p>`
      : framed.length > 1
        ? this.framedPanel(framed, world)
      : spot && open
        ? `${grip}<button class="icon-button panel-close" type="button" data-act="close-details" aria-label="${escape(words.closeDetails)}" title="${escape(words.closeDetails)}">${glyph("close", 16)}</button>${spot.kind === "hub" ? this.hubPanel(spot.hub, world, acts) : spot.kind === "lent" ? this.lentPanel(spot.lent, world, now, acts) : this.machinePanel(spot.machine, world, now, acts)}`
        : "";
    if (alone) this.panel.hidden = false;
    if (html !== this.drawn) {
      this.drawn = html;
      redrawKeepingFocus(this.panel, () => {
        this.panel.innerHTML = html;
      });
    }
    this.float();
  }

  /** The drawing: each hub the account is on with the ring that adds a
   *  machine there, each machine it may work on, and each machine of
   *  another an agent is lent on. A machine come to a hub one was on its
   *  way to is circled as it arrives. */
  private drawMap(world: WorldNow): void {
    const { hubs, nodes, hubLabels, links } = mapOf(world, this.options.words);
    for (const [hub, before] of this.waiting) {
      const arrived = nodes.filter((node) => node.hubs.includes(hub) && !before.has(node.key));
      if (!arrived.length) continue;
      this.waiting.delete(hub);
      for (const node of arrived) {
        this.fresh.add(node.key);
        setTimeout(() => {
          this.fresh.delete(node.key);
          this.redraw();
        }, ARRIVED_MS);
      }
    }
    const adds = hubs.filter((hub) => this.hubLogin(hub.id)).map((hub) => hub.id);
    this.map.update({
      hubs,
      nodes,
      hubLabels,
      adds,
      waiting: new Set(this.waiting.keys()),
      selected: this.selected,
      fresh: this.fresh,
      links,
      opening: this.opening,
      framed: this.framing.length ? this.framing : this.framed,
      folded: this.foldedOf(nodes),
    });
  }

  /** What can be done to a spot, as `world` stands. */
  private actsOf(key: string, world: WorldNow): Act[] {
    const { words } = this.options;
    const spot = this.spots(world).get(key);
    if (!spot) return [];
    const details: Act = { act: "details", label: words.details, icon: "info" };
    if (spot.kind === "hub") {
      const logins = loginsOf(world.accounts).filter((login) => login.hub === spot.hub.hub);
      const top = logins.some((login) => spot.hub.accounts.find((account) => account.name === login.account)?.level === "super");
      return [
        ...(logins.length ? [{ act: "invite", label: words.inviteDevice, icon: "plus" as const }] : []),
        ...(top ? [{ act: "manage", label: words.manageHub, icon: "settings" as const }] : []),
        details,
      ];
    }
    if (spot.kind === "lent") return [{ act: "talk", label: words.talk, icon: "plus" }, details];
    const owners = this.ownersOf(spot.machine, world);
    // Direct connections are the network's administrator's to allow: to a
    // computer, while it is online to be checked, and no longer at any time.
    const admin = this.adminOf(spot.machine, world);
    const member = admin ? this.memberOf(spot.machine, admin, world) : undefined;
    const direct: Act[] =
      member?.kind !== "computer" ? [] : member.direct?.length ? [{ act: "direct", label: words.stopDirect, icon: "link" }] : member.reach ? [{ act: "direct", label: words.allowDirect, icon: "link" }] : [];
    return [
      { act: "talk", label: words.talk, icon: "plus" },
      { act: "agents", label: words.hosts, icon: "agents" },
      ...(owners.length ? [{ act: "share", label: words.share, icon: "colleague" as const }] : []),
      ...direct,
      details,
      ...(owners.length && !spot.machine.here ? [{ act: "remove", label: words.remove, icon: "close" as const, danger: true }] : []),
    ];
  }

  /** The actions of the one chosen as a row in its sheet, each with its
   *  name, large enough for a finger. */
  private sheetActs(key: string, world: WorldNow): string {
    const acts = this.actsOf(key, world).filter((act) => act.act !== "details");
    return acts.length
      ? `<div class="sheet-acts">${acts
          .map(
            (act) =>
              `<button class="sheet-act${act.danger ? " danger" : ""}" type="button" data-map-act="${act.act}" data-key="${escape(key)}">${glyph(act.icon, 18)}<span>${escape(act.label)}</span></button>`,
          )
          .join("")}</div>`
      : "";
  }

  /** The logins whose own a machine is: this computer's account on each
   *  hub it is registered on, or the one that registered another. */
  private ownersOf(machine: Machine, world: WorldNow): Login[] {
    const logins = loginsOf(world.accounts);
    if (machine.here) return logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account));
    const owner = logins.find((login) => owns(machine, login));
    return owner ? [owner] : [];
  }

  /** Does an action chosen beside a spot or in its menu. */
  private clickAct(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-map-act]");
    const world = this.options.store.world.peek();
    if (!button || !world) return;
    const key = button.dataset.key ?? "";
    const act = button.dataset.mapAct!;
    this.closeMenu();
    if (act === "fit") return this.map.fit();
    const spot = this.spots(world).get(key);
    if (!spot) return;
    if (act === "details") return this.details(key);
    if (spot.kind === "hub") {
      const login = this.hubLogin(spot.hub.hub);
      if (act === "invite" && login) this.invite(login);
      else if (act === "manage") location.hash = link({ kind: "admin" });
      return;
    }
    if (spot.kind === "lent") {
      if (act === "talk") this.options.talk(spot.lent[0]!.written);
      return;
    }
    const machine = spot.machine;
    const login = this.ownersOf(machine, world)[0];
    switch (act) {
      case "talk":
        this.options.talk(machine.written);
        return;
      case "agents":
        location.hash = link({ kind: "agents", machine: machine.written });
        return;
      case "share":
        if (login) this.share(login, machine.name);
        return;
      case "remove":
        if (login) this.remove(login, machine.name);
        return;
      case "direct": {
        const admin = this.adminOf(machine, world);
        const member = admin ? this.memberOf(machine, admin, world) : undefined;
        if (!admin || !member) return;
        if (member.direct?.length) this.stopDirect(admin, machine, key, member.direct);
        else this.allowDirect(admin, machine, key, member);
        return;
      }
    }
  }

  /** The login of the network's administrator among this device's, on a
   *  hub `machine` is on. */
  private adminOf(machine: Machine, world: WorldNow): Login | undefined {
    return loginsOf(world.accounts).find((login) => machine.on.some((on) => on.hub === login.hub) && isSuper(world, login));
  }

  /** What `login`'s hub says of `machine`. */
  private memberOf(machine: Machine, login: Login, world: WorldNow): MemberSeen | undefined {
    return world.accounts.hubs.find((hub) => hub.hub === login.hub)?.members.find((member) => member.name === machine.name);
  }

  /** The card that lets other machines connect to one directly: a small
   *  drawing of their lines meeting it, its addresses to choose from (its
   *  network cards' and where the server sees it, never an example), and
   *  the port. The server then tries it from outside; until it has, the
   *  socket on its ring is pencil, and what kept the server out is said in
   *  the card. */
  private allowDirect(login: Login, machine: Machine, key: string, member: MemberSeen): void {
    const { words } = this.options;
    const seen = member.reach?.seen ? [hostOf(member.reach.seen)] : [];
    const cards = (member.reach?.interfaces ?? []).map(hostOf).filter((host) => !seen.includes(host));
    const choice = (host: string, note: string, checked: boolean) =>
      `<label class="choice map-card-choice"><input type="checkbox" name="host" value="${escape(host)}"${checked ? " checked" : ""}><span class="mono">${escape(host)}</span><span class="choice-note">${escape(note)}</span></label>`;
    const at = this.map.screenOf(key) ?? [0, 0];
    this.openCard(
      key,
      at,
      `<p class="map-card-title">${escape(words.directTitle(machine.written))}</p>
       <div class="map-card-pictures">${directPicture(true)}</div>
       <fieldset class="map-card-field"><legend>${escape(words.directAddresses)}</legend><div class="choice-options">${[
         ...seen.map((host) => choice(host, words.directSeen, true)),
         ...cards.map((host) => choice(host, words.directCard, !seen.length)),
       ].join("")}</div></fieldset>
       <label class="field map-card-port"><span class="field-label">${escape(words.directPort)}</span><input class="mono" name="port" inputmode="numeric" value="${DIRECT_PORT}" autocomplete="off" spellcheck="false" required></label>
       <p class="map-card-wrong" role="alert" hidden></p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.directWhat)}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.directGo)}</button></footer>`,
      async () => {
        const wrong = this.card.querySelector<HTMLElement>(".map-card-wrong")!;
        const hosts = [...this.card.querySelectorAll<HTMLInputElement>("input[name=host]:checked")].map((input) => input.value);
        const port = Number(this.card.querySelector<HTMLInputElement>("input[name=port]")!.value.trim());
        const refuse = (said: string) => {
          wrong.textContent = said;
          wrong.hidden = false;
          return new Error("kept open");
        };
        if (!hosts.length) throw refuse(words.directPickOne);
        if (!Number.isInteger(port) || port < 1 || port > 65535) throw refuse(words.directPortWrong);
        wrong.textContent = words.directChecking;
        wrong.hidden = false;
        wrong.classList.add("quiet");
        this.opening.add(key);
        this.redraw();
        try {
          await this.options.change(login, { change: "direct", machine: machine.name, at: hosts.map((host) => (host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`)) });
        } catch (error) {
          wrong.classList.remove("quiet");
          throw refuse(error instanceof Error ? error.message : String(error));
        } finally {
          this.opening.delete(key);
          this.redraw();
        }
      },
    );
  }

  /** Takes direct connections to a machine away at once, after a small
   *  card of before and after; for a moment they can be given back. */
  private stopDirect(login: Login, machine: Machine, key: string, before: string[]): void {
    const { words } = this.options;
    const at = this.map.screenOf(key) ?? [0, 0];
    this.openCard(
      key,
      at,
      `<p class="map-card-title">${escape(words.stopDirectTitle(machine.written))}</p>
       <div class="map-card-pictures">${directPicture(true)}<span aria-hidden="true">→</span>${directPicture(false)}</div>
       <p class="map-card-said">${escape(words.stopDirectSaid)}</p>
       <p class="map-card-wrong" role="alert" hidden></p>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.confirm)}</button></footer>`,
      async () => {
        await this.options.change(login, { change: "direct", machine: machine.name, at: [] });
        this.offerUndo(words.directStopped(machine.written), () => this.options.change(login, { change: "direct", machine: machine.name, at: before }));
      },
    );
  }

  /** A card beside a ring, or rising from below on a narrow page; `go`
   *  runs when it is confirmed, and an error it throws stays in the card. */
  private openCard(key: string, at: Point, html: string, go: () => Promise<void>): void {
    this.closeMenu();
    this.cardFor = key;
    this.float();
    this.card.innerHTML = html;
    const sheet = this.narrow();
    this.card.classList.toggle("as-sheet", sheet);
    this.card.hidden = false;
    if (!sheet) {
      // Beside the ring, on its right unless what is open on the right, the
      // panel or the canvas's edge, leaves no room there.
      const canvas = this.canvas.getBoundingClientRect();
      const right = this.panel.hidden ? canvas.width : Math.min(canvas.width, this.panel.getBoundingClientRect().left - canvas.left);
      const width = this.card.offsetWidth;
      const x = at[0] + 16 + width + 8 <= right ? at[0] + 16 : at[0] - 16 - width;
      const y = Math.min(this.canvas.clientHeight - this.card.offsetHeight - 8, at[1] - 24);
      this.card.style.transform = `translate(${Math.max(8, Math.min(x, right - width - 8))}px, ${Math.max(8, y)}px)`;
    } else this.card.style.transform = "";
    this.card.onsubmit = (event) => {
      event.preventDefault();
      const button = this.card.querySelector<HTMLButtonElement>(".map-card-go");
      if (button) button.disabled = true;
      go().then(
        () => {
          this.closeCard();
          void this.options.store.refresh().then(() => this.redraw());
        },
        (error: unknown) => {
          if (button) button.disabled = false;
          if (error instanceof Error && error.message === "kept open") return;
          const wrong = this.card.querySelector<HTMLElement>(".map-card-wrong");
          const said = error instanceof Error ? error.message : String(error);
          if (wrong) {
            wrong.textContent = said;
            wrong.hidden = false;
          } else {
            this.note = { machine: key.replace(/^m:/, ""), text: this.options.access.failed(said) };
            this.redraw();
          }
        },
      );
    };
    this.card.focus({ preventScroll: true });
  }

  /** Says which line the pointer rests on, beside it. */
  private tell(event: PointerEvent): void {
    if (event.pointerType === "touch" || event.buttons) return this.untell();
    const box = this.map.element.getBoundingClientRect();
    const at: Point = [event.clientX - box.left, event.clientY - box.top];
    const seen = (event.target as HTMLElement).closest("[data-item]") ? null : this.map.lineAt(at);
    const said = seen ? this.lineSaid(seen) : "";
    if (!said) return this.untell();
    this.tip.textContent = said;
    this.tip.hidden = false;
    const x = Math.min(this.canvas.clientWidth - this.tip.offsetWidth - 8, at[0] + 14);
    const y = Math.max(8, at[1] - this.tip.offsetHeight - 10);
    this.tip.style.transform = `translate(${Math.max(8, x).toFixed(0)}px, ${y.toFixed(0)}px)`;
  }

  private untell(): void {
    if (!this.tip.hidden) this.tip.hidden = true;
  }

  private lineSaid(seen: LineSeen): string {
    const { words } = this.options;
    const world = this.options.store.world.peek();
    if (!world) return "";
    const { nodes } = mapOf(world, words);
    const name = (key: string) => nodes.find((node) => node.key === key)?.name ?? key.replace(/^m:/, "");
    switch (seen.kind) {
      case "hub":
        return words.lineHub(name(seen.machine), seen.ms);
      case "through":
        return words.lineThrough(name(seen.machine), name(seen.through));
    }
  }

  private closeCard(): void {
    if (!this.cardFor) return;
    this.cardFor = null;
    this.card.hidden = true;
    this.card.innerHTML = "";
    this.card.onsubmit = null;
    this.float();
  }

  /** A change made on the drawing, undone for a moment from its corner. */
  private offerUndo(said: string, redo: () => Promise<void>): void {
    if (this.undoing) clearTimeout(this.undoing.timer);
    this.undoing = { timer: window.setTimeout(() => this.endUndo(), UNDO_MS), redo };
    this.undoBar.innerHTML = `<span>${escape(said)}</span><button class="text-button map-undo-go" type="button">${escape(this.options.words.undo)}</button>`;
    this.undoBar.hidden = false;
  }

  private async undo(): Promise<void> {
    const undoing = this.undoing;
    if (!undoing) return;
    this.endUndo();
    try {
      await undoing.redo();
    } catch (error) {
      this.note = { machine: "", text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
    }
    await this.options.store.refresh();
    this.redraw();
  }

  private endUndo(): void {
    if (this.undoing) clearTimeout(this.undoing.timer);
    this.undoing = null;
    this.undoBar.hidden = true;
    this.undoBar.innerHTML = "";
  }

  /** The icons beside the one chosen, in a row to the right of its name,
   *  or under its ring where the right has no room; the names on the
   *  drawing keep clear of them. */
  private float(): void {
    const world = this.options.store.world.peek();
    const key = this.menuFor || this.cardFor || this.narrow() ? null : this.selected;
    const at = key ? this.map.screenOf(key) : null;
    if (!world || !key || !at) {
      if (!this.acts.hidden) {
        this.acts.hidden = true;
        this.acts.innerHTML = "";
        this.floated = "";
      }
      this.map.reserve([]);
      return;
    }
    const acts = this.actsOf(key, world);
    const html = acts
      .map(
        (act) =>
          `<button class="map-act${act.danger ? " danger" : ""}" type="button" data-map-act="${act.act}" data-key="${escape(key)}" aria-label="${escape(act.label)}" title="${escape(act.label)}">${glyph(act.icon, 18)}</button>`,
      )
      .join("");
    if (html !== this.floated) {
      this.floated = html;
      this.acts.innerHTML = html;
    }
    this.acts.hidden = false;
    const width = this.canvas.clientWidth;
    const height = this.canvas.clientHeight;
    const size = matchMedia("(pointer: coarse)").matches ? 44 : 36;
    const gap = 6;
    const count = this.acts.children.length;
    const row = count * size + (count - 1) * gap;
    // Past where its name stands right of its ring, measured from the ring
    // so the row does not follow the name when the name makes way for it.
    const right = at[0] + Math.max(RING * this.scale, 14) + 4 + this.map.nameWidth(key) + 14;
    const beside = right + row <= width - 4;
    const startX = beside ? right : Math.min(width - row - 4, Math.max(4, at[0] - row / 2));
    const y = beside ? at[1] - size / 2 : Math.min(height - size - 4, at[1] + Math.max(RING * this.scale, 14) + 10);
    const boxes = [...this.acts.children].map((button, index) => {
      const x = startX + index * (size + gap);
      (button as HTMLElement).style.transform = `translate(${x.toFixed(1)}px, ${Math.max(4, y).toFixed(1)}px)`;
      return { x, y, width: size, height: size };
    });
    this.map.reserve(boxes);
  }

  /** The menu of a spot, or of the drawing itself, where the right button
   *  was pressed; on a phone it rises from below. */
  private openMenu(key: string | null, at: Point, sheet: boolean): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const { words } = this.options;
    if (key?.startsWith("add:")) return this.choose(key);
    const acts: Act[] = key ? this.actsOf(key, world) : [{ act: "fit", label: words.fit, icon: "expand" }];
    if (!acts.length) return;
    if (key) this.selected = key;
    this.menuFor = { key, at, sheet };
    this.menu.innerHTML = acts
      .map(
        (act) =>
          `<button class="map-menu-item${act.danger ? " danger" : ""}" type="button" role="menuitem" data-map-act="${act.act}" data-key="${escape(key ?? "")}">${glyph(act.icon, 16)}<span>${escape(act.label)}</span></button>`,
      )
      .join("");
    this.menu.classList.toggle("as-sheet", sheet);
    this.menu.hidden = false;
    if (!sheet) {
      const x = Math.min(this.canvas.clientWidth - this.menu.offsetWidth - 8, at[0]);
      const y = Math.min(this.canvas.clientHeight - this.menu.offsetHeight - 8, at[1]);
      this.menu.style.transform = `translate(${Math.max(8, x)}px, ${Math.max(8, y)}px)`;
    } else this.menu.style.transform = "";
    this.menu.querySelector<HTMLElement>("button")?.focus();
    this.redraw();
  }

  private closeMenu(): void {
    if (!this.menuFor) return;
    this.menuFor = null;
    this.menu.hidden = true;
    this.menu.innerHTML = "";
    this.float();
  }

  private stateOf(online: boolean | null, since: string | null, here: boolean, now: Date): { kind: string; said: string } {
    const { words } = this.options;
    const at = since ? this.options.when(new Date(since), now) : null;
    if (here || online === true) return { kind: "online", said: at ? words.online(at) : words.onlineNow };
    if (online === false) return { kind: "offline", said: words.offline(at) };
    return { kind: "unknown", said: words.unknown };
  }

  /** The head of the panel: a name, its tags, and whether it is online. */
  private panelHead(name: string, tags: string[], state: { kind: string; said: string }, lent = false): string {
    return `<header class="panel-head ${state.kind}${lent ? " lent" : ""}"><span class="dot" aria-hidden="true"></span><h2 class="panel-title">${escape(name)}</h2>${tags.join("")}<span class="panel-state">${escape(state.said)}</span></header>`;
  }

  private tag(text: string, kind = "", title = ""): string {
    return text ? `<span class="tag${kind ? ` ${kind}` : ""}"${title ? ` title="${escape(title)}"` : ""}>${kind === "tag-reach" ? glyph("shield", 12) : ""}${escape(text)}</span>` : "";
  }

  /** A row of accounts something is given to, each taken back with its
   *  cross, and the button that gives it to another. */
  private givenRow(label: string, given: string[], take: (to: string) => string, add: string): string {
    const { words } = this.options;
    const chips = given
      .map((to) => `<span class="chip">${escape(to)}<button class="chip-x" type="button" ${take(to)} aria-label="${escape(words.takeBack(to))}" title="${escape(words.takeBack(to))}">${glyph("close", 12)}</button></span>`)
      .join("");
    return `<div class="given-row"><span class="given-label">${escape(label)}</span>${chips}${add}</div>`;
  }

  private button(act: string, label: string, extra = "", icon = ""): string {
    return `<button class="text-button quiet-button" type="button" data-act="${act}"${extra}>${icon}<span>${escape(label)}</span></button>`;
  }

  /** The machines framed together: each, and what is done to all of them
   *  at once where the account owns every one. */
  private framedPanel(keys: string[], world: WorldNow): string {
    const { words } = this.options;
    const spots = this.spots(world);
    const machines = keys.map((key) => spots.get(key)).flatMap((spot) => (spot?.kind === "machine" ? [spot.machine] : []));
    const owned = this.ownedTogether(machines, world);
    const rows = machines
      .map((machine) => {
        const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
        const state = machine.online === false ? " offline" : "";
        return `<li><button class="framed-row${state}" type="button" data-act="framed-pick" data-key="m:${escape(machine.written)}"><span class="dot" aria-hidden="true"></span><span class="framed-name">${escape(machine.written)}</span><span class="framed-sub">${escape(facts ? osName(facts.os) : "")}</span></button></li>`;
      })
      .join("");
    return `<header class="panel-head"><h2 class="panel-title">${escape(words.framedTitle(machines.length))}</h2></header>
      <ul class="framed-list">${rows}</ul>
      <div class="panel-acts">${owned ? `${this.button("framed-share", words.framedShare, "", glyph("colleague", 14))}${machines.some((machine) => machine.here) ? "" : this.button("framed-remove", words.framedRemove, " data-danger", glyph("close", 14))}` : ""}${this.button("framed-clear", words.framedClear)}</div>`;
  }

  /** The one login that owns every machine in `machines`, if one does. */
  private ownedTogether(machines: Machine[], world: WorldNow): Login | undefined {
    const first = machines[0] ? this.ownersOf(machines[0], world)[0] : undefined;
    return first && machines.every((machine) => this.ownersOf(machine, world).some((login) => sameLogin(login, first))) ? first : undefined;
  }

  private framedMachines(): { machines: Machine[]; login: Login } | null {
    const world = this.options.store.world.peek();
    if (!world) return null;
    const spots = this.spots(world);
    const machines = this.framed.map((key) => spots.get(key)).flatMap((spot) => (spot?.kind === "machine" ? [spot.machine] : []));
    const login = this.ownedTogether(machines, world);
    return login && machines.length ? { machines, login } : null;
  }

  /** Shares every framed machine with the same accounts: those all of them
   *  are shared with are shown, and a change applies to each. */
  private shareFramed(): void {
    const framed = this.framedMachines();
    if (!framed) return;
    const { access, words } = this.options;
    const { machines, login } = framed;
    const together = () => {
      const each = machines.map((machine) => new Set(this.sharedWith(login, machine.name)));
      return [...(each[0] ?? [])].filter((account) => each.every((set) => set.has(account)));
    };
    this.giving.open({ title: words.framedShareTitle(machines.length), what: access.shareWhat, sees: access.shareSees, given: access.sharedTo, go: access.shareGo }, together(), async (to, shared) => {
      for (const machine of machines) await this.options.change(login, { change: "share", machine: machine.name, to, shared });
      await this.options.store.refresh();
      return together();
    });
  }

  /** Removes every framed machine, after one red card naming them. */
  private removeFramed(): void {
    const framed = this.framedMachines();
    if (!framed) return;
    const { access, words } = this.options;
    const { machines, login } = framed;
    this.options.confirm.ask({
      title: words.framedRemoveTitle(machines.length),
      what: words.framedRemoveWhat(machines.map((machine) => machine.written)),
      go: access.removeGo,
      act: async () => {
        for (const machine of machines) await this.options.change(login, { change: "remove", name: machine.name, reason: access.reasonDefault });
      },
      done: () => {
        this.framed = [];
        void this.options.store.refresh();
      },
    });
  }

  /** What is known of a machine the account may work on. */
  private machinePanel(machine: Machine, world: WorldNow, now: Date, acts = ""): string {
    const { words } = this.options;
    const logins = loginsOf(world.accounts);
    const owner = logins.find((login) => owns(machine, login)) ?? null;
    const mine = machine.here || !!owner;
    const ownerName = mine ? null : (logins.map((login) => ownerOf(machine, login)).find(Boolean) ?? null);
    const viewer = logins.find((login) => machine.on.some((on) => on.hub === login.hub)) ?? null;
    const shared = !!viewer && !mine && this.sharedWith(viewer, machine.name).includes(viewer.account);
    const bySuper = !!viewer && !shared && isSuper(world, viewer);
    const reaching = !mine && logins.some((login) => machineActsFor(world, machine.name, login));
    const tags = [
      machine.here ? this.tag(words.here) : "",
      owner && !machine.here ? this.tag(words.yours, "tag-own") : "",
      ownerName ? this.tag(bySuper ? words.superInShort(ownerName) : words.sharedInShort(ownerName), "", bySuper ? words.superIn(ownerName) : words.sharedIn(ownerName)) : "",
      reaching ? this.tag(words.reaching, "tag-reach", words.reachingWhat) : "",
    ];
    const facts = "Ok" in machine.facts && machine.facts.Ok ? machine.facts.Ok : null;
    const route = !machine.here && machine.online === true && machine.through ? words.via(machine.through) : "";
    const factLine = facts
      ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, route].filter(Boolean).map(escape).join(`<span class="sep">·</span>`)
      : "Err" in machine.facts && machine.facts.Err && machine.online !== false
        ? `<span title="${escape(machine.facts.Err)}">${escape(words.factsUnknown(machine.facts.Err))}</span>`
        : escape(route);
    // This computer, hub by hub: how it reaches each, and whose it is there.
    const hubs = machine.here
      ? world.nodes.hubs
          .map((hub) => {
            const stand = hub.home ? words.registeredUnder(hub.home) : hub.logins.length ? words.onlyLogins(hub.logins.join(words.separator)) : "";
            return `<li class="panel-hub ${hub.link.kind}"><span class="panel-hub-name">${escape(hub.name)}</span><span${linkWhy(hub.link) ? ` title="${escape(linkWhy(hub.link))}"` : ""}>${escape([linkSaid(hub.link, words), stand].filter(Boolean).join(" · "))}</span></li>`;
          })
          .join("")
      : "";
    // Its owner's own: whom it is shared with, and the rest it sets.
    const owners = machine.here ? logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) : owner ? [owner] : [];
    const sharing = owners
      .map(
        (login) =>
          `<div class="panel-give" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">${this.givenRow(
            owners.length > 1 ? `${words.sharedTo} · ${login.hubName}` : words.sharedTo,
            this.sharedWith(login, machine.name),
            (to) => `data-act="unshare" data-to="${escape(to)}"`,
            this.button("share", words.share, "", glyph("plus", 13)),
          )}</div>`,
      )
      .join("");
    const own = owners.length
      ? `<div class="panel-acts" data-hub="${escape(owners[0]!.hub)}" data-account="${escape(owners[0]!.account)}">${machine.here ? "" : this.button("remove", words.remove, " data-danger", glyph("close", 14))}</div>`
      : "";
    // Whom an agent on it is lent to, taken back here as on the agents page.
    const lending = owners
      .map((login) => {
        const lends = (this.hubOf(login)?.lends ?? []).filter((lend) => lend.machine === machine.name);
        if (!lends.length) return "";
        const chips = lends
          .map((lend) => {
            const label = `${agentName(lend.agent)} · ${lend.account}`;
            return `<span class="chip">${escape(label)}<button class="chip-x" type="button" data-act="unlend" data-to="${escape(lend.account)}" data-agent="${escape(lend.agent)}"${lend.home ? ` data-home="${escape(lend.home)}"` : ""} aria-label="${escape(words.takeBack(label))}" title="${escape(words.takeBack(label))}">${glyph("close", 12)}</button></span>`;
          })
          .join("");
        return `<div class="panel-give" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}"><div class="given-row"><span class="given-label">${escape(words.lentTo)}</span>${chips}</div></div>`;
      })
      .join("");
    const note = this.note?.machine === machine.name ? `<p class="machine-note">${escape(this.note.text)}</p>` : "";
    const state = this.stateOf(machine.online, machine.since, machine.here, now);
    const direct = this.directSaid(machine, world);
    return `<article class="panel-machine" data-machine="${escape(machine.name)}">
      ${this.panelHead(machine.written, tags, state)}
      ${acts}
      ${factLine ? `<p class="panel-facts">${factLine}</p>` : ""}
      ${facts ? `<p class="panel-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${hubs ? `<ul class="panel-hubs">${hubs}</ul>` : ""}
      ${direct}
      ${sharing}
      ${lending}
      ${this.present(machine.name, world, viewer)}
      ${this.agentsLink(machine.written)}
      ${note}
      ${own}
    </article>`;
  }

  /** A machine's direct links: with whom they are up and how fast, which
   *  are down and why, and where others connect to it when it takes them. */
  private directSaid(machine: Machine, world: WorldNow): string {
    const { words } = this.options;
    const { nodes, links } = mapOf(world, words);
    const key = `m:${machine.written}`;
    const name = (other: string) => nodes.find((node) => node.key === other)?.name ?? other.replace(/^m:/, "");
    const mine = links.filter((link) => link.a === key || link.b === key).sort((a, b) => Number(b.up) - Number(a.up));
    const at = nodes.find((node) => node.key === key)?.directAt ?? [];
    if (!mine.length && !at.length) return "";
    const said = mine.map((link) => {
      const peer = name(link.a === key ? link.b : link.a);
      return link.up ? `<span class="direct-up">${escape(words.directUp(peer, link.ms))}</span>` : `<span class="direct-down">${escape(words.directDown(peer, link.why))}</span>`;
    });
    return `<div class="panel-direct"><span class="given-label">${escape(words.directRow)}</span><span class="panel-direct-list">${said.join(`<span class="sep">·</span>`)}</span>${at.length ? `<p class="panel-direct-at">${escape(words.takesDirectAt(at.join("、")))}</p>` : ""}</div>`;
  }

  /** What is known of a machine of another's an agent is lent on: whose
   *  it is, that it is no machine of the account's to work on, and the
   *  lent logins, each a way into a conversation. */
  private lentPanel(lent: Borrowed[], world: WorldNow, now: Date, acts = ""): string {
    const { words } = this.options;
    const first = lent[0]!;
    const reaching = machineActsFor(world, first.machine, first.login);
    const tags = [this.tag(words.lentShort(first.owner ?? ""), "tag-lent", words.borrowedSaid(first.owner ?? "")), reaching ? this.tag(words.reaching, "tag-reach", words.reachingWhat) : ""];
    const state = this.stateOf(first.online, first.since, false, now);
    return `<article class="panel-machine" data-machine="${escape(first.machine)}">
      ${this.panelHead(first.written, tags, state, true)}
      ${acts}
      <p class="panel-said" title="${escape(words.borrowedSaid(first.owner ?? ""))}">${glyph("colleague", 14)}<span>${escape(words.borrowedNote)}</span></p>
      ${this.present(first.machine, world, first.login)}
      ${this.agentsLink(first.written)}
    </article>`;
  }

  /** The way to the agents on a machine, which have a page of their own. */
  private agentsLink(machine: string): string {
    const { words } = this.options;
    return `<a class="text-button quiet-button panel-agents" href="${link({ kind: "agents", machine })}">${glyph("agents", 14)}<span>${escape(words.hosts)}</span></a>`;
  }

  /** What is known of a hub: how this computer reaches it, the account's
   *  level there, who its super administrator is, and its machines. */
  private hubPanel(hub: HubAccounts, world: WorldNow, acts = ""): string {
    const { words } = this.options;
    const logins = loginsOf(world.accounts).filter((login) => login.hub === hub.hub);
    const state = { kind: hub.link.kind === "online" ? "online" : "offline", said: linkSaid(hub.link, words) || words.hubOff };
    const rows = logins
      .map((login) => {
        const me = hub.accounts.find((account) => account.name === login.account);
        const admin = superOf(world, login);
        const top = me?.level === "super";
        return `<li class="panel-login" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">
          <span class="panel-login-name">${glyph("person", 14)}<span>${escape(login.account)}</span>${this.tag(top ? words.levelSuper : words.levelMember, top ? "tag-own" : "")}</span>
          ${!top && admin ? `<span class="panel-said" title="${escape(words.superNote(admin))}">${glyph("shield", 13)}<span>${escape(words.superShort(admin))}</span></span>` : ""}
          <span class="panel-login-acts">${this.button("invite", words.inviteDevice, "", glyph("plus", 13))}${top && !me?.locked ? `<a class="text-button quiet-button" href="${link({ kind: "admin" })}">${glyph("settings", 13)}<span>${escape(words.manageHub)}</span></a>` : ""}</span>
        </li>`;
      })
      .join("");
    const machines = hub.members.filter((member) => member.kind === "computer" && member.home).length;
    return `<article class="panel-hub-card">
      <header class="panel-head ${state.kind}"><span class="knot" aria-hidden="true"></span><h2 class="panel-title">${escape(words.hubTitle(hub.name))}</h2><span class="panel-state">${escape(state.said)}</span></header>
      ${acts}
      <p class="panel-facts">${escape(words.hubMachines(machines))}</p>
      <ul class="panel-logins">${rows}</ul>
    </article>`;
  }

  /** Who is at work on a machine and what runs there, as `login` sees it;
   *  nothing when no one is. */
  private present(machine: string, world: WorldNow, login: Login | null): string {
    const { words } = this.options;
    const group = login ? (groupsOf(world).find((one) => sameLogin(one.login, login)) ?? null) : null;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine && (r.now?.activity === "busy" || r.now?.waiting_for_user) && (!group || residentIn(world, group, r)),
    );
    const jobs = jobsOf(world).filter((job) => job.machine === machine);
    const agents = residents.length
      ? `<ul class="machine-agents">${residents
          .map(
            (r) =>
              `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape([agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" · "))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
          )
          .join("")}</ul>`
      : "";
    return `${agents}${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}`;
  }

}

/** Why a hub is not reached, in the kernel's words, for the hover. */
export function linkWhy(link: LinkState): string {
  return link.kind === "connecting" ? (link.last_error ?? "") : link.kind === "stopped" ? link.reason : "";
}

/** How this computer reaches a hub, in words. */
export function linkSaid(link: LinkState, words: MachinesWords): string {
  switch (link.kind) {
    case "online":
      return link.through ? words.linkVia(link.through, link.hub) : words.linkOnline(link.hub);
    case "connecting":
      return words.linkConnecting(link.last_error);
    case "stopped":
      return words.linkStopped(link.reason);
    case "alone":
      return "";
  }
}

/** An address as the hub or a network card gives it, without a port. */
function hostOf(address: string): string {
  const bracketed = /^\[([^\]]+)\](?::\d+)?$/.exec(address);
  if (bracketed) return bracketed[1]!;
  return /^[^:]+:\d+$/.test(address) ? address.slice(0, address.lastIndexOf(":")) : address;
}

/** A machine others connect to directly, drawn small for a card: its ring
 *  in the middle with its line to the hub's knot above, and two machines
 *  beside it with theirs; with direct connections, straight clay lines from
 *  the two to it. */
function directPicture(direct: boolean): string {
  const random = seeded(`direct-picture:${direct}`);
  const knot: [number, number] = [66, 7];
  const middle: [number, number] = [66, 30];
  const sides: [number, number][] = [
    [18, 26],
    [114, 26],
  ];
  const rings = [circle(middle, 8, 1.5, random), ...sides.map((side) => circle(side, 6, 1.3, random))];
  const bowed = (from: [number, number], reach: number) => {
    const dx = knot[0] - from[0];
    const dy = knot[1] - from[1];
    const length = Math.hypot(dx, dy);
    const start: [number, number] = [from[0] + (dx / length) * (reach + 2), from[1] + (dy / length) * (reach + 2)];
    const end: [number, number] = [knot[0] - (dx / length) * 5, knot[1] - (dy / length) * 5];
    return outline([start, [(start[0] + end[0]) / 2 + 2, (start[1] + end[1]) / 2 - 2], end], 1.1, random, { pressure: 0.3, taper: 0.1 });
  };
  const lines = [bowed(middle, 8), ...sides.map((side) => bowed(side, 6))];
  const straight = direct ? sides.map((side) => ruled([side[0] + (side[0] < 66 ? 8 : -8), side[1] + 1], [middle[0] + (side[0] < 66 ? -10 : 10), middle[1]], 1.1, random)) : [];
  return `<svg class="map-card-picture" viewBox="0 0 132 40" aria-hidden="true"><g class="ink graphite" filter="url(#pen-marker)"><path d="${circle(knot, 3.5, 1.2, random)}"/><path d="${blot(knot, 1.5, random)}"/>${[...rings, ...lines].map((d) => `<path d="${d}"/>`).join("")}</g><g class="ink clay" filter="url(#pen-marker)">${straight.map((d) => `<path d="${d}"/>`).join("")}</g></svg>`;
}
