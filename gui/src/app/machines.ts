// The machines a login may use (`world-gui.md`, 账号、机器、共享与协助),
// drawn: the account's World as a hand-drawn map that moves under the hand
// (`machine-map.ts`), or set out as a list (`machine-list.ts`), as chosen in
// the page's head and kept on this device. A hub or a machine chosen on the
// drawing has a small bar beside it (`machine-spot.ts`): what is done to it
// most, a few marks, and "⋯" for the rest, which the right button lists
// too; on a narrow page the bar lies along the bottom. A machine's details
// are its own page; a hub's, a phone's and an assisting agent's are a small
// card opened from the bar, or from a row of the list. Beside each hub a
// dashed ring adds a machine: it says one is on its way once a way in was
// given, and the machine is circled as it arrives. The network's
// administrator manages its people here too (`map-people.ts`). What asks
// for one thing, a name, a group, a way to connect, is a small card beside
// the ring (`machine-cards.ts`). Long sayings stand behind a pointer's rest,
// not on the page.

import { glyph } from "../conversation/glyphs";
import { layerOpened, type Layer } from "./back";
import { onHeld } from "./press";
import { escape } from "../conversation/prose";
import { Film } from "../film/film";
import { network } from "../film/network";
import { watchFilm } from "../film/theater";
import type { Point } from "../map-canvas";
import type { AccountChange, HubAccounts, Login, MemberSeen } from "../world/accounts";
import { MachineCards, type Renaming } from "./machine-cards";
import { moreEntries, nameOf, spotActs, SpotViews, type Act, type Spot } from "./machine-spot";
import { ActsMenu } from "./acts-menu";
import { MachineUpdates, type UpdatesAccess } from "./machine-updates";
import { copyFrom } from "./copy-button";
import { loginsOf, names } from "../world/accounts";
import { renamerOf } from "./rename";
import type { WorldNow } from "../world/bridge";
import { agentName } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { AssistSheet, assistChange, assistancesIn, namesOn, type Assistance } from "./assist-sheet";
import { HubMoving, type HubMovingOptions } from "./hub-moving";
import { holdUntilNamed, typedName } from "./typed-name";
import { GiveSheet } from "./machine-sheets";
import { MachineLog, type ReadLog } from "./machine-log";
import { boxParts, groupingOf, groupMoreKey, groupMoreOf, regroup, renameGroups, type GroupBox, type Grouping } from "./machine-groups";
import { MachineMap, MAP_ORDER, personKey } from "./machine-map";
import { isManaged, listOrder, machineList } from "./machine-list";
import { whenNear } from "./read-on";
import { MapPeople } from "./map-people";
import type { PeopleWords } from "./people-words";
import { RING } from "./map-layout";
import { foundIn, MachineFind } from "./machine-find";
import { mapOf, phonesOf } from "./map-of";
import { deviceKey, type MapNode } from "./map-nodes";
import { link } from "./places";
import { runsHub, holds, assistantsOf, isComputer, isSuper, isVisitor, machinesOf, owns, type Machine } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { BatchWords } from "./machine-batch-words";
import { ChosenBar } from "./machine-framed";
import { removerLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import { added, between, pickable, pickableIn, toggled } from "./machine-picking";
import { RemoveMachineSheet } from "./machine-remove";
import { removeMachines } from "./machine-removing";
import { WaysSheet } from "./machine-ways";
import type { Store } from "./store";

export interface MachinesOptions {
  store: Store;
  words: MachinesWords;
  access: AccessWords;
  people: PeopleWords;
  head(title: string, actions?: string): string;
  /** The machine to bring into view, when one was named. */
  focus: string | null;
  /** Opens on the update view instead, that machine ticked in it: a
   *  machine's page sends here to update it. */
  ticked?: boolean;
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
  /** Moving the server to another machine. */
  moving: Pick<HubMovingOptions, "move" | "arrive" | "remote">;
  /** Renaming a machine. */
  renaming: Renaming;
  /** The network's updates, for the update view (`machine-updates.ts`). */
  updates?: UpdatesAccess | undefined;
  /** The words of machines chosen together, and of taking machines off. */
  batch: BatchWords;
  manage: ManageWords;
}

const spotsMade = new WeakMap<WorldNow, Map<string, Spot>>();

/** The menu of the right button on the drawing, and behind the "⋯" of the
 *  one chosen. */
let spotMenu: ActsMenu | null = null;
const menuOf = () => (spotMenu ??= new ActsMenu());

/** How long a taking back can be undone from the drawing's corner. */
const UNDO_MS = 6_000;

/** How long a machine that just arrived stays circled: the drawing-in of
 *  the circle and a moment to see it. */
const ARRIVED_MS = 6_000;

/** Where this device keeps whether the machines show as a drawing or a list. */
const VIEW_KEY = "machines.view";

type View = "map" | "list" | "updates";

function keptView(): View {
  try {
    const kept = localStorage.getItem(VIEW_KEY);
    return kept === "list" || kept === "updates" ? kept : "map";
  } catch {
    return "map";
  }
}

/** Where this device keeps which of the list's groups and people the viewer
 *  opened or closed, by their keys. */
const FOLDS_KEY = "machines.folds";

/** How many of the list's rows are drawn at first, and how many more each
 *  time the end of those comes near. */
const LIST_ROWS = 150;

function keptFolds(): Map<string, boolean> {
  try {
    const kept = JSON.parse(localStorage.getItem(FOLDS_KEY) ?? "{}") as Record<string, unknown>;
    return new Map(Object.entries(kept).filter((entry): entry is [string, boolean] => typeof entry[1] === "boolean"));
  } catch {
    return new Map();
  }
}

export class MachinesPage implements Page {
  /** The page was left: nothing more is drawn. */
  private left = false;
  /** The update view, where this device updates machines. */
  private readonly updateView: MachineUpdates | null;
  readonly element: HTMLElement;
  readonly title: string;
  private readonly panel: HTMLElement;
  private readonly canvas: HTMLElement;
  private readonly map: MachineMap;
  /** The bar beside the one chosen and the card of a hub, a phone or an
   *  assisting agent, and the moment to undo a taking back. */
  private readonly spotViews: SpotViews;
  private readonly spotBar: HTMLElement;
  private spotDrawn = "";
  /** Whether the drawing wrote no names as the bar was last drawn. */
  private spotNamed = false;
  private readonly undoBar: HTMLElement;
  private undoing: { timer: number; redo: () => Promise<void> } | null = null;
  /** The card asking for one thing about a machine, a group or a person. */
  private readonly cards: MachineCards;
  private drawn = "";
  /** The list's groups and people the viewer opened or closed. */
  private readonly listFolds = keptFolds();
  /** Finding machines: the bar, its glass in the head, and what was last
   *  found for what was looked for. */
  private readonly finder: MachineFind;
  private readonly findToggle: HTMLElement;
  private found: { world: WorldNow; query: string; keys: string[]; order?: string[] } | null = null;
  /** The match gone to in the list. */
  private findCurrent: string | null = null;
  private readonly stop: () => void;
  /** The spot chosen, by its key on the map. */
  private selected: string | null;
  /** Whether what is known of a hub, a phone or an assisting agent is open. */
  private detailsOpen: boolean;
  /** The details, the bar along a phone's bottom, the menu and the card
   *  over the drawing, as the system back closes them. */
  private readonly detailsLayer: Layer = {
    isOpen: () => !!this.selected && !this.panel.hidden && this.element.isConnected,
    close: () => this.closeDetails(),
  };
  private readonly barLayer: Layer = {
    isOpen: () => !!this.selected && !this.spotBar.hidden && this.narrow() && this.element.isConnected,
    close: () => this.choose(null),
  };
  /** Redraws when the page turns narrow or wide: the sheet and what floats
   *  beside the one chosen depend on it. */
  private readonly resized: ResizeObserver;
  private readonly log: MachineLog;
  /** The one to bring into view once the drawing has drawn its next
   *  frame at its size: until then where it stands is not yet known. */
  private bring: string | null = null;
  /** The spot whose menu the right button opened, while it is open; the
   *  drawing's own, "". */
  private menuFor: string | null = null;
  private scale = 1;
  private note: { machine: string; text: string } | null = null;
  private readonly giving: GiveSheet;
  private readonly assistSheet: AssistSheet;
  private readonly moving: HubMoving;
  private readonly people: MapPeople;
  /** While the server's new place is chosen: what to do, and why a machine
   *  pressed cannot take it. */
  private readonly chooseBar: HTMLElement;
  private whyNot: { said: string; detail: string } | null = null;
  /** Hubs a machine is on its way to, with the machines each had when a
   *  way in was given. */
  private readonly waiting = new Map<string, Set<string>>();
  private readonly fresh = new Set<string>();
  /** Machines direct connections were just allowed to, while the hub
   *  checks it reaches them. */
  private readonly opening = new Set<string>();
  /** Machines chosen together, with Ctrl, a frame, a box or the list's
   *  boxes, and those inside the frame while it is drawn. On the drawing
   *  one alone is the one chosen. */
  private framed: string[] = [];
  private framing: string[] = [];
  /** The list's row a range chosen with Shift starts from. */
  private anchor: string | null = null;
  /** Choosing several on a touch screen: begun by a finger held on a
   *  machine's row, the rows show their boxes, a press ticks one, and the
   *  page's head says how many until it is let go. */
  private selecting = false;
  private readonly selectLayer: Layer = { isOpen: () => this.selecting && this.element.isConnected, close: () => this.setPicked([]) };
  private readonly chosen: ChosenBar;
  /** What is done to the machines chosen, floating at the foot of the page. */
  private readonly bar: HTMLElement;
  private readonly removing: RemoveMachineSheet;
  /** Come to a spot named in the address, until the drawing knows it. */
  private focusing: boolean;
  /** The machine named for the update view is ticked once it is known. */
  private ticking: boolean;

  /** In no network: the film and the ways into one, instead of the
   *  drawing. */
  private readonly intro: HTMLElement;
  private readonly layout: HTMLElement;
  private film: Film | null = null;
  /** The machines as a drawing or as a list, and the list as last drawn. */
  private view: View;
  private readonly views: HTMLElement;
  private readonly list: HTMLElement;
  private listed = "";
  /** How many of the list's rows are drawn, where its machines stand in it,
   *  and what stops watching the end of those coming near. */
  private listReach = LIST_ROWS;
  private listRows = new Map<string, number>();
  private listUnwatch = () => {};
  private barDrawn = "";

  constructor(private readonly options: MachinesOptions) {
    const { words } = options;
    this.giving = new GiveSheet(options.access);
    this.assistSheet = new AssistSheet(options.access);
    this.removing = new RemoveMachineSheet(options.manage);
    const updating = options.updates;
    this.chosen = new ChosenBar({
      words: options.words,
      batch: options.batch,
      manage: options.manage,
      access: options.access,
      change: options.change,
      refresh: () => options.store.refresh(),
      removing: this.removing,
      log: options.log,
      ways: new WaysSheet(options.batch, options.manage.close),
      share: (machines, login) => this.shareMachines(machines, login),
      group: (machines) => this.groupMachines(machines),
      clear: () => this.setPicked([]),
      toUpdates: () => this.show("updates"),
      updates: updating
        ? {
            words: updating.view,
            of: (machine) => this.updateView?.of(machine.written),
            schedule: (machines, version) => this.updateView!.schedule(machines, version),
            repair: (machines, cancel) => this.updateView!.repair(machines, cancel),
            ways: (machines, auto, relays) => this.updateView!.setWays(machines, auto, relays),
            state: updating.state,
            versions: updating.versions,
          }
        : undefined,
    });
    this.updateView = updating
      ? new MachineUpdates({
          updates: updating,
          access: options.access,
          world: () => options.store.world.peek(),
          shown: () => this.view === "updates" && !this.left,
          draw: () => {
            if (!this.left) this.draw(options.store.world.peek(), options.store.clock.peek());
          },
          failure: () => (this.note?.machine === "" ? this.note.text : null),
          failed: (text) => {
            if (text !== null) this.note = { machine: "", text };
            else if (this.note?.machine === "") this.note = null;
          },
          width: () => this.list.clientWidth,
        })
      : null;
    this.moving = new HubMoving({ ...options.moving, words, access: options.access, confirm: options.confirm, changed: () => this.redraw() });
    this.spotViews = new SpotViews({
      words,
      access: options.access,
      when: (at, now) => options.when(at, now),
      moving: (hub, here) => this.moving.section(hub, here),
    });
    this.people = new MapPeople({
      words: options.people,
      store: options.store,
      confirm: options.confirm,
      change: options.change,
      card: (key, html, go) => this.cards.show(key, html, go),
      undo: (said, redo) => this.offerUndo(said, redo),
      failed: (said) => {
        this.note = { machine: "", text: said };
        this.redraw();
      },
      redraw: () => this.redraw(),
      ownedBy: (account) => words.ownedBy(account),
    });
    this.title = words.title;
    this.selected = options.focus ? `m:${options.focus}` : null;
    const ticked = !!options.ticked && !!options.updates;
    // Come to one from elsewhere, what is known of it is open; a machine's
    // own page is where its details are, so it is only chosen.
    this.detailsOpen = !!options.focus && !ticked;
    this.focusing = !!options.focus && !ticked;
    this.ticking = ticked && !!options.focus;
    this.element = document.createElement("div");
    this.element.className = "machines-page";
    this.element.innerHTML = `
      ${options.head(
        `<h1 class="page-title">${escape(words.title)}</h1>`,
        `<button class="icon-button mf-toggle" type="button" aria-label="${escape(words.find.find)}" title="${escape(words.find.find)}" hidden>${glyph("search", 18)}</button><div class="ml-views" role="radiogroup" aria-label="${escape(words.views)}" hidden>${(options.updates ? (["map", "list", "updates"] as View[]) : (["map", "list"] as View[]))
          .map((view) => {
            const label = view === "map" ? words.viewMap : view === "list" ? words.viewList : options.updates!.view.view;
            return `<button class="ml-view" type="button" role="radio" data-view="${view}" title="${escape(label)}">${glyph(view === "map" ? "tree" : view === "list" ? "rows" : "update", 16)}<span>${escape(label)}</span>${view === "updates" ? `<span class="ml-view-count" hidden></span>` : ""}</button>`;
          })
          .join("")}</div><button class="text-button add-device" type="button" hidden>${glyph("plus", 16)}<span>${escape(words.add)}</span></button>`,
      )}
      <div class="machines-intro" hidden><div class="machines-film"></div><p class="machines-hub-is">${escape(words.hubIs)}</p><div class="machines-ways"><a class="sheet-go" href="${link({ kind: "login", way: "new" })}">${escape(words.newNetwork)}</a><a class="text-button quiet-button" href="${link({ kind: "login", way: "join" })}">${escape(words.joinNetwork)}</a></div></div>
      <div class="machines-layout"><div class="machines-canvas"><div class="ml-list scroller" role="list" hidden></div><div class="map-undo" role="status" hidden></div><div class="map-choose" role="status" hidden></div><form class="map-card" tabindex="-1" hidden></form></div><div class="ms-bar" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><div class="spot-bar" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><aside class="machine-panel scroller" aria-live="polite" hidden></aside></div>`;
    this.panel = this.element.querySelector<HTMLElement>(".machine-panel")!;
    this.panel.tabIndex = -1;
    this.bar = this.element.querySelector<HTMLElement>(".ms-bar")!;
    this.spotBar = this.element.querySelector<HTMLElement>(".spot-bar")!;
    this.spotBar.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const more = target.closest<HTMLElement>("[data-spot-more]");
      if (more) return this.openMore(more);
      if (target.closest("[data-spot=close]")) return this.choose(null);
      this.clickAct(event);
    });
    this.bar.addEventListener("click", (event) => {
      const act = (event.target as HTMLElement).closest<HTMLElement>("[data-act]")?.dataset.act;
      const world = this.options.store.world.peek();
      if (act === "framed-clear") this.setPicked([]);
      else if (act && world) this.chosen.click(act, this.pickedMachines(world), world);
    });
    // Pointed at, an action that does not reach every machine chosen has
    // those it leaves stand back.
    this.bar.addEventListener("pointerover", (event) => {
      const reach = (event.target as HTMLElement).closest<HTMLElement>(".ms-act")?.dataset.reach;
      this.standBack(reach === undefined ? null : new Set(reach.split("\n").filter(Boolean)));
    });
    this.bar.addEventListener("pointerleave", () => this.standBack(null));
    // Choosing several on a touch screen has a head of its own over the
    // page's: let them go, how many, every one.
    const selectHead = document.createElement("div");
    selectHead.className = "ml-select-head";
    selectHead.innerHTML = `<button class="icon-button" type="button" data-select="stop" aria-label="${escape(options.batch.clear)}" title="${escape(options.batch.clear)}">${glyph("close", 18)}</button><span class="ml-select-count" aria-live="polite"></span><button class="text-button" type="button" data-select="all"></button>`;
    this.element.querySelector(".page-head")?.append(selectHead);
    selectHead.addEventListener("click", (event) => {
      const what = (event.target as HTMLElement).closest<HTMLElement>("[data-select]")?.dataset.select;
      if (what === "stop") this.setPicked([]);
      else if (what === "all") this.selectAll();
    });
    this.intro = this.element.querySelector<HTMLElement>(".machines-intro")!;
    this.layout = this.element.querySelector<HTMLElement>(".machines-layout")!;
    this.canvas = this.element.querySelector<HTMLElement>(".machines-canvas")!;
    this.undoBar = this.element.querySelector<HTMLElement>(".map-undo")!;
    this.chooseBar = this.element.querySelector<HTMLElement>(".map-choose")!;
    this.cards = new MachineCards(this.element.querySelector<HTMLElement>(".map-card")!, {
      words,
      access: options.access,
      renaming: options.renaming,
      change: (login, change) => options.change(login, change),
      refresh: () => options.store.refresh(),
      world: () => options.store.world.peek(),
      anchorOf: (key) => this.anchorOf(key),
      room: () => {
        const canvas = this.canvas.getBoundingClientRect();
        const right = this.panel.hidden ? canvas.width : Math.min(canvas.width, this.panel.getBoundingClientRect().left - canvas.left);
        return { right, bottom: this.canvas.clientHeight };
      },
      narrow: () => this.narrow(),
      opened: () => {
        this.closeMenu();
        this.redraw();
      },
      closed: (held) => {
        this.redraw();
        if (held) this.refocus();
      },
      redraw: () => this.redraw(),
      failed: (key, said) => {
        this.note = { machine: key.replace(/^m:/, ""), text: options.access.failed(said) };
        this.redraw();
      },
      undo: (said, redo) => this.offerUndo(said, redo),
      arrived: (keys, chosen) => {
        if (chosen) this.selected = chosen;
        this.arrived(keys);
      },
      checking: (key, on) => {
        if (on) this.opening.add(key);
        else this.opening.delete(key);
        this.redraw();
      },
    });
    this.list = this.element.querySelector<HTMLElement>(".ml-list")!;
    this.views = this.element.querySelector<HTMLElement>(".ml-views")!;
    this.finder = new MachineFind(words.find, {
      changed: () => {
        this.findCurrent = null;
        this.listReach = LIST_ROWS;
        this.redraw();
      },
      reach: (key) => this.reachFound(key),
    });
    this.canvas.append(this.finder.element);
    this.findToggle = this.element.querySelector<HTMLElement>(".mf-toggle")!;
    this.findToggle.addEventListener("click", () => (this.finder.open ? this.finder.close() : this.finder.show()));
    document.addEventListener("keydown", this.findKey);
    // A machine named in the address is shown on the drawing, or ticked in
    // the update view.
    this.view = ticked ? "updates" : options.focus ? "map" : keptView();
    if (this.view === "updates" && !options.updates) this.view = "list";
    this.views.addEventListener("click", (event) => {
      const view = (event.target as HTMLElement).closest<HTMLElement>("[data-view]")?.dataset.view as View | undefined;
      if (view) this.show(view);
    });
    this.list.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const press = event as MouseEvent;
      if (this.updateView?.press(target)) {
        event.preventDefault();
        event.stopPropagation();
        return;
      }
      const all = target.closest<HTMLElement>("[data-ml-pick-all]")?.dataset.mlPickAll;
      if (all) {
        event.preventDefault();
        return this.pickAll(all);
      }
      const box = target.closest<HTMLElement>("[data-ml-pick]")?.dataset.mlPick;
      const row = target.closest<HTMLElement>("[data-ml-key]")?.dataset.mlKey;
      // While several are being chosen on a touch screen, a press ticks.
      if (box || (row && (press.ctrlKey || press.metaKey || this.selecting))) {
        event.preventDefault();
        return this.pickOne((box ?? row)!);
      }
      if (row && press.shiftKey) {
        event.preventDefault();
        return this.pickRange(row);
      }
      const fold = target.closest<HTMLElement>("[data-ml-fold]");
      if (fold?.dataset.mlFold) return this.foldRow(fold.dataset.mlFold, fold.getAttribute("aria-expanded") !== "true");
      const open = target.closest<HTMLElement>("[data-ml-open]")?.dataset.mlOpen;
      if (open) return this.details(open);
      const hub = target.closest<HTMLElement>("[data-ml-add]")?.dataset.mlAdd;
      const login = hub ? this.hubLogin(hub) : null;
      if (login) this.invite(login);
    });
    this.longPresses();
    // The film of what a network is for, over the drawing, once in one:
    // the last word of the line of hints under it.
    const again = document.createElement("button");
    again.type = "button";
    again.className = "text-button quiet-button machines-film-again";
    again.title = words.filmAgainWhat;
    again.innerHTML = `${glyph("regenerate", 14)}<span>${escape(words.filmAgain)}</span>`;
    again.addEventListener("click", () => {
      watchFilm((shape) => network(words.film, shape), words.filmAgainWhat, words.filmClose);
    });
    this.map = new MachineMap({
      afterHints: again,
      words,
      people: options.people,
      storageKey: MAP_ORDER,
      event: (event) => {
        if (event.kind === "select") this.choose(event.key, event.add);
        else if (event.kind === "marquee") this.frame(event.keys, event.done, event.add);
        else if (event.kind === "context") this.openMenu(event.key, event.at);
        else if (event.kind === "open") this.opened(event.key);
      },
      frame: (frame) => {
        this.scale = frame.view.scale;
        if (this.selected && !this.spotBar.hidden && this.map.distant !== this.spotNamed) this.redraw();
        this.placeSpot();
        if (this.bring && frame.view.height > 0) {
          const key = this.bring;
          this.bring = null;
          this.bringIntoView(key);
        }
      },
    });
    this.canvas.prepend(this.map.element);
    this.log = new MachineLog(words, options.log, () => loginsOf(options.store.world.peek()?.accounts), (at) => options.when(at, options.store.clock.peek()));
    // The record of changes is the first of the drawing's controls, in its
    // corner; it opens over them.
    this.map.element.querySelector(".canvas-controls")?.prepend(this.log.button);
    this.canvas.append(this.log.panel);
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
    this.panel.addEventListener("click", (event) => {
      const section = (event.target as HTMLElement).closest<HTMLElement>(".move-section");
      const hub = this.selected?.startsWith("hub:") ? this.selected.slice(4) : null;
      if (section && hub) void this.moving.click(event.target as HTMLElement, hub).catch((error: unknown) => console.warn("the move could not go on", error));
    });
    this.panel.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const more = target.closest<HTMLElement>("[data-spot-more]");
      const copy = target.closest<HTMLElement>(".spot-copy .copy-button");
      if (more) this.openMore(more);
      else if (copy) void copyFrom(copy, options.manage.copied);
      else this.clickAct(event);
    });
    this.undoBar.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".map-undo-go")) void this.undo();
    });
    this.chooseBar.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-choose=new]")) this.moving.toNewServer(() => this.showMove());
      else if (target.closest("[data-choose=cancel]")) this.stopChoosing();
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.cards.open) {
        event.stopPropagation();
        this.cards.close();
      } else if (event.key === "Escape" && (this.moving.chosenHub || this.whyNot)) {
        event.stopPropagation();
        this.stopChoosing();
      } else if (event.key === "Escape" && this.framed.length) {
        event.stopPropagation();
        this.setPicked([]);
      } else if (event.key === "Escape" && this.selected) {
        event.stopPropagation();
        if (this.detailsOpen) this.closeDetails();
        else this.choose(null);
      }
    });
    const stopWorld = effect(() => this.draw(options.store.world.get(), options.store.clock.get()));
    const updates = options.updates;
    const stopUpdates = updates
      ? effect(() => {
          updates.state.get();
          this.redraw();
        })
      : () => {};
    this.stop = () => {
      stopWorld();
      stopUpdates();
      this.updateView?.stop();
    };
    this.updateView?.watch();
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
    this.left = true;
    this.stop();
    this.film?.destroy();
    this.moving.dispose();
    this.resized.disconnect();
    this.log.dispose();
    if (this.undoing) clearTimeout(this.undoing.timer);
    document.removeEventListener("keydown", this.findKey);
    this.listUnwatch();
    this.map.dispose();
  }

  /** Ctrl+F finds machines while the page is shown, the drawing, the list
   *  or the update view alike. */
  private readonly findKey = (event: KeyboardEvent) => {
    if (!this.element.isConnected || this.findToggle.hidden || !(event.ctrlKey || event.metaKey) || event.key.toLowerCase() !== "f" || event.altKey) return;
    event.preventDefault();
    this.finder.show();
  };

  /** The machines that match what is looked for, by key, in the order the
   *  World gives them; null while nothing is. */
  private foundIn(world: WorldNow): string[] | null {
    const query = this.finder.query;
    if (!query) return null;
    const kept = this.found;
    if (kept?.world === world && kept.query === query) return kept.keys;
    const keys = foundIn(mapOf(world, this.options.words).nodes, query);
    this.found = { world, query, keys };
    return keys;
  }

  /** The machines found, in the order the page shows them: the list's rows
   *  down, drawn yet or not; on the drawing, the list's order too, as the
   *  drawing reads by whose they are and in which group. */
  private foundShown(found: readonly string[]): string[] {
    if (this.view !== "map") return [...this.listRows.keys()];
    const kept = this.found;
    if (kept?.keys === found) return (kept.order ??= listOrder(kept.world, new Set(found), this.options.words));
    const world = this.options.store.world.peek();
    return world ? listOrder(world, new Set(found), this.options.words) : [...found];
  }

  /** Goes to a machine looked for and found: on the drawing it is chosen
   *  and flown to, near enough to read; in the list its row is washed and
   *  comes into view, what is ticked and what is open beside it left as they
   *  are. */
  private reachFound(key: string): void {
    if (this.view !== "map") {
      this.findCurrent = key;
      // Its row is drawn, however far down it stands.
      const at = this.listRows.get(key);
      if (at !== undefined && at >= this.listReach) this.listReach = at + LIST_ROWS;
      this.redraw();
      requestAnimationFrame(() => this.list.querySelector(`[data-ml-row="${CSS.escape(key)}"]`)?.scrollIntoView({ block: "nearest" }));
      return;
    }
    this.selected = key;
    this.detailsOpen = false;
    this.redraw();
    requestAnimationFrame(() => {
      if (this.narrow()) return this.bringIntoView(key);
      this.map.flyTo(key);
    });
  }

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
      case "unshare": {
        const to = button.dataset.to ?? "";
        if (login) void this.takeBack(button, login, to, { change: "share", machine, to, shared: false }, { change: "share", machine, to, shared: true });
        return;
      }
      case "assist-edit":
        if (login) this.assist(login, machine, button.dataset.agent ?? "", button.dataset.home ?? null, button.dataset.to ?? null);
        return;
      case "assist-end": {
        const to = button.dataset.to ?? "";
        const agent = button.dataset.agent ?? "";
        const home = button.dataset.home ? { home: button.dataset.home } : {};
        // Given again by the undo, it ends when it was to, and keeps its allowances.
        const until = button.dataset.until ? { until: button.dataset.until } : {};
        const caps = {
          ...(button.dataset.daily ? { daily: Number(button.dataset.daily) } : {}),
          ...(button.dataset.weekly ? { weekly: Number(button.dataset.weekly) } : {}),
          ...(button.dataset.fiveHour ? { five_hour: Number(button.dataset.fiveHour) } : {}),
        };
        if (login)
          void this.takeBack(
            button,
            login,
            `${agentName(agent)} ${to}`,
            { change: "assist", machine, to, agent, ...home, assisting: false },
            { change: "assist", machine, to, agent, ...home, assisting: true, ...until, ...caps },
          );
        return;
      }
      case "invite":
        if (login) this.invite(login);
        return;
      case "remove":
        this.removeNamed(machine, login?.hub ?? null);
        return;
      case "framed-clear":
        this.setPicked([]);
        return;
      case "close-details":
        this.closeDetails();
        return;
    }
    const act = button.dataset.act ?? "";
    const world = this.options.store.world.peek();
    if (act.startsWith("framed-") && world) this.chosen.click(act, this.pickedMachines(world), world);
  }

  /** Whether the page is narrow enough that what is known of the one
   *  chosen rises as a sheet from below (`machines.css`, 860 pixels). */
  private narrow(): boolean {
    const width = this.element.clientWidth;
    return width > 0 && width <= 860;
  }

  /** On a narrow page the one chosen is kept clear of its bar, or its
   *  card: where they would lie over it, the drawing moves up only as far as
   *  brings it into what they leave, a little air round it. */
  private bringIntoView(key: string): void {
    const at = this.map.screenOf(key);
    const below = !this.panel.hidden ? this.panel : !this.spotBar.hidden ? this.spotBar : null;
    if (!at || !below) return;
    const box = this.canvas.getBoundingClientRect();
    // It stands on the bottom of the window; its height, not where it is
    // while it still rises.
    const top = document.documentElement.clientHeight - below.offsetHeight;
    const room = Math.max(0, Math.min(box.height, top - box.top));
    const air = Math.max(RING * this.scale, 16) + 24;
    const dy = at[1] > room - air ? room - air - at[1] : at[1] < air ? air - at[1] : 0;
    const dx = at[0] < air ? air - at[0] : at[0] > box.width - air ? box.width - air - at[0] : 0;
    this.map.glide(dx, dy);
  }

  /** Ticks the machine a machine's page sent here for, and brings its row
   *  into view; on a narrow page as a finger held on it does. */
  private tick(key: string): void {
    this.ticking = false;
    if (this.isPickable(key)) {
      if (this.element.isConnected ? this.narrow() : window.innerWidth <= 860) {
        this.selecting = true;
        layerOpened(this.selectLayer);
        this.anchor = key;
      }
      this.framed = [key];
      this.selected = null;
    }
    requestAnimationFrame(() => this.list.querySelector(`[data-ml-key="${CSS.escape(key)}"]`)?.scrollIntoView({ block: "nearest" }));
  }

  /** Shows the machines as a drawing or a list, and keeps the choice on
   *  this device. */
  private show(view: View): void {
    if (view === this.view) return;
    this.view = view;
    try {
      localStorage.setItem(VIEW_KEY, view);
    } catch {
      // Kept for this visit only.
    }
    // What is chosen stays chosen: one machine alone is the one chosen on
    // the drawing, and ticked in the list and the update view. On a narrow
    // page the list ticks only what a long press began to choose.
    this.updateView?.watch();
    if (view !== "map" && !this.framed.length && this.selected && this.isPickable(this.selected) && !this.narrow()) {
      this.framed = [this.selected];
      this.selected = null;
    } else if (view === "map" && this.framed.length === 1) {
      this.selected = this.framed[0]!;
      this.framed = [];
    }
    this.detailsOpen = false;
    this.closeMenu();
    this.cards.close();
    this.redraw();
    if (view === "map") requestAnimationFrame(() => this.map.refresh());
  }

  /** The machines chosen that an action pointed at leaves stand back, on
   *  the list and the drawing; none once nothing is pointed at. */
  private standBack(reach: Set<string> | null): void {
    for (const row of this.list.querySelectorAll<HTMLElement>("[data-ml-key]")) {
      row.classList.toggle("ms-left", !!reach && this.framed.includes(row.dataset.mlKey!) && !reach.has(row.dataset.mlKey!));
    }
    for (const label of this.element.querySelectorAll<HTMLElement>("[data-label]")) {
      label.classList.toggle("ms-left", !!reach && this.framed.includes(label.dataset.label!) && !reach.has(label.dataset.label!));
    }
  }

  /** Shows the film and the ways into a network while this computer is in
   *  none, the drawing once it is in one. */
  private alone(alone: boolean): void {
    this.intro.hidden = !alone;
    this.layout.hidden = alone;
    this.element.classList.toggle("alone", alone);
    if (alone && !this.film) {
      const film = new Film((shape) => network(this.options.words.film, shape), this.options.words.title);
      this.film = film;
      this.intro.querySelector(".machines-film")!.replaceChildren(film.element);
    } else if (!alone && this.film) {
      this.film.destroy();
      this.film = null;
    }
  }

  /** A frame drawn round machines: one chosen, or several together; with
   *  Ctrl, those framed are added to those chosen. */
  private frame(keys: string[], done: boolean, add: boolean): void {
    const can = this.pickableKeys();
    const inside = keys.filter((key) => can.has(key));
    const machines = add ? added(this.pickedNow(), inside) : inside;
    if (!done) {
      if (JSON.stringify(machines) === JSON.stringify(this.framing)) return;
      this.framing = machines;
      const world = this.options.store.world.peek();
      if (world) this.drawMap(world);
      return;
    }
    this.framing = [];
    if (!add && machines.length < 2) return this.choose(machines[0] ?? null);
    this.setPicked(machines);
  }

  /** The machines chosen together; on the drawing one alone is the one
   *  chosen, its actions beside it. */
  private setPicked(keys: string[]): void {
    if (this.view === "map" && keys.length === 1) {
      this.framed = [];
      this.selected = keys[0]!;
    } else {
      this.framed = keys;
      if (keys.length) this.selected = null;
    }
    if (!this.framed.length) this.selecting = false;
    this.closeMenu();
    this.cards.close();
    this.redraw();
  }

  /** Those chosen now: the one chosen on the drawing among them. */
  private pickedNow(): string[] {
    if (this.framed.length) return this.framed;
    return this.selected && this.isPickable(this.selected) ? [this.selected] : [];
  }

  /** The machines chosen, as the World now has them. */
  private pickedMachines(world: WorldNow): Machine[] {
    const spots = this.spots(world);
    return this.framed.flatMap((key) => {
      const spot = spots.get(key);
      return spot?.kind === "machine" ? [spot.machine] : [];
    });
  }

  /** The keys of the machines that may be chosen together now. */
  private pickableKeys(): Set<string> {
    const world = this.options.store.world.peek();
    return new Set(world ? mapOf(world, this.options.words).nodes.filter(pickable).map((node) => node.key) : []);
  }

  private isPickable(key: string): boolean {
    return this.pickableKeys().has(key);
  }

  /** A finger held on a machine's row starts choosing several, that one
   *  ticked, as phones do; the system's own long-press menu gives way. */
  private longPresses(): void {
    this.list.dataset.ownPress = "";
    onHeld(this.list, "[data-ml-key]", (row, _event, input) => {
      if (input !== "touch") return false;
      if (this.selecting) return;
      const key = row.dataset.mlKey!;
      if (!this.isPickable(key)) return false;
      this.selecting = true;
      layerOpened(this.selectLayer);
      this.anchor = key;
      this.setPicked([key]);
    });
  }

  /** The head of choosing several: how many, and every one or none. */
  private drawSelectHead(count: number): void {
    const head = this.element.querySelector<HTMLElement>(".ml-select-head");
    if (!head) return;
    const { words, batch } = this.options;
    head.querySelector(".ml-select-count")!.textContent = words.framedTitle(count);
    const every = this.pickableKeys().size;
    head.querySelector("[data-select=all]")!.textContent = every && count >= every ? batch.selectNone : batch.selectAll;
  }

  /** Every machine that may be chosen, or none once they all are. */
  private selectAll(): void {
    const all = [...this.pickableKeys()];
    const now = this.pickedNow();
    this.setPicked(all.every((key) => now.includes(key)) ? [] : all);
  }

  /** A row's box, or a row pressed with Ctrl: it is added or taken out. */
  private pickOne(key: string): void {
    this.anchor = key;
    this.setPicked(toggled(this.pickedNow(), key));
  }

  /** A row pressed with Shift: every row from the last one chosen to it. */
  private pickRange(key: string): void {
    const order = [...this.list.querySelectorAll<HTMLElement>("[data-ml-key]")].map((row) => row.dataset.mlKey!);
    this.setPicked(added(this.pickedNow(), between(order, this.anchor, key)));
    this.anchor = key;
  }

  /** A box, a group's band or a heading: all of its machines chosen, or let
   *  go when they all were. */
  private pickAll(key: string): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const keys = pickableIn(mapOf(world, this.options.words).nodes, key);
    const now = this.pickedNow();
    this.setPicked(keys.every((one) => now.includes(one)) ? now.filter((one) => !keys.includes(one)) : added(now, keys));
  }

  private choose(key: string | null, add = false): void {
    // Choosing where the server goes: a machine that can take it asks to
    // be confirmed; any other says why it cannot; the paper ends choosing.
    if (this.moving.chosenHub) {
      const machine = key?.startsWith("m:") ? this.machineNamed(key) : null;
      if (!machine) return this.stopChoosing();
      this.whyNot = this.moving.whyNot(machine);
      if (!this.whyNot) this.moving.confirmTo(machine, () => this.showMove());
      this.redraw();
      return;
    }
    // Pressed with Ctrl, a machine is added to those chosen or taken out,
    // and a box's strip or a group's band adds all of its machines.
    if (add && key) {
      if (key.startsWith("group:")) {
        const world = this.options.store.world.peek();
        if (world) this.setPicked(added(this.pickedNow(), pickableIn(mapOf(world, this.options.words).nodes, key)));
        return;
      }
      if (this.isPickable(key)) return this.setPicked(toggled(this.pickedNow(), key));
    }
    // The dashed box that adds a person is itself the way to add one; a
    // person's "⋯" lists what may be done to them.
    if (key?.startsWith("adding:")) {
      this.people.add(key.slice("adding:".length), key);
      return;
    }
    if (key?.startsWith("who:") || key?.startsWith("gm:")) {
      const at = this.map.screenOf(key);
      if (at) this.openMenu(key, [at[0] - 20, at[1] + 18]);
      return;
    }
    // A box or a group pressed: brought to fill the view, near enough to see
    // its machines.
    if (key?.startsWith("group:")) {
      this.map.flyTo(key);
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
    this.detailsOpen = false;
    this.note = null;
    this.redraw();
    if (key && this.narrow()) requestAnimationFrame(() => this.bringIntoView(key));
  }

  /** What is known of the one chosen closes; on the drawing its bar is
   *  there again. */
  private closeDetails(): void {
    const held = this.panel.contains(document.activeElement);
    this.detailsOpen = false;
    this.redraw();
    if (held) this.refocus();
  }

  /** A group's or a person's row pressed: it opens or closes, and stays so
   *  on this device. */
  private foldRow(key: string, open: boolean): void {
    this.listFolds.set(key, open);
    try {
      localStorage.setItem(FOLDS_KEY, JSON.stringify(Object.fromEntries(this.listFolds)));
    } catch {
      // Kept for this visit only.
    }
    this.redraw();
  }

  /** A ring pressed twice opens what is known of it: a machine's own page,
   *  or the card of a hub, a phone or an assisting agent. */
  private opened(key: string): void {
    const world = this.options.store.world.peek();
    if (world && this.spots(world).has(key)) this.details(key);
  }

  /** What is known of `key`: a machine's own page; the card of a hub, a
   *  phone or an assisting agent. */
  private details(key: string): void {
    if (key.startsWith("m:")) {
      location.hash = link({ kind: "machine", name: key.slice("m:".length) });
      return;
    }
    this.selected = key;
    this.detailsOpen = true;
    this.closeMenu();
    this.redraw();
    this.panel.scrollTop = 0;
    // Keys go to the card: Escape closes it.
    this.panel.focus({ preventScroll: true });
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

  /** Takes the machine written `name` off, one of three ways; a name of
   *  its hub's directory where `hub` says which. */
  private removeNamed(name: string, hub: string | null = null): void {
    const world = this.options.store.world.peek();
    const machine = world
      ? machinesOf(world).find((one) => (hub === null ? one.written === name : one.name === name && one.on.some((on) => on.hub === hub)))
      : undefined;
    if (!world || !machine) return;
    const { manage, access } = this.options;
    removeMachines([machine], world, manage.removeTitle(machine.written), {
      manage,
      access,
      sheet: this.removing,
      running: (one) => this.options.updates?.state.peek()?.network?.machines?.[one],
      change: this.options.change,
      log: this.options.log,
      done: () => void this.options.store.refresh(),
    });
  }

  /** Dissolves `hub`'s network, once its administrator typed its name:
   *  every machine and phone leaves it, this computer too. */
  private dissolve(login: Login, hub: HubAccounts): void {
    const { words } = this.options;
    const confirm = this.options.confirm;
    const away = hub.members.filter((member) => !member.online).length;
    confirm.ask({
      title: words.dissolveTitle(hub.name),
      what: words.dissolveWhat(hub.name, away),
      fields: typedName(words.typeName(hub.name)),
      go: words.dissolveGo,
      act: () => this.options.change(login, { change: "dissolve", name: hub.name }),
      done: () => void this.options.store.refresh(),
    });
    holdUntilNamed(confirm.body, hub.name);
  }

  /** Signs a phone out of the account, once asked. */
  private removePhone(login: Login, name: string): void {
    const { access, words } = this.options;
    this.options.confirm.ask({
      title: access.removeTitle(name),
      what: words.removePhoneWhat(name),
      go: access.removeGo,
      act: () => this.options.change(login, { change: "remove", name, reason: access.reasonDefault }),
      done: () => void this.options.store.refresh(),
    });
  }

  /** The assist sheet for `agent`'s login `home` on `machine`, given as
   *  `login`, with `editing`'s deadline open. */
  private assist(login: Login, machine: string, agent: string, home: string | null, editing: string | null): void {
    const { options } = this;
    const assistances = (): Assistance[] => assistancesIn(this.hubOf(login)?.assists ?? [], machine, agent, home);
    const name = home ? `${agentName(agent)} ${home.split(/[\\/]/).filter(Boolean).pop() ?? home}` : agentName(agent);
    this.assistSheet.open({
      title: options.access.assistTitle(name, machine),
      assistances: assistances(),
      assist: async (to, terms) => {
        await options.change(login, assistChange(machine, agent, home, to, terms));
        await options.store.refresh();
        this.redraw();
        return assistances();
      },
      editing,
      costs: agent === "claude",
      names: namesOn(this.hubOf(login), login),
    });
  }

  /** Takes back a share or ends an assistance at once; for a moment it can be given
   *  again from the drawing's corner. */
  private async takeBack(button: HTMLButtonElement, login: Login, label: string, take: AccountChange, give: AccountChange): Promise<void> {
    const machine = button.closest<HTMLElement>("[data-machine]")?.dataset.machine ?? "";
    button.disabled = true;
    try {
      await this.options.change(login, take);
      this.note = null;
    } catch (error) {
      this.note = { machine, text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
      button.disabled = false;
      this.redraw();
      return;
    }
    this.offerUndo(this.options.words.tookBack(label), () => this.options.change(login, give));
    await this.options.store.refresh();
    this.redraw();
  }

  private redraw(): void {
    // A timer set before the page was left draws nothing after.
    if (this.left) return;
    this.drawn = "";
    this.listed = "";
    this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** Everything on the map, by its key, made once for each World. */
  private spots(world: WorldNow): Map<string, Spot> {
    const kept = spotsMade.get(world);
    if (kept) return kept;
    const spots = new Map<string, Spot>();
    spotsMade.set(world, spots);
    for (const hub of world.accounts.hubs) if (hub.logins.length) spots.set(`hub:${hub.hub}`, { kind: "hub", hub });
    for (const machine of machinesOf(world)) spots.set(`m:${machine.written}`, { kind: "machine", machine });
    for (const assistant of assistantsOf(world)) {
      const key = `assist:${assistant.written}`;
      const known = spots.get(key);
      if (known?.kind === "assist") known.assistants.push(assistant);
      else spots.set(key, { kind: "assist", assistants: [assistant] });
    }
    for (const { hub, member, login, owner } of phonesOf(world))
      spots.set(deviceKey(hub.hub, member.name), { kind: "device", hub, member, login, owner, here: member.name === world.accounts.machine && !isComputer(world) });
    return spots;
  }

  private draw(world: WorldNow | null, now: Date): void {
    if (!world) return;
    const { words } = this.options;
    const spots = this.spots(world);
    const found = this.foundIn(world);
    // A machine named in the address may be one whose agent assists.
    if (this.selected?.startsWith("m:") && !spots.has(this.selected) && spots.has(`assist:${this.selected.slice(2)}`)) this.selected = `assist:${this.selected.slice(2)}`;
    if (this.selected && !spots.has(this.selected)) this.selected = null;
    // The drawing is laid out only while it is the view shown: showing it
    // again draws it as the World then stands (`show`).
    if (this.view === "map") this.drawMap(world);
    const spot = this.selected ? spots.get(this.selected) : undefined;
    if (this.focusing && spot) {
      this.focusing = false;
      if (spot.kind === "machine") this.detailsOpen = false;
    }
    if (spot?.kind === "machine") this.detailsOpen = false;
    if (this.ticking && spot) this.tick(this.selected!);
    // On a narrow page nothing floats beside the one chosen: its sheet rises
    // at once, its actions a row in it.
    const narrow = this.narrow();
    const framed = this.framed.filter((key) => spots.get(key)?.kind === "machine");
    // Choosing several on a touch screen ends with the list, or once none
    // is chosen.
    if (this.selecting && (this.view === "map" || !framed.length)) this.selecting = false;
    this.element.classList.toggle("selecting", this.selecting);
    if (this.selecting) this.drawSelectHead(framed.length);
    // In the list one machine ticked is enough for what is done to them
    // together; on the drawing one alone is the one chosen. What is done to
    // them is the bar at the foot of the page, which takes no room from the
    // list; the panel is only ever the details of one.
    const together = framed.length > (this.view === "map" ? 1 : 0);
    // The one chosen on the drawing has its bar beside it, out of the way
    // of a menu or a card of changes while they are open, and of the
    // server's new place being chosen; what is known of a hub, a phone or
    // an assisting agent opens when asked for, on the drawing in the bar's
    // place, beside the list, or from below.
    const anchored = !narrow && this.view === "map";
    const open = !!spot && !together && this.detailsOpen;
    const barred = !!spot && !together && !open && this.view === "map" && this.menuFor === null && !this.cards.open && !this.moving.chosenHub;
    if (open && this.panel.hidden) layerOpened(this.detailsLayer);
    if (barred && narrow && this.spotBar.hidden) layerOpened(this.barLayer);
    this.panel.hidden = !open;
    this.panel.classList.toggle("anchored", anchored);
    this.spotBar.hidden = !barred;
    this.spotBar.classList.toggle("phone", narrow);
    // Laid beside the one chosen only on a wide page's drawing.
    if (!anchored) this.panel.style.transform = this.spotBar.style.transform = "";
    // In no network, the page is the film of what a network is for and the
    // ways into one; the drawing waits until there is one.
    const alone = !loginsOf(world.accounts).length && isComputer(world);
    this.alone(alone);
    const listing = !alone && this.view !== "map";
    const updating = listing && this.view === "updates";
    this.views.hidden = alone;
    // Nothing is found where there is only this computer.
    this.findToggle.hidden = alone;
    if (alone && this.finder.open) queueMicrotask(() => this.finder.close());
    for (const button of this.views.querySelectorAll<HTMLElement>("[data-view]")) {
      const chosen = button.dataset.view === this.view;
      button.classList.toggle("chosen", chosen);
      button.setAttribute("aria-checked", String(chosen));
    }
    this.canvas.classList.toggle("listing", listing);
    this.list.hidden = !listing;
    this.list.classList.toggle("picking", framed.length > 0);
    this.list.classList.toggle("updates", updating);
    const able = this.updateView?.able() ?? 0;
    const count = this.views.querySelector<HTMLElement>(".ml-view-count");
    if (count) {
      count.hidden = !able;
      count.textContent = able ? String(able) : "";
    }
    this.list.classList.toggle("selecting", this.selecting);
    // Beside the list on a wide page the panel takes its own room.
    this.canvas.classList.toggle("beside", listing && open && !narrow);
    if (listing) {
      const adds = world.accounts.hubs.filter((hub) => this.hubLogin(hub.hub)).map((hub) => hub.hub);
      const { html, rows } = machineList(
        {
          world,
          now,
          selected: open ? this.selected : this.findCurrent && found?.includes(this.findCurrent) ? this.findCurrent : null,
          adds,
          // The list's columns are narrow: a day before today is its date.
          when: (at, now) => (at.toDateString() === now.toDateString() ? this.options.when(at, now) : this.options.access.moment.short(at, now)),
          picked: new Set(framed),
          pick: { one: this.options.batch.pickOne, all: this.options.batch.pickAll },
          open: (key, byDefault) => this.listFolds.get(key) ?? byDefault,
          updates: updating ? this.updateView?.now(narrow) : undefined,
          found: found && new Set(found),
          reach: this.listReach,
        },
        words,
      );
      this.listRows = rows;
      if (html !== this.listed) {
        this.listed = html;
        // The update view's machines scroll on their own, and are where
        // they were after each drawing.
        const scrolled = this.list.querySelector<HTMLElement>(".uv-rows")?.scrollTop ?? 0;
        redrawKeepingFocus(this.list, () => {
          this.list.innerHTML = html;
          const rows = this.list.querySelector<HTMLElement>(".uv-rows");
          if (rows) rows.scrollTop = scrolled;
        });
        this.listUnwatch();
        this.listUnwatch = whenNear(this.list, this.list.querySelector(".uv-rows") ?? this.list, () => {
          this.listReach += LIST_ROWS;
          this.redraw();
        });
      }
    }
    // What can be done to it: on its bar, the one done most and a few
    // marks, the rest behind "⋯"; at the foot of its card the same.
    const acts = spot && (open || barred) ? spotActs(this.actsOf(this.selected!, world)) : null;
    const note = this.note && spot && this.note.machine && this.note.machine === nameOf(spot) ? { said: this.note.text, detail: this.note.text } : null;
    // Where the drawing writes no names, the bar says whose it is.
    this.spotNamed = this.map.distant;
    const named = narrow || (spot?.kind !== "hub" && this.map.distant);
    const barHtml = barred && acts ? this.spotViews.bar(spot!, this.selected!, acts, named, note) : "";
    if (barHtml !== this.spotDrawn) {
      this.spotDrawn = barHtml;
      redrawKeepingFocus(this.spotBar, () => {
        this.spotBar.innerHTML = barHtml;
      });
    }
    const chosenBar = !alone && together ? this.chosen.html(this.pickedMachines(world), world, updating) : "";
    this.bar.hidden = !chosenBar;
    this.bar.classList.toggle("phone", narrow);
    if (chosenBar !== this.barDrawn) {
      this.barDrawn = chosenBar;
      redrawKeepingFocus(this.bar, () => {
        this.bar.innerHTML = chosenBar;
      });
    }
    const html = alone
      ? ""
      : spot && open && acts
        ? `<button class="icon-button panel-close" type="button" data-act="close-details" aria-label="${escape(words.closeDetails)}" title="${escape(words.closeDetails)}">${glyph("close", 16)}</button>${this.spotViews.card(spot, this.selected!, world, now, acts, note)}`
        : "";
    if (html !== this.drawn) {
      this.drawn = html;
      redrawKeepingFocus(this.panel, () => {
        this.panel.innerHTML = html;
      });
    }
    this.drawChoosing();
    // The server's details follow its move while they are open, for the one
    // who may move it.
    const top = spot?.kind === "hub" && open ? this.superLogin(spot.hub, world) : undefined;
    if (top && spot?.kind === "hub") this.moving.watch(top, spot.hub.hub);
    else this.moving.unwatch();
    this.placeSpot();
    this.finder.showing(found ? this.foundShown(found) : []);
  }

  /** The strip over the drawing while the server's new place is chosen. */
  private drawChoosing(): void {
    const { words } = this.options;
    const choosing = !!this.moving.chosenHub;
    this.chooseBar.hidden = !choosing && !this.whyNot;
    if (this.chooseBar.hidden) return;
    const said = this.whyNot
      ? `<span class="map-choose-said why"${this.whyNot.detail ? ` title="${escape(this.whyNot.detail)}"` : ""}>${escape(this.whyNot.said)}</span>`
      : `<span class="map-choose-said">${escape(words.moveChoose)}</span>`;
    const fresh = choosing && this.options.moving.remote ? `<button class="text-button quiet-button" type="button" data-choose="new">${glyph("server", 14)}<span>${escape(words.moveNewServer)}</span></button>` : "";
    const html = `${said}${fresh}<button class="text-button quiet-button" type="button" data-choose="cancel"><span>${escape(choosing ? words.cancel : words.closeDetails)}</span></button>`;
    if (this.chooseBar.innerHTML !== html) this.chooseBar.innerHTML = html;
  }

  private stopChoosing(): void {
    this.whyNot = null;
    this.moving.stopChoosing();
    this.redraw();
  }

  /** The server whose move just began, its details open to follow it. */
  private showMove(): void {
    const world = this.options.store.world.peek();
    const hub = world?.accounts.hubs.find((one) => this.superLogin(one, world));
    this.whyNot = null;
    if (hub) {
      this.selected = `hub:${hub.hub}`;
      this.detailsOpen = true;
    }
    this.redraw();
  }


  private superLogin(hub: HubAccounts, world: WorldNow): Login | undefined {
    return loginsOf(world.accounts).find((login) => login.hub === hub.hub && hub.accounts.find((account) => account.name === login.account)?.level === "super");
  }

  /** The machine a key on the drawing stands for, by its name. */
  private machineNamed(key: string): string | null {
    const world = this.options.store.world.peek();
    const spot = world ? this.spots(world).get(key) : undefined;
    return spot?.kind === "machine" ? spot.machine.name : null;
  }

  /** The keys of the machines that can take the server. */
  private litKeys(world: WorldNow): Map<string, { sleeps: boolean }> {
    const candidates = this.moving.candidates();
    const lit = new Map<string, { sleeps: boolean }>();
    for (const machine of machinesOf(world)) {
      const candidate = candidates.get(machine.name);
      if (candidate) lit.set(`m:${machine.written}`, candidate);
    }
    return lit;
  }

  /** The drawing: each hub the account is on with the ring that adds a
   *  machine there, each machine it may work on, and each machine of
   *  another whose agent assists it. A machine come to a hub one was on its
   *  way to is circled as it arrives. */
  private drawMap(world: WorldNow): void {
    const made = mapOf(world, this.options.words);
    const found = this.foundIn(world);
    const { hubs, nodes, links, people } = made;
    const hubLabels = new Map(made.hubLabels);
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
    // While the server moves, under its name: where to, then who is still to
    // follow.
    for (const hub of hubs) {
      const moving = this.moving.label(hub.id);
      if (moving) hubLabels.set(hub.id, moving);
    }
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
      lit: this.moving.chosenHub ? this.litKeys(world) : null,
      found: found && new Set(found),
      people: this.people.shown(people),
      adding: this.people.hubs(world),
      grouped: this.groupedBoxes(world, nodes),
    });
  }

  /** What can be done to a spot, as `world` stands. */
  private actsOf(key: string, world: WorldNow): Act[] {
    const { words } = this.options;
    // A box's or a group's machines are chosen together from its "⋯", or
    // its strip pressed with the right button.
    const all: Act[] = pickableIn(mapOf(world, words).nodes, key).length ? [{ act: "pick-group", label: this.options.batch.pickGroup, icon: "check" }] : [];
    const person = personOf(key);
    if (person) return [...all, ...this.people.acts(world, person.hub, person.account)];
    if (groupMoreOf(key))
      return [
        ...all,
        ...(this.groupBox(key, world)
          ? [
              { act: "rename-group", label: words.groups.renameGroup, icon: "edit" as const },
              { act: "end-group", label: words.groups.endGroup, icon: "unarchive" as const },
            ]
          : []),
      ];
    if (key.startsWith("group:")) return all;
    const spot = this.spots(world).get(key);
    if (!spot) return [];
    const details: Act = { act: "details", label: words.details, icon: "info" };
    if (spot.kind === "hub") {
      const logins = loginsOf(world.accounts).filter((login) => login.hub === spot.hub.hub);
      const top = logins.some((login) => spot.hub.accounts.find((account) => account.name === login.account)?.level === "super");
      const open = spot.hub.self_registration;
      return [
        ...(logins.length ? [{ act: "invite", label: words.inviteDevice, icon: "plus" as const }] : []),
        ...(top ? [{ act: "move", label: words.moveHub, icon: "move" as const }] : []),
        ...(top ? [{ act: "signup", label: this.options.people.signUpState(open), icon: "signup" as const, on: open }] : []),
        ...(top && spot.hub.versions && this.options.updates ? [{ act: "updates", label: this.options.updates.words.policy, icon: "update" as const }] : []),
        details,
        ...(top && spot.hub.dissolves ? [{ act: "dissolve", label: words.dissolve, icon: "close" as const, danger: true }] : []),
      ];
    }
    if (spot.kind === "assist") return [{ act: "talk", label: words.talk, icon: "plus" }, details];
    // A phone is only signed in: nothing runs on it to work with.
    if (spot.kind === "device") return [details, ...(spot.here ? [] : [{ act: "remove", label: words.removePhone, icon: "close" as const, danger: true }])];
    const holders = this.holdersOf(spot.machine, world);
    // Direct connections are the network's administrator's to allow: to a
    // computer, while it is online to be checked, and no longer at any time.
    const admin = this.adminOf(spot.machine, world);
    const member = admin ? this.memberOf(spot.machine, admin, world) : undefined;
    const direct: Act[] =
      member?.kind !== "computer" ? [] : member.direct?.length ? [{ act: "direct", label: words.stopDirect, icon: "link", on: true }] : member.reach ? [{ act: "direct", label: words.allowDirect, icon: "link", on: false }] : [];
    // Relaying for its owner's other machines on its network is for those
    // who hold it to allow, a visitor too, on a hub that keeps relays; where
    // it takes direct connections, they forward already.
    const own = holders[0];
    const mine = own ? this.memberOf(spot.machine, own, world) : undefined;
    const relays = !!own && !!world.accounts.hubs.find((hub) => hub.hub === own.hub)?.lan_relays;
    const relay: Act[] =
      relays && mine?.kind === "computer" && !mine.direct?.length && (mine.relay?.length || mine.reach?.interfaces?.length)
        ? [{ act: "relay", label: words.relay, icon: "relay", on: !!mine.relay?.length }]
        : [];
    // Those who hold it and the network's administrator rename it.
    const rename: Act[] = renamerOf(spot.machine, world, this.options.change, this.options.renaming.here)
      ? [{ act: "rename", label: this.options.renaming.words.rename, icon: "edit" }]
      : [];
    // And they group it.
    const group: Act[] = groupingOf(spot.machine, world) ? [{ act: "group", label: words.groups.group, icon: "folder" }] : [];
    // A managed machine has no agents of anyone's on it; a visitor shares
    // nothing it holds, and the machine the hub runs on is shared with no one.
    const managed = isManaged(world, spot.machine.name);
    const shares = !!this.giverOf(spot.machine, world);
    return [
      ...(managed
        ? []
        : [
            { act: "talk", label: words.talk, icon: "plus" as const },
            { act: "agents", label: words.hosts, icon: "agents" as const },
          ]),
      ...(shares ? [{ act: "share", label: words.share, icon: "colleague" as const }] : []),
      ...direct,
      ...relay,
      ...rename,
      ...group,
      details,
      ...(removerLoginOf(spot.machine, world) ? [{ act: "remove", label: words.remove, icon: "close" as const, danger: true }] : []),
    ];
  }

  /** The logins a machine is in the hands of: this computer's account on
   *  each hub it is registered on; another's owner first, then those it is
   *  shared with whole, who manage it alike. */
  private holdersOf(machine: Machine, world: WorldNow): Login[] {
    const logins = loginsOf(world.accounts);
    if (machine.here) return logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account));
    return [...logins.filter((login) => owns(machine, login)), ...logins.filter((login) => !owns(machine, login) && holds(machine, login, world))];
  }

  /** The login that shares `machine` on: one holding it that is no visitor,
   *  where it does not run the hub. */
  private giverOf(machine: Machine, world: WorldNow): Login | undefined {
    return this.holdersOf(machine, world).find((login) => !isVisitor(world, login) && !runsHub(machine, login, world));
  }

  /** Does an action pressed on a spot's bar or card, or in its menu. */
  private clickAct(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-map-act]");
    if (!button) return;
    this.closeMenu();
    this.runAct(button.dataset.key ?? "", button.dataset.mapAct!);
  }

  /** What lies behind the "⋯" of the one chosen, under it. */
  private openMore(anchor: HTMLElement): void {
    const world = this.options.store.world.peek();
    const key = anchor.dataset.spotMore ?? "";
    const spot = world ? this.spots(world).get(key) : undefined;
    if (!world || !spot) return;
    const { more } = spotActs(this.actsOf(key, world));
    if (!more.length) return;
    menuOf().open(anchor, nameOf(spot), moreEntries(more, this.options.words, this.options.people), (act) => this.runAct(key, act));
  }

  /** Does `act` to `key`. */
  private runAct(key: string, act: string): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    if (act === "fit") return this.map.fit();
    if (act === "pick-group") return this.setPicked(added(this.pickedNow(), pickableIn(mapOf(world, this.options.words).nodes, key)));
    const person = personOf(key);
    if (person) return this.people.run(act, person.hub, person.account, key);
    if (groupMoreOf(key)) {
      if (act === "rename-group") this.renameGroup(key);
      else if (act === "end-group") void this.endGroup(key);
      return;
    }
    const spot = this.spots(world).get(key);
    if (!spot) return;
    if (act === "details") return this.details(key);
    if (spot.kind === "hub") {
      const login = this.hubLogin(spot.hub.hub);
      if (act === "invite" && login) this.invite(login);
      else if (act === "signup") this.people.signUp(spot.hub.hub);
      else if (act === "updates") this.show("updates");
      else if (act === "dissolve") {
        const top = this.superLogin(spot.hub, world);
        if (top) this.dissolve(top, spot.hub);
      }
      else if (act === "move") {
        const top = this.superLogin(spot.hub, world);
        if (!top) return;
        // The server's new place is chosen on the drawing.
        this.show("map");
        this.selected = null;
        this.whyNot = null;
        this.moving.choose(top, spot.hub).catch((error: unknown) => {
          const said = error instanceof Error ? error.message : String(error);
          this.whyNot = { said: this.options.access.failed(said), detail: said };
          this.redraw();
        });
        this.redraw();
      }
      return;
    }
    if (spot.kind === "assist") {
      if (act === "talk") this.options.talk(spot.assistants[0]!.written);
      return;
    }
    if (spot.kind === "device") {
      if (act === "remove") this.removePhone(spot.login, spot.member.name);
      return;
    }
    const machine = spot.machine;
    switch (act) {
      case "talk":
        this.options.talk(machine.written);
        return;
      case "agents":
        location.hash = link({ kind: "agents", machine: machine.written });
        return;
      case "share": {
        const giver = this.giverOf(machine, world);
        if (giver) this.share(giver, machine.name);
        return;
      }
      case "remove":
        this.removeNamed(machine.written);
        return;
      case "rename":
        this.cards.rename(key, machine);
        return;
      case "group": {
        const grouping = groupingOf(machine, world);
        if (grouping) this.cards.group(key, [machine], [grouping]);
        return;
      }
      case "direct": {
        const admin = this.adminOf(machine, world);
        const member = admin ? this.memberOf(machine, admin, world) : undefined;
        if (!admin || !member) return;
        if (member.direct?.length) this.cards.stopDirect(admin, machine, key, member.direct);
        else this.cards.direct(admin, machine, key, member);
        return;
      }
      case "relay": {
        const own = this.holdersOf(machine, world)[0];
        const member = own ? this.memberOf(machine, own, world) : undefined;
        if (!own || !member) return;
        if (member.relay?.length) void this.cards.stopRelay(own, machine, member.relay);
        else this.cards.relay(own, machine, key, member);
        return;
      }
    }
  }

  /** Puts `machines`, all one owner's, in one group, from one card beside
   *  the first. */
  private groupMachines(machines: Machine[]): void {
    const world = this.options.store.world.peek();
    const groupings = world ? machines.map((machine) => groupingOf(machine, world)) : [];
    if (!machines.length || groupings.some((one) => !one)) return;
    this.cards.group(`m:${machines[0]!.written}`, machines, groupings as Grouping[]);
  }

  /** The card that renames the group of the box `key` names. */
  private renameGroup(key: string): void {
    const world = this.options.store.world.peek();
    const box = world ? this.groupBox(key, world) : null;
    if (box) this.cards.renameGroup(key, box);
  }

  /** Ends a group at once: its machines stand in their owner's box again,
   *  and for a moment the group can be given back from the drawing's
   *  corner. Nothing about the machines changes, so nothing asks first. */
  private async endGroup(key: string): Promise<void> {
    const world = this.options.store.world.peek();
    const box = world ? this.groupBox(key, world) : null;
    if (!box) return;
    try {
      await renameGroups(box, null, (login, change) => this.options.change(login, change));
      this.note = null;
    } catch (error) {
      this.note = { machine: "", text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
      this.redraw();
      return;
    }
    this.circle(box.machines);
    this.offerUndo(this.options.words.groups.endedGroup(box.group), () => regroup(box, (login, change) => this.options.change(login, change)));
    await this.options.store.refresh();
    this.redraw();
  }

  /** A group's box on the drawing, by its "⋯": its name, its machines and
   *  how each is grouped from here; none unless this device may group
   *  every one of them. */
  private groupBox(key: string, world: WorldNow, nodes: MapNode[] = mapOf(world, this.options.words).nodes): GroupBox | null {
    const box = groupMoreOf(key);
    if (!box) return null;
    return this.groupBoxOf(box, world, nodes.filter((node) => node.group?.key === box.kind && node.hubs.includes(box.hub)));
  }

  /** As `groupBox`, of the nodes the box holds. */
  private groupBoxOf(box: { hub: string; group: string }, world: WorldNow, inside: MapNode[]): GroupBox | null {
    const spots = this.spots(world);
    const machines = inside.flatMap((node) => {
      const spot = spots.get(node.key);
      return spot?.kind === "machine" ? [spot.machine] : [];
    });
    const groupings = machines.map((machine) => groupingOf(machine, world, box.hub));
    if (!machines.length || groupings.some((one) => !one)) return null;
    return { group: box.group, machines, groupings: groupings as Grouping[] };
  }

  /** The boxes of groups whose "⋯" the drawing shows, by its keys for
   *  boxes: the nodes sorted into their boxes once, each box then asked. */
  private groupedBoxes(world: WorldNow, nodes: MapNode[]): Set<string> {
    const inside = new Map<string, { hub: string; kind: string; nodes: MapNode[] }>();
    for (const node of nodes) {
      const kind = node.group?.key;
      if (!kind || boxParts(kind).group === null) continue;
      for (const hub of node.hubs) {
        const key = `group:${hub}:${kind}`;
        const known = inside.get(key);
        if (known) known.nodes.push(node);
        else inside.set(key, { hub, kind, nodes: [node] });
      }
    }
    const boxes = new Set<string>();
    for (const [key, { hub, kind, nodes: held }] of inside) {
      const box = groupMoreOf(groupMoreKey(hub, kind));
      if (box && this.groupBoxOf(box, world, held)) boxes.add(key);
    }
    return boxes;
  }

  /** Circles machines for a moment, as they stand somewhere new. */
  private circle(machines: Machine[]): void {
    this.arrived(machines.map((machine) => `m:${machine.written}`));
  }

  /** Circles spots for a moment, by their keys. */
  private arrived(keys: string[]): void {
    for (const key of keys) {
      this.fresh.add(key);
      setTimeout(() => {
        this.fresh.delete(key);
        this.redraw();
      }, ARRIVED_MS);
    }
  }

  /** The login of the network's administrator among this device's, on a
   *  hub `machine` is on. */
  private adminOf(machine: Machine, world: WorldNow): Login | undefined {
    return loginsOf(world.accounts).find((login) => machine.on.some((on) => on.hub === login.hub) && isSuper(world, login));
  }

  /** What `login`'s hub says of `machine`, by its name or one it had. */
  private memberOf(machine: Machine, login: Login, world: WorldNow): MemberSeen | undefined {
    return world.accounts.hubs.find((hub) => hub.hub === login.hub)?.members.find((member) => names(member, machine.name));
  }

  /** Where a card about `key` stands beside: its ring on the drawing, or
   *  its row in the list, which the drawing is not laid out under. */
  private anchorOf(key: string): Point | null {
    if (this.view === "map") return this.map.screenOf(key);
    const row = this.list.querySelector<HTMLElement>(`[data-ml-key="${CSS.escape(key)}"]`);
    if (!row) return null;
    const canvas = this.canvas.getBoundingClientRect();
    const name = (row.querySelector<HTMLElement>(".ml-name") ?? row).getBoundingClientRect();
    return [name.right - canvas.left, name.top - canvas.top + name.height / 2];
  }

  /** Back on the drawing, or the list, once a card or a menu over it is
   *  gone: keys go on to the page, Escape letting the one chosen go. */
  private refocus(): void {
    if (this.element.contains(document.activeElement) && document.activeElement !== document.body) return;
    (this.view === "map" ? this.map.element : this.list).focus({ preventScroll: true });
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

  /** On a wide page's drawing, lays the bar of the one chosen beside it,
   *  or the card asked for in its place: right of its name, else left of
   *  its ring, else under it; always inside the drawing, and following it
   *  as the drawing moves. The bar stands level with the ring, the card
   *  from a little over it. */
  private placeSpot(): void {
    const key = this.selected;
    if (!key || this.narrow() || this.view !== "map") return;
    const laid = !this.spotBar.hidden ? this.spotBar : !this.panel.hidden && this.panel.classList.contains("anchored") ? this.panel : null;
    const at = laid ? this.map.screenOf(key) : null;
    if (!laid || !at) return;
    const width = this.canvas.clientWidth;
    const height = this.canvas.clientHeight;
    const size = { width: laid.offsetWidth, height: laid.offsetHeight };
    // A hub's name stands a little right of its knot, a machine's right of
    // its ring (`machines.css`, .mw-hub and .mw-machine).
    const hub = key.startsWith("hub:");
    const ring = hub ? 8 : Math.max(RING * this.scale, 8);
    const gap = 12;
    const name = this.map.nameWidth(key);
    const right = at[0] + (name ? (hub ? Math.max(14, 16 * this.scale) : (RING + 15) * this.scale) + name : ring) + gap;
    const left = at[0] - ring - gap - size.width;
    let x = right;
    let y = laid === this.spotBar ? at[1] - size.height / 2 : at[1] - ring - 8;
    if (right + size.width > width - 8) {
      if (left >= 8) x = left;
      else {
        x = at[0] - size.width / 2;
        y = at[1] + ring + gap;
      }
    }
    x = Math.max(8, Math.min(width - size.width - 8, x));
    y = Math.max(8, Math.min(height - size.height - 8, y));
    laid.style.transform = `translate(${(this.canvas.offsetLeft + x).toFixed(1)}px, ${(this.canvas.offsetTop + y).toFixed(1)}px)`;
  }

  /** The menu of a spot, or of the drawing itself, where the right button
   *  was pressed; on a phone it rises from below, saying whose it is. */
  private openMenu(key: string | null, at: Point): void {
    const world = this.options.store.world.peek();
    if (!world) return;
    const { words } = this.options;
    if (key?.startsWith("add:") || key?.startsWith("adding:")) return this.choose(key);
    // A person's strip, pressed with the right button, lists what their "⋯"
    // does.
    const strip = key?.match(/^group:([^:]+):of:([^/]+)$/);
    if (strip && this.people.acts(world, strip[1]!, strip[2]!).length) key = personKey(strip[1]!, strip[2]!);
    // So does a group's.
    const groupStrip = key?.match(/^group:([^:]+):(.+)$/);
    if (groupStrip && boxParts(groupStrip[2]!).group !== null && this.groupBox(groupMoreKey(groupStrip[1]!, groupStrip[2]!), world)) key = groupMoreKey(groupStrip[1]!, groupStrip[2]!);
    const acts: Act[] = key ? this.actsOf(key, world) : [{ act: "fit", label: words.fit, icon: "expand" }];
    if (!acts.length) return;
    // A person's or a group's menu stands for them alone: what was chosen
    // on the drawing gives way, and a sheet says whose or which it is.
    const alone = key ? (personOf(key)?.account ?? groupMoreOf(key)?.group ?? null) : null;
    if (alone !== null || key?.startsWith("group:")) {
      this.selected = null;
      this.detailsOpen = false;
    } else if (key) this.selected = key;
    this.menuFor = key ?? "";
    const spot = key ? this.spots(world).get(key) : undefined;
    const box = this.canvas.getBoundingClientRect();
    menuOf().openAt(
      [box.left + at[0], box.top + at[1]],
      alone ?? (spot ? nameOf(spot) : ""),
      moreEntries(acts, words, this.options.people),
      (act) => this.runAct(key ?? "", act),
      () => {
        // Closed however it was, the bar of the one chosen is back.
        if (this.menuFor === null) return;
        this.menuFor = null;
        this.redraw();
        queueMicrotask(() => this.refocus());
      },
    );
    this.redraw();
  }

  private closeMenu(): void {
    if (this.menuFor === null) return;
    this.menuFor = null;
    spotMenu?.close();
    this.redraw();
  }

  /** Shares `machines`, all `login`'s, with the same accounts: those all of
   *  them are shared with are shown, and a change applies to each. */
  private shareMachines(machines: Machine[], login: Login): void {
    const { access, words } = this.options;
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

}

/** The person a key on the drawing stands for: their "⋯". */
function personOf(key: string): { hub: string; account: string } | null {
  const found = key.match(/^who:([^:]+):(.+)$/);
  return found ? { hub: found[1]!, account: found[2]! } : null;
}
