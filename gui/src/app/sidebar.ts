// The sidebar: the places of the World, always at hand, so who is here can
// be seen from any page. The World and its machines first, each a line
// with its mark; then each project, open to the conversations in it now,
// and the conversations in no project, one line each with a dot for what
// it is doing and the rest a hover away. Logged in to more than one
// account, the projects and conversations come account by account, each
// only what that account may use (`world-gui.md`, 账号、机器与授权). A
// conversation is begun from here; the account and the settings are at
// its foot.

import { glyph } from "../conversation/glyphs";
import { escape } from "../conversation/prose";
import { redrawKeepingFocus } from "./focus";
import type { Address } from "../world/events";
import { loginsOf, type Login } from "../world/accounts";
import type { WorldNow } from "../world/bridge";
import type { ResidentNow } from "../world/world";
import type { Place } from "./places";
import { link } from "./places";
import { groupsOf, isComputer, loginLabel, machinesOf, residentIn } from "./reach";
import { effect } from "./signals";
import { residentsOf, type Store } from "./store";
import type { ShellWords } from "./words";
import { agentName, titleOf } from "../world/names";

export interface SidebarOptions {
  store: Store;
  words: ShellWords;
  /** The place shown now. */
  place(): Place;
  newTalk(): void;
  /** A new project, in the account whose part of the sidebar it was
   *  asked from. */
  newProject(login: Login | null): void;
  fold(): void;
}

/** An answer usually comes within a second and is asked for every two;
 *  after this many seconds without one, the sidebar says so. */
