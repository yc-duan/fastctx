// The machines a login may use (`world-gui.md`, 账号、机器、共享与借出),
// drawn: the account's World as a hand-drawn map that moves under the hand
// (`machine-map.ts`). Choosing a hub or a machine floats its actions beside
// it, small icons that say what they do when pointed at; the right button,
// or a long press, lists them with their names. What is known of the one
// chosen stands in a panel that opens only when asked for. Beside each hub
// a dashed ring adds a machine: it says one is on its way once a way in was
// given, and the machine is circled as it arrives. A machine of the
// account's own is shared, given its relays and removed from here; whom it
// is shared with is taken back with a press. Long sayings stand behind a
// pointer's rest, not on the page.

import { glyph } from "../conversation/glyphs";
import { layerOpened, type Layer } from "./back";
import { escape } from "../conversation/prose";
import type { Bounds, Point } from "../map-canvas";
import type { AccountChange, HubAccounts, Login, Relay } from "../world/accounts";
import { loginsOf } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, osName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { GiveSheet, RelaysSheet, socketAddress } from "./machine-sheets";
import { MachineLog, type ReadLog } from "./machine-log";
import { MachineMap, type Way } from "./machine-map";
import type { MapNode } from "./network-map";
import { blot, circle, outline, seeded } from "../pen/shapes";
import { RING } from "./map-layout";
import { mapOf } from "./map-of";
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

/** Something of a machine's given to another account, hung on its ring:
 *  the change that takes it back, and the one that gives it again. */
interface Given {
  key: string;
  label: string;
  to: string;
  login: Login;
  take: AccountChange;
  give: AccountChange;
}

/** How long a taking back can be undone from the drawing's corner. */
const UNDO_MS = 6_000;

/** How long a machine is given to go the way it was asked: it finds its
 *  new address list within a minute (`world-network.md`, 转接站), and its
 *  next try of each address within another. */
const ROUTE_MS = 120_000;

/** How long a machine that just arrived stays circled: the drawing-in of
 *  the circle and a moment to see it. */
