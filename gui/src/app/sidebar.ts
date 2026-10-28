// The sidebar: the places of the World, always at hand, so who is working
// can be seen from any page (`world-gui.md`, 界面的结构). At its top, the
// network shown, by its server's name (this computer's own name while it is
// in none); pressed, every network this device is in, the way to make a new
// one or join one, and each one's management. Then the agents, the
// machines and the features, each a line with its mark. With the workbench on,
// the front page and a new conversation come first, and below are each
// project, its name a way to its overview and its folder opening and closing
// on its conversations (the rest from its "⋯"), and the conversations in
// no project, most recent first; a conversation is listed as long as it is not archived,
// whether its agent's process runs or not, and the archived ones are folded
// away at the end. Each conversation's "⋯" (a long press on a touch screen)
// moves, renames, archives or deletes it (`talk-menu.ts`). Logged in to more
// than one account of the network, the projects and conversations come
// account by account, each only what that account may use. The account and
// the settings are at its foot.

import { glyph } from "../conversation/glyphs";
import { sectionOf, sectionRows } from "./agent-sections";
import type { ProfileWords } from "./profile-words";
import { ActsMenu, type MenuAct, type MenuEntry } from "./acts-menu";
import { Popover } from "../conversation/popover";
import { escape } from "../conversation/prose";
import { deviceFeatures } from "./device-features";
import { networksOf, shownNetwork } from "./network";
import { redrawKeepingFocus } from "./focus";
import { loginsOf, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { MachineFacts, ResidentNow } from "../world/world";
import type { Place } from "./places";
import { link } from "./places";
import { assistantsOf, assistingOwner, groupsOf, isComputer, loginLabel, machinesOf, residentIn } from "./reach";
import { effect, type Signal } from "./signals";
import type { Store } from "./store";
import type { TalkActs } from "./talk-acts";
import type { TalkMenu } from "./talk-menu";
import type { TalkMenuWords } from "./talk-menu-words";
import { isUnreached } from "./store";
import { TalkList } from "./talk-list";
import { factsByMachine, talkRow, talksOf } from "./talk-row";
import type { ShellWords } from "./words";
import type { Offered } from "./updates";
import type { UpdateWords } from "./update-words";
import { onHeld } from "./press";

export interface SidebarOptions {
  store: Store;
  words: ShellWords;
  /** The agents' sections, named under 「智能体」 while one is open. */
  sections: ProfileWords;
  /** The place shown now. */
  place(): Place;
  newTalk(): void;
  /** A new project, in the account whose part of the sidebar it was
   *  asked from. */
  newProject(login: Login | null): void;
  fold(): void;
  acts: TalkActs;
  menu: TalkMenu;
  menuWords: TalkMenuWords;
  /** A new version of the application, offered above the foot. */
  updates?: { offered: Signal<Offered | null>; open(): void; words: UpdateWords } | undefined;
  /** What is done to a project from its "⋯": a new conversation in it, its
   *  record; then sharing it and handing it on, where the reader may. */
  projects?: ProjectMenuOptions | undefined;
}

export interface ProjectMenuOptions {
  newTalk(town: string): void;
  edit(town: string): void;
  /** Sharing and handing on, as the project's own page offers them. */
  acts(town: string): MenuAct[];
  run(act: string, town: string, name: string): void;
}

/** An answer usually comes within a second and is asked for every two;
 *  after this many seconds without one, the sidebar says so. */
const SLOW = 5;

/** How many conversations a project, or the conversations in none, show
 *  before the rest are behind "all": enough for a day's work, few enough
 *  that the projects below stay in sight. */
const SHOWN = 6;

/** How long a project's folder takes to open or close: no redraw cuts it
 *  short meanwhile. A little over the motion's own `--fold`. */
const FOLDING_MS = 260;

/** Projects, and accounts' parts, folded shut by the reader, kept on this
 *  device. */
const folded = new Set<string>(
  (() => {
    try {
      return JSON.parse(localStorage.getItem("folded") ?? "[]") as string[];
    } catch {
      return [];
    }
  })(),
);

function keepFolded(): void {
  try {
    localStorage.setItem("folded", JSON.stringify([...folded]));
  } catch {
    // A device that keeps nothing opens every project again next time.
  }
}

export class Sidebar {
  readonly element: HTMLElement;
  private readonly list: HTMLElement;
  private readonly foot: HTMLElement;
  private readonly switcher: HTMLButtonElement;
  private readonly networks: HTMLElement;
  private readonly popover: Popover;
  private drawn = "";
  /** The conversations' menus, presses and renaming. */
  private readonly talkList: TalkList;
  /** A project's "⋯". */
  private readonly projectMenu = new ActsMenu();
  /** Until when a folder opens or closes: drawing waits for it. */
  private folding = 0;
  private foldingTimer = 0;
  /** Until when the agents' sections slide in, once they come; null while
   *  they are not shown. */
  private sectionsSince: number | null = null;

  constructor(private readonly options: SidebarOptions) {
    const { words } = options;
    this.element = document.createElement("nav");
    this.element.className = "sidebar";
    this.element.setAttribute("aria-label", words.app);
    this.element.innerHTML = `
      <div class="sidebar-head">
        <button class="network-switch" type="button" aria-haspopup="menu">${mark()}<span class="network-name"></span>${glyph("chevron", 14)}</button>
        <button class="icon-button sidebar-fold" type="button" aria-label="${escape(words.foldSidebar)}" title="${escape(words.foldSidebar)} Ctrl+B">${glyph("sidebar", 18)}</button>
        <div class="network-pop" role="menu" aria-label="${escape(words.networks)}" hidden></div>
      </div>
      <button class="new-talk" type="button">${glyph("plus", 18)}<span>${escape(words.newTalk)}</span><kbd>Ctrl N</kbd></button>
      <div class="sidebar-places"></div>
      <div class="sidebar-update" hidden></div>
      <div class="sidebar-foot"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".sidebar-places")!;
    this.foot = this.element.querySelector<HTMLElement>(".sidebar-foot")!;
    this.switcher = this.element.querySelector<HTMLButtonElement>(".network-switch")!;
    this.networks = this.element.querySelector<HTMLElement>(".network-pop")!;
    this.popover = new Popover(this.networks, this.switcher);
    this.switcher.addEventListener("click", () => {
      if (!this.popover.open) this.drawNetworks();
      this.popover.toggle();
    });
    this.networks.addEventListener("click", (event) => {
      const target = event.target as HTMLElement;
      const pick = target.closest<HTMLElement>("[data-network]");
      if (pick) {
        options.store.showNetwork(pick.dataset.network!);
        this.popover.hide();
        this.drawn = "";
        this.draw();
        return;
      }
      // A link in it goes where it says, and the list folds away.
      if (target.closest("a")) this.popover.hide();
    });
    this.element.querySelector(".new-talk")!.addEventListener("click", () => options.newTalk());
    const updates = options.updates;
    if (updates) {
      const bar = this.element.querySelector<HTMLElement>(".sidebar-update")!;
      bar.addEventListener("click", (event) => {
        if ((event.target as HTMLElement).closest("button")) updates.open();
      });
      effect(() => drawUpdate(bar, updates.offered.get(), updates.words));
    }
    this.element.querySelector(".sidebar-fold")!.addEventListener("click", () => options.fold());
    this.talkList = new TalkList(this.list, {
      acts: options.acts,
      menu: options.menu,
      menuWords: options.menuWords,
      reachable: (r) => !isUnreached(r),
      redraw: () => {
        this.drawn = "";
        this.draw();
      },
    });
    this.list.addEventListener("click", (event) => {
      const add = (event.target as HTMLElement).closest<HTMLElement>("[data-new-project]");
      if (add) {
        const group = add.dataset.newProject;
        options.newProject(loginsOf(options.store.world.peek()?.accounts).find((login) => `${login.hub}/${login.account}` === group) ?? null);
        return;
      }
      const more = (event.target as HTMLElement).closest<HTMLElement>("[data-project-more]");
      if (more) {
        this.openProjectMenu(more);
        return;
      }
      const toggle = (event.target as HTMLElement).closest<HTMLElement>("[data-fold]");
      if (!toggle) return;
      event.preventDefault();
      const key = toggle.dataset.fold!;
      if (folded.has(key)) folded.delete(key);
      else folded.add(key);
      keepFolded();
      // A project's folder opens and closes where it is, its conversations
      // sliding in and out under it; the rest of the list is drawn again.
      const place = toggle.closest<HTMLElement>(".project-place");
      if (place && this.foldInPlace(place, !folded.has(key))) return;
      this.drawn = "";
      this.draw();
    });
    // A project held on a touch screen, or pressed with the right button,
    // opens its "⋯" too.
    onHeld(this.list, ".project-row", (row) => {
      const more = row.querySelector<HTMLElement>("[data-project-more]");
      if (!more) return false;
      this.openProjectMenu(more);
    });
    effect(() => {
      options.store.world.get();
      options.acts.changed.get();
      deviceFeatures.conversations.get();
      options.store.trouble.get();
      options.store.clock.get();
      options.store.unanswered.get();
      this.draw();
    });
  }

  /** Draws again, as when the place shown changed. */
  draw(): void {
    // A name being typed is kept until it is done.
    if (this.talkList.held) return;
    // A folder opening or closing is not cut short; the list is drawn as
    // it stands once it has.
    const wait = this.folding - performance.now();
    if (wait > 0) {
      clearTimeout(this.foldingTimer);
      this.foldingTimer = window.setTimeout(() => this.draw(), wait);
      return;
    }
    const html = this.compose();
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }

  /** The list as things now stand, and everything around it brought up to
   *  date: the switcher, the new conversation's line, the foot. */
  private compose(): string {
    const { store, words } = this.options;
    const world = store.world.peek();
    const place = this.options.place();
    const now = store.clock.peek();
    // The new conversation outside any project is a place of its own.
    const talking = deviceFeatures.conversations.peek();
    this.drawSwitcher();
    const begin = this.element.querySelector<HTMLElement>(".new-talk");
    if (begin) begin.hidden = !talking;
    const beginning = place.kind === "new" && place.town === null;
    begin?.classList.toggle("current", beginning);
    if (beginning) begin?.setAttribute("aria-current", "page");
    else begin?.removeAttribute("aria-current");
    const { listed: talks, archived } = talksOf(world, this.options.acts);
    this.talkList.drawing([...talks, ...archived]);
    const facts = factsByMachine(world);
    const waiting = talks.filter((r) => r.now?.waiting_for_user).length;

    const machines = !!world && machinesOf(world).length > 0;
    // Agents of others assisting the user show on the agents page too.
    const assisted = assistantsOf(world).length > 0;
    const loggedIn = loginsOf(world?.accounts).length > 0;
    const at = (kinds: Place["kind"][]) => (kinds.includes(place.kind) ? " current" : "");
    const top = (href: string, current: string, mark: string, name: string, extra = "") =>
      `<a class="place home${current}" href="${href}"${current.includes("current") ? ' aria-current="page"' : ""}><span class="place-mark">${mark}</span><span class="place-name">${escape(name)}</span>${extra}</a>`;
    // While one of the agents' sections is open they stand under it, and
    // it is the section that is current; they slide in once as they come.
    const section = sectionOf(place);
    if (section === null) this.sectionsSince = null;
    else this.sectionsSince ??= performance.now();
    const opening = this.sectionsSince !== null && performance.now() - this.sectionsSince < FOLDING_MS ? " opening" : "";
    const sections = section === null ? "" : `<div class="asec-fold${opening}"><div class="asec-inner">${sectionRows(section, loggedIn, this.options.sections)}</div></div>`;
    // The agents come first: they are what the interface is for on a
    // computer of its own.
    const places = `<div class="side-places">
        ${talking ? top(link({ kind: "home" }), at(["home"]), knotMark(), words.world, waiting ? `<span class="count waiting"><span class="spoken">${escape(words.waiting)}</span>${waiting}</span>` : "") : ""}
        ${machines || assisted ? top(link({ kind: "agents", machine: null }), section === null ? "" : " open", glyph("agents", 18), words.agents) + sections : ""}
        ${machines ? top(link({ kind: "machines" }), at(["machines", "machine"]), glyph("machine", 18), words.machines) : ""}
        ${machines || loggedIn ? top(link({ kind: "features" }), at(["features", "feature"]), glyph("features", 18), words.features) : ""}
      </div>
      ${store.trouble.peek() ? `<p class="sidebar-trouble" title="${escape(store.trouble.peek() ?? "")}">${escape(words.unreachable)}</p>` : ""}
      ${store.unanswered.peek() >= SLOW ? `<p class="sidebar-trouble" role="status">${escape(words.unanswered(store.unanswered.peek()))}</p>` : ""}`;

    // Logged in to more than one account, the sidebar is the World as each
    // of them sees it, one after another; with one, it is that account's.
    const groups = groupsOf(world);
    let body: string;
    if (!talking) {
      body = "";
    } else if (world && groups.length > 1) {
      body = groups
        .map((group) => {
          const key = `${group.login.hub}/${group.login.account}`;
          const shut = folded.has(`group:${key}`);
          const mine = talks.filter((r) => residentIn(world, group, r));
          const asking = mine.some((r) => r.now?.waiting_for_user);
          return `<section class="sidebar-account${shut ? " shut" : ""}">
            <h2 class="side-account-title"><button class="side-account-fold" type="button" data-fold="group:${escape(key)}" aria-expanded="${!shut}" aria-label="${escape(words.groupOf(loginLabel(group.login)))}">${glyph("chevron", 14)}<span class="side-account-name">${escape(group.login.hubName)}<span class="side-account-sep"></span>${escape(group.login.account)}</span>${shut && asking ? `<span class="mark mark-ask" aria-hidden="true"></span>` : ""}</button></h2>
            ${shut ? "" : `<div class="side-account-body">${this.projects(group.towns, mine, place, now, facts, key)}${this.loose(mine, place, now, facts)}</div>`}
          </section>`;
        })
        .join("");
    } else {
      const towns = groups[0]?.towns ?? (world?.towns ?? []).map((town) => town.town);
      body = this.projects(towns, talks, place, now, facts, null) + this.loose(talks, place, now, facts);
    }
    if (talking) body += this.archived(archived, place, now, facts);

    this.drawFoot(world, place);
    return places + body;
  }

  /** The network shown, by name: its server's, or this computer's own while
   *  it is in none. In none, the name is only a name, opening nothing: the
   *  network begins on the machines page (`world-gui.md`, 界面的结构). */
  private drawSwitcher(): void {
    const { store, words } = this.options;
    const whole = store.whole.peek();
    const networks = networksOf(whole);
    const shown = shownNetwork(whole);
    const network = networks.find((one) => one.hub === shown);
    const name = network?.name ?? (whole && isComputer(whole) ? whole.here : words.app);
    const label = this.switcher.querySelector(".network-name")!;
    if (label.textContent !== name) label.textContent = name;
    const alone = networks.length === 0;
    this.switcher.disabled = alone;
    this.switcher.classList.toggle("alone", alone);
    if (alone) {
      this.popover.hide();
      this.switcher.removeAttribute("aria-haspopup");
      this.switcher.removeAttribute("title");
    } else {
      this.switcher.setAttribute("aria-haspopup", "menu");
      this.switcher.title = words.networks;
    }
  }

  /** Every network this device is in, the one shown checked; then the
   *  ways into another. The people and machines of one are on its drawing,
   *  the user's own things on 「我」. */
  private drawNetworks(): void {
    const { store, words } = this.options;
    const whole = store.whole.peek();
    const shown = shownNetwork(whole);
    const rows = networksOf(whole)
      .map(
        (network) => `<li class="network-row">
          <button class="network-pick${network.hub === shown ? " current" : ""}" type="button" role="menuitemradio" aria-checked="${network.hub === shown}" data-network="${escape(network.hub)}">
            <span class="network-dot${network.online ? " online" : ""}" aria-hidden="true"></span>
            <span class="network-row-text"><span class="network-row-name">${escape(network.name)}</span><span class="network-row-note">${escape(network.admin ? words.admin : words.member)} ${escape(words.machinesCount(network.machines))}</span></span>
            ${network.hub === shown ? glyph("check", 14) : ""}
          </button>
        </li>`,
      )
      .join("");
    const computer = !whole || isComputer(whole);
    this.networks.innerHTML = `${rows ? `<ul class="network-rows">${rows}</ul>` : ""}
      <div class="network-ways">
        <a class="network-way" href="${link({ kind: "login", way: "join" })}" role="menuitem">${glyph("link", 16)}<span>${escape(rows ? words.joinAnother : words.joinNetwork)}</span></a>
        ${computer ? `<a class="network-way" href="${link({ kind: "login", way: "new" })}" role="menuitem">${glyph("plus", 16)}<span>${escape(words.newNetwork)}</span></a>` : ""}
      </div>`;
  }

  /** Where the foot is on the screen, while the sidebar shows whole. */
  footBox(): DOMRect | null {
    const box = this.foot.getBoundingClientRect();
    return box.width > 0 && box.left >= 0 && box.right <= innerWidth ? box : null;
  }

  /** The foot: the user, by their name in the network shown, marked when a
   *  device came into their account; and the settings. */
  private drawFoot(world: WorldNow | null, place: Place): void {
    const { words } = this.options;
    // Managed machines that came in by one code are one thing to see.
    const arrivals = (world?.accounts.hubs ?? []).reduce((n, hub) => n + new Set(hub.arrivals.map((arrival) => arrival.code ?? `${arrival.account}\n${arrival.name}`)).size, 0);
    const [me] = loginsOf(world?.accounts);
    const account = me
      ? `<a class="place account-place${place.kind === "account" ? " current" : ""}" href="${link({ kind: "account" })}" title="${escape(words.account)}">${glyph("person", 18)}<span class="place-name">${escape(me.account)}</span>${arrivals ? `<span class="count waiting"><span class="spoken">${escape(words.arrivals(arrivals))}</span>${arrivals}</span>` : ""}</a>`
      : "";
    const html = `${account}<a class="place settings${place.kind === "settings" || place.kind === "data" ? " current" : ""}" href="${link({ kind: "settings" })}">${glyph("settings", 18)}<span class="place-name">${escape(words.settings)}</span></a>`;
    if (html !== this.foot.innerHTML) this.foot.innerHTML = html;
  }

  /** The projects, each open to its conversations. A conversation may live
   *  in a project this machine keeps no ledger of; it is listed under the
   *  project's short name all the same. */
  private projects(towns: string[], talks: ResidentNow[], place: Place, now: Date, facts: Map<string, MachineFacts>, group: string | null): string {
    const { store, words } = this.options;
    const known = store.world.peek()?.towns ?? [];
    // Only a project this computer keeps no ledger of is named after its
    // conversations; one it knows belongs to the account it is in.
    const named = new Set([...towns, ...known.map((town) => town.town)]);
    const unknown = [...new Set(talks.map((r) => r.town).filter((t): t is string => !!t && !named.has(t)))];
    const rows = [...towns, ...unknown]
      .map((town) => {
        const name = known.find((t) => t.town === town)?.record.name ?? town;
        const inside = talks.filter((r) => r.town === town);
        const shut = folded.has(town);
        const asking = inside.some((r) => r.now?.waiting_for_user);
        const working = inside.some((r) => r.now?.activity === "busy");
        const current = (place.kind === "project" || place.kind === "memory" || place.kind === "page") && place.town === town;
        const shutMark = asking ? "mark-ask" : working ? "mark-busy" : "";
        // Its name leads to its overview; its folder opens and closes on its
        // conversations where it is. Shut, it says whether something in it
        // waits or works, and how many are in it.
        return `<li class="project-place${shut ? " shut" : ""}" data-town="${escape(town)}" data-name="${escape(name)}">
          <div class="place-row project-row" data-own-press>
            <button class="project-fold" type="button" data-fold="${escape(town)}" aria-expanded="${!shut}" aria-label="${escape(words.agentsIn(name))}" title="${escape(words.agentsIn(name))}">${glyph(shut ? "folder" : "folder-open", 18)}</button>
            <a class="place project-link${current ? " current" : ""}" href="${link({ kind: "project", town })}"${current ? ' aria-current="page"' : ""}>
              <span class="place-name">${escape(name)}</span>
              <span class="project-shut-marks">${shutMark ? `<span class="mark ${shutMark}" aria-hidden="true"></span>` : ""}${inside.length ? `<span class="count">${inside.length}</span>` : ""}</span>
            </a>
            ${this.options.projects ? `<button class="side-project-more" type="button" data-project-more="${escape(town)}" aria-haspopup="menu" aria-expanded="false" aria-label="${escape(words.projectMore)}" title="${escape(words.projectMore)}">${glyph("more", 18)}</button>` : ""}
          </div>
          <div class="project-talks${shut ? " shut" : ""}"${shut ? " inert" : ""}><div class="project-talks-inner">${
            inside.length ? this.talks(inside, place, now, facts, town, "talks in-project") : `<p class="project-empty">${escape(words.noTalks)}</p>`
          }</div></div>
        </li>`;
      })
      .join("");
    return `<section class="sidebar-group"><h2 class="side-group-title">${escape(words.projects)}<button class="side-add" type="button" data-new-project="${escape(group ?? "")}" aria-label="${escape(words.newProject)}" title="${escape(words.newProject)}">${glyph("plus", 14)}</button></h2>
      ${rows ? `<ul class="projects-places">${rows}</ul>` : ""}
    </section>`;
  }

  /** Opens or closes a project's folder where it is: its mark, its state,
   *  and its conversations sliding in or out, the list's drawing brought to
   *  match without drawing it again. Says whether it could. */
  private foldInPlace(place: HTMLElement, open: boolean): boolean {
    const row = place.querySelector<HTMLElement>(".project-fold");
    const talks = place.querySelector<HTMLElement>(".project-talks");
    if (!row || !talks) return false;
    place.classList.toggle("shut", !open);
    talks.classList.toggle("shut", !open);
    talks.inert = !open;
    row.setAttribute("aria-expanded", String(open));
    row.innerHTML = glyph(open ? "folder-open" : "folder", 18);
    // The next drawing is the one that matches what is now shown.
    this.drawn = this.compose();
    this.folding = performance.now() + FOLDING_MS;
    return true;
  }

  /** A project's "⋯": a new conversation in it, its record; then sharing
   *  and handing it on where the reader may. */
  private openProjectMenu(button: HTMLElement): void {
    const projects = this.options.projects;
    const place = button.closest<HTMLElement>(".project-place");
    if (!projects || !place) return;
    const { words } = this.options;
    const town = place.dataset.town!;
    const name = place.dataset.name ?? town;
    const more = projects.acts(town);
    const entries: MenuEntry[] = [
      { act: "new", icon: "plus", label: words.projectNewTalk },
      { act: "edit", icon: "edit", label: words.projectEdit },
      ...(more.length ? [{ rule: true as const }, ...more] : []),
    ];
    this.projectMenu.open(button, name, entries, (act) => {
      if (act === "new") projects.newTalk(town);
      else if (act === "edit") projects.edit(town);
      else projects.run(act, town, name);
    });
  }

  /** The conversations in no project. */
  private loose(talks: ResidentNow[], place: Place, now: Date, facts: Map<string, MachineFacts>): string {
    const outside = talks.filter((r) => !r.town);
    return outside.length
      ? `<section class="sidebar-group"><h2 class="side-group-title">${escape(this.options.words.outside)}</h2>${this.talks(outside, place, now, facts, null, "talks")}</section>`
      : "";
  }

  /** The most recent conversations of a list, and a way to all of them.
   *  One waiting on the reader is never left behind "all". */
  private talks(talks: ResidentNow[], place: Place, now: Date, facts: Map<string, MachineFacts>, town: string | null, kind: string): string {
    const { words } = this.options;
    const shown = talks.filter((r, i) => i < SHOWN || r.now?.waiting_for_user || (place.kind === "talk" && place.address.resident === r.about.id && place.address.machine === r.about.machine));
    const rest = talks.length - shown.length;
    const all = place.kind === "talks" && place.town === town;
    return `<ul class="${kind}">${shown.map((r) => talkRow(r, place, now, facts, words, this.talkList.rowActs(r), assistingOwner(this.options.store.world.peek(), r.about.machine))).join("")}${
      rest > 0
        ? `<li><a class="talk-all${all ? " current" : ""}" href="${link({ kind: "talks", town })}"><span class="talk-host"></span><span class="talk-name">${escape(words.all)}</span><span class="count">${talks.length}</span></a></li>`
        : ""
    }</ul>`;
  }

  /** The archived conversations, folded away at the end: the most recent,
   *  and a way to all of them. */
  private archived(archived: ResidentNow[], place: Place, now: Date, facts: Map<string, MachineFacts>): string {
    if (!archived.length) return "";
    const { words, menuWords } = this.options;
    const open = folded.has("archived-open");
    const shown = archived.slice(0, SHOWN);
    const rows = shown.map((r) => talkRow(r, place, now, facts, words, this.talkList.rowActs(r), assistingOwner(this.options.store.world.peek(), r.about.machine))).join("");
    const all =
      archived.length > shown.length
        ? `<li><a class="talk-all${place.kind === "archived" ? " current" : ""}" href="${link({ kind: "archived" })}"><span class="talk-host"></span><span class="talk-name">${escape(words.all)}</span><span class="count">${archived.length}</span></a></li>`
        : "";
    return `<section class="side-archived${open ? "" : " shut"}">
      <button class="side-archived-head" type="button" data-fold="archived-open" aria-expanded="${open}">${glyph("chevron", 14)}<span>${escape(menuWords.archivedGroup)}</span><span class="side-archived-count">${archived.length}</span></button>
      ${open ? `<ul class="talks archived-talks">${rows}${all}</ul>` : ""}
    </section>`;
  }
}

/** The World's mark: machines as rings round the knot where they meet. */

function mark(): string {
  return `<svg class="wordmark-mark" width="22" height="22" viewBox="0 0 22 22" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="11" cy="11" r="2"/><circle cx="4.5" cy="6" r="2.6"/><circle cx="17.5" cy="6.5" r="2.6"/><circle cx="11" cy="18" r="2.6"/><path d="M6.6 7.4L9.3 9.7M15.4 7.8l-2.7 1.9M11 13.1v2.3"/></g></svg>`;
}

function knotMark(): string {
  return `<svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="10" cy="10" r="1.8"/><circle cx="4.6" cy="6" r="2.1"/><circle cx="15.6" cy="6.4" r="2.1"/><circle cx="10" cy="16.2" r="2.1"/></g></svg>`;
}

/** The button a new version is offered by: its version, or how far it has
 *  come, or that it is ready. */
function drawUpdate(bar: HTMLElement, offered: Offered | null, words: UpdateWords): void {
  bar.hidden = !offered;
  if (!offered) {
    bar.innerHTML = "";
    return;
  }
  const { stage } = offered;
  const percent = stage.kind === "fetching" && stage.total ? Math.min(100, Math.floor((stage.received / stage.total) * 100)) : null;
  const said =
    stage.kind === "fetching"
      ? words.barFetching(percent)
      : stage.kind === "ready" || stage.kind === "waiting" || stage.kind === "permission"
        ? words.barReady
        : stage.kind === "installing"
          ? words.barInstalling
          : words.bar;
  // A row like the places above it; one drop of the accent on its mark says
  // a version waits, and while it comes the row fills as far as it has come.
  const fill = stage.kind === "fetching" ? `<span class="update-fill" style="width:${percent ?? 4}%"></span>` : "";
  const html = `<button class="place update-place" type="button">${fill}<span class="update-mark">${glyph("update", 18)}</span><span class="place-name">${escape(said)}</span><span class="update-version">${escape(offered.manifest.version)}</span></button>`;
  if (bar.innerHTML !== html) bar.innerHTML = html;
}
