// The machines a login may use (`world-gui.md`, 账号、机器、共享与协助),
// drawn: the account's World as a hand-drawn map that moves under the hand
// (`machine-map.ts`), or set out as a list (`machine-list.ts`), as chosen in
// the page's head and kept on this device. Choosing a hub or a machine floats its actions beside
// it, small icons that say what they do when pointed at; the right button,
// or a long press, lists them with their names. What is known of the one
// chosen stands in a panel that opens only when asked for. Beside each hub
// a dashed ring adds a machine: it says one is on its way once a way in was
// given, and the machine is circled as it arrives. A machine of the
// account's own is shared and removed from here; whom it is shared with is
// taken back with a press. The network's administrator manages its people
// here too (`map-people.ts`). Long sayings stand behind a pointer's rest,
// not on the page.

import { glyph } from "../conversation/glyphs";
import { layerOpened, type Layer } from "./back";
import { onHeld } from "./press";
import { escape } from "../conversation/prose";
import { Film } from "../film/film";
import { network } from "../film/network";
import { watchFilm } from "../film/theater";
import type { Point } from "../map-canvas";
import { blot, circle, outline, ruled, seeded } from "../pen/shapes";
import type { AccountChange, HubAccounts, Login, MemberSeen } from "../world/accounts";
import { loginsOf, names } from "../world/accounts";
import type { NameProblem } from "../world/bridge";
import type { MachinePageWords } from "./machine-page-words";
import type { NameSaid } from "./name-words";
import { RenameField, renamerOf } from "./rename";
import type { WorldNow } from "../world/bridge";
import type { LinkState } from "../world/world";
import { agentName, osName, titleOf } from "../world/names";
import type { AccessWords, MachinesWords } from "./account-words";
import type { Confirm } from "./confirm";
import { redrawKeepingFocus } from "./focus";
import { AssistSheet, assistChange, assistancesIn, namesOn, type Assistance } from "./assist-sheet";
import { HubMoving, type HubMovingOptions } from "./hub-moving";
import { holdUntilNamed, typedName } from "./typed-name";
import { GiveSheet } from "./machine-sheets";
import { MachineLog, type ReadLog } from "./machine-log";
import { boxParts, chosenGroup, groupChange, groupForm, groupingOf, groupMoreKey, groupMoreOf, groupNameOf, groupRenameForm, wireGroupForm, type Grouping } from "./machine-groups";
import { foldedOf, MachineMap, MAP_ORDER, personKey } from "./machine-map";
import { isManaged, listOrder, machineList } from "./machine-list";
import { whenNear } from "./read-on";
import { MapPeople } from "./map-people";
import type { PeopleWords } from "./people-words";
import { RING } from "./map-layout";
import { foundIn, MachineFind } from "./machine-find";
import { mapOf, phonesOf } from "./map-of";
import { deviceKey, type MapNode } from "./map-nodes";
import { link, talkLink } from "./places";
import { runsHub, holds, assistantsOf, groupsOf, isComputer, isSuper, isVisitor, machineActsFor, machinesOf, ownerOf, owns, residentIn, sameLogin, superOf, type Assistant, type Machine } from "./reach";
import type { Page } from "./shell";
import { effect } from "./signals";
import type { UpdateWords } from "./update-words";
import { compareVersions, type MachineUpdate, type UpdateCall, type UpdateState, type UpdatesView, type UpdatesViewCall, type UpdatesViewReply } from "../world/updates";
import type { BatchWords } from "./machine-batch-words";
import { ChosenBar } from "./machine-framed";
import { rulerOf } from "./update-ruler";
import { lineOf } from "./update-marks";
import type { UpdateViewWords } from "./update-view-words";
import type { UpdatesNow } from "./machine-list";
import { removerLoginOf } from "./machine-logins";
import type { ManageWords } from "./machine-manage-words";
import { added, between, pickable, pickableIn, toggled } from "./machine-picking";
import { RemoveMachineSheet } from "./machine-remove";
import { removeMachines } from "./machine-removing";
import { WaysSheet } from "./machine-ways";
import type { Signal } from "./signals";
import type { VersionWindow } from "./version-window";
import { jobsOf, residentsOf, type Store } from "./store";
import { detailOf } from "./problem";

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
  /** Renaming a machine: the words of its field, the rule a name is
   *  checked by, and renaming this computer. */
  renaming: {
    words: MachinePageWords;
    nameSaid: NameSaid;
    check(text: string): Promise<NameProblem | null>;
    here(to: string): Promise<void>;
  };
  /** The network's update policy, as this device last heard it, and the
   *  words of setting it; what this device knows of releases and of the
   *  network's machines, and asking other machines about their updates. */
  updates?:
    | {
        words: UpdateWords;
        /** The update view's words. */
        view: UpdateViewWords;
        /** Asks `account`'s hub for its update view, or to schedule or set. */
        ask(hub: string, account: string, call: UpdatesViewCall): Promise<UpdatesViewReply>;
        /** The window of versions, picking the network's lowest one, or none. */
        minimum(current: string | null, pick: (version: string | null) => void): void;
        state: Signal<UpdateState | null>;
        remote(machine: string, call: UpdateCall): Promise<UpdateState>;
        /** Has a machine of an earlier release update to the newest on its line. */
        older(machine: string): Promise<unknown>;
        versions: VersionWindow;
      }
    | undefined;
  /** The words of machines chosen together, and of taking machines off. */
  batch: BatchWords;
  manage: ManageWords;
}

/** A group's box on the drawing: its name, its machines, and how each is
 *  grouped from this device, in the same order. */
interface GroupBox {
  group: string;
  machines: Machine[];
  groupings: Grouping[];
}

/** One thing on the map, as the page knows it. */
type Spot =
  | { kind: "machine"; machine: Machine }
  | { kind: "assist"; assistants: Assistant[] }
  | { kind: "hub"; hub: HubAccounts }
  | { kind: "device"; hub: HubAccounts; member: MemberSeen; login: Login; owner: string | null; here: boolean };

const spotsMade = new WeakMap<WorldNow, Map<string, Spot>>();

/** Opening the boxes of every machine found may show at most this many
 *  machines more: past it the drawing grows too large to read, or to lay
 *  out again at every letter typed. */
const OPEN_FOUND = 240;

/** The boxes folded, but those holding a machine looked for and found: they
 *  stand open while it is, as the user left them after. Where that would
 *  show more than `OPEN_FOUND` machines more, only the box of the one gone
 *  to opens. Says whether every one found then stands open. */
function unfolded(folded: Set<string>, nodes: readonly MapNode[], found: readonly string[] | null, current: string | null): { folded: Set<string>; whole: boolean } {
  if (!found?.length) return { folded, whole: true };
  const boxesOf = (node: MapNode) => {
    const group = node.group?.key ?? "mine";
    return node.hubs.flatMap((hub) => [`group:${hub}:${boxParts(group).owner}`, `group:${hub}:${group}`]);
  };
  const openTo = (keys: readonly string[]) => {
    const open = new Set(folded);
    const wanted = new Set(keys);
    for (const node of nodes) if (wanted.has(node.key)) for (const box of boxesOf(node)) open.delete(box);
    return open;
  };
  const shown = (shut: Set<string>) => nodes.filter((node) => !boxesOf(node).some((box) => shut.has(box))).length;
  const every = openTo(found);
  if (every.size === folded.size || shown(every) - shown(folded) <= OPEN_FOUND) return { folded: every, whole: true };
  return { folded: current && found.includes(current) ? openTo([current]) : folded, whole: false };
}