const ARRIVED_MS = 6_000;

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
  private readonly tags: HTMLElement;
  /** The order of the chosen machine's ways, a number on each line. */
  private readonly orders: HTMLElement;
  private ordered = "";
  private readonly undoBar: HTMLElement;
  private tagged = "";
  private given: Given[] = [];
  private undoing: { timer: number; redo: () => Promise<void> } | null = null;
  private level = 0;
  /** The card asking to confirm a change made on the drawing, or to fill
   *  in what one needs. */
  private readonly card: HTMLElement;
  private cardFor: string | null = null;
  /** Ways asked for and not yet taken: a machine, what it was asked to go
   *  through (a relay's machine or its hub), the relay's label, and when. */
  private readonly pending = new Map<string, { target: string; label: string | null; since: number }>();
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
  /** The details, the menu and the card over the drawing, as the system
   *  back closes them. */
  private readonly detailsLayer: Layer = {
    isOpen: () => !!this.selected && !this.panel.hidden && this.element.isConnected,
    close: () => this.closeDetails(),
  };
  private readonly menuLayer: Layer = { isOpen: () => !!this.menuFor && this.element.isConnected, close: () => this.closeMenu() };
  private readonly cardLayer: Layer = { isOpen: () => !!this.cardFor && this.element.isConnected, close: () => this.closeCard() };
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
  private readonly relaying: RelaysSheet;
  /** Hubs a machine is on its way to, with the machines each had when a
   *  way in was given. */
  private readonly waiting = new Map<string, Set<string>>();
  private readonly fresh = new Set<string>();

  constructor(private readonly options: MachinesOptions) {
    const { words } = options;
    this.giving = new GiveSheet(options.access);
    this.relaying = new RelaysSheet(options.access);
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
      <div class="machines-layout"><div class="machines-canvas"><div class="map-tags"></div><div class="map-orders"></div><div class="map-undo" role="status" hidden></div><div class="map-acts" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><div class="map-menu" role="menu" aria-label="${escape(words.actions)}" hidden></div><form class="map-card" tabindex="-1" hidden></form></div><aside class="machine-panel scroller" aria-live="polite" hidden></aside></div>`;
    this.panel = this.element.querySelector<HTMLElement>(".machine-panel")!;
    this.canvas = this.element.querySelector<HTMLElement>(".machines-canvas")!;
    this.acts = this.element.querySelector<HTMLElement>(".map-acts")!;
    this.menu = this.element.querySelector<HTMLElement>(".map-menu")!;
    this.tags = this.element.querySelector<HTMLElement>(".map-tags")!;
    this.orders = this.element.querySelector<HTMLElement>(".map-orders")!;
    this.orders.addEventListener("pointerdown", (event) => this.dragOrder(event));
    this.undoBar = this.element.querySelector<HTMLElement>(".map-undo")!;
    this.card = this.element.querySelector<HTMLElement>(".map-card")!;
    this.map = new MachineMap({
      words,
      storageKey: "machines-map",
      event: (event) => {
        if (event.kind === "select") this.choose(event.key);
        else if (event.kind === "context") this.openMenu(event.key, event.at, event.input !== "mouse" || matchMedia("(max-width: 760px)").matches);
        else if (event.kind === "drop") this.proposeRoute(event.handle.source, event.target, event.at);
      },
      validRoute: (source, target) => this.validRoute(source, target),
      frame: (frame) => {
        this.scale = frame.view.scale;
        this.level = frame.view.level;
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
    this.tags.addEventListener("click", (event) => {
      const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-given]");
      const given = button ? this.given[Number(button.dataset.given)] : undefined;
      if (button && given) void this.takeBack(button, given);
    });
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
      case "relays":
        if (login) this.relays(login, machine);
        return;
      case "remove":
        if (login) this.remove(login, machine);
        return;
      case "close-details":
        this.closeDetails();
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
  private choose(key: string | null): void {
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

  private closeDetails(): void {
    // On a narrow page the sheet is the choice itself.
    if (this.narrow()) {
      this.selected = null;
      this.grown = false;
    } else this.detailsOpen = false;
    this.redraw();
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

  /** The sheet that sets the relays `machine` connects through. */
  private relays(login: Login, machine: string): void {
    const member = this.hubOf(login)?.members.find((member) => member.name === machine);
    this.relaying.open(
      machine,
      member?.relays ?? [],
      (relays) => this.options.change(login, { change: "relays", machine, relays }),
      () => void this.options.store.refresh(),
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
    const open = !!spot && (narrow || this.detailsOpen);
    if (open && this.panel.hidden) layerOpened(this.detailsLayer);
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
    const { hubs, nodes, hubLabels } = mapOf(world, this.options.words);
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
    this.settleRoutes(nodes);
    const chosen = this.selected ? nodes.find((node) => node.key === this.selected) : undefined;
    const route = chosen && this.routable(chosen, world) ? chosen.key : null;
    const ways = route && chosen ? waysOf(chosen, nodes) : [];
    this.map.update({
      hubs,
      nodes,
      hubLabels,
      adds,
      waiting: new Set(this.waiting.keys()),
      selected: this.selected,
      fresh: this.fresh,
      route,
      pending: new Map([...this.pending].map(([key, way]) => [key, way.target])),
      ways,
    });
  }

  /** What can be done to a spot, as `world` stands; the menu lists a few
   *  more than float beside it. */
  private actsOf(key: string, world: WorldNow, menu = false): Act[] {
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
    const relaying = !!this.memberOf(spot.machine, world)?.relay_at?.length;
    return [
      { act: "talk", label: words.talk, icon: "plus" },
      { act: "agents", label: words.hosts, icon: "agents" },
      ...(owners.length ? [{ act: "share", label: words.share, icon: "colleague" as const }] : []),
      ...(owners.length ? [relaying ? { act: "stop-relay", label: words.stopRelay, icon: "relay" as const } : { act: "be-relay", label: words.beRelay, icon: "relay" as const }] : []),
      ...(owners.length && menu ? [{ act: "relays", label: words.relays, icon: "relay" as const }] : []),
      details,
      ...(owners.length && !spot.machine.here ? [{ act: "remove", label: words.remove, icon: "close" as const, danger: true }] : []),
    ];
  }

  /** The actions of the one chosen as a row in its sheet, each with its
   *  name, large enough for a finger. */
  private sheetActs(key: string, world: WorldNow): string {
    const acts = this.actsOf(key, world, true).filter((act) => act.act !== "details");
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
      case "relays":
        if (login) this.relays(login, machine.name);
        return;
      case "remove":
        if (login) this.remove(login, machine.name);
        return;
      case "be-relay":
        if (login) this.askRelay(login, machine, key);
        return;
      case "stop-relay":
        if (login) void this.setRelay(login, machine, []);
        return;
    }
  }

  /** The numbers on the chosen machine's ways, where their lines are; only
   *  a machine with more than one way has them, and only a relay's number
   *  can be moved: the hub is always tried last. */
  private drawOrders(): void {
    const points = this.menuFor || this.cardFor ? [] : this.map.wayPoints();
    const shown = points.length > 1 ? points : [];
    const html = shown
      .map(({ way }) => `<button class="map-order${way.current ? " current" : ""}" type="button" data-order="${escape(way.to)}"${way.to.startsWith("hub:") ? " disabled" : ""}>${way.order}</button>`)
      .join("");
    if (html !== this.ordered) {
      this.ordered = html;
      this.orders.innerHTML = html;
    }
    const buttons = [...this.orders.children] as HTMLElement[];
    shown.forEach(({ at }, index) => {
      const button = buttons[index];
      if (button && !button.classList.contains("dragging")) button.style.transform = `translate(${(at[0] - 13).toFixed(1)}px, ${(at[1] - 13).toFixed(1)}px)`;
    });
  }

  /** A relay's number dragged onto another's swaps their places in the
   *  order the machine tries them; the card asks before it is changed. */
  private dragOrder(event: PointerEvent): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>(".map-order");
    if (!button || button.disabled || !this.selected) return;
    event.preventDefault();
    button.setPointerCapture(event.pointerId);
    button.classList.add("dragging");
    const box = this.canvas.getBoundingClientRect();
    const start = button.style.transform;
    const move = (moved: PointerEvent) => {
      button.style.transform = `translate(${(moved.clientX - box.left - 13).toFixed(1)}px, ${(moved.clientY - box.top - 13).toFixed(1)}px)`;
    };
    const up = (ended: PointerEvent) => {
      button.removeEventListener("pointermove", move);
      button.classList.remove("dragging");
      button.style.transform = start;
      const under = document.elementsFromPoint(ended.clientX, ended.clientY).find((element) => element !== button && element.classList.contains("map-order")) as HTMLButtonElement | undefined;
      if (under && !under.disabled && this.selected) this.proposeOrder(this.selected, button.dataset.order!, under.dataset.order!, [ended.clientX - box.left, ended.clientY - box.top]);
    };
    button.addEventListener("pointermove", move);
    button.addEventListener("pointerup", up, { once: true });
    button.addEventListener("pointercancel", () => {
      button.removeEventListener("pointermove", move);
      button.classList.remove("dragging");
      button.style.transform = start;
    }, { once: true });
  }

  /** The card for swapping two relays in a machine's order: the order
   *  before and after, in words. */
  private proposeOrder(source: string, first: string, second: string, at: Point): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const { words } = this.options;
    const spot = this.spots(world).get(source);
    const { nodes } = mapOf(world, words);
    if (spot?.kind !== "machine") return;
    const login = this.ownersOf(spot.machine, world)[0];
    const before = this.memberOf(spot.machine, world)?.relays ?? [];
    const name = (key: string) => nodes.find((node) => node.key === key)?.name;
    const a = before.findIndex((relay) => relay.label === name(first));
    const b = before.findIndex((relay) => relay.label === name(second));
    if (!login || a < 0 || b < 0 || a === b) return;
    const relays = [...before];
    [relays[a], relays[b]] = [relays[b]!, relays[a]!];
    const said = words.routeList(relays.map((relay) => relay.label));
    this.openCard(
      source,
      at,
      `<p class="map-card-said">${escape(said)}</p>
       <p class="map-card-was">${escape(words.routeList(before.map((relay) => relay.label)))}</p>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.confirm)}</button></footer>`,
      async () => {
        await this.options.change(login, { change: "relays", machine: spot.machine.name, relays });
        this.offerUndo(said, () => this.options.change(login, { change: "relays", machine: spot.machine.name, relays: before }));
      },
    );
  }

  /** The hub's record of one of the account's machines. */
  private memberOf(machine: Machine, world: WorldNow) {
    return world.accounts.hubs.flatMap((hub) => hub.members).find((member) => member.name === machine.name && member.kind === "computer");
  }

  /** Whether a machine's way to its hub can be changed here: one of the
   *  account's own, online. */
  private routable(node: MapNode, world: WorldNow): boolean {
    const spot = this.spots(world).get(node.key);
    return spot?.kind === "machine" && node.online === true && this.ownersOf(spot.machine, world).length > 0;
  }

  /** Where a machine's handle may be dropped: a relay's machine on one of
   *  its hubs, online, or one of its hubs itself. */
  private validRoute(source: string, target: string): boolean {
    const world = this.options.store.world.peek();
    if (!world) return false;
    const { nodes } = mapOf(world, this.options.words);
    const node = nodes.find((one) => one.key === source);
    if (!node || !this.routable(node, world)) return false;
    if (target.startsWith("hub:")) return node.hubs.includes(target.slice(4));
    const relay = nodes.find((one) => one.key === target);
    return !!relay && relay.key !== source && relay.hold !== "lent" && relay.online === true && !!relay.relayAt?.length && relay.hubs.some((hub) => node.hubs.includes(hub));
  }

  /** A way taken drops from the pending; one not taken in time is said. */
  private settleRoutes(nodes: MapNode[]): void {
    for (const [key, way] of this.pending) {
      const node = nodes.find((one) => one.key === key);
      const taken = !!node && node.online === true && (way.label === null ? !node.through : node.through === way.label);
      if (taken) this.pending.delete(key);
      else if (Date.now() - way.since > ROUTE_MS) {
        this.pending.delete(key);
        this.say(this.options.words.routeFailed(node?.name ?? key.slice(2), way.label ?? this.options.words.hubOnline));
      }
    }
  }

  /** The small card where a handle was dropped: the way before and after,
   *  in two small drawings, one sentence, and what changes underneath
   *  folded away; confirmed, the machine is asked to go that way. */
  private proposeRoute(source: string, target: string, at: Point): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const { words } = this.options;
    const spot = this.spots(world).get(source);
    const { nodes } = mapOf(world, words);
    const node = nodes.find((one) => one.key === source);
    if (spot?.kind !== "machine" || !node) return;
    const login = this.ownersOf(spot.machine, world)[0];
    if (!login) return;
    const before = this.memberOf(spot.machine, world)?.relays ?? [];
    const relay = target.startsWith("hub:") ? null : nodes.find((one) => one.key === target);
    const relays: Relay[] = relay ? [{ label: relay.name, addresses: relay.relayAt ?? [] }, ...before.filter((one) => one.label !== relay.name)] : [];
    const now = node.through ?? null;
    if ((relay?.name ?? null) === now && relays.length === before.length) return;
    const sentence = relay ? words.routeVia(node.name, relay.name) : words.routeDirect(node.name);
    this.openCard(
      source,
      at,
      `<div class="map-card-pictures">${routePicture(now, false)}<span aria-hidden="true">→</span>${routePicture(relay?.name ?? null, true)}</div>
       <p class="map-card-said">${escape(sentence)}</p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.routeList(relays.map((one) => `${one.label}（${one.addresses.join("、")}）`)))}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.confirm)}</button></footer>`,
      async () => {
        await this.options.change(login, { change: "relays", machine: spot.machine.name, relays });
        this.pending.set(source, { target, label: relay?.name ?? null, since: Date.now() });
        this.offerUndo(sentence, () => this.options.change(login, { change: "relays", machine: spot.machine.name, relays: before }));
      },
    );
  }

  /** The card that makes a machine a relay: the address others dial it
   *  at, and what being a relay means, folded away. */
  private askRelay(login: Login, machine: Machine, key: string): void {
    const { words } = this.options;
    const at = this.map.screenOf(key) ?? [0, 0];
    this.openCard(
      key,
      at,
      `<p class="map-card-title">${escape(words.relayTitle(machine.written))}</p>
       <label class="field"><span class="field-label">${escape(words.relayAddress)}</span><input class="mono" name="address" autocomplete="off" spellcheck="false" placeholder="203.0.113.20:7444" required></label>
       <p class="map-card-wrong" hidden>${escape(words.relayAddressWrong)}</p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.relayWhat)}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.confirm)}</button></footer>`,
      async () => {
        const address = this.card.querySelector<HTMLInputElement>("[name=address]")!.value.trim();
        if (!socketAddress.test(address)) {
          this.card.querySelector<HTMLElement>(".map-card-wrong")!.hidden = false;
          throw new Error("kept open");
        }
        await this.setRelay(login, machine, [address], false);
      },
    );
    this.card.querySelector<HTMLInputElement>("[name=address]")?.focus();
  }

  /** Makes a machine a relay at `addresses`, or with none no longer; taken
   *  away, it can be given back for a moment. */
  private async setRelay(login: Login, machine: Machine, addresses: string[], undoable = true): Promise<void> {
    const world = this.options.store.world.peek();
    const before = (world && this.memberOf(machine, world)?.relay_at) ?? [];
    try {
      await this.options.change(login, { change: "relay_at", machine: machine.name, addresses });
    } catch (error) {
      this.say(this.options.access.failed(error instanceof Error ? error.message : String(error)));
      throw error;
    }
    if (undoable && before.length) this.offerUndo(this.options.words.stopRelay, () => this.options.change(login, { change: "relay_at", machine: machine.name, addresses: before }));
    await this.options.store.refresh();
    this.redraw();
  }

  private openCard(key: string, at: Point, html: string, go: () => Promise<void>): void {
    this.closeMenu();
    this.cardFor = key;
    this.float();
    this.card.innerHTML = html;
    const sheet = matchMedia("(max-width: 760px)").matches;
    this.card.classList.toggle("as-sheet", sheet);
    this.card.hidden = false;
    layerOpened(this.cardLayer);
    if (!sheet) {
      const x = Math.min(this.canvas.clientWidth - this.card.offsetWidth - 8, at[0] + 16);
      const y = Math.min(this.canvas.clientHeight - this.card.offsetHeight - 8, at[1] + 16);
      this.card.style.transform = `translate(${Math.max(8, x)}px, ${Math.max(8, y)}px)`;
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
          if (!(error instanceof Error && error.message === "kept open")) this.say(this.options.access.failed(error instanceof Error ? error.message : String(error)));
        },
      );
    };
    this.card.focus({ preventScroll: true });
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

  /** A line in the drawing's corner for a moment. */
  private say(text: string): void {
    if (this.undoing) clearTimeout(this.undoing.timer);
    this.undoing = { timer: window.setTimeout(() => this.endUndo(), UNDO_MS), redo: async () => {} };
    this.undoBar.innerHTML = `<span>${escape(text)}</span>`;
    this.undoBar.hidden = false;
  }

  /** Whom each of the account's own machines is shared with, and whom an
   *  agent on it is lent to. */
  private givenOf(world: WorldNow): Given[] {
    const given: Given[] = [];
    for (const machine of machinesOf(world)) {
      for (const login of this.ownersOf(machine, world)) {
        const hub = world.accounts.hubs.find((one) => one.hub === login.hub);
        for (const share of hub?.shares ?? []) {
          if (share.machine !== machine.name || share.account === login.account) continue;
          given.push({ key: `m:${machine.written}`, label: share.account, to: share.account, login, take: { change: "share", machine: machine.name, to: share.account, shared: false }, give: { change: "share", machine: machine.name, to: share.account, shared: true } });
        }
        for (const lend of hub?.lends ?? []) {
          if (lend.machine !== machine.name) continue;
          const home = lend.home ? { home: lend.home } : {};
          given.push({
            key: `m:${machine.written}`,
            label: `${agentName(lend.agent)} · ${lend.account}`,
            to: lend.account,
            login,
            take: { change: "lend", machine: machine.name, to: lend.account, agent: lend.agent, ...home, lent: false },
            give: { change: "lend", machine: machine.name, to: lend.account, agent: lend.agent, ...home, lent: true },
          });
        }
      }
    }
    return given;
  }

  /** Takes back a share or a loan at once; for a moment it can be given
   *  again from the drawing's corner. */
  private async takeBack(button: HTMLButtonElement, given: Given): Promise<void> {
    const { words } = this.options;
    button.disabled = true;
    try {
      await this.options.change(given.login, given.take);
    } catch (error) {
      this.note = { machine: given.key.slice(2), text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
      button.disabled = false;
      this.redraw();
      return;
    }
    this.offerUndo(words.tookBack(given.to), () => this.options.change(given.login, given.give));
    await this.options.store.refresh();
    this.redraw();
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

  /** The tags of whom a machine is given to, hung at its ring's lower right
   *  when the drawing is near enough to read them; not on the one chosen,
   *  whose actions float there instead. */
  private hang(): Bounds[] {
    const world = this.options.store.world.peek();
    this.given = world ? this.givenOf(world) : [];
    // A narrow page has no room for them among the rings; there they are in
    // the chosen one's sheet.
    const shown = this.level >= 1 && !this.narrow() ? this.given.map((given, index) => ({ given, index })).filter(({ given }) => given.key !== this.selected) : [];
    const html = shown
      .map(
        ({ given, index }) =>
          `<span class="map-tag" data-for="${escape(given.key)}">${escape(given.label)}<button class="chip-x" type="button" data-given="${index}" aria-label="${escape(this.options.words.takeBack(given.label))}" title="${escape(this.options.words.takeBack(given.label))}">${glyph("close", 12)}</button></span>`,
      )
      .join("");
    if (html !== this.tagged) {
      this.tagged = html;
      this.tags.innerHTML = html;
    }
    const below = new Map<string, number>();
    const boxes: Bounds[] = [];
    for (const tag of this.tags.children as HTMLCollectionOf<HTMLElement>) {
      const key = tag.dataset.for!;
      const at = this.map.screenOf(key);
      if (!at) continue;
      const row = below.get(key) ?? 0;
      below.set(key, row + 1);
      const x = at[0] + RING * this.scale * 0.7;
      const y = at[1] + RING * this.scale * 0.7 + row * 28;
      tag.style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px)`;
      boxes.push({ x, y, width: tag.offsetWidth, height: tag.offsetHeight });
    }
    return boxes;
  }

  /** The icons beside the one chosen, on an arc round its ring, kept on the
   *  side with room; the names on the drawing keep clear of them. */
  private float(): void {
    this.drawOrders();
    const world = this.options.store.world.peek();
    const key = this.menuFor || this.cardFor || this.narrow() ? null : this.selected;
    const at = key ? this.map.screenOf(key) : null;
    const hung = this.hang();
    if (!world || !key || !at) {
      if (!this.acts.hidden) {
        this.acts.hidden = true;
        this.acts.innerHTML = "";
        this.floated = "";
      }
      this.map.reserve(hung);
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
    // Clear of the ring, and far enough out that neighbours on the arc do
    // not touch: 38 degrees apart, with 6 pixels between.
    const step = (38 * Math.PI) / 180;
    const reach = Math.max(Math.max(RING * this.scale, 14) + size / 2 + 12, (size + 6) / (2 * Math.sin(step / 2)));
    // Away from the handle on its line, where it has one; else down the
    // right of the ring, or the left where the right has no room.
    const handle = this.map.handleOf(key);
    const left = at[0] + reach + size > width;
    const centre = handle ? Math.atan2(at[1] - handle[1], at[0] - handle[0]) : left ? Math.PI : 0;
    const boxes = [...this.acts.children].map((button, index, all) => {
      const degrees = -((all.length - 1) * 38) / 2 + index * 38;
      const angle = centre + (((left && !handle) || (handle && Math.cos(centre) < 0) ? -degrees : degrees) * Math.PI) / 180;
      const x = Math.min(width - size - 4, Math.max(4, at[0] + Math.cos(angle) * reach - size / 2));
      const y = Math.min(height - size - 4, Math.max(4, at[1] + Math.sin(angle) * reach - size / 2));
      (button as HTMLElement).style.transform = `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px)`;
      return { x, y, width: size, height: size };
    });
    // The blot of the handle on its line stays clear of the names too;
    // its wider reach for a finger may lie under one.
    const pulled = handle ? [{ x: handle[0] - 9, y: handle[1] - 9, width: 18, height: 18 }] : [];
    this.map.reserve([...boxes, ...hung, ...pulled]);
  }

  /** The menu of a spot, or of the drawing itself, where the right button
   *  was pressed; on a phone it rises from below. */
  private openMenu(key: string | null, at: Point, sheet: boolean): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const { words } = this.options;
    if (key?.startsWith("add:")) return this.choose(key);
    const acts: Act[] = key ? this.actsOf(key, world, true) : [{ act: "fit", label: words.fit, icon: "expand" }];
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
    layerOpened(this.menuLayer);
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
    const route = machine.here ? "" : machine.online === true ? (machine.through ? words.via(machine.through) : words.direct) : "";
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
      ? `<div class="panel-acts" data-hub="${escape(owners[0]!.hub)}" data-account="${escape(owners[0]!.account)}">${this.button("relays", words.relays, "", glyph("relay", 14))}${machine.here ? "" : this.button("remove", words.remove, " data-danger", glyph("close", 14))}</div>`
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
    return `<article class="panel-machine" data-machine="${escape(machine.name)}">
      ${this.panelHead(machine.written, tags, state)}
      ${acts}
      ${factLine ? `<p class="panel-facts">${factLine}</p>` : ""}
      ${facts ? `<p class="panel-home"><span>${escape(words.home)}</span><code>${escape(facts.home)}</code></p>` : ""}
      ${hubs ? `<ul class="panel-hubs">${hubs}</ul>` : ""}
      ${sharing}
      ${lending}
      ${this.present(machine.name, world, viewer)}
      ${this.agentsLink(machine.written)}
      ${note}
      ${own}
    </article>`;
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
      return link.relay ? words.linkVia(link.relay, link.hub) : words.linkOnline(link.hub);
    case "connecting":
      return words.linkConnecting(link.last_error);
    case "stopped":
      return words.linkStopped(link.reason);
    case "alone":
      return "";
  }
}