const SLOW = 5;

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
  private drawn = "";

  constructor(private readonly options: SidebarOptions) {
    const { words } = options;
    this.element = document.createElement("nav");
    this.element.className = "sidebar";
    this.element.setAttribute("aria-label", words.app);
    this.element.innerHTML = `
      <div class="sidebar-head">
        <a class="wordmark" href="#/">${mark()}<span>${escape(words.app)}</span></a>
        <button class="icon-button sidebar-fold" type="button" aria-label="${escape(words.foldSidebar)}" title="${escape(words.foldSidebar)} · Ctrl+B">${glyph("sidebar", 18)}</button>
      </div>
      <button class="new-talk" type="button">${glyph("plus", 18)}<span>${escape(words.newTalk)}</span><kbd>Ctrl N</kbd></button>
      <div class="sidebar-places"></div>
      <div class="sidebar-foot"></div>`;
    this.list = this.element.querySelector<HTMLElement>(".sidebar-places")!;
    this.foot = this.element.querySelector<HTMLElement>(".sidebar-foot")!;
    this.element.querySelector(".new-talk")!.addEventListener("click", () => options.newTalk());
    this.element.querySelector(".sidebar-fold")!.addEventListener("click", () => options.fold());
    this.list.addEventListener("click", (event) => {
      const add = (event.target as HTMLElement).closest<HTMLElement>("[data-new-project]");
      if (add) {
        const group = add.dataset.newProject;
        options.newProject(loginsOf(options.store.world.peek()?.accounts).find((login) => `${login.hub}/${login.account}` === group) ?? null);
        return;
      }
      const toggle = (event.target as HTMLElement).closest<HTMLElement>("[data-fold]");
      if (!toggle) return;
      event.preventDefault();
      const key = toggle.dataset.fold!;
      if (folded.has(key)) folded.delete(key);
      else folded.add(key);
      keepFolded();
      this.drawn = "";
      this.draw();
    });
    effect(() => {
      options.store.world.get();
      options.store.trouble.get();
      options.store.clock.get();
      options.store.unanswered.get();
      this.draw();
    });
  }

  /** Draws again, as when the place shown changed. */
  draw(): void {
    const { store, words } = this.options;
    const world = store.world.peek();
    const place = this.options.place();
    const now = store.clock.peek();
    const residents = residentsOf(world).filter((r) => r.here || r.now?.waiting_for_user);
    const waiting = residents.filter((r) => r.now?.waiting_for_user).length;

    const machinesHere = place.kind === "machines" || place.kind === "machine";
    const home = `<a class="place home${place.kind === "home" ? " current" : ""}" href="${link({ kind: "home" })}">
        <span class="place-mark">${knotMark()}</span><span class="place-name">${escape(words.world)}</span>
        ${waiting ? `<span class="count waiting"><span class="spoken">${escape(words.waiting)}</span>${waiting}</span>` : ""}
      </a>
      ${world && machinesOf(world).length ? `<a class="place home${machinesHere ? " current" : ""}" href="${link({ kind: "machines" })}"><span class="place-mark">${glyph("machine", 18)}</span><span class="place-name">${escape(words.machines)}</span></a>` : ""}
      ${store.trouble.peek() ? `<p class="sidebar-trouble">${escape(words.unreachable)}</p>` : ""}
      ${store.unanswered.peek() >= SLOW ? `<p class="sidebar-trouble" role="status">${escape(words.unanswered(store.unanswered.peek()))}</p>` : ""}`;

    // Logged in to more than one account, the sidebar is the World as each
    // of them sees it, one after another; with one, it is that account's.
    const groups = groupsOf(world);
    let body: string;
    if (world && groups.length > 1) {
      body = groups
        .map((group) => {
          const key = `${group.login.hub}/${group.login.account}`;
          const shut = folded.has(`group:${key}`);
          const mine = residents.filter((r) => residentIn(world, group, r));
          const asking = mine.some((r) => r.now?.waiting_for_user);
          return `<section class="sidebar-account${shut ? " shut" : ""}">
            <h2 class="side-account-title"><button class="side-account-fold" type="button" data-fold="group:${escape(key)}" aria-expanded="${!shut}" aria-label="${escape(words.groupOf(loginLabel(group.login)))}">${glyph("chevron", 14)}<span class="side-account-name">${escape(group.login.hubName)}<span class="side-account-sep">·</span>${escape(group.login.account)}</span>${shut && asking ? `<span class="mark mark-ask" aria-hidden="true"></span>` : ""}</button></h2>
            ${shut ? "" : `<div class="side-account-body">${this.projects(group.towns, mine, place, now, key)}${this.loose(mine, place, now)}</div>`}
          </section>`;
        })
        .join("");
    } else {
      const towns = groups[0]?.towns ?? (world?.towns ?? []).map((town) => town.town);
      body = this.projects(towns, residents, place, now, null) + this.loose(residents, place, now);
    }

    // On its own, this computer is a World of one; the way in is kept at
    // hand until it logs in.
    const alone =
      world && isComputer(world) && !groups.length
        ? `<section class="sidebar-group"><a class="place home${place.kind === "login" ? " current" : ""}" href="${link({ kind: "login" })}"><span class="place-mark">${glyph("person", 18)}</span><span class="place-name">${escape(words.logIn)}</span></a></section>`
        : "";
    this.drawFoot(world, place);
    const html = home + body + alone;
    if (html === this.drawn) return;
    this.drawn = html;
    redrawKeepingFocus(this.list, () => {
      this.list.innerHTML = html;
    });
  }

  /** The foot: the accounts, marked when someone came into one, and the
   *  settings. */
  private drawFoot(world: WorldNow | null, place: Place): void {
    const { words } = this.options;
    const arrivals = (world?.accounts.hubs ?? []).reduce((n, hub) => n + hub.arrivals.length, 0);
    const account = loginsOf(world?.accounts).length
      ? `<a class="place account-place${place.kind === "account" || place.kind === "admin" ? " current" : ""}" href="${link({ kind: "account" })}">${glyph("person", 18)}<span class="place-name">${escape(words.account)}</span>${arrivals ? `<span class="count waiting"><span class="spoken">${escape(words.arrivals(arrivals))}</span>${arrivals}</span>` : ""}</a>`
      : "";
    const html = `${account}<a class="place settings${place.kind === "settings" ? " current" : ""}" href="${link({ kind: "settings" })}">${glyph("settings", 18)}<span class="place-name">${escape(words.settings)}</span></a>`;
    if (html !== this.foot.innerHTML) this.foot.innerHTML = html;
  }

  /** The projects, each open to the agents in it now. A resident may live
   *  in a project this machine keeps no ledger of; it is listed under the
   *  project's short name all the same. */
  private projects(towns: string[], residents: ResidentNow[], place: Place, now: Date, group: string | null): string {
    const { store, words } = this.options;
    const known = store.world.peek()?.towns ?? [];
    // Only a project this computer keeps no ledger of is named after its
    // residents; one it knows belongs to the account it is in.
    const named = new Set([...towns, ...known.map((town) => town.town)]);
    const unknown = [...new Set(residents.map((r) => r.town).filter((t): t is string => !!t && !named.has(t)))];
    const rows = [...towns, ...unknown]
      .map((town) => {
        const name = known.find((t) => t.town === town)?.record.name ?? town;
        const inside = order(residents.filter((r) => r.town === town));
        const shut = folded.has(town);
        const asking = inside.some((r) => r.now?.waiting_for_user);
        const current = (place.kind === "project" || place.kind === "journal" || place.kind === "page") && place.town === town;
        return `<li class="project-place${shut ? " shut" : ""}">
          <div class="place-row">
            <a class="place${current ? " current" : ""}" href="${link({ kind: "project", town })}"><span class="place-name">${escape(name)}</span>
              ${shut && asking ? `<span class="mark mark-ask" aria-hidden="true"></span>` : ""}
              ${shut && inside.length ? `<span class="count">${inside.length}</span>` : ""}</a>
            ${inside.length ? `<button class="place-fold" type="button" data-fold="${escape(town)}" aria-expanded="${!shut}" aria-label="${escape(words.agentsIn(name))}">${glyph("chevron", 16)}</button>` : ""}
          </div>
          ${shut || !inside.length ? "" : `<ul class="agents">${inside.map((r) => this.agent(r, place, now)).join("")}</ul>`}
        </li>`;
      })
      .join("");
    return `<section class="sidebar-group"><h2 class="side-group-title">${escape(words.projects)}<button class="side-add" type="button" data-new-project="${escape(group ?? "")}" aria-label="${escape(words.newProject)}" title="${escape(words.newProject)}">${glyph("plus", 14)}</button></h2>
      ${rows ? `<ul class="projects-places">${rows}</ul>` : ""}
    </section>`;
  }

  /** The agents in no project. */
  private loose(residents: ResidentNow[], place: Place, now: Date): string {
    const outside = order(residents.filter((r) => !r.town));
    return outside.length
      ? `<section class="sidebar-group"><h2 class="side-group-title">${escape(this.options.words.outside)}</h2><ul class="agents loose">${outside.map((r) => this.agent(r, place, now)).join("")}</ul></section>`
      : "";
  }

  private agent(r: ResidentNow, place: Place, now: Date): string {
    const { words } = this.options;
    const address: Address = { resident: r.about.id, machine: r.about.machine };
    const current = place.kind === "talk" && place.address.resident === address.resident && place.address.machine === address.machine;
    const name = words.someone(agentName(r.about.host.agent), r.about.machine);
    const waiting = r.now?.waiting_for_user;
    const busy = r.now?.activity === "busy";
    const title = titleOf(r) ?? name;
    const state = waiting
      ? words.waiting
      : busy
        ? words.busy(words.span(now.getTime() - Date.parse(r.now!.since)))
        : r.now?.activity === "asleep"
          ? words.asleep
          : words.idle;
    const kind = waiting ? "mark-ask" : busy ? "mark-busy" : "mark-idle";
    const hover = [title !== name ? name : "", state].filter(Boolean).join(" · ");
    return `<li><a class="place agent-place ${kind}${current ? " current" : ""}" href="${link({ kind: "talk", address })}" title="${escape(hover)}">
      <span class="mark ${kind}" aria-hidden="true"></span><span class="place-name">${escape(title)}</span><span class="spoken">${escape(state)}</span>
    </a></li>`;
  }
}