type GlyphName = Parameters<typeof glyph>[0];

/** What can be done to a hub or machine, as an icon beside it and a line
 *  in its menu. */
interface Act {
  act: string;
  label: string;
  icon: GlyphName;
  danger?: boolean;
  /** For a setting that is on or off, whether it is on. */
  on?: boolean;
}


/** How long a taking back can be undone from the drawing's corner. */
const UNDO_MS = 6_000;

/** How long a machine that just arrived stays circled: the drawing-in of
 *  the circle and a moment to see it. */
const ARRIVED_MS = 6_000;

/** Past this many machines others' groups start folded. */

/** The port offered first for direct connections: the one after the hub's
 *  own, 7443. */
const DIRECT_PORT = 7444;

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

/** The first release that sets a machine's choices one by one
 *  (`UpdateCall::Preferences`); earlier ones take the whole of them. */
const PREFERENCES_SINCE = "1.0.1-beta.8";

/** The first release whose node takes an update kept for it past a change
 *  of World program of its own stuck on the way; an earlier one online is
 *  also asked at once, which it takes regardless. */
const ARRANGED_PAST_STUCK_SINCE = "1.0.1-beta.10";

/** How often the update view asks while it is shown, and while the page
 *  is out of sight. */
const UPDATES_EVERY_MS = 2500;
const UPDATES_HIDDEN_MS = 15000;