/** A machine's way to its hub, drawn small for the card: its ring, the
 *  relay it goes through if any, and the hub's knot; the new way in clay. */
function routePicture(via: string | null, after: boolean): string {
  const random = seeded(`route:${via ?? ""}:${after}`);
  const ring = circle([16, 18], 10, 1.6, random);
  const knot = circle([116, 18], 4, 1.4, random);
  const dot = blot([116, 18], 1.6, random);
  const line = (from: number, to: number) => outline([[from, 18], [(from + to) / 2, 16.5], [to, 18]], 1.3, random, { pressure: 0.3, taper: 0.1 });
  const relay = via ? circle([66, 18], 8, 1.4, random) : "";
  const lines = via ? `${line(28, 56)} ${line(76, 110)}` : line(28, 110);
  const tone = after ? "clay" : "graphite";
  return `<svg class="map-card-picture" viewBox="0 0 132 36" role="img" aria-label="${escape(via ?? "")}"><g class="ink graphite" filter="url(#pen-marker)"><path d="${ring}"/><path d="${knot}"/><path d="${dot}"/></g><g class="ink ${tone}" filter="url(#pen-marker)"><path d="${lines}"/>${relay ? `<path d="${relay}"/>` : ""}</g>${via ? `<text x="66" y="34" text-anchor="middle">${escape(via)}</text>` : ""}</svg>`;
}

/** A machine's ways to its hub in the order it tries them: the relays of
 *  its list that are machines on the drawing, then the hub. */
function waysOf(node: MapNode, nodes: MapNode[]): Way[] {
  const relays = node.relays ?? [];
  const ways: Way[] = [];
  relays.forEach((label, index) => {
    const relay = nodes.find((one) => one.name === label && one.relayAt?.length && one.key !== node.key);
    if (relay) ways.push({ machine: node.key, to: relay.key, order: index + 1, current: node.through === label });
  });
  const hub = node.hubs[0];
  if (hub) ways.push({ machine: node.key, to: `hub:${hub}`, order: relays.length + 1, current: !node.through || !ways.some((way) => way.current) });
  return ways;
}