/** Who waits on the reader first, then who works, then the rest; within
 *  each, the most recently stirred first. */
function order(residents: ResidentNow[]): ResidentNow[] {
  const rank = (r: ResidentNow) => (r.now?.waiting_for_user ? 0 : r.now?.activity === "busy" ? 1 : 2);
  const stirred = (r: ResidentNow) => Date.parse(r.now?.since ?? r.about.born_at);
  return [...residents].sort((a, b) => rank(a) - rank(b) || stirred(b) - stirred(a));
}

/** The World's mark: machines as rings round the knot where they meet. */
function mark(): string {
  return `<svg class="wordmark-mark" width="22" height="22" viewBox="0 0 22 22" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="11" cy="11" r="2"/><circle cx="4.5" cy="6" r="2.6"/><circle cx="17.5" cy="6.5" r="2.6"/><circle cx="11" cy="18" r="2.6"/><path d="M6.6 7.4L9.3 9.7M15.4 7.8l-2.7 1.9M11 13.1v2.3"/></g></svg>`;
}

function knotMark(): string {
  return `<svg width="18" height="18" viewBox="0 0 20 20" aria-hidden="true"><g fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round"><circle cx="10" cy="10" r="1.8"/><circle cx="4.6" cy="6" r="2.1"/><circle cx="15.6" cy="6.4" r="2.1"/><circle cx="10" cy="16.2" r="2.1"/></g></svg>`;
}