/** How long what scheduling a machine said stays on its row. */
const SAID_MS = 15000;

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
  /** The update view as each login of this device was last told it, the
   *  machines in it as one, and which login's view each came from. */
  private updateViews = new Map<string, UpdatesView>();
  private updatesByName = new Map<string, MachineUpdate>();
  private updatesLogin = new Map<string, Login>();
  /** Each machine's name on its own hub, by how it is written here. */
  private updatesBare = new Map<string, string>();
  /** The hub answers no update view: it runs an older release. */
  private updatesOld = false;
  /** The page was left: nothing more is drawn. */
  private left = false;
  /** Why the update view could not be read, while none was; and which ask
   *  was the latest, so an earlier answer coming later is dropped. */
  private updatesFailed: string | null = null;
  private updatesAsked = 0;
  private updatesTimer = 0;
  /** What scheduling or setting a machine just said, by its name. */
  private readonly updatesSaid = new Map<string, string>();
  /** Whether the network's policy is unfolded on a phone. */
  private policyOpen = false;
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
  /** The list's groups and people the viewer opened or closed. */
  private readonly listFolds = keptFolds();
  /** Finding machines: the bar, its glass in the head, and what was last
   *  found for what was looked for. */
  private readonly finder: MachineFind;
  private readonly findToggle: HTMLElement;
  private found: { world: WorldNow; query: string; keys: string[]; order?: string[] } | null = null;
  /** Whether every machine found stands open on the drawing. */
  private foundWhole = true;
  /** The match gone to in the list. */
  private findCurrent: string | null = null;
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
  private readonly menuLayer: Layer = {
    isOpen: () => !!this.menuFor && this.element.isConnected,
    close: () => this.closeMenu(),
    holds: (target) => this.menu.contains(target),
  };
  private readonly cardLayer: Layer = {
    isOpen: () => !!this.cardFor && this.element.isConnected,
    close: () => this.closeCard(),
    holds: (target) => this.card.contains(target),
  };
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
  /** Groups the user folded or opened, by key; the rest follow the
   *  default. */
  private readonly folds = new Map<string, boolean>();
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
            of: (machine) => this.updatesByName.get(machine.written),
            schedule: (machines, version) => this.schedule(machines, version),
            repair: (machines, cancel) => this.arrangeRepair(machines, cancel),
            ways: (machines, auto, relays) => this.setWays(machines, auto, relays),
            state: updating.state,
            versions: updating.versions,
          }
        : undefined,
    });
    this.moving = new HubMoving({ ...options.moving, words, access: options.access, confirm: options.confirm, changed: () => this.redraw() });
    this.people = new MapPeople({
      words: options.people,
      store: options.store,
      confirm: options.confirm,
      change: options.change,
      card: (key, html, go) => {
        this.openCard(key, this.anchorOf(key) ?? [0, 0], html, go);
        return this.card;
      },
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
      <div class="machines-layout"><div class="machines-canvas"><div class="ml-list scroller" role="list" hidden></div><div class="map-undo" role="status" hidden></div><div class="map-choose" role="status" hidden></div><div class="map-acts" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><div class="map-menu" role="menu" aria-label="${escape(words.actions)}" hidden></div><form class="map-card" tabindex="-1" hidden></form></div><div class="ms-bar" role="toolbar" aria-label="${escape(words.actions)}" hidden></div><aside class="machine-panel scroller" aria-live="polite" hidden></aside></div>`;
    this.panel = this.element.querySelector<HTMLElement>(".machine-panel")!;
    this.bar = this.element.querySelector<HTMLElement>(".ms-bar")!;
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
    this.acts = this.element.querySelector<HTMLElement>(".map-acts")!;
    this.menu = this.element.querySelector<HTMLElement>(".map-menu")!;
    this.undoBar = this.element.querySelector<HTMLElement>(".map-undo")!;
    this.chooseBar = this.element.querySelector<HTMLElement>(".map-choose")!;
    this.card = this.element.querySelector<HTMLElement>(".map-card")!;
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
      if (this.updatesPress(target)) {
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
    // A ring pressed twice opens what is known of it: a machine's own page,
    // or the details of a hub or of a machine whose agent assists.
    this.map.element.addEventListener("dblclick", (event) => {
      const key = (event.target as HTMLElement).closest<HTMLElement>("[data-item]")?.dataset.item;
      if (!key || key.startsWith("add:")) return;
      if (key.startsWith("m:")) location.hash = link({ kind: "machine", name: key.slice("m:".length) });
      else this.details(key);
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
    this.panel.addEventListener("click", (event) => {
      const section = (event.target as HTMLElement).closest<HTMLElement>(".move-section");
      const hub = this.selected?.startsWith("hub:") ? this.selected.slice(4) : null;
      if (section && hub) void this.moving.click(event.target as HTMLElement, hub).catch((error: unknown) => console.warn("the move could not go on", error));
    });
    this.panel.addEventListener("click", (event) => this.clickAct(event));
    this.panel.addEventListener("pointerdown", (event) => this.pull(event));
    this.acts.addEventListener("click", (event) => this.clickAct(event));
    this.menu.addEventListener("click", (event) => this.clickAct(event));
    this.undoBar.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest(".map-undo-go")) void this.undo();
    });
    this.chooseBar.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      if (target.closest("[data-choose=new]")) this.moving.toNewServer(() => this.showMove());
      else if (target.closest("[data-choose=cancel]")) this.stopChoosing();
    });
    this.element.addEventListener("keydown", (event) => {
      if (event.key === "Escape" && this.menuFor) {
        event.stopPropagation();
        this.closeMenu();
      } else if (event.key === "Escape" && this.cardFor) {
        event.stopPropagation();
        this.closeCard();
      } else if (event.key === "Escape" && (this.moving.chosenHub || this.whyNot)) {
        event.stopPropagation();
        this.stopChoosing();
      } else if (event.key === "Escape" && this.framed.length) {
        event.stopPropagation();
        this.setPicked([]);
      }
    });
    this.card.addEventListener("click", (event) => {
      if ((event.target as HTMLElement).closest("[data-card=cancel]")) this.closeCard();
    });
    document.addEventListener("pointerdown", this.outside, true);
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
      clearTimeout(this.updatesTimer);
    };
    this.askUpdatesNow();
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
    document.removeEventListener("pointerdown", this.outside, true);
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
   *  down, drawn yet or not; the drawing's columns across and each down,
   *  or, where not all of them stand open on it, the list's order. */
  private foundShown(found: readonly string[]): string[] {
    if (this.view !== "map") return [...this.listRows.keys()];
    if (!this.foundWhole) {
      const kept = this.found;
      if (kept?.keys === found) return (kept.order ??= listOrder(kept.world, new Set(found), this.options.words));
    }
    const at = new Map(found.map((key) => [key, this.map.screenOf(key)] as const));
    return found.filter((key) => at.get(key)).sort((a, b) => at.get(a)![0] - at.get(b)![0] || at.get(a)![1] - at.get(b)![1]);
  }

  /** Goes to a machine looked for and found: on the drawing it is chosen
   *  and brought to the middle; in the list its row is washed and comes
   *  into view, what is ticked and what is open beside it left as they are. */
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
      const at = this.map.screenOf(key);
      if (!at) return;
      const box = this.canvas.getBoundingClientRect();
      this.map.glide(box.width / 2 - at[0], box.height / 2 - at[1]);
    });
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
      case "grow":
        if (Date.now() - this.pulled < 400) return;
        this.grown = !this.grown;
        this.redraw();
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
    this.askUpdatesNow();
    if (view !== "map" && !this.framed.length && this.selected && this.isPickable(this.selected) && !this.narrow()) {
      this.framed = [this.selected];
      this.selected = null;
    } else if (view === "map" && this.framed.length === 1) {
      this.selected = this.framed[0]!;
      this.framed = [];
    }
    this.detailsOpen = false;
    this.closeMenu();
    this.closeCard();
    this.redraw();
    if (view === "map") requestAnimationFrame(() => this.map.refresh());
  }

  /** Asks for the update view now and every few seconds while it is
   *  shown; stops asking once it is not. */
  private askUpdatesNow(): void {
    clearTimeout(this.updatesTimer);
    if (this.view !== "updates" || !this.options.updates) return;
    void this.askUpdates().finally(() => {
      if (this.view !== "updates" || !this.element.isConnected) return;
      this.updatesTimer = window.setTimeout(() => this.askUpdatesNow(), document.hidden ? UPDATES_HIDDEN_MS : UPDATES_EVERY_MS);
    });
  }

  /** Each login of this device asks its hub for its update view; the
   *  machines in them are one list, each from the view that lets the most
   *  be done to it. */
  private async askUpdates(): Promise<void> {
    const updates = this.options.updates;
    const world = this.options.store.world.peek();
    if (!updates || !world) return;
    const logins = loginsOf(world.accounts);
    const asked = ++this.updatesAsked;
    const answers = await Promise.allSettled(logins.map((login) => updates.ask(login.hubName, login.account, { do: "view" })));
    if (asked !== this.updatesAsked) return;
    const views = new Map<string, UpdatesView>();
    const byName = new Map<string, MachineUpdate>();
    const from = new Map<string, Login>();
    const bare = new Map<string, string>();
    // A machine is known by how it is written here: two hubs' machines of
    // one name stay two, and each is asked by its own.
    const written = new Map<string, string>();
    const keyOf = (hub: string, name: string) => `${hub}\n${name}`;
    for (const machine of machinesOf(world)) {
      for (const on of machine.on) if (!written.has(keyOf(on.hub, machine.name))) written.set(keyOf(on.hub, machine.name), machine.written);
    }
    const writtenOf = (login: Login, name: string) => written.get(keyOf(login.hub, name)) ?? name;
    let old = false;
    let failed: string | null = null;
    answers.forEach((answer, index) => {
      const login = logins[index]!;
      const key = `${login.hub}\n${login.account}`;
      let view: UpdatesView | undefined;
      if (answer.status === "rejected") {
        const why = detailOf(answer.reason);
        // Only a hub before the view says so; anything else passes, and
        // what was read last stands meanwhile.
        if (/does not support the updates view/.test(why)) old = true;
        else {
          failed = why;
          view = this.updateViews.get(key);
        }
      } else if (answer.value.kind === "view") view = answer.value.view;
      if (!view) return;
      views.set(key, view);
      for (const told of view.machines) {
        const written = writtenOf(login, told.machine);
        const machine = written === told.machine ? told : { ...told, machine: written };
        const kept = byName.get(written);
        const more = (one: MachineUpdate) => Number(one.actions.schedule.allowed) + Number(one.actions.settings.allowed) + Number(one.actions.update.allowed);
        if (!kept || more(machine) > more(kept)) {
          byName.set(written, machine);
          from.set(written, login);
          bare.set(written, told.machine);
        }
      }
    });
    this.updateViews = views;
    this.updatesByName = byName;
    this.updatesLogin = from;
    this.updatesBare = bare;
    this.updatesOld = !views.size && old;
    this.updatesFailed = views.size || old ? null : failed;
    // Drawn again as any change is: what is the same as it was is left as
    // it is, every few seconds while the view is open.
    if (!this.left) this.draw(this.options.store.world.peek(), this.options.store.clock.peek());
  }

  /** What the list draws in the update view: the machines that may be
   *  updated, the ruler and the network's policy over them. */
  private updatesNow(narrow: boolean): UpdatesNow | undefined {
    const updates = this.options.updates;
    if (!updates) return undefined;
    const words = updates.view;
    const views = [...this.updateViews.values()];
    const newest = views.flatMap((view) => [view.stable?.version, view.beta?.version].filter((version): version is string => !!version));
    const hub = views[0]?.hub ?? null;
    const top = [...newest].sort(compareVersions).pop() ?? hub;
    const line = lineOf(top);
    const policies = [...this.updateViews.entries()].filter(([, view]) => view.policy);
    const policy = policies[0]?.[1].policy ?? null;
    const machines = [...this.updatesByName.values()];
    let head = "";
    if (this.updatesOld) head = `<p class="uv-old">${escape(words.hubOld)}</p>`;
    else if (this.updatesFailed) head = `<p class="uv-old" title="${escape(this.updatesFailed)}">${escape(words.unread)}</p>`;
    // What the page itself was asked and could not do, said over it.
    if (this.note?.machine === "") head = `<p class="uv-old">${escape(this.note.text)}</p>${head}`;
    else if (machines.length) {
      const width = Math.max(0, this.list.clientWidth - (narrow ? 32 : 48));
      head = rulerOf({ machines, newest, hub, minimum: policy?.minimum ?? null, line, width: width || 900, phone: narrow }, words);
      if (policies.length) {
        // Each hub's own, its name first where there are several.
        const logins = loginsOf(this.options.store.world.peek()?.accounts);
        const rows = policies
          .map(([key, view]) => {
            const one = view.policy!;
            const minimum = one.minimum ?? null;
            const managed = one.managed_auto !== false;
            const hubName = policies.length > 1 ? (logins.find((login) => `${login.hub}\n${login.account}` === key)?.hubName ?? "") : "";
            return `<div class="uv-policy">${hubName ? `<span class="uv-policy-hub">${escape(hubName)}</span>` : ""}<span class="uv-policy-item"><span>${escape(words.minimum)}</span><button class="uv-chip" type="button" data-uv-minimum="${escape(key)}"><span class="ml-num">${escape(minimum ?? words.noMinimum)}</span>${glyph("chevron", 12)}</button></span><span class="uv-policy-item"><span>${escape(words.managedAuto)}</span><span class="uv-auto${managed ? " on" : ""}" role="switch" aria-checked="${managed}" aria-label="${escape(words.managedAuto)}" data-uv-managed="${escape(key)}"><span class="switch-track"><span class="switch-knob"></span></span></span></span></div>`;
          })
          .join("");
        // On a phone the policy is folded into one line until it is opened.
        head += narrow
          ? `<details class="uv-policy-fold"${this.policyOpen ? " open" : ""}><summary>${glyph("chevron", 14)}<span>${escape(words.policy)}</span></summary>${rows}</details>`
          : rows;
      }
    }
    return { byName: this.updatesByName, line, words, head, said: this.updatesSaid };
  }

  /** How many machines may take a newer release, beside the view's name. */
  private drawUpdateCount(): void {
    const count = this.views.querySelector<HTMLElement>(".ml-view-count");
    if (!count) return;
    const able = [...this.updatesByName.values()].filter((machine) => machine.offer && machine.actions.schedule.allowed).length;
    count.hidden = !able;
    count.textContent = able ? String(able) : "";
  }

  /** The update view's own presses in the list: a machine's switch, and the
   *  network's policy. Whether the press was one of them. */
  private updatesPress(target: HTMLElement): boolean {
    const updates = this.options.updates;
    if (!updates || this.view !== "updates") return false;
    const fold = target.closest<HTMLElement>(".uv-policy-fold > summary");
    if (fold) {
      this.policyOpen = !this.policyOpen;
      this.redraw();
      return true;
    }
    const auto = target.closest<HTMLElement>("[data-uv-auto]")?.dataset.uvAuto;
    if (auto) {
      const machine = this.updatesByName.get(auto);
      if (machine) void this.setWays([machine], !machine.auto, null);
      return true;
    }
    const minimumOf = target.closest<HTMLElement>("[data-uv-minimum]")?.dataset.uvMinimum;
    const managedOf = target.closest<HTMLElement>("[data-uv-managed]")?.dataset.uvManaged;
    const key = minimumOf ?? managedOf;
    const policy = key === undefined ? undefined : this.updateViews.get(key)?.policy;
    if (minimumOf !== undefined && policy) {
      updates.minimum(policy.minimum ?? null, (version) => void this.setPolicy(minimumOf, version, policy.managed_auto !== false));
      return true;
    }
    if (managedOf !== undefined && policy) {
      void this.setPolicy(managedOf, policy.minimum ?? null, policy.managed_auto === false);
      return true;
    }
    return false;
  }

  /** Schedules `version` for each of `machines` with the hub whose view
   *  let it, or takes back what was scheduled; what did not go is said on
   *  its row for a while. */
  private async schedule(machines: MachineUpdate[], version: string | null): Promise<void> {
    const updates = this.options.updates;
    if (!updates) return;
    const words = updates.view;
    const byLogin = new Map<string, { login: Login; names: string[] }>();
    for (const machine of machines) {
      const login = this.updatesLogin.get(machine.machine);
      if (!login) continue;
      const key = `${login.hub}\n${login.account}`;
      const entry = byLogin.get(key) ?? { login, names: [] };
      // The hub knows it by its name there.
      entry.names.push(this.updatesBare.get(machine.machine) ?? machine.machine);
      byLogin.set(key, entry);
    }
    for (const { login, names } of byLogin.values()) {
      // Back from the hub's name to how it is written here.
      const written = (name: string) =>
        [...this.updatesBare.entries()].find(([as, one]) => one === name && this.updatesLogin.get(as) === login)?.[0] ?? name;
      try {
        const reply = await updates.ask(login.hubName, login.account, { do: "schedule", machines: names, version });
        if (reply.kind !== "scheduled") continue;
        for (const result of reply.results) {
          if (result.done) this.updatesSaid.delete(written(result.machine));
          else this.sayOnRow(written(result.machine), words.notScheduled(result.reason ? words.reasons[result.reason] : (result.why ?? words.reasons.other)));
        }
      } catch (error) {
        for (const name of names) this.sayOnRow(written(name), words.notScheduled(error instanceof Error ? error.message : String(error)));
      }
    }
    if (version !== null) {
      const stuck = machines.filter((machine) => machine.online && machine.kernel && compareVersions(machine.kernel, ARRANGED_PAST_STUCK_SINCE) < 0);
      await inTurns(stuck, WAYS_AT_ONCE, async (machine) => {
        try {
          await updates.remote(machine.machine, { do: "kernel", version });
        } catch (error) {
          this.sayOnRow(machine.machine, words.notScheduled(error instanceof Error ? error.message : String(error)));
        }
      });
    }
    await this.askUpdates();
  }

  /** Has each machine's hub keep a repair for it, made at once where it is
   *  online and once it is back where not; or calls off the repairs kept
   *  (`dev-notes/2026-10-22-更新的修复-设计.md`, 别的设备替它). */
  private async arrangeRepair(machines: MachineUpdate[], cancel: boolean): Promise<void> {
    const updates = this.options.updates;
    if (!updates) return;
    const words = updates.view;
    const byLogin = new Map<string, { login: Login; names: string[] }>();
    for (const machine of machines) {
      const login = this.updatesLogin.get(machine.machine);
      if (!login) continue;
      const key = `${login.hub}\n${login.account}`;
      const entry = byLogin.get(key) ?? { login, names: [] };
      entry.names.push(this.updatesBare.get(machine.machine) ?? machine.machine);
      byLogin.set(key, entry);
    }
    for (const { login, names } of byLogin.values()) {
      const written = (name: string) =>
        [...this.updatesBare.entries()].find(([as, one]) => one === name && this.updatesLogin.get(as) === login)?.[0] ?? name;
      try {
        const reply = await updates.ask(login.hubName, login.account, cancel ? { do: "cancel_repair", machines: names } : { do: "repair", machines: names, version: null });
        if (reply.kind !== "scheduled") continue;
        for (const result of reply.results) {
          if (result.done) this.updatesSaid.delete(written(result.machine));
          else this.sayOnRow(written(result.machine), words.notScheduled(result.reason ? words.reasons[result.reason] : (result.why ?? words.reasons.other)));
        }
      } catch (error) {
        for (const name of names) this.sayOnRow(written(name), words.notScheduled(error instanceof Error ? error.message : String(error)));
      }
    }
    await this.askUpdates();
  }

  /** Sets how each of `machines` updates, as far as it says: a release
   *  that sets them one by one is told only what changes, an earlier one
   *  the whole of them. */
  private async setWays(machines: MachineUpdate[], auto: boolean | null, relays: boolean | null): Promise<void> {
    const updates = this.options.updates;
    if (!updates) return;
    // A room of machines is asked a few at a time, not all at once.
    await inTurns(machines, WAYS_AT_ONCE, async (machine) => {
      const call: UpdateCall =
        machine.kernel && compareVersions(machine.kernel, PREFERENCES_SINCE) >= 0
          ? { do: "preferences", auto, relays }
          : { do: "set", settings: { auto: auto ?? !!machine.auto, chosen: auto !== null || machine.auto_chosen, relays: relays ?? machine.relays ?? true } };
      try {
        await updates.remote(machine.machine, call);
        this.updatesSaid.delete(machine.machine);
      } catch (error) {
        this.sayOnRow(machine.machine, error instanceof Error ? error.message : String(error));
      }
    });
    await this.askUpdates();
  }

  /** A network's update policy, set by the login `key` of this device that
   *  is its super administrator. */
  private async setPolicy(key: string, minimum: string | null, managedAuto: boolean): Promise<void> {
    const updates = this.options.updates;
    if (!updates) return;
    const login = loginsOf(this.options.store.world.peek()?.accounts).find((one) => `${one.hub}\n${one.account}` === key);
    if (!login) return;
    try {
      await updates.ask(login.hubName, login.account, { do: "policy", minimum, managed_auto: managedAuto });
      if (this.note?.machine === "") this.note = null;
    } catch (error) {
      this.note = { machine: "", text: this.options.access.failed(detailOf(error)) };
    }
    await this.askUpdates();
  }

  /** Says `text` on a machine's row in the update view for a while. */
  private sayOnRow(name: string, text: string): void {
    this.updatesSaid.set(name, text);
    window.setTimeout(() => {
      if (this.updatesSaid.get(name) !== text) return;
      this.updatesSaid.delete(name);
      this.redraw();
    }, SAID_MS);
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

  /** The groups folded to their knot. Past two dozen machines the drawing
   *  fitted to the page is too small to read them all, so others' groups
   *  start folded and the account's own open; what the user opened or
   *  folded stays so. */
  private foldedOf(nodes: MapNode[]): Set<string> {
    return foldedOf(nodes, this.folds);
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
    this.closeCard();
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
      if (at) this.openMenu(key, [at[0] - 20, at[1] + 18], this.narrow());
      return;
    }
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

  private closeDetails(): void {
    // On a narrow page the sheet is the choice itself.
    if (this.narrow()) {
      this.selected = null;
      this.grown = false;
    } else this.detailsOpen = false;
    this.redraw();
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
    const open = !!spot && (narrow || this.detailsOpen) && !together;
    if (open && this.panel.hidden) layerOpened(this.detailsLayer);
    this.panel.hidden = !open;
    this.panel.classList.toggle("peek", narrow && !this.grown);
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
    this.drawUpdateCount();
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
          updates: updating ? this.updatesNow(narrow) : undefined,
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
    // Beside the list nothing floats: the actions are a row in the details.
    const acts = (narrow || listing) && this.selected ? this.sheetActs(this.selected, world) : "";
    const grip = narrow
      ? `<button class="sheet-grip" type="button" data-act="grow" aria-expanded="${this.grown}" aria-label="${escape(this.grown ? words.closeDetails : words.details)}"></button>`
      : "";
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
      : spot && open
        ? `${grip}<button class="icon-button panel-close" type="button" data-act="close-details" aria-label="${escape(words.closeDetails)}" title="${escape(words.closeDetails)}">${glyph("close", 16)}</button>${spot.kind === "hub" ? this.hubPanel(spot.hub, world, acts) : spot.kind === "assist" ? this.assistPanel(spot.assistants, world, now, acts) : spot.kind === "device" ? this.phonePanel(spot.member, spot.owner, spot.here, now, acts) : this.machinePanel(spot.machine, world, now, acts)}`
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
    this.float();
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
    const shown = unfolded(this.foldedOf(nodes), nodes, found, this.selected);
    this.foundWhole = shown.whole;
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
      folded: shown.folded,
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

  /** The actions of the one chosen as a row in its sheet, each with its
   *  name, large enough for a finger. */
  private sheetActs(key: string, world: WorldNow): string {
    // The sheet is a hub's details; a machine's are on its own page.
    const acts = this.actsOf(key, world).filter((act) => act.act !== "details" || key.startsWith("m:"));
    // Verbs share one row of equal columns; a setting is a row of its own
    // with its switch, and what cannot be taken back stands apart below.
    const verbs = acts.filter((act) => act.on === undefined && !act.danger);
    const settings = acts.filter((act) => act.on !== undefined);
    const dangers = acts.filter((act) => act.danger);
    const { words } = this.options;
    const row = verbs.length
      ? `<div class="sheet-acts" style="--acts:${verbs.length}">${verbs
          .map((act) => `<button class="sheet-act" type="button" data-map-act="${act.act}" data-key="${escape(key)}">${glyph(act.icon, 18)}<span>${escape(act.label)}</span></button>`)
          .join("")}</div>`
      : "";
    const switches = settings
      .map(
        (act) =>
          `<button class="sheet-row sheet-switch" type="button" role="switch" aria-checked="${act.on}" data-map-act="${act.act}" data-key="${escape(key)}">${glyph(act.icon, 18)}<span class="sheet-row-label">${escape(act.act === "direct" ? words.allowDirect : act.act === "signup" ? this.options.people.signUp : act.label)}</span><span class="switch-track" aria-hidden="true"><span class="switch-knob"></span></span></button>`,
      )
      .join("");
    const removals = dangers
      .map((act) => `<button class="sheet-row danger" type="button" data-map-act="${act.act}" data-key="${escape(key)}">${glyph(act.icon, 18)}<span class="sheet-row-label">${escape(act.label)}</span></button>`)
      .join("");
    return row + (switches || removals ? `<div class="sheet-rows">${switches}${removals}</div>` : "");
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

  /** Does an action chosen beside a spot or in its menu. */
  private clickAct(event: Event): void {
    const button = (event.target as HTMLElement).closest<HTMLButtonElement>("[data-map-act]");
    const world = this.options.store.world.peek();
    if (!button || !world) return;
    const key = button.dataset.key ?? "";
    const act = button.dataset.mapAct!;
    this.closeMenu();
    if (act === "fit") return this.map.fit();
    if (act === "pick-group") return this.setPicked(added(this.pickedNow(), pickableIn(mapOf(world, this.options.words).nodes, key)));
    const person = personOf(key);
    if (person) return this.people.run(act, person.hub, person.account, key);
    if (groupMoreOf(key)) {
      if (act === "rename-group") this.renameGroupCard(key);
      else if (act === "end-group") void this.endGroup(key);
      return;
    }
    const spot = this.spots(world).get(key);
    if (!spot) return;
    if (act === "details") {
      if (spot.kind !== "machine") return this.details(key);
      location.hash = link({ kind: "machine", name: spot.machine.written });
      return;
    }
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
        this.renameCard(key, machine);
        return;
      case "group": {
        const grouping = groupingOf(machine, world);
        if (grouping) this.groupCard(key, [machine], [grouping]);
        return;
      }
      case "direct": {
        const admin = this.adminOf(machine, world);
        const member = admin ? this.memberOf(machine, admin, world) : undefined;
        if (!admin || !member) return;
        if (member.direct?.length) this.stopDirect(admin, machine, key, member.direct);
        else this.allowDirect(admin, machine, key, member);
        return;
      }
      case "relay": {
        const own = this.holdersOf(machine, world)[0];
        const member = own ? this.memberOf(machine, own, world) : undefined;
        if (!own || !member) return;
        if (member.relay?.length) void this.stopRelay(own, machine, member.relay);
        else this.allowRelay(own, machine, key, member);
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
    this.groupCard(`m:${machines[0]!.written}`, machines, groupings as Grouping[]);
  }

  /** The card that puts machines in a group, beside the ring of the one, or
   *  the first of those framed together: one of their owner's groups, a new
   *  one, or none. Put there, each is circled in its new box, and for a
   *  moment the change can be undone from the drawing's corner. */
  private groupCard(key: string, machines: Machine[], groupings: Grouping[]): void {
    const words = this.options.words.groups;
    const first = groupings[0];
    if (!first) return;
    const now = groupings.every((one) => one.now === first.now) ? first.now : undefined;
    const title = machines.length === 1 ? words.groupTitle(machines[0]!.written) : words.groupMany(machines.length);
    this.openCard(key, this.anchorOf(key) ?? [0, 0], groupForm(words, title, first.groups, now), async () => {
      const chosen = chosenGroup(this.card, words);
      if ("wrong" in chosen) throw this.cardWrong(chosen.wrong);
      const moving = machines.filter((_, at) => groupings[at]!.now !== chosen.group);
      if (!moving.length) return;
      const before = new Map(machines.map((machine, at) => [machine.name, groupings[at]!.now]));
      for (const machine of moving) await this.options.change(first.login, groupChange(machine.name, chosen.group));
      this.circle(moving);
      this.offerUndo(machines.length === 1 ? words.movedOne(machines[0]!.written, chosen.group) : words.movedMany(moving.length, chosen.group), async () => {
        for (const machine of moving) await this.options.change(first.login, groupChange(machine.name, before.get(machine.name) ?? null));
      });
    });
    wireGroupForm(this.card);
  }

  /** The card that renames a group: its machines are in the group of the
   *  new name, with any already in one of that name. */
  private renameGroupCard(key: string): void {
    const world = this.options.store.world.peek();
    const box = world ? this.groupBox(key, world) : null;
    if (!box) return;
    const words = this.options.words.groups;
    this.openCard(key, this.anchorOf(key) ?? [0, 0], groupRenameForm(words, box.group), async () => {
      const named = groupNameOf(this.card.querySelector<HTMLInputElement>("input[name=name]")?.value ?? "", words);
      if ("wrong" in named) throw this.cardWrong(named.wrong);
      if (named.name === box.group) return;
      await this.renameGroups(box, named.name);
      this.circle(box.machines);
      this.offerUndo(words.renamedGroup(box.group, named.name), () => this.regroup(box));
    });
    this.card.querySelector<HTMLInputElement>("input[name=name]")?.select();
  }

  /** Ends a group at once: its machines stand in their owner's box again,
   *  and for a moment the group can be given back from the drawing's
   *  corner. Nothing about the machines changes, so nothing asks first. */
  private async endGroup(key: string): Promise<void> {
    const world = this.options.store.world.peek();
    const box = world ? this.groupBox(key, world) : null;
    if (!box) return;
    try {
      await this.renameGroups(box, null);
      this.note = null;
    } catch (error) {
      this.note = { machine: "", text: this.options.access.failed(error instanceof Error ? error.message : String(error)) };
      this.redraw();
      return;
    }
    this.circle(box.machines);
    this.offerUndo(this.options.words.groups.endedGroup(box.group), () => this.regroup(box));
    await this.options.store.refresh();
    this.redraw();
  }

  /** Renames a box's group for each owner it holds machines of, or ends it
   *  where `to` is none. */
  private async renameGroups(box: GroupBox, to: string | null): Promise<void> {
    const done: Grouping[] = [];
    for (const grouping of box.groupings) {
      if (done.some((one) => sameLogin(one.login, grouping.login) && one.owner === grouping.owner)) continue;
      done.push(grouping);
      await this.options.change(grouping.login, { change: "rename_group", owner: grouping.owner, from: box.group, to });
    }
  }

  /** Puts a box's machines back in its group, as they were before. */
  private async regroup(box: GroupBox): Promise<void> {
    for (const [at, machine] of box.machines.entries()) await this.options.change(box.groupings[at]!.login, groupChange(machine.name, box.group));
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
    for (const machine of machines) {
      const key = `m:${machine.written}`;
      this.fresh.add(key);
      setTimeout(() => {
        this.fresh.delete(key);
        this.redraw();
      }, ARRIVED_MS);
    }
  }

  /** Says in the open card why it does not go, and keeps it open. */
  private cardWrong(said: string): Error {
    const wrong = this.card.querySelector<HTMLElement>(".map-card-wrong");
    if (wrong) {
      wrong.textContent = said;
      wrong.hidden = false;
    }
    return new Error("kept open");
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

  /** A card beside the ring with its name to change. Renamed, it is
   *  circled under its new name, and for a moment the old name can be given
   *  back from the drawing's corner: a name a machine had stays its own. */
  private renameCard(key: string, machine: Machine): void {
    const world = this.options.store.world.peek();
    const rename = world ? renamerOf(machine, world, this.options.change, this.options.renaming.here) : null;
    if (!rename) return;
    const { words, nameSaid, check } = this.options.renaming;
    const from = machine.name;
    this.openCard(key, this.anchorOf(key) ?? [0, 0], `<p class="map-card-title">${escape(words.renameTitle(machine.written))}</p><div class="map-card-rename"></div>`, async () => {});
    const field = new RenameField({
      words,
      nameSaid,
      name: from,
      note: !machine.here && machine.online === false ? words.renameOffline(machine.written) : undefined,
      check,
      rename: async (to) => {
        await rename(to);
        await this.options.store.refresh();
      },
      done: (to) => {
        field.dispose();
        this.closeCard();
        const renamed = `m:${to}`;
        this.selected = renamed;
        this.fresh.add(renamed);
        setTimeout(() => {
          this.fresh.delete(renamed);
          this.redraw();
        }, ARRIVED_MS);
        this.offerUndo(words.renamed(from, to), () => rename(from, to));
        this.redraw();
      },
      cancel: () => {
        field.dispose();
        this.closeCard();
      },
    });
    this.card.querySelector(".map-card-rename")!.append(field.element);
    field.focus();
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
    const at = this.anchorOf(key) ?? [0, 0];
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

  /** The card that lets one's own computer relay for one's other machines
   *  on its network: its network cards' addresses to choose from, and the
   *  port. Nothing checks it from outside: only its own network reaches it. */
  private allowRelay(login: Login, machine: Machine, key: string, member: MemberSeen): void {
    const { words } = this.options;
    // An IPv6 link-local address is named by each machine's own card, so no
    // other machine reaches the relay at it.
    const cards = (member.reach?.interfaces ?? []).map(hostOf).filter((host) => !/^fe[89ab][0-9a-f]:/i.test(host));
    const at = this.anchorOf(key) ?? [0, 0];
    this.openCard(
      key,
      at,
      `<p class="map-card-title">${escape(words.relayTitle(machine.written))}</p>
       <fieldset class="map-card-field"><legend>${escape(words.relayAddresses)}</legend><div class="choice-options">${cards
         .map((host, index) => `<label class="choice map-card-choice"><input type="checkbox" name="host" value="${escape(host)}"${index === 0 ? " checked" : ""}><span class="mono">${escape(host)}</span><span class="choice-note">${escape(words.directCard)}</span></label>`)
         .join("")}</div></fieldset>
       <label class="field map-card-port"><span class="field-label">${escape(words.directPort)}</span><input class="mono" name="port" inputmode="numeric" value="${DIRECT_PORT}" autocomplete="off" spellcheck="false" required></label>
       <p class="map-card-wrong" role="alert" hidden></p>
       <details class="map-card-more"><summary>${escape(words.more)}</summary><p>${escape(words.relayWhat)}</p></details>
       <footer class="map-card-acts"><button class="text-button" type="button" data-card="cancel">${escape(words.cancel)}</button><button class="text-button map-card-go" type="submit">${escape(words.relayGo)}</button></footer>`,
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
        try {
          await this.options.change(login, { change: "relay", machine: machine.name, at: hosts.map((host) => (host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`)) });
        } catch (error) {
          throw refuse(error instanceof Error ? error.message : String(error));
        }
      },
    );
  }

  /** Stops one's own computer relaying at once; for a moment it can be
   *  turned on again where it was. */
  private async stopRelay(login: Login, machine: Machine, before: string[]): Promise<void> {
    const { words } = this.options;
    await this.options.change(login, { change: "relay", machine: machine.name, at: [] });
    this.offerUndo(words.relayStopped(machine.written), () => this.options.change(login, { change: "relay", machine: machine.name, at: before }));
  }

  /** Takes direct connections to a machine away at once, after a small
   *  card of before and after; for a moment they can be given back. */
  private stopDirect(login: Login, machine: Machine, key: string, before: string[]): void {
    const { words } = this.options;
    const at = this.anchorOf(key) ?? [0, 0];
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

  /** A card beside a ring, or rising from below on a narrow page; `go`
   *  runs when it is confirmed, and an error it throws stays in the card. */
  private openCard(key: string, at: Point, html: string, go: () => Promise<void | "keep">): void {
    this.closeMenu();
    this.cardFor = key;
    this.float();
    this.card.innerHTML = html;
    const sheet = this.narrow();
    this.card.classList.toggle("as-sheet", sheet);
    this.card.hidden = false;
    layerOpened(this.cardLayer);
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
        (kept) => {
          if (kept === "keep") {
            if (button) button.disabled = false;
            return;
          }
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
    const key = this.menuFor || this.cardFor || this.narrow() || this.view !== "map" ? null : this.selected;
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
          `<button class="map-act${act.danger ? " danger" : ""}${act.on ? " on" : ""}" type="button" data-map-act="${act.act}" data-key="${escape(key)}" aria-label="${escape(act.label)}" title="${escape(act.label)}"${act.on === undefined ? "" : ` aria-pressed="${act.on}"`}>${glyph(act.icon, 18)}</button>`,
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
    this.menuFor = { key, at, sheet };
    this.menu.innerHTML = (alone !== null && sheet ? `<p class="map-menu-head">${escape(alone)}</p>` : "") + acts
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

  /** The head of the panel: a name, grey while it is away, its tags, and
   *  whether it is online in words. */
  private panelHead(name: string, tags: string[], state: { kind: string; said: string }): string {
    return `<header class="panel-head ${state.kind}"><h2 class="panel-title">${escape(name)}</h2>${tags.join("")}<span class="panel-state">${escape(state.said)}</span></header>`;
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

  /** What is known of a machine the account may work on. */
  private machinePanel(machine: Machine, world: WorldNow, now: Date, acts = ""): string {
    const { words, access } = this.options;
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
      ? [osName(facts.os), facts.arch, `fastctx ${facts.version}`, route].filter(Boolean).map(escape).join(`<span class="sep"></span>`)
      : "Err" in machine.facts && machine.facts.Err && machine.online !== false
        ? `<span title="${escape(machine.facts.Err)}">${escape(words.factsUnknown(machine.facts.Err))}</span>`
        : escape(route);
    // This computer, hub by hub: how it reaches each, and whose it is there.
    const hubs = machine.here
      ? world.nodes.hubs
          .map((hub) => {
            const stand = hub.home ? words.registeredUnder(hub.home) : hub.logins.length ? words.onlyLogins(hub.logins.join(words.separator)) : "";
            return `<li class="panel-hub ${hub.link.kind}"><span class="panel-hub-name">${escape(hub.name)}</span><span${linkWhy(hub.link) ? ` title="${escape(linkWhy(hub.link))}"` : ""}>${escape([linkSaid(hub.link, words), stand].filter(Boolean).join(" "))}</span></li>`;
          })
          .join("")
      : "";
    // Its owner's own: whom it is shared with, and the rest it sets.
    const owners = machine.here ? logins.filter((login) => world.nodes.hubs.some((hub) => hub.hub === login.hub && hub.home === login.account)) : owner ? [owner] : [];
    const sharing = owners
      .map(
        (login) =>
          `<div class="panel-give" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}">${this.givenRow(
            owners.length > 1 ? `${words.sharedTo} ${login.hubName}` : words.sharedTo,
            this.sharedWith(login, machine.name),
            (to) => `data-act="unshare" data-to="${escape(to)}"`,
            this.button("share", words.share, "", glyph("plus", 13)),
          )}</div>`,
      )
      .join("");
    const own = owners.length
      ? `<div class="panel-acts" data-hub="${escape(owners[0]!.hub)}" data-account="${escape(owners[0]!.account)}">${machine.here ? "" : this.button("remove", words.remove, " data-danger", glyph("close", 14))}</div>`
      : "";
    // Whom an agent on it assists, ended here as on the agents page.
    const assisting = owners
      .map((login) => {
        const assists = (this.hubOf(login)?.assists ?? []).filter((assist) => assist.machine === machine.name);
        if (!assists.length) return "";
        const chips = assists
          .map((assist) => {
            const label = `${agentName(assist.agent)} ${assist.account}`;
            const at = `data-to="${escape(assist.account)}" data-agent="${escape(assist.agent)}"${assist.home ? ` data-home="${escape(assist.home)}"` : ""}${assist.until ? ` data-until="${escape(assist.until)}"` : ""}${assist.daily ? ` data-daily="${assist.daily}"` : ""}${assist.weekly ? ` data-weekly="${assist.weekly}"` : ""}${assist.five_hour ? ` data-five-hour="${assist.five_hour}"` : ""}`;
            const until = assist.until ? new Date(assist.until) : null;
            const said = until ? access.moment.until(until, now) : access.when.none;
            const day = until ? `<span class="assist-tag-until">${glyph("history", 12)}${escape(access.moment.short(until, now))}</span>` : "";
            return `<span class="chip assist-chip"><button class="assist-tag-open" type="button" data-act="assist-edit" ${at} title="${escape(`${access.changeUntil}\n${said}`)}">${escape(label)}${day}</button><button class="chip-x" type="button" data-act="assist-end" ${at} aria-label="${escape(words.endAssist(assist.account))}" title="${escape(words.endAssist(assist.account))}">${glyph("close", 12)}</button></span>`;
          })
          .join("");
        return `<div class="panel-give" data-hub="${escape(login.hub)}" data-account="${escape(login.account)}"><div class="given-row"><span class="given-label">${escape(words.assisting)}</span>${chips}</div></div>`;
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
      ${assisting}
      ${this.present(machine.written, world, viewer)}
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
    return `<div class="panel-direct"><span class="given-label">${escape(words.directRow)}</span><span class="panel-direct-list">${said.join(`<span class="sep"></span>`)}</span>${at.length ? `<p class="panel-direct-at">${escape(words.takesDirectAt(at.join("、")))}</p>` : ""}</div>`;
  }

  /** What is known of a machine of another's whose agent assists: whose
   *  it is, that it is no machine of the account's to work on, and the
   *  assisting logins, each a way into a conversation. */
  private assistPanel(assistants: Assistant[], world: WorldNow, now: Date, acts = ""): string {
    const { words } = this.options;
    const first = assistants[0]!;
    const reaching = machineActsFor(world, first.machine, first.login);
    const tags = [this.tag(words.assistShort(first.owner ?? ""), "tag-assist", words.assistantSaid(first.owner ?? "")), reaching ? this.tag(words.reaching, "tag-reach", words.reachingWhat) : ""];
    const state = this.stateOf(first.online, first.since, false, now);
    return `<article class="panel-machine" data-machine="${escape(first.machine)}">
      ${this.panelHead(words.assistAgents(first.owner ?? ""), tags, state)}
      ${acts}
      <p class="panel-said" title="${escape(words.assistantSaid(first.owner ?? ""))}">${glyph("colleague", 14)}<span>${escape(words.assistantNote)}</span></p>
      ${assistants
        .filter((one) => one.until)
        .map((one) => `<p class="panel-said">${glyph("history", 14)}<span>${escape(`${agentName(one.agent)} ${this.options.access.moment.until(new Date(one.until!), now)}`)}</span></p>`)
        .join("")}
      ${this.present(first.machine, world, first.login, false)}
      ${this.agentsLink(first.written)}
    </article>`;
  }

  /** A phone signed in to the account, or to `owner`'s for the network's
   *  administrator: whether it is there, and since when it is signed in. */
  private phonePanel(member: MemberSeen, owner: string | null, here: boolean, now: Date, acts = ""): string {
    const { words } = this.options;
    const state = this.stateOf(member.online, member.since ?? null, here, now);
    const tags = [this.tag(words.phone, "", words.phoneWhat), owner ? this.tag(words.ownedBy(owner)) : "", here ? this.tag(words.here) : ""];
    return `<article class="panel-machine" data-device="${escape(member.name)}">
      ${this.panelHead(member.name, tags, state)}
      ${acts}
      <p class="panel-said">${glyph("phone", 14)}<span>${escape(words.phoneSince(this.options.when(new Date(member.added), now)))}</span></p>
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
          <span class="panel-login-acts">${this.button("invite", words.inviteDevice, "", glyph("plus", 13))}</span>
        </li>`;
      })
      .join("");
    const machines = hub.members.filter((member) => member.kind === "computer" && member.home).length;
    return `<article class="panel-hub-card">
      <header class="panel-head ${state.kind}"><span class="knot" aria-hidden="true"></span><h2 class="panel-title">${escape(words.hubTitle(hub.name))}</h2><span class="panel-state">${escape(state.said)}</span></header>
      <p class="panel-what">${escape(words.hubIs)}</p>
      ${acts}
      <p class="panel-facts">${escape(words.hubMachines(machines))}</p>
      <ul class="panel-logins">${rows}</ul>
      ${this.superLogin(hub, world) ? this.moving.section(hub.hub, world.nodes.machine ?? null) : ""}
    </article>`;
  }

  /** Who is at work on a machine and what runs there, as `login` sees it;
   *  nothing when no one is. */
  private present(machine: string, world: WorldNow, login: Login | null, counted = true): string {
    const { words } = this.options;
    const group = login ? (groupsOf(world).find((one) => sameLogin(one.login, login)) ?? null) : null;
    const residents = residentsOf(world).filter(
      (r) => r.about.machine === machine && (r.now?.activity === "busy" || r.now?.waiting_for_user) && (!group || residentIn(world, group, r)),
    );
    // Where agents assist the user, what runs there is its owner's, not counted.
    const jobs = counted ? jobsOf(world).filter((job) => job.machine === machine) : [];
    const agents = residents.length
      ? `<ul class="machine-agents">${residents
          .map(
            (r) =>
              `<li><a href="${talkLink({ resident: r.about.id, machine: r.about.machine })}"><span class="mark ${r.now?.waiting_for_user ? "mark-ask" : r.now?.activity === "busy" ? "mark-busy" : "mark-idle"}" aria-hidden="true"></span>${escape([agentName(r.about.host.agent), titleOf(r)].filter(Boolean).join(" "))}${r.town ? `<span class="tag">${escape(r.town)}</span>` : ""}</a></li>`,
          )
          .join("")}</ul>`
      : "";
    return `${agents}${jobs.length ? `<p class="machine-jobs">${escape(words.jobs(jobs.length))}</p>` : ""}`;
  }

}

/** The person a key on the drawing stands for: their "⋯". */
function personOf(key: string): { hub: string; account: string } | null {
  const found = key.match(/^who:([^:]+):(.+)$/);
  return found ? { hub: found[1]!, account: found[2]! } : null;
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
      return link.dissolved ? words.linkDissolved : words.linkStopped(link.reason);
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

/** How many machines a change of their ways is sent to at once. */
const WAYS_AT_ONCE = 6;

/** Runs `each` over `items`, at most `width` at a time. */
async function inTurns<T>(items: T[], width: number, each: (item: T) => Promise<void>): Promise<void> {
  let next = 0;
  const lane = async (): Promise<void> => {
    while (next < items.length) await each(items[next++]!);
  };
  await Promise.all(Array.from({ length: Math.min(width, items.length) }, lane));
}
